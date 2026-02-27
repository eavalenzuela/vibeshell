use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use common::contracts::{
    CanvasState, Cluster, ClusterId, ContextStripDirection, OutputState, Viewport, Window,
    WindowId, WindowRole, WindowState, ZoomLevel,
};
use serde::Serialize;
use serde_json::json;
use swayipc::Connection;
use tracing::info;

static STATE_OWNER: OnceLock<Mutex<StateOwner>> = OnceLock::new();

pub fn with_state_owner<T>(f: impl FnOnce(&mut StateOwner) -> T) -> T {
    let owner = STATE_OWNER.get_or_init(|| Mutex::new(StateOwner::new()));
    let mut guard = owner.lock().expect("state owner mutex poisoned");
    f(&mut guard)
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct FocusFreezeMetadata {
    pub frozen: bool,
    pub window_id: Option<WindowId>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictOutcome {
    None,
    PreservedViewport,
    PreservedClusterPosition,
    MissingWindow,
    MissingCluster,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationType {
    SwayIngest,
    GetState,
    ActivateCluster,
    SetFocusZoomTarget,
    ZoomInMode,
    ZoomOutMode,
    CycleContextStrip,
}

#[derive(Debug, Clone)]
pub struct StateOwner {
    canvas_state: CanvasState,
    selected_cluster_id: Option<ClusterId>,
    focus_freeze: FocusFreezeMetadata,
}

impl StateOwner {
    pub fn new() -> Self {
        Self {
            canvas_state: CanvasState::default(),
            selected_cluster_id: None,
            focus_freeze: FocusFreezeMetadata::default(),
        }
    }

    pub fn state(&self) -> CanvasState {
        self.canvas_state.clone()
    }

    pub fn selected_cluster_id(&self) -> Option<ClusterId> {
        self.selected_cluster_id
    }

    pub fn ingest_sway_facts(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let prior = self.canvas_state.state_revision;
        let snapshot = sway_snapshot()?;
        let mut outcome = ConflictOutcome::None;

        let mut existing_cluster_positions = self
            .canvas_state
            .clusters
            .iter()
            .map(|cluster| (cluster.id, (cluster.x, cluster.y)))
            .collect::<BTreeMap<_, _>>();

        let mut clusters = snapshot.clusters;
        for cluster in &mut clusters {
            if let Some((x, y)) = existing_cluster_positions.remove(&cluster.id) {
                cluster.x = x;
                cluster.y = y;
                outcome = ConflictOutcome::PreservedClusterPosition;
            }
        }

        let previous_viewport = self.canvas_state.viewport.clone();
        self.canvas_state.clusters = clusters;
        self.canvas_state.windows = snapshot.windows;
        self.canvas_state.output = snapshot.output;
        self.canvas_state.viewport = previous_viewport;
        if self.canvas_state.viewport != Viewport::default() {
            outcome = ConflictOutcome::PreservedViewport;
        }

        if let Some(current) = self.selected_cluster_id {
            if !self
                .canvas_state
                .clusters
                .iter()
                .any(|cluster| cluster.id == current)
            {
                self.selected_cluster_id = self.canvas_state.clusters.first().map(|c| c.id);
            }
        } else {
            self.selected_cluster_id = self.canvas_state.clusters.first().map(|c| c.id);
        }

        if matches!(self.canvas_state.zoom, ZoomLevel::Focus(window_id)
            if !self.canvas_state.windows.iter().any(|window| window.id == window_id))
        {
            self.focus_freeze = FocusFreezeMetadata {
                frozen: true,
                window_id: None,
                reason: Some("focused_window_missing_after_ingest".to_owned()),
            };
            if let Some(cluster_id) = self.selected_cluster_id {
                self.canvas_state.zoom = ZoomLevel::Cluster(cluster_id);
            } else {
                self.canvas_state.zoom = ZoomLevel::Overview;
            }
        }

        self.bump_revision(prior, MutationType::SwayIngest, outcome);
        Ok(())
    }

    pub fn mutate_get_state(&mut self) {
        let prior = self.canvas_state.state_revision;
        self.bump_revision(prior, MutationType::GetState, ConflictOutcome::None);
    }

    pub fn activate_cluster(&mut self, cluster_id: ClusterId) -> Result<(), String> {
        let prior = self.canvas_state.state_revision;
        if self
            .canvas_state
            .clusters
            .iter()
            .any(|cluster| cluster.id == cluster_id)
        {
            self.selected_cluster_id = Some(cluster_id);
            self.canvas_state.zoom = ZoomLevel::Cluster(cluster_id);
            self.focus_freeze = FocusFreezeMetadata::default();
            self.bump_revision(prior, MutationType::ActivateCluster, ConflictOutcome::None);
            Ok(())
        } else {
            self.bump_revision(
                prior,
                MutationType::ActivateCluster,
                ConflictOutcome::MissingCluster,
            );
            Err(
                json!({"error":"invalid_state","reason":"cluster_not_found","cluster":cluster_id})
                    .to_string(),
            )
        }
    }

    pub fn set_focus_zoom_target(&mut self, window_id: WindowId) -> Result<(), String> {
        let prior = self.canvas_state.state_revision;
        let cluster_id = self
            .canvas_state
            .windows
            .iter()
            .find(|window| window.id == window_id)
            .and_then(|window| window.cluster_id)
            .ok_or_else(|| {
                self.bump_revision(
                    prior,
                    MutationType::SetFocusZoomTarget,
                    ConflictOutcome::MissingWindow,
                );
                json!({"error":"invalid_focus_target","window":window_id}).to_string()
            })?;

        self.selected_cluster_id = Some(cluster_id);
        self.canvas_state.zoom = ZoomLevel::Focus(window_id);
        self.focus_freeze = FocusFreezeMetadata {
            frozen: true,
            window_id: Some(window_id),
            reason: Some("explicit_focus_zoom_target".into()),
        };
        self.bump_revision(
            prior,
            MutationType::SetFocusZoomTarget,
            ConflictOutcome::None,
        );
        Ok(())
    }

    pub fn zoom_in_mode(&mut self) -> Result<(), String> {
        let prior = self.canvas_state.state_revision;
        match self.canvas_state.zoom {
            ZoomLevel::Overview => {
                let cluster_id = self
                    .selected_cluster_id
                    .or_else(|| self.canvas_state.clusters.first().map(|c| c.id))
                    .ok_or_else(|| {
                        json!({"error":"invalid_state","reason":"no_clusters_available"})
                            .to_string()
                    })?;
                self.canvas_state.zoom = ZoomLevel::Cluster(cluster_id);
                self.selected_cluster_id = Some(cluster_id);
                self.bump_revision(prior, MutationType::ZoomInMode, ConflictOutcome::None);
                Ok(())
            }
            ZoomLevel::Cluster(cluster_id) => {
                let cluster = self
                    .canvas_state
                    .clusters
                    .iter()
                    .find(|cluster| cluster.id == cluster_id)
                    .ok_or_else(|| {
                        json!({"error":"invalid_state","reason":"active_cluster_missing"})
                            .to_string()
                    })?;
                let window_id = cluster.last_focus.or_else(|| cluster.windows.first().copied())
                    .ok_or_else(|| json!({"error":"unsupported_state_combination","reason":"cluster_has_no_windows"}).to_string())?;
                self.canvas_state.zoom = ZoomLevel::Focus(window_id);
                self.focus_freeze = FocusFreezeMetadata {
                    frozen: true,
                    window_id: Some(window_id),
                    reason: Some("zoom_in_mode".into()),
                };
                self.bump_revision(prior, MutationType::ZoomInMode, ConflictOutcome::None);
                Ok(())
            }
            ZoomLevel::Focus(_) => {
                self.bump_revision(prior, MutationType::ZoomInMode, ConflictOutcome::None);
                Err(json!({"error":"unsupported_state_combination","reason":"already_in_focus_zoom"}).to_string())
            }
        }
    }

    pub fn zoom_out_mode(&mut self) -> Result<(), String> {
        let prior = self.canvas_state.state_revision;
        match self.canvas_state.zoom {
            ZoomLevel::Focus(window_id) => {
                let cluster_id = self
                    .canvas_state
                    .clusters
                    .iter()
                    .find(|cluster| cluster.windows.contains(&window_id))
                    .map(|cluster| cluster.id)
                    .or(self.selected_cluster_id)
                    .ok_or_else(|| {
                        json!({"error":"invalid_state","reason":"focused_window_cluster_missing"})
                            .to_string()
                    })?;
                self.canvas_state.zoom = ZoomLevel::Cluster(cluster_id);
                self.selected_cluster_id = Some(cluster_id);
                self.focus_freeze = FocusFreezeMetadata::default();
                self.bump_revision(prior, MutationType::ZoomOutMode, ConflictOutcome::None);
                Ok(())
            }
            ZoomLevel::Cluster(_) => {
                self.canvas_state.zoom = ZoomLevel::Overview;
                self.bump_revision(prior, MutationType::ZoomOutMode, ConflictOutcome::None);
                Ok(())
            }
            ZoomLevel::Overview => {
                self.bump_revision(prior, MutationType::ZoomOutMode, ConflictOutcome::None);
                Err(json!({"error":"unsupported_state_combination","reason":"already_in_overview_zoom"}).to_string())
            }
        }
    }

    pub fn cycle_context_strip(
        &mut self,
        direction: ContextStripDirection,
    ) -> Result<WindowId, String> {
        let prior = self.canvas_state.state_revision;
        let focused = match self.canvas_state.zoom {
            ZoomLevel::Focus(id) => id,
            _ => {
                self.bump_revision(
                    prior,
                    MutationType::CycleContextStrip,
                    ConflictOutcome::None,
                );
                return Err(json!({"error":"unsupported_state_combination","reason":"context_strip_requires_focus_zoom"}).to_string());
            }
        };

        let Some(cluster) = self
            .canvas_state
            .clusters
            .iter()
            .find(|c| c.windows.contains(&focused))
        else {
            self.bump_revision(
                prior,
                MutationType::CycleContextStrip,
                ConflictOutcome::MissingCluster,
            );
            return Err(
                json!({"error":"invalid_state","reason":"focused_window_cluster_missing"})
                    .to_string(),
            );
        };
        let mut order = if cluster.recency.is_empty() {
            cluster.windows.clone()
        } else {
            cluster.recency.clone()
        };
        for window_id in &cluster.windows {
            if !order.contains(window_id) {
                order.push(*window_id);
            }
        }
        order.retain(|window_id| *window_id != focused);
        let target = match direction {
            ContextStripDirection::Next => *order.first().ok_or_else(|| {
                json!({"error":"unsupported_state_combination","reason":"context_strip_empty"})
                    .to_string()
            })?,
            ContextStripDirection::Previous => *order.last().ok_or_else(|| {
                json!({"error":"unsupported_state_combination","reason":"context_strip_empty"})
                    .to_string()
            })?,
        };
        self.canvas_state.zoom = ZoomLevel::Focus(target);
        self.focus_freeze = FocusFreezeMetadata {
            frozen: true,
            window_id: Some(target),
            reason: Some("cycle_context_strip".into()),
        };
        self.bump_revision(
            prior,
            MutationType::CycleContextStrip,
            ConflictOutcome::None,
        );
        Ok(target)
    }

    fn bump_revision(&mut self, prior: u64, mutation: MutationType, conflict: ConflictOutcome) {
        self.canvas_state.state_revision = prior.saturating_add(1);
        let next = self.canvas_state.state_revision;
        info!(
            module = "state_store",
            mutation_type = ?mutation,
            prior_revision = prior,
            next_revision = next,
            conflict_outcome = ?conflict,
            selected_cluster_id = self.selected_cluster_id,
            focus_freeze = ?self.focus_freeze,
            "deterministic mutation log"
        );
    }
}

struct SwaySnapshot {
    clusters: Vec<Cluster>,
    windows: Vec<Window>,
    output: OutputState,
}

fn sway_snapshot() -> Result<SwaySnapshot, Box<dyn std::error::Error>> {
    let mut connection = Connection::new()?;
    let tree = connection.get_tree()?;
    let workspaces = connection.get_workspaces()?;
    let outputs = connection.get_outputs()?;

    let mut clusters = Vec::new();
    for workspace in workspaces.into_iter().filter(|w| w.num >= 0) {
        clusters.push(Cluster {
            id: workspace.id as ClusterId,
            name: workspace.name,
            x: workspace.rect.x as f64,
            y: workspace.rect.y as f64,
            enabled: workspace.visible,
            windows: Vec::new(),
            last_focus: None,
            recency: Vec::new(),
        });
    }

    let mut windows = Vec::new();
    collect_windows_from_tree(&tree, None, &mut windows);
    windows.sort_by_key(|window| window.id);

    let mut windows_by_cluster: BTreeMap<ClusterId, Vec<WindowId>> = BTreeMap::new();
    for window in &windows {
        if let Some(cluster_id) = window.cluster_id {
            windows_by_cluster
                .entry(cluster_id)
                .or_default()
                .push(window.id);
        }
    }

    for cluster in &mut clusters {
        cluster.windows = windows_by_cluster.remove(&cluster.id).unwrap_or_default();
        cluster.recency = cluster.windows.clone();
        cluster.last_focus = cluster.windows.first().copied();
    }

    let output = outputs
        .into_iter()
        .find(|output| output.focused)
        .map(|output| OutputState {
            name: output.name,
            width: output.rect.width,
            height: output.rect.height,
            scale: output.scale.unwrap_or(1.0),
        })
        .unwrap_or_default();

    Ok(SwaySnapshot {
        clusters,
        windows,
        output,
    })
}

fn collect_windows_from_tree(
    node: &swayipc::Node,
    cluster: Option<ClusterId>,
    out: &mut Vec<Window>,
) {
    let cluster_id = if matches!(node.node_type, swayipc::NodeType::Workspace) {
        Some(node.id as ClusterId)
    } else {
        cluster
    };

    if matches!(
        node.node_type,
        swayipc::NodeType::Con | swayipc::NodeType::FloatingCon
    ) && node.pid.is_some()
    {
        let title = node.name.clone().unwrap_or_default();
        let app_id = node.app_id.clone();
        let class = node
            .window_properties
            .as_ref()
            .and_then(|props| props.class.clone());
        let transient_for = node
            .window_properties
            .as_ref()
            .and_then(|props| props.transient_for)
            .map(|id| id as WindowId);
        let app_id_lower = app_id.as_deref().map(str::to_ascii_lowercase);
        let class_lower = class.as_deref().map(str::to_ascii_lowercase);
        let title_lower = title.to_ascii_lowercase();
        let has_overlay_hint = ["overlay", "popup"]
            .iter()
            .any(|hint| title_lower.contains(hint))
            || app_id_lower
                .as_deref()
                .is_some_and(|value| value.contains("overlay") || value.contains("popup"))
            || class_lower
                .as_deref()
                .is_some_and(|value| value.contains("overlay") || value.contains("popup"));

        let role = if has_overlay_hint {
            WindowRole::Utility
        } else if node.floating.is_some() || transient_for.is_some() {
            WindowRole::Dialog
        } else {
            WindowRole::Normal
        };

        out.push(Window {
            id: node.id as WindowId,
            title,
            app_id,
            class,
            role,
            state: if node.fullscreen_mode.unwrap_or(0) > 0 {
                WindowState::Fullscreen
            } else if node.floating.is_some() {
                WindowState::Floating
            } else {
                WindowState::Tiled
            },
            cluster_id,
            transient_for,
            manual_cluster_override: false,
            manual_position_override: has_overlay_hint,
        });
    }

    for child in &node.nodes {
        collect_windows_from_tree(child, cluster_id, out);
    }
    for child in &node.floating_nodes {
        collect_windows_from_tree(child, cluster_id, out);
    }
}
