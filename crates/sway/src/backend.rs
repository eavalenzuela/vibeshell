use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

pub type ClusterId = i64;
pub type WindowId = i64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMetadata {
    pub id: Option<i64>,
    pub num: Option<i32>,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct WorkspaceTransitionController {
    continuum_workspace: String,
    original_workspace_by_window: HashMap<WindowId, WorkspaceMetadata>,
}

impl WorkspaceTransitionController {
    pub fn new(continuum_workspace: impl Into<String>) -> Self {
        Self {
            continuum_workspace: continuum_workspace.into(),
            original_workspace_by_window: HashMap::new(),
        }
    }

    pub fn enter_cluster_zoom(
        &mut self,
        target_windows: &[WindowId],
        workspace_by_window: &HashMap<WindowId, WorkspaceMetadata>,
    ) -> Vec<String> {
        tracing::info!(
            stage = "enter_cluster_zoom",
            target_windows = target_windows.len(),
            tracked_windows = self.original_workspace_by_window.len(),
            continuum_workspace = self.continuum_workspace.as_str(),
            "starting cluster zoom transition"
        );

        let mut commands = Vec::new();
        for window_id in target_windows {
            let Some(workspace) = workspace_by_window.get(window_id) else {
                tracing::warn!(
                    stage = "enter_cluster_zoom",
                    window_id,
                    "unable to move window to continuum workspace because current workspace metadata is missing"
                );
                continue;
            };

            self.original_workspace_by_window
                .entry(*window_id)
                .or_insert_with(|| workspace.clone());

            if let Some(command) = self.move_to_continuum(*window_id, workspace) {
                commands.push(command);
            }
        }

        commands
    }

    fn move_to_continuum(
        &self,
        window_id: WindowId,
        workspace: &WorkspaceMetadata,
    ) -> Option<String> {
        let already_on_continuum = workspace.name == self.continuum_workspace;

        tracing::info!(
            stage = "move_to_continuum",
            window_id,
            source_workspace = workspace.name.as_str(),
            continuum_workspace = self.continuum_workspace.as_str(),
            already_on_continuum,
            "evaluating move to continuum workspace"
        );

        (!already_on_continuum).then(|| {
            let continuum_workspace = self.continuum_workspace.replace('"', "\\\"");
            format!("[con_id={window_id}] move container to workspace \"{continuum_workspace}\"")
        })
    }

    pub fn restore_workspace(
        &mut self,
        window_ids: &[WindowId],
        live_windows: &HashSet<WindowId>,
    ) -> Vec<String> {
        tracing::info!(
            stage = "restore_workspace",
            requested_windows = window_ids.len(),
            tracked_windows = self.original_workspace_by_window.len(),
            "restoring pre-zoom workspace placements"
        );

        let mut commands = Vec::new();

        for window_id in window_ids {
            if !live_windows.contains(window_id) {
                self.original_workspace_by_window.remove(window_id);
                tracing::info!(
                    stage = "restore_workspace",
                    window_id,
                    "window no longer exists; pruned stale restore entry"
                );
                continue;
            }

            let Some(original_workspace) = self.original_workspace_by_window.remove(window_id)
            else {
                tracing::debug!(
                    stage = "restore_workspace",
                    window_id,
                    "no tracked workspace metadata for window; skipping restore"
                );
                continue;
            };

            let command = match original_workspace.num {
                Some(num) => {
                    format!("[con_id={window_id}] move container to workspace number {num}")
                }
                None => {
                    let escaped = original_workspace.name.replace('"', "\\\"");
                    format!("[con_id={window_id}] move container to workspace \"{escaped}\"")
                }
            };
            commands.push(command);
        }

        self.prune_stale_entries(live_windows);
        commands
    }

    pub fn prune_stale_entries(&mut self, live_windows: &HashSet<WindowId>) {
        self.original_workspace_by_window
            .retain(|window_id, _| live_windows.contains(window_id));
    }

    pub fn tracked_windows(&self) -> &HashMap<WindowId, WorkspaceMetadata> {
        &self.original_workspace_by_window
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterLayoutInput {
    pub cluster_id: ClusterId,
    pub area: Rect,
    /// Canonical cluster-local order (persistent model order), not WM tree order.
    pub windows: Vec<WindowId>,
    /// Fallback ordering key for windows not present in `windows`.
    pub first_seen_at: HashMap<WindowId, u64>,
}

fn canonical_cluster_window_order(cluster: &ClusterLayoutInput) -> Vec<WindowId> {
    let mut explicit = Vec::new();
    let mut seen = BTreeSet::new();

    for &window_id in &cluster.windows {
        if seen.insert(window_id) {
            explicit.push(window_id);
        }
    }

    let mut fallback: Vec<_> = cluster
        .first_seen_at
        .iter()
        .filter_map(|(&window_id, &first_seen)| {
            (!seen.contains(&window_id)).then_some((window_id, first_seen))
        })
        .collect();
    fallback.sort_unstable_by_key(|(window_id, first_seen)| (*first_seen, *window_id));

    explicit.extend(fallback.into_iter().map(|(window_id, _)| window_id));

    if explicit != cluster.windows {
        let common: BTreeSet<_> = cluster.windows.iter().copied().collect();
        let unexpected_reorder = cluster
            .windows
            .iter()
            .all(|window_id| explicit.contains(window_id))
            && explicit.iter().all(|window_id| common.contains(window_id));
        tracing::warn!(
            cluster_id = cluster.cluster_id,
            previous_order = ?cluster.windows,
            next_order = ?explicit,
            unexpected_reorder,
            "cluster window order changed during canonicalization"
        );
    }

    explicit
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendEvent {
    WorkspaceChanged {
        cluster_id: ClusterId,
    },
    WindowChanged {
        cluster_id: ClusterId,
        window_id: WindowId,
    },
}

impl BackendEvent {
    fn cluster_id(self) -> ClusterId {
        match self {
            Self::WorkspaceChanged { cluster_id } | Self::WindowChanged { cluster_id, .. } => {
                cluster_id
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffThresholds {
    pub position_px: i32,
    pub size_px: i32,
}

impl Default for DiffThresholds {
    fn default() -> Self {
        Self {
            position_px: 1,
            size_px: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutOp {
    pub window_id: WindowId,
    pub target: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameResult {
    pub drained_events: Vec<BackendEvent>,
    pub computed_targets: HashMap<WindowId, Rect>,
    pub applied_ops: Vec<LayoutOp>,
    pub command_batch: Option<String>,
}

#[derive(Debug, Default)]
pub struct LayoutEngine;

impl LayoutEngine {
    pub fn compute(&self, clusters: &[ClusterLayoutInput]) -> HashMap<WindowId, Rect> {
        let mut targets = HashMap::new();

        for cluster in clusters {
            let ordered_windows = canonical_cluster_window_order(cluster);
            if ordered_windows.is_empty() {
                continue;
            }

            let count = ordered_windows.len() as i32;
            let base_width = (cluster.area.width / count).max(1);
            let mut x = cluster.area.x;

            for (idx, window_id) in ordered_windows.iter().copied().enumerate() {
                let is_last = idx + 1 == ordered_windows.len();
                let width = if is_last {
                    (cluster.area.x + cluster.area.width - x).max(1)
                } else {
                    base_width
                };

                targets.insert(
                    window_id,
                    Rect {
                        x,
                        y: cluster.area.y,
                        width,
                        height: cluster.area.height,
                    },
                );

                x += width;
            }
        }

        targets
    }

    pub fn apply(ops: &[LayoutOp]) -> Option<String> {
        if ops.is_empty() {
            return None;
        }

        Some(
            ops.iter()
                .map(|op| {
                    format!(
                        "[con_id={}] move position {} {}, resize set width {} px height {} px",
                        op.window_id, op.target.x, op.target.y, op.target.width, op.target.height
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
        )
    }
}

pub fn diff_targets(
    current: &HashMap<WindowId, Rect>,
    targets: &HashMap<WindowId, Rect>,
    thresholds: DiffThresholds,
) -> Vec<LayoutOp> {
    targets
        .iter()
        .filter_map(|(window_id, target)| {
            let changed = match current.get(window_id) {
                Some(existing) => {
                    (existing.x - target.x).abs() > thresholds.position_px
                        || (existing.y - target.y).abs() > thresholds.position_px
                        || (existing.width - target.width).abs() > thresholds.size_px
                        || (existing.height - target.height).abs() > thresholds.size_px
                }
                None => true,
            };

            changed.then_some(LayoutOp {
                window_id: *window_id,
                target: *target,
            })
        })
        .collect()
}

#[derive(Debug)]
pub struct FramePipeline {
    debounce_window: Duration,
    thresholds: DiffThresholds,
    layout_engine: LayoutEngine,
    pending_events: Vec<BackendEvent>,
    pending_clusters: BTreeSet<ClusterId>,
    last_queued_at: Option<Instant>,
}

impl FramePipeline {
    pub fn new(debounce_window: Duration, thresholds: DiffThresholds) -> Self {
        Self {
            debounce_window,
            thresholds,
            layout_engine: LayoutEngine,
            pending_events: Vec::new(),
            pending_clusters: BTreeSet::new(),
            last_queued_at: None,
        }
    }

    pub fn queue_event(&mut self, event: BackendEvent, now: Instant) {
        self.pending_clusters.insert(event.cluster_id());
        self.pending_events.push(event);
        self.last_queued_at = Some(now);

        tracing::info!(
            stage = "queued_events",
            queued_events = self.pending_events.len(),
            pending_clusters = self.pending_clusters.len(),
            ?event,
            "frame pipeline queued event"
        );
    }

    pub fn try_build_frame(
        &mut self,
        now: Instant,
        clusters: &[ClusterLayoutInput],
        current_geometry: &HashMap<WindowId, Rect>,
    ) -> Option<FrameResult> {
        let queued_at = self.last_queued_at?;
        if now.duration_since(queued_at) < self.debounce_window {
            return None;
        }

        let pending_clusters = std::mem::take(&mut self.pending_clusters);
        let drained_events = std::mem::take(&mut self.pending_events);
        self.last_queued_at = None;

        let affected: Vec<_> = clusters
            .iter()
            .filter(|cluster| pending_clusters.contains(&cluster.cluster_id))
            .cloned()
            .collect();

        let computed_targets = self.layout_engine.compute(&affected);
        tracing::info!(
            stage = "computed_windows",
            computed_windows = computed_targets.len(),
            affected_clusters = affected.len(),
            "frame pipeline computed targets"
        );

        let applied_ops = diff_targets(current_geometry, &computed_targets, self.thresholds);
        let command_batch = LayoutEngine::apply(&applied_ops);
        tracing::info!(
            stage = "applied_ops",
            applied_ops = applied_ops.len(),
            batched_command = command_batch.is_some(),
            "frame pipeline prepared sway command batch"
        );

        Some(FrameResult {
            drained_events,
            computed_targets,
            applied_ops,
            command_batch,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_pipeline_debounces_and_batches_once() {
        let mut pipeline = FramePipeline::new(
            Duration::from_millis(24),
            DiffThresholds {
                position_px: 0,
                size_px: 0,
            },
        );

        let start = Instant::now();
        pipeline.queue_event(BackendEvent::WorkspaceChanged { cluster_id: 7 }, start);
        pipeline.queue_event(
            BackendEvent::WindowChanged {
                cluster_id: 7,
                window_id: 42,
            },
            start,
        );

        let clusters = vec![ClusterLayoutInput {
            cluster_id: 7,
            area: Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 50,
            },
            windows: vec![42, 43],
            first_seen_at: HashMap::from([(42, 10), (43, 20)]),
        }];

        let current = HashMap::from([
            (
                42,
                Rect {
                    x: 0,
                    y: 0,
                    width: 40,
                    height: 50,
                },
            ),
            (
                43,
                Rect {
                    x: 40,
                    y: 0,
                    width: 40,
                    height: 50,
                },
            ),
        ]);

        assert!(pipeline
            .try_build_frame(start + Duration::from_millis(8), &clusters, &current)
            .is_none());

        let frame = pipeline
            .try_build_frame(start + Duration::from_millis(24), &clusters, &current)
            .expect("frame should be emitted once debounce window elapsed");

        assert_eq!(frame.drained_events.len(), 2);
        assert_eq!(frame.computed_targets.len(), 2);
        assert_eq!(frame.applied_ops.len(), 2);
        assert!(frame
            .command_batch
            .as_deref()
            .expect("command batch expected")
            .contains("[con_id=42]"));

        assert!(pipeline
            .try_build_frame(start + Duration::from_millis(40), &clusters, &current)
            .is_none());
    }

    #[test]
    fn diff_targets_respects_thresholds() {
        let current = HashMap::from([(
            7,
            Rect {
                x: 10,
                y: 10,
                width: 200,
                height: 100,
            },
        )]);
        let targets = HashMap::from([(
            7,
            Rect {
                x: 11,
                y: 10,
                width: 201,
                height: 100,
            },
        )]);

        let ignored = diff_targets(
            &current,
            &targets,
            DiffThresholds {
                position_px: 2,
                size_px: 2,
            },
        );
        assert!(ignored.is_empty());

        let applied = diff_targets(
            &current,
            &targets,
            DiffThresholds {
                position_px: 0,
                size_px: 0,
            },
        );
        assert_eq!(applied.len(), 1);
    }

    #[test]
    fn canonical_order_prefers_explicit_then_first_seen_fallback() {
        let cluster = ClusterLayoutInput {
            cluster_id: 5,
            area: Rect {
                x: 0,
                y: 0,
                width: 90,
                height: 40,
            },
            windows: vec![7, 9],
            first_seen_at: HashMap::from([(7, 30), (8, 10), (9, 20)]),
        };

        assert_eq!(canonical_cluster_window_order(&cluster), vec![7, 9, 8]);
    }

    #[test]
    fn enter_cluster_zoom_tracks_original_workspace_and_builds_move_commands() {
        let mut controller = WorkspaceTransitionController::new("__continuum");
        let workspace_by_window = HashMap::from([
            (
                10,
                WorkspaceMetadata {
                    id: Some(1),
                    num: Some(1),
                    name: "1:web".into(),
                },
            ),
            (
                11,
                WorkspaceMetadata {
                    id: Some(2),
                    num: None,
                    name: "__continuum".into(),
                },
            ),
        ]);

        let commands = controller.enter_cluster_zoom(&[10, 11, 12], &workspace_by_window);
        assert_eq!(commands.len(), 1);
        assert_eq!(
            commands[0],
            "[con_id=10] move container to workspace \"__continuum\""
        );
        assert_eq!(controller.tracked_windows().len(), 2);
    }

    #[test]
    fn restore_workspace_moves_live_windows_back_and_prunes_stale_entries() {
        let mut controller = WorkspaceTransitionController::new("__continuum");
        let workspace_by_window = HashMap::from([
            (
                20,
                WorkspaceMetadata {
                    id: Some(1),
                    num: Some(2),
                    name: "2:code".into(),
                },
            ),
            (
                21,
                WorkspaceMetadata {
                    id: Some(2),
                    num: None,
                    name: "scratch".into(),
                },
            ),
        ]);
        controller.enter_cluster_zoom(&[20, 21], &workspace_by_window);

        let live_windows = HashSet::from([20]);
        let commands = controller.restore_workspace(&[20, 21], &live_windows);

        assert_eq!(
            commands,
            vec!["[con_id=20] move container to workspace number 2"]
        );
        assert!(controller.tracked_windows().is_empty());
    }
}
