use std::collections::BTreeSet;

use crate::contracts::{CanvasState, Cluster, ClusterId, Window, WindowId, ZoomLevel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteClusterMode {
    BlockIfNonEmpty,
    ReassignTo { fallback_cluster: ClusterId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenAssignPolicy {
    ActiveCluster,
    FallbackCluster(ClusterId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    ClusterAlreadyExists(ClusterId),
    ClusterNameConflict(String),
    ClusterNotFound(ClusterId),
    WindowNotFound(WindowId),
    WindowAlreadyTracked(WindowId),
    ClusterNotEmpty {
        cluster: ClusterId,
        window_count: usize,
    },
    InvalidFallbackCluster(ClusterId),
    ActiveClusterMissing,
    InvariantViolation(ModelInvariantError),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MutationResult {
    CreatedCluster(ClusterId),
    RenamedCluster {
        cluster: ClusterId,
        old_name: String,
        new_name: String,
    },
    MovedCluster {
        cluster: ClusterId,
        old_x: f64,
        old_y: f64,
        new_x: f64,
        new_y: f64,
    },
    DeletedCluster {
        cluster: ClusterId,
        reassigned_windows: usize,
    },
    WindowAssignmentChanged {
        window: WindowId,
        old_cluster: ClusterId,
        new_cluster: ClusterId,
        manual_override: bool,
    },
    WindowAssignmentUnchanged {
        window: WindowId,
        cluster: ClusterId,
        manual_override: bool,
    },
    WindowOpened {
        window: WindowId,
        cluster: ClusterId,
    },
    WindowClosed {
        window: WindowId,
        cluster: ClusterId,
    },
    WindowCloseNoop(WindowId),
    FocusUpdated {
        window: WindowId,
        cluster: ClusterId,
    },
    FocusNoop(WindowId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelInvariantError {
    WindowMissingCluster(WindowId),
    WindowReferencesUnknownCluster {
        window: WindowId,
        cluster: ClusterId,
    },
    WindowMissingFromCluster {
        window: WindowId,
        cluster: ClusterId,
    },
    UnknownWindowInCluster {
        window: WindowId,
        cluster: ClusterId,
    },
    DuplicateWindowInCluster {
        window: WindowId,
        cluster: ClusterId,
    },
    ActiveClusterDoesNotExist(ClusterId),
}

#[derive(Debug, Clone)]
pub struct CanvasModel {
    state: CanvasState,
    active_cluster: ClusterId,
}

impl CanvasModel {
    pub fn new(mut state: CanvasState, active_cluster: ClusterId) -> Result<Self, ModelError> {
        ensure_cluster_metadata_defaults(&mut state);
        let model = Self {
            state,
            active_cluster,
        };
        model
            .check_invariants()
            .map_err(ModelError::InvariantViolation)?;
        Ok(model)
    }

    pub fn state(&self) -> &CanvasState {
        &self.state
    }

    pub fn active_cluster(&self) -> ClusterId {
        self.active_cluster
    }

    pub fn create_cluster(
        &mut self,
        id: ClusterId,
        name: impl Into<String>,
        x: f64,
        y: f64,
    ) -> Result<MutationResult, ModelError> {
        let name = name.into();
        if self.state.clusters.iter().any(|cluster| cluster.id == id) {
            return Err(ModelError::ClusterAlreadyExists(id));
        }
        if self
            .state
            .clusters
            .iter()
            .any(|cluster| cluster.name == name)
        {
            return Err(ModelError::ClusterNameConflict(name));
        }

        self.state.clusters.push(Cluster {
            id,
            name,
            x,
            y,
            enabled: true,
            windows: Vec::new(),
            last_focus: None,
            recency: Vec::new(),
        });

        self.validate_after(MutationResult::CreatedCluster(id))
    }

    pub fn rename_cluster(
        &mut self,
        id: ClusterId,
        new_name: impl Into<String>,
    ) -> Result<MutationResult, ModelError> {
        let new_name = new_name.into();
        if self
            .state
            .clusters
            .iter()
            .any(|cluster| cluster.id != id && cluster.name == new_name)
        {
            return Err(ModelError::ClusterNameConflict(new_name));
        }

        let cluster = self
            .state
            .clusters
            .iter_mut()
            .find(|cluster| cluster.id == id)
            .ok_or(ModelError::ClusterNotFound(id))?;

        let old_name = std::mem::replace(&mut cluster.name, new_name.clone());
        self.validate_after(MutationResult::RenamedCluster {
            cluster: id,
            old_name,
            new_name,
        })
    }

    pub fn move_cluster(
        &mut self,
        id: ClusterId,
        new_x: f64,
        new_y: f64,
    ) -> Result<MutationResult, ModelError> {
        let cluster = self
            .state
            .clusters
            .iter_mut()
            .find(|cluster| cluster.id == id)
            .ok_or(ModelError::ClusterNotFound(id))?;
        let old_x = cluster.x;
        let old_y = cluster.y;
        cluster.x = new_x;
        cluster.y = new_y;

        self.validate_after(MutationResult::MovedCluster {
            cluster: id,
            old_x,
            old_y,
            new_x,
            new_y,
        })
    }

    pub fn delete_cluster(
        &mut self,
        id: ClusterId,
        mode: DeleteClusterMode,
    ) -> Result<MutationResult, ModelError> {
        let idx = self
            .state
            .clusters
            .iter()
            .position(|cluster| cluster.id == id)
            .ok_or(ModelError::ClusterNotFound(id))?;

        let existing_windows = self.state.clusters[idx].windows.clone();
        let reassigned_windows = existing_windows.len();

        match mode {
            DeleteClusterMode::BlockIfNonEmpty if !existing_windows.is_empty() => {
                return Err(ModelError::ClusterNotEmpty {
                    cluster: id,
                    window_count: existing_windows.len(),
                });
            }
            DeleteClusterMode::BlockIfNonEmpty => {}
            DeleteClusterMode::ReassignTo { fallback_cluster } => {
                if fallback_cluster == id {
                    return Err(ModelError::InvalidFallbackCluster(fallback_cluster));
                }
                if !self
                    .state
                    .clusters
                    .iter()
                    .any(|cluster| cluster.id == fallback_cluster)
                {
                    return Err(ModelError::InvalidFallbackCluster(fallback_cluster));
                }
                for window_id in existing_windows {
                    self.reassign_window(window_id, fallback_cluster, false)?;
                }
            }
        }

        self.state.clusters.remove(idx);
        if self.active_cluster == id {
            self.active_cluster = self
                .state
                .clusters
                .first()
                .map(|cluster| cluster.id)
                .ok_or(ModelError::ActiveClusterMissing)?;
            self.state.zoom = ZoomLevel::Cluster(self.active_cluster);
        }

        self.validate_after(MutationResult::DeletedCluster {
            cluster: id,
            reassigned_windows,
        })
    }

    pub fn assign_window_to_cluster_manual(
        &mut self,
        window_id: WindowId,
        cluster_id: ClusterId,
    ) -> Result<MutationResult, ModelError> {
        self.reassign_window(window_id, cluster_id, true)
    }

    pub fn on_window_open(
        &mut self,
        mut window: Window,
        policy: OpenAssignPolicy,
    ) -> Result<MutationResult, ModelError> {
        if self
            .state
            .windows
            .iter()
            .any(|tracked| tracked.id == window.id)
        {
            return Err(ModelError::WindowAlreadyTracked(window.id));
        }
        let target_cluster = match policy {
            OpenAssignPolicy::ActiveCluster => self.active_cluster,
            OpenAssignPolicy::FallbackCluster(cluster_id) => cluster_id,
        };
        let _ = self.find_cluster_mut(target_cluster)?;

        window.cluster_id = Some(target_cluster);
        window.manual_cluster_override = false;
        self.state.windows.push(window);
        self.push_window_to_cluster(
            target_cluster,
            self.state.windows.last().expect("window exists").id,
        );

        self.validate_after(MutationResult::WindowOpened {
            window: self.state.windows.last().expect("window exists").id,
            cluster: target_cluster,
        })
    }

    pub fn on_window_close(&mut self, window_id: WindowId) -> Result<MutationResult, ModelError> {
        let Some(position) = self
            .state
            .windows
            .iter()
            .position(|window| window.id == window_id)
        else {
            return Ok(MutationResult::WindowCloseNoop(window_id));
        };
        let closed = self.state.windows.remove(position);
        let Some(cluster_id) = closed.cluster_id else {
            return Err(ModelError::InvariantViolation(
                ModelInvariantError::WindowMissingCluster(window_id),
            ));
        };

        let cluster = self.find_cluster_mut(cluster_id)?;
        cluster.windows.retain(|&id| id != window_id);
        cluster.recency.retain(|&id| id != window_id);
        if cluster.last_focus == Some(window_id) {
            cluster.last_focus = cluster.recency.first().copied();
        }

        self.validate_after(MutationResult::WindowClosed {
            window: window_id,
            cluster: cluster_id,
        })
    }

    pub fn on_focus_change(&mut self, window_id: WindowId) -> Result<MutationResult, ModelError> {
        let window = self
            .state
            .windows
            .iter()
            .find(|window| window.id == window_id)
            .ok_or(ModelError::WindowNotFound(window_id))?;
        let cluster_id = window.cluster_id.ok_or(ModelError::InvariantViolation(
            ModelInvariantError::WindowMissingCluster(window_id),
        ))?;

        let cluster = self.find_cluster_mut(cluster_id)?;
        let previous = cluster.last_focus;
        cluster.last_focus = Some(window_id);
        cluster.recency.retain(|&id| id != window_id);
        cluster.recency.insert(0, window_id);
        self.active_cluster = cluster_id;

        if previous == Some(window_id) {
            self.validate_after(MutationResult::FocusNoop(window_id))
        } else {
            self.validate_after(MutationResult::FocusUpdated {
                window: window_id,
                cluster: cluster_id,
            })
        }
    }

    pub fn check_invariants(&self) -> Result<(), ModelInvariantError> {
        let known_clusters: BTreeSet<ClusterId> = self
            .state
            .clusters
            .iter()
            .map(|cluster| cluster.id)
            .collect();
        if !known_clusters.contains(&self.active_cluster) {
            return Err(ModelInvariantError::ActiveClusterDoesNotExist(
                self.active_cluster,
            ));
        }

        let known_windows: BTreeSet<WindowId> =
            self.state.windows.iter().map(|window| window.id).collect();

        for window in &self.state.windows {
            let cluster = window
                .cluster_id
                .ok_or(ModelInvariantError::WindowMissingCluster(window.id))?;
            if !known_clusters.contains(&cluster) {
                return Err(ModelInvariantError::WindowReferencesUnknownCluster {
                    window: window.id,
                    cluster,
                });
            }
            let owner = self
                .state
                .clusters
                .iter()
                .find(|candidate| candidate.id == cluster)
                .expect("cluster exists due to known_clusters");
            if !owner.windows.contains(&window.id) {
                return Err(ModelInvariantError::WindowMissingFromCluster {
                    window: window.id,
                    cluster,
                });
            }
        }

        for cluster in &self.state.clusters {
            let mut seen = BTreeSet::new();
            for &window_id in &cluster.windows {
                if !known_windows.contains(&window_id) {
                    return Err(ModelInvariantError::UnknownWindowInCluster {
                        window: window_id,
                        cluster: cluster.id,
                    });
                }
                if !seen.insert(window_id) {
                    return Err(ModelInvariantError::DuplicateWindowInCluster {
                        window: window_id,
                        cluster: cluster.id,
                    });
                }
            }
        }

        Ok(())
    }

    fn validate_after(&self, result: MutationResult) -> Result<MutationResult, ModelError> {
        self.check_invariants()
            .map_err(ModelError::InvariantViolation)
            .map(|_| result)
    }

    fn reassign_window(
        &mut self,
        window_id: WindowId,
        cluster_id: ClusterId,
        manual_override: bool,
    ) -> Result<MutationResult, ModelError> {
        let _ = self.find_cluster_mut(cluster_id)?;

        let current_cluster = self
            .state
            .windows
            .iter()
            .find(|window| window.id == window_id)
            .ok_or(ModelError::WindowNotFound(window_id))?
            .cluster_id
            .ok_or(ModelError::InvariantViolation(
                ModelInvariantError::WindowMissingCluster(window_id),
            ))?;

        if current_cluster == cluster_id {
            if let Some(window) = self
                .state
                .windows
                .iter_mut()
                .find(|window| window.id == window_id)
            {
                window.manual_cluster_override = manual_override;
            }
            return self.validate_after(MutationResult::WindowAssignmentUnchanged {
                window: window_id,
                cluster: cluster_id,
                manual_override,
            });
        }

        if let Some(window) = self
            .state
            .windows
            .iter_mut()
            .find(|window| window.id == window_id)
        {
            window.cluster_id = Some(cluster_id);
            window.manual_cluster_override = manual_override;
        }

        if let Some(cluster) = self
            .state
            .clusters
            .iter_mut()
            .find(|cluster| cluster.id == current_cluster)
        {
            cluster.windows.retain(|&id| id != window_id);
            cluster.recency.retain(|&id| id != window_id);
            if cluster.last_focus == Some(window_id) {
                cluster.last_focus = cluster.recency.first().copied();
            }
        }

        self.push_window_to_cluster(cluster_id, window_id);

        self.validate_after(MutationResult::WindowAssignmentChanged {
            window: window_id,
            old_cluster: current_cluster,
            new_cluster: cluster_id,
            manual_override,
        })
    }

    fn push_window_to_cluster(&mut self, cluster_id: ClusterId, window_id: WindowId) {
        if let Some(cluster) = self
            .state
            .clusters
            .iter_mut()
            .find(|cluster| cluster.id == cluster_id)
        {
            if !cluster.windows.contains(&window_id) {
                cluster.windows.push(window_id);
            }
            cluster.recency.retain(|&id| id != window_id);
            cluster.recency.insert(0, window_id);
            cluster.last_focus = Some(window_id);
        }
    }

    fn find_cluster_mut(&mut self, cluster_id: ClusterId) -> Result<&mut Cluster, ModelError> {
        self.state
            .clusters
            .iter_mut()
            .find(|cluster| cluster.id == cluster_id)
            .ok_or(ModelError::ClusterNotFound(cluster_id))
    }
}

fn ensure_cluster_metadata_defaults(state: &mut CanvasState) {
    for cluster in &mut state.clusters {
        cluster.recency.retain(|window_id| *window_id != 0);
        let mut unique = BTreeSet::new();
        cluster
            .windows
            .retain(|window_id| unique.insert(*window_id));
        cluster
            .recency
            .retain(|window_id| unique.contains(window_id));
        for &window_id in cluster.windows.iter().rev() {
            if !cluster.recency.contains(&window_id) {
                cluster.recency.push(window_id);
            }
        }
        if cluster.last_focus.is_none() {
            cluster.last_focus = cluster.recency.first().copied();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{OutputState, Viewport, WindowRole, WindowState};

    fn fixture_window(id: WindowId, cluster_id: ClusterId) -> Window {
        Window {
            id,
            title: format!("Window {id}"),
            app_id: Some("fixture".into()),
            class: Some("fixture".into()),
            role: WindowRole::Normal,
            state: WindowState::Tiled,
            cluster_id: Some(cluster_id),
            transient_for: None,
            manual_cluster_override: false,
            manual_position_override: false,
        }
    }

    fn model_fixture_with_cluster_windows(window_ids: &[WindowId]) -> CanvasModel {
        let cluster_id = 1;
        let state = CanvasState {
            state_revision: 0,
            zoom: ZoomLevel::Cluster(cluster_id),
            viewport: Viewport::default(),
            output_viewports: std::collections::HashMap::new(),
            clusters: vec![Cluster {
                id: cluster_id,
                name: "fixture".into(),
                x: 0.0,
                y: 0.0,
                enabled: true,
                windows: window_ids.to_vec(),
                last_focus: window_ids.first().copied(),
                recency: window_ids.to_vec(),
            }],
            windows: window_ids
                .iter()
                .map(|window_id| fixture_window(*window_id, cluster_id))
                .collect(),
            output: OutputState::default(),
        };

        CanvasModel::new(state, cluster_id).expect("fixture model should be valid")
    }

    fn base_state() -> CanvasState {
        CanvasState {
            state_revision: 0,
            zoom: ZoomLevel::Cluster(1),
            viewport: Viewport::default(),
            output_viewports: std::collections::HashMap::new(),
            clusters: vec![
                Cluster {
                    id: 1,
                    name: "one".into(),
                    x: 0.0,
                    y: 0.0,
                    enabled: true,
                    windows: vec![10],
                    last_focus: Some(10),
                    recency: vec![10],
                },
                Cluster {
                    id: 2,
                    name: "two".into(),
                    x: 1.0,
                    y: 1.0,
                    enabled: true,
                    windows: vec![],
                    last_focus: None,
                    recency: vec![],
                },
            ],
            windows: vec![Window {
                id: 10,
                title: "Terminal".into(),
                app_id: Some("foot".into()),
                class: Some("foot".into()),
                role: WindowRole::Normal,
                state: WindowState::Tiled,
                cluster_id: Some(1),
                transient_for: None,
                manual_cluster_override: false,
                manual_position_override: false,
            }],
            output: OutputState::default(),
        }
    }

    #[test]
    fn duplicate_create_is_rejected() {
        let mut model = CanvasModel::new(base_state(), 1).expect("valid model");
        let err = model
            .create_cluster(1, "dupe", 0.0, 0.0)
            .expect_err("duplicate id should fail");
        assert_eq!(err, ModelError::ClusterAlreadyExists(1));
    }

    #[test]
    fn rename_conflict_is_rejected() {
        let mut model = CanvasModel::new(base_state(), 1).expect("valid model");
        let err = model
            .rename_cluster(1, "two")
            .expect_err("name conflict should fail");
        assert_eq!(err, ModelError::ClusterNameConflict("two".into()));
    }

    #[test]
    fn delete_active_cluster_reassigns_and_switches_active() {
        let mut model = CanvasModel::new(base_state(), 1).expect("valid model");

        let result = model
            .delete_cluster(
                1,
                DeleteClusterMode::ReassignTo {
                    fallback_cluster: 2,
                },
            )
            .expect("delete should succeed");

        assert_eq!(
            result,
            MutationResult::DeletedCluster {
                cluster: 1,
                reassigned_windows: 1,
            }
        );
        assert_eq!(model.active_cluster(), 2);
        let moved = model
            .state()
            .windows
            .iter()
            .find(|window| window.id == 10)
            .expect("window exists");
        assert_eq!(moved.cluster_id, Some(2));
    }

    #[test]
    fn close_during_reassignment_cleans_stale_refs() {
        let mut model = CanvasModel::new(base_state(), 1).expect("valid model");
        model
            .assign_window_to_cluster_manual(10, 2)
            .expect("reassignment should succeed");

        let closed = model.on_window_close(10).expect("close should succeed");
        assert_eq!(
            closed,
            MutationResult::WindowClosed {
                window: 10,
                cluster: 2,
            }
        );
        let destination = model
            .state()
            .clusters
            .iter()
            .find(|cluster| cluster.id == 2)
            .expect("cluster 2 exists");
        assert!(destination.windows.is_empty());
        assert!(destination.recency.is_empty());
    }

    #[test]
    fn focus_changes_update_recency_without_reordering_unfocused_windows() {
        let mut state = base_state();
        state.clusters[0].windows = vec![10, 11, 12];
        state.clusters[0].recency = vec![10, 11, 12];
        state.clusters[0].last_focus = Some(10);
        state.windows.extend([
            Window {
                id: 11,
                title: "Editor".into(),
                app_id: Some("code".into()),
                class: Some("code".into()),
                role: WindowRole::Normal,
                state: WindowState::Tiled,
                cluster_id: Some(1),
                transient_for: None,
                manual_cluster_override: false,
                manual_position_override: false,
            },
            Window {
                id: 12,
                title: "Browser".into(),
                app_id: Some("firefox".into()),
                class: Some("firefox".into()),
                role: WindowRole::Normal,
                state: WindowState::Tiled,
                cluster_id: Some(1),
                transient_for: None,
                manual_cluster_override: false,
                manual_position_override: false,
            },
        ]);

        let mut model = CanvasModel::new(state, 1).expect("valid model");

        let first = model
            .on_focus_change(12)
            .expect("focus update should succeed");
        assert_eq!(
            first,
            MutationResult::FocusUpdated {
                window: 12,
                cluster: 1,
            }
        );

        let cluster = model
            .state()
            .clusters
            .iter()
            .find(|cluster| cluster.id == 1)
            .expect("cluster exists");
        assert_eq!(cluster.last_focus, Some(12));
        assert_eq!(cluster.recency, vec![12, 10, 11]);

        let second = model
            .on_focus_change(11)
            .expect("focus update should succeed");
        assert_eq!(
            second,
            MutationResult::FocusUpdated {
                window: 11,
                cluster: 1,
            }
        );
        let cluster = model
            .state()
            .clusters
            .iter()
            .find(|cluster| cluster.id == 1)
            .expect("cluster exists");
        assert_eq!(cluster.last_focus, Some(11));
        assert_eq!(cluster.recency, vec![11, 12, 10]);
    }

    #[test]
    fn metadata_defaults_seed_deterministic_recency_for_existing_windows() {
        let state = CanvasState {
            state_revision: 0,
            zoom: ZoomLevel::Cluster(1),
            viewport: Viewport::default(),
            output_viewports: std::collections::HashMap::new(),
            clusters: vec![Cluster {
                id: 1,
                name: "one".into(),
                x: 0.0,
                y: 0.0,
                enabled: true,
                windows: vec![20, 10, 30],
                last_focus: None,
                recency: vec![],
            }],
            windows: vec![
                Window {
                    id: 10,
                    title: "A".into(),
                    app_id: None,
                    class: None,
                    role: WindowRole::Normal,
                    state: WindowState::Tiled,
                    cluster_id: Some(1),
                    transient_for: None,
                    manual_cluster_override: false,
                    manual_position_override: false,
                },
                Window {
                    id: 20,
                    title: "B".into(),
                    app_id: None,
                    class: None,
                    role: WindowRole::Normal,
                    state: WindowState::Tiled,
                    cluster_id: Some(1),
                    transient_for: None,
                    manual_cluster_override: false,
                    manual_position_override: false,
                },
                Window {
                    id: 30,
                    title: "C".into(),
                    app_id: None,
                    class: None,
                    role: WindowRole::Normal,
                    state: WindowState::Tiled,
                    cluster_id: Some(1),
                    transient_for: None,
                    manual_cluster_override: false,
                    manual_position_override: false,
                },
            ],
            output: OutputState::default(),
        };

        let model = CanvasModel::new(state, 1).expect("valid model");
        let cluster = model.state().clusters.first().expect("cluster exists");
        assert_eq!(cluster.recency, vec![30, 10, 20]);
        assert_eq!(cluster.last_focus, Some(30));
    }
    #[test]
    fn manual_assignment_is_idempotent() {
        let mut model = CanvasModel::new(base_state(), 1).expect("valid model");

        let unchanged = model
            .assign_window_to_cluster_manual(10, 1)
            .expect("idempotent assignment should succeed");

        assert_eq!(
            unchanged,
            MutationResult::WindowAssignmentUnchanged {
                window: 10,
                cluster: 1,
                manual_override: true,
            }
        );
    }

    #[test]
    fn transition_cycles_preserve_deterministic_focus_order_for_1_2_and_3plus_windows() {
        let fixtures = [vec![1001], vec![2001, 2002], vec![3001, 3002, 3003, 3004]];

        for window_ids in fixtures {
            let mut model = model_fixture_with_cluster_windows(&window_ids);
            for cycle in 0..20 {
                for (offset, window_id) in window_ids.iter().enumerate() {
                    let result = model
                        .on_focus_change(*window_id)
                        .expect("focus update should succeed");
                    if cycle == 0 && offset == 0 {
                        assert_eq!(result, MutationResult::FocusNoop(*window_id));
                    }
                }
            }

            let cluster = model.state().clusters.first().expect("cluster exists");
            let mut expected = window_ids.clone();
            expected.reverse();
            assert_eq!(cluster.recency, expected);
            assert_eq!(cluster.last_focus, window_ids.last().copied());
            assert_eq!(cluster.windows, window_ids);
        }
    }
}
