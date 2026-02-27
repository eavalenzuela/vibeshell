use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

pub type ClusterId = i64;
pub type WindowId = i64;

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
    pub windows: Vec<WindowId>,
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
            if cluster.windows.is_empty() {
                continue;
            }

            let count = cluster.windows.len() as i32;
            let base_width = (cluster.area.width / count).max(1);
            let mut x = cluster.area.x;

            for (idx, window_id) in cluster.windows.iter().copied().enumerate() {
                let is_last = idx + 1 == cluster.windows.len();
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
}
