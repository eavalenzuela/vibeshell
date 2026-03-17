use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::contracts::{CanvasState, Cluster, ClusterId, Viewport, WindowId, ZoomLevel};

const DEFAULT_DEBOUNCE_MS: u64 = 250;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedCluster {
    pub id: ClusterId,
    pub name: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PersistedOverviewState {
    pub viewport: Viewport,
    pub output_viewports: BTreeMap<String, Viewport>,
    pub clusters: Vec<PersistedCluster>,
    pub manual_window_assignments: BTreeMap<WindowId, ClusterId>,
    pub active_cluster: Option<ClusterId>,
    pub cluster_history: Vec<ClusterId>,
}

impl PersistedOverviewState {
    pub fn from_canvas(state: &CanvasState) -> Self {
        Self {
            viewport: state.viewport.clone(),
            output_viewports: state.output_viewports.clone().into_iter().collect(),
            clusters: state
                .clusters
                .iter()
                .map(|cluster| PersistedCluster {
                    id: cluster.id,
                    name: cluster.name.clone(),
                    x: cluster.x,
                    y: cluster.y,
                })
                .collect(),
            manual_window_assignments: state
                .windows
                .iter()
                .filter(|window| window.manual_cluster_override)
                .filter_map(|window| window.cluster_id.map(|cluster| (window.id, cluster)))
                .collect(),
            active_cluster: match state.zoom {
                ZoomLevel::Cluster(id) => Some(id),
                _ => None,
            },
            cluster_history: Vec::new(),
        }
    }

    pub fn apply_to_canvas_seed(&self, state: &mut CanvasState) {
        state.viewport = self.viewport.clone();
        state.output_viewports = self.output_viewports.clone().into_iter().collect();
        state.clusters = self
            .clusters
            .iter()
            .map(|cluster| Cluster {
                id: cluster.id,
                name: cluster.name.clone(),
                x: cluster.x,
                y: cluster.y,
                enabled: true,
                windows: Vec::new(),
                last_focus: None,
                recency: Vec::new(),
            })
            .collect();
        if let Some(cluster_id) = self.active_cluster {
            state.zoom = ZoomLevel::Cluster(cluster_id);
        }
    }

    pub fn merge_into_live_canvas(&self, state: &mut CanvasState) {
        state.viewport = self.viewport.clone();
        state.output_viewports = self.output_viewports.clone().into_iter().collect();

        let mut coords = self
            .clusters
            .iter()
            .map(|cluster| (cluster.id, (cluster.x, cluster.y)))
            .collect::<BTreeMap<_, _>>();
        for cluster in &mut state.clusters {
            if let Some((x, y)) = coords.remove(&cluster.id) {
                cluster.x = x;
                cluster.y = y;
            }
        }

        for window in &mut state.windows {
            if let Some(cluster_id) = self.manual_window_assignments.get(&window.id).copied() {
                window.cluster_id = Some(cluster_id);
                window.manual_cluster_override = true;
            }
        }

        if let Some(cluster_id) = self.active_cluster {
            if state.zoom == ZoomLevel::Overview {
                state.zoom = ZoomLevel::Cluster(cluster_id);
            }
        }
    }
}

#[derive(Debug)]
pub struct OverviewPersistence {
    path: PathBuf,
    debounce: Duration,
    pending: Option<PersistedOverviewState>,
    pending_deadline: Option<Instant>,
}

impl Default for OverviewPersistence {
    fn default() -> Self {
        Self::new()
    }
}

impl OverviewPersistence {
    pub fn new() -> Self {
        Self::with_debounce(Duration::from_millis(DEFAULT_DEBOUNCE_MS))
    }

    pub fn with_debounce(debounce: Duration) -> Self {
        Self {
            path: default_persistence_path(),
            debounce,
            pending: None,
            pending_deadline: None,
        }
    }

    pub fn load(&self) -> io::Result<Option<PersistedOverviewState>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&self.path)?;
        let parsed = serde_json::from_str(&content).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to parse persisted overview state: {error}"),
            )
        })?;
        Ok(Some(parsed))
    }

    pub fn persist_immediate(&mut self, state: &CanvasState) -> io::Result<()> {
        let snapshot = PersistedOverviewState::from_canvas(state);
        self.pending = None;
        self.pending_deadline = None;
        write_atomic_json(&self.path, &snapshot)
    }

    pub fn persist_debounced(&mut self, state: &CanvasState) {
        self.pending = Some(PersistedOverviewState::from_canvas(state));
        self.pending_deadline = Some(Instant::now() + self.debounce);
    }

    pub fn flush_due(&mut self) -> io::Result<bool> {
        let Some(deadline) = self.pending_deadline else {
            return Ok(false);
        };
        if Instant::now() < deadline {
            return Ok(false);
        }
        self.flush_pending()
    }

    pub fn flush_pending(&mut self) -> io::Result<bool> {
        let Some(snapshot) = self.pending.take() else {
            self.pending_deadline = None;
            return Ok(false);
        };
        self.pending_deadline = None;
        write_atomic_json(&self.path, &snapshot)?;
        Ok(true)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn default_persistence_path() -> PathBuf {
    let state_root = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")));
    if let Some(root) = state_root {
        return root.join("vibeshell").join("overview-state.json");
    }

    let config_root = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    config_root.join("vibeshell").join("overview-state.json")
}

fn write_atomic_json(path: &Path, state: &PersistedOverviewState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp = path.with_extension("json.tmp");
    let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to serialize persisted overview state: {error}"),
        )
    })?;
    fs::write(&tmp, encoded)?;
    fs::rename(tmp, path)?;
    Ok(())
}
