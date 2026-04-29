//! Workspace + window-id model for vibewm.
//!
//! Smithay's `Window` is an opaque handle; the daemon needs a stable `u64`
//! it can ship over IPC. `VibewmModel` is the registry that assigns those
//! ids and tracks which cluster each window belongs to.
//!
//! For W1c-2 the model is purely informational: the rest of vibewm still
//! treats the world as one flat `Space<Window>`. W1c-3 will use the
//! `active_cluster` field to drive what's actually visible.

use std::collections::BTreeMap;

use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::IsAlive;
use wm::layout::{ClusterId, WindowId};

#[derive(Debug, Clone)]
pub struct ClusterEntry {
    pub id: ClusterId,
    pub name: String,
    /// Window ids assigned to this cluster, in stable insertion order.
    pub windows: Vec<WindowId>,
}

pub struct VibewmModel {
    next_window_id: u64,
    next_cluster_id: u64,
    /// Stable id → smithay handle. Iteration order is insertion order
    /// (BTreeMap by id).
    pub windows: BTreeMap<WindowId, Window>,
    pub clusters: Vec<ClusterEntry>,
    pub active_cluster: ClusterId,
    /// Previous cluster, used by `BackAndForthWorkspace`. None on first boot.
    pub previous_cluster: Option<ClusterId>,
}

impl VibewmModel {
    pub fn new() -> Self {
        let mut model = Self {
            next_window_id: 1,
            next_cluster_id: 0, // bumped by first create_cluster below
            windows: BTreeMap::new(),
            clusters: Vec::new(),
            active_cluster: 0,
            previous_cluster: None,
        };
        // Default cluster mirrors sway's "workspace 1" boot convention.
        let default_id = model.create_cluster("1".to_owned());
        model.active_cluster = default_id;
        model
    }

    /// Allocate a fresh `WindowId` and bind it to the given smithay handle.
    /// Assigns the window to the currently-active cluster.
    pub fn register_window(&mut self, window: Window) -> WindowId {
        let id = self.next_window_id;
        self.next_window_id += 1;
        self.windows.insert(id, window);
        if let Some(cluster) = self
            .clusters
            .iter_mut()
            .find(|c| c.id == self.active_cluster)
        {
            cluster.windows.push(id);
        }
        id
    }

    pub fn unregister_window(&mut self, id: WindowId) {
        self.windows.remove(&id);
        for cluster in &mut self.clusters {
            cluster.windows.retain(|&w| w != id);
        }
    }

    /// Drop entries whose underlying surface is no longer alive.
    /// Cheap to call on every snapshot — smithay handles the hard part.
    pub fn prune_dead(&mut self) {
        let dead: Vec<_> = self
            .windows
            .iter()
            .filter_map(|(&id, w)| {
                let alive = w
                    .toplevel()
                    .map(|t| t.wl_surface().alive())
                    .unwrap_or(false);
                (!alive).then_some(id)
            })
            .collect();
        for id in dead {
            self.unregister_window(id);
        }
    }

    pub fn create_cluster(&mut self, name: String) -> ClusterId {
        self.next_cluster_id += 1;
        let id = self.next_cluster_id;
        self.clusters.push(ClusterEntry {
            id,
            name,
            windows: Vec::new(),
        });
        id
    }

    pub fn find_cluster_by_name(&self, name: &str) -> Option<ClusterId> {
        self.clusters.iter().find(|c| c.name == name).map(|c| c.id)
    }

    /// Activate the cluster, recording the previous active for `back_and_forth`.
    /// Returns `false` if the id is unknown.
    pub fn activate_cluster(&mut self, id: ClusterId) -> bool {
        if !self.clusters.iter().any(|c| c.id == id) {
            return false;
        }
        if self.active_cluster != id {
            self.previous_cluster = Some(self.active_cluster);
            self.active_cluster = id;
        }
        true
    }

    /// Swap with `previous_cluster` if any. Returns `false` if no previous.
    pub fn back_and_forth(&mut self) -> bool {
        match self.previous_cluster {
            Some(prev) => {
                let cur = self.active_cluster;
                self.active_cluster = prev;
                self.previous_cluster = Some(cur);
                true
            }
            None => false,
        }
    }

    pub fn window_id_for_surface(&self, surface: &WlSurface) -> Option<WindowId> {
        self.windows.iter().find_map(|(&id, w)| {
            w.toplevel()
                .and_then(|t| (t.wl_surface() == surface).then_some(id))
        })
    }
}

impl Default for VibewmModel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_has_default_cluster_named_one() {
        let model = VibewmModel::new();
        assert_eq!(model.clusters.len(), 1);
        assert_eq!(model.clusters[0].name, "1");
        assert_eq!(model.active_cluster, model.clusters[0].id);
        assert!(model.previous_cluster.is_none());
    }

    #[test]
    fn create_then_activate_records_previous() {
        let mut model = VibewmModel::new();
        let default_id = model.active_cluster;
        let new_id = model.create_cluster("play".into());
        assert!(model.activate_cluster(new_id));
        assert_eq!(model.active_cluster, new_id);
        assert_eq!(model.previous_cluster, Some(default_id));
    }

    #[test]
    fn back_and_forth_swaps_active_and_previous() {
        let mut model = VibewmModel::new();
        let a = model.active_cluster;
        let b = model.create_cluster("b".into());
        model.activate_cluster(b);
        assert!(model.back_and_forth());
        assert_eq!(model.active_cluster, a);
        assert_eq!(model.previous_cluster, Some(b));
    }

    #[test]
    fn back_and_forth_noop_without_previous() {
        let mut model = VibewmModel::new();
        let a = model.active_cluster;
        assert!(!model.back_and_forth());
        assert_eq!(model.active_cluster, a);
    }

    #[test]
    fn activate_unknown_cluster_fails() {
        let mut model = VibewmModel::new();
        assert!(!model.activate_cluster(9999));
    }

    #[test]
    fn find_cluster_by_name() {
        let mut model = VibewmModel::new();
        model.create_cluster("project".into());
        assert!(model.find_cluster_by_name("project").is_some());
        assert!(model.find_cluster_by_name("missing").is_none());
    }
}
