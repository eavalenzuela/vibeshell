use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use common::contracts::{
    CanvasState, Cluster, ClusterId, ContextStripDirection, OutputState, Viewport, Window,
    WindowId, WindowRole, WindowState, ZoomLevel,
};
use common::persistence::{OverviewPersistence, PersistedOverviewState};
use config::schema::AssignmentHint;
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
    SelectCluster,
    SetFocusZoomTarget,
    ZoomInMode,
    ZoomOutMode,
    CycleContextStrip,
    OverviewPan,
    OverviewZoom,
    CreateCluster,
    UpdateClusterDrag,
    CommitClusterDrag,
    CancelClusterDrag,
    EnterKeyboardMoveMode,
    KeyboardMoveBy,
    CommitKeyboardMove,
    CancelKeyboardMove,
    CycleCluster,
}

#[derive(Debug)]
pub struct StateOwner {
    canvas_state: CanvasState,
    selected_cluster_id: Option<ClusterId>,
    focus_freeze: FocusFreezeMetadata,
    persistence: OverviewPersistence,
    boot_persisted: Option<PersistedOverviewState>,
    focused_output: Option<String>,
    assignment_hints: Vec<AssignmentHint>,
    auto_cluster: bool,
    cluster_history: Vec<ClusterId>,
    last_applied_geometry: BTreeMap<WindowId, (i32, i32)>,
    drag_origin: Option<(ClusterId, f64, f64)>,
    keyboard_move_origin: Option<(ClusterId, f64, f64)>,
}

impl StateOwner {
    pub fn new() -> Self {
        let mut canvas_state = CanvasState::default();
        let persistence = OverviewPersistence::with_debounce(Duration::from_millis(250));
        let boot_persisted = persistence.load().ok().flatten();
        if let Some(persisted) = &boot_persisted {
            persisted.apply_to_canvas_seed(&mut canvas_state);
        }
        let config = config::Config::load().unwrap_or_default();
        let assignment_hints = config.continuum.assignment_hints;
        let auto_cluster = config.continuum.auto_cluster;
        let cluster_history = boot_persisted
            .as_ref()
            .map(|p| p.cluster_history.clone())
            .unwrap_or_default();

        Self {
            canvas_state,
            selected_cluster_id: None,
            focus_freeze: FocusFreezeMetadata::default(),
            persistence,
            boot_persisted,
            focused_output: None,
            assignment_hints,
            auto_cluster,
            cluster_history,
            last_applied_geometry: BTreeMap::new(),
            drag_origin: None,
            keyboard_move_origin: None,
        }
    }

    pub fn state(&self) -> CanvasState {
        self.canvas_state.clone()
    }

    pub fn selected_cluster_id(&self) -> Option<ClusterId> {
        self.selected_cluster_id
    }

    pub fn overview_pan(&mut self, dx: f64, dy: f64, output: Option<&str>, link_outputs: bool) {
        let prior = self.canvas_state.state_revision;
        let previous_state = self.canvas_state.clone();
        if link_outputs {
            self.canvas_state.viewport.x += dx;
            self.canvas_state.viewport.y += dy;
            for viewport in self.canvas_state.output_viewports.values_mut() {
                viewport.x += dx;
                viewport.y += dy;
            }
        } else {
            let output_name = output.or(self.focused_output.as_deref());
            let entry = output_name
                .map(|name| {
                    self.canvas_state
                        .output_viewports
                        .entry(name.to_owned())
                        .or_insert_with(|| self.canvas_state.viewport.clone())
                })
                .unwrap_or(&mut self.canvas_state.viewport);
            entry.x += dx;
            entry.y += dy;
            self.canvas_state.viewport = entry.clone();
        }
        self.persist_after_mutation(&previous_state);
        self.bump_revision(
            prior,
            MutationType::OverviewPan,
            ConflictOutcome::PreservedViewport,
        );
    }

    pub fn set_cluster_position_by_name(
        &mut self,
        name: &str,
        x: f64,
        y: f64,
    ) -> Result<(), String> {
        let prior = self.canvas_state.state_revision;
        let previous_state = self.canvas_state.clone();
        if let Some(cluster) = self
            .canvas_state
            .clusters
            .iter_mut()
            .find(|c| c.name == name)
        {
            cluster.x = x;
            cluster.y = y;
            self.persist_after_mutation(&previous_state);
            self.bump_revision(prior, MutationType::CreateCluster, ConflictOutcome::None);
            Ok(())
        } else {
            self.bump_revision(
                prior,
                MutationType::CreateCluster,
                ConflictOutcome::MissingCluster,
            );
            Err(json!({"error":"cluster_not_found_after_create","name":name}).to_string())
        }
    }

    pub fn overview_zoom(
        &mut self,
        delta: f64,
        anchor_canvas_x: f64,
        anchor_canvas_y: f64,
        output: Option<&str>,
        link_outputs: bool,
    ) {
        const MIN_SCALE: f64 = 0.35;
        const MAX_SCALE: f64 = 2.50;
        const STEP: f64 = 1.12;

        let prior = self.canvas_state.state_revision;
        let previous_state = self.canvas_state.clone();
        let factor = if delta < 0.0 { 1.0 / STEP } else { STEP };
        if link_outputs {
            apply_zoom_to_viewport(
                &mut self.canvas_state.viewport,
                factor,
                anchor_canvas_x,
                anchor_canvas_y,
                MIN_SCALE,
                MAX_SCALE,
            );
            for viewport in self.canvas_state.output_viewports.values_mut() {
                apply_zoom_to_viewport(
                    viewport,
                    factor,
                    anchor_canvas_x,
                    anchor_canvas_y,
                    MIN_SCALE,
                    MAX_SCALE,
                );
            }
        } else {
            let output_name = output.or(self.focused_output.as_deref());
            let entry = output_name
                .map(|name| {
                    self.canvas_state
                        .output_viewports
                        .entry(name.to_owned())
                        .or_insert_with(|| self.canvas_state.viewport.clone())
                })
                .unwrap_or(&mut self.canvas_state.viewport);
            apply_zoom_to_viewport(
                entry,
                factor,
                anchor_canvas_x,
                anchor_canvas_y,
                MIN_SCALE,
                MAX_SCALE,
            );
            self.canvas_state.viewport = entry.clone();
        }
        self.persist_after_mutation(&previous_state);
        self.bump_revision(
            prior,
            MutationType::OverviewZoom,
            ConflictOutcome::PreservedViewport,
        );
    }

    pub fn ingest_sway_facts(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let prior = self.canvas_state.state_revision;
        let previous_state = self.canvas_state.clone();
        let previous_zoom = self.canvas_state.zoom.clone();
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
        let previous_focused_output = self.focused_output.clone();
        self.canvas_state.clusters = clusters;

        // Mark manually resized windows: if a tiled window's geometry diverged
        // from the last known geometry by >10px, flag it as manual_position_override.
        let mut windows = snapshot.windows;
        if !self.last_applied_geometry.is_empty() {
            for window in &mut windows {
                if window.manual_position_override || window.state != WindowState::Tiled {
                    continue;
                }
                if let Some(&(prev_w, prev_h)) = self.last_applied_geometry.get(&window.id) {
                    if let Some(&(cur_w, cur_h)) = snapshot.window_geometry.get(&window.id) {
                        if (cur_w - prev_w).abs() > 10 || (cur_h - prev_h).abs() > 10 {
                            tracing::debug!(
                                window_id = window.id,
                                prev_w,
                                prev_h,
                                cur_w,
                                cur_h,
                                "marking manually resized window"
                            );
                            window.manual_position_override = true;
                        }
                    }
                }
            }
        }
        self.last_applied_geometry = snapshot.window_geometry;

        self.canvas_state.windows = windows;
        self.canvas_state.output = snapshot.output.clone();
        self.focused_output = Some(snapshot.output.name.clone());
        self.canvas_state.viewport = previous_viewport;

        if let Some(persisted) = &self.boot_persisted {
            persisted.merge_into_live_canvas(&mut self.canvas_state);
        }
        self.apply_assignment_hints();
        if self.auto_cluster {
            self.auto_cluster_by_app_id();
        }
        self.anchor_transient_dialogs();

        let connected_outputs = snapshot.outputs.iter().cloned().collect::<HashSet<_>>();
        self.canvas_state
            .output_viewports
            .retain(|name, _| connected_outputs.contains(name));
        if let Some(current) = self.focused_output.clone() {
            let default_viewport = self.canvas_state.viewport.clone();
            let viewport = self
                .canvas_state
                .output_viewports
                .entry(current)
                .or_insert(default_viewport)
                .clone();
            self.canvas_state.viewport = viewport;
        }

        if let Some(previous_output) = previous_focused_output {
            if !connected_outputs.contains(&previous_output) {
                if let Some(primary) = snapshot.primary_output.clone() {
                    if let Some(old_viewport) =
                        self.canvas_state.output_viewports.remove(&previous_output)
                    {
                        self.canvas_state
                            .output_viewports
                            .entry(primary.clone())
                            .or_insert(old_viewport);
                    }
                    if self.focused_output.as_deref() != Some(primary.as_str()) {
                        self.focused_output = Some(primary.clone());
                    }
                    let fallback = self
                        .canvas_state
                        .output_viewports
                        .get(&primary)
                        .cloned()
                        .unwrap_or_else(|| self.canvas_state.viewport.clone());
                    self.canvas_state.viewport = fallback;
                }
            }
        }
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

        let keep_zoom = match &self.canvas_state.zoom {
            ZoomLevel::Cluster(id) => self
                .canvas_state
                .windows
                .iter()
                .any(|w| w.cluster_id == Some(*id) && w.state == WindowState::Fullscreen),
            ZoomLevel::Focus(wid) => self
                .canvas_state
                .windows
                .iter()
                .any(|w| w.id == *wid && w.state == WindowState::Fullscreen),
            ZoomLevel::Overview => false,
        };
        if keep_zoom {
            self.canvas_state.zoom = previous_zoom;
        }

        self.persist_after_mutation(&previous_state);
        self.bump_revision(prior, MutationType::SwayIngest, outcome);
        Ok(())
    }

    pub fn mutate_get_state(&mut self) {
        let prior = self.canvas_state.state_revision;
        self.bump_revision(prior, MutationType::GetState, ConflictOutcome::None);
    }

    pub fn activate_cluster(&mut self, cluster_id: ClusterId) -> Result<(), String> {
        let prior = self.canvas_state.state_revision;
        let previous_state = self.canvas_state.clone();
        if self
            .canvas_state
            .clusters
            .iter()
            .any(|cluster| cluster.id == cluster_id)
        {
            self.selected_cluster_id = Some(cluster_id);
            self.canvas_state.zoom = ZoomLevel::Cluster(cluster_id);
            self.focus_freeze = FocusFreezeMetadata::default();
            self.cluster_history.retain(|&id| id != cluster_id);
            self.cluster_history.insert(0, cluster_id);
            self.persist_after_mutation(&previous_state);
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
        let previous_state = self.canvas_state.clone();
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
                self.persist_after_mutation(&previous_state);
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
                self.persist_after_mutation(&previous_state);
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
        let previous_state = self.canvas_state.clone();
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
                self.persist_after_mutation(&previous_state);
                self.bump_revision(prior, MutationType::ZoomOutMode, ConflictOutcome::None);
                Ok(())
            }
            ZoomLevel::Cluster(_) => {
                self.canvas_state.zoom = ZoomLevel::Overview;
                self.persist_after_mutation(&previous_state);
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

    pub fn flush_pending_persistence(&mut self) {
        if let Err(error) = self.persistence.flush_pending() {
            tracing::warn!(?error, "failed to flush pending persisted overview state");
        }
    }

    pub fn begin_cluster_drag(&mut self, cluster_id: ClusterId) {
        self.drag_origin = self
            .canvas_state
            .clusters
            .iter()
            .find(|c| c.id == cluster_id)
            .map(|c| (cluster_id, c.x, c.y));
    }

    pub fn update_cluster_drag(&mut self, cluster_x: f64, cluster_y: f64) {
        let Some((cluster_id, _, _)) = self.drag_origin else {
            return;
        };
        let prior = self.canvas_state.state_revision;
        if let Some(cluster) = self
            .canvas_state
            .clusters
            .iter_mut()
            .find(|c| c.id == cluster_id)
        {
            cluster.x = cluster_x;
            cluster.y = cluster_y;
        }
        self.bump_revision(
            prior,
            MutationType::UpdateClusterDrag,
            ConflictOutcome::None,
        );
    }

    pub fn commit_cluster_drag(&mut self) {
        let prior = self.canvas_state.state_revision;
        if let Err(e) = self.persistence.persist_immediate(&self.canvas_state) {
            tracing::warn!(?e, "commit_cluster_drag: persist failed");
        } else {
            self.update_boot_persisted();
        }
        self.drag_origin = None;
        self.bump_revision(
            prior,
            MutationType::CommitClusterDrag,
            ConflictOutcome::None,
        );
    }

    pub fn cancel_cluster_drag(&mut self) {
        let prior = self.canvas_state.state_revision;
        if let Some((cluster_id, origin_x, origin_y)) = self.drag_origin.take() {
            if let Some(cluster) = self
                .canvas_state
                .clusters
                .iter_mut()
                .find(|c| c.id == cluster_id)
            {
                cluster.x = origin_x;
                cluster.y = origin_y;
            }
        }
        self.bump_revision(
            prior,
            MutationType::CancelClusterDrag,
            ConflictOutcome::None,
        );
    }

    pub fn select_cluster(&mut self, cluster_id: ClusterId) -> Result<(), String> {
        let prior = self.canvas_state.state_revision;
        if self
            .canvas_state
            .clusters
            .iter()
            .any(|c| c.id == cluster_id)
        {
            self.selected_cluster_id = Some(cluster_id);
            self.bump_revision(prior, MutationType::SelectCluster, ConflictOutcome::None);
            Ok(())
        } else {
            self.bump_revision(
                prior,
                MutationType::SelectCluster,
                ConflictOutcome::MissingCluster,
            );
            Err(
                json!({"error":"invalid_state","reason":"cluster_not_found","cluster":cluster_id})
                    .to_string(),
            )
        }
    }

    pub fn enter_keyboard_move_mode(&mut self, cluster_id: ClusterId) {
        self.keyboard_move_origin = self
            .canvas_state
            .clusters
            .iter()
            .find(|c| c.id == cluster_id)
            .map(|c| (cluster_id, c.x, c.y));
        let prior = self.canvas_state.state_revision;
        self.bump_revision(
            prior,
            MutationType::EnterKeyboardMoveMode,
            ConflictOutcome::None,
        );
    }

    pub fn keyboard_move_by(&mut self, dx: f64, dy: f64) {
        let Some((cluster_id, _, _)) = self.keyboard_move_origin else {
            return;
        };
        let prior = self.canvas_state.state_revision;
        if let Some(cluster) = self
            .canvas_state
            .clusters
            .iter_mut()
            .find(|c| c.id == cluster_id)
        {
            cluster.x += dx;
            cluster.y += dy;
        }
        self.bump_revision(prior, MutationType::KeyboardMoveBy, ConflictOutcome::None);
    }

    pub fn commit_keyboard_move(&mut self) {
        let prior = self.canvas_state.state_revision;
        if let Err(e) = self.persistence.persist_immediate(&self.canvas_state) {
            tracing::warn!(?e, "commit_keyboard_move: persist failed");
        } else {
            self.update_boot_persisted();
        }
        self.keyboard_move_origin = None;
        self.bump_revision(
            prior,
            MutationType::CommitKeyboardMove,
            ConflictOutcome::None,
        );
    }

    pub fn cancel_keyboard_move(&mut self) {
        let prior = self.canvas_state.state_revision;
        if let Some((cluster_id, origin_x, origin_y)) = self.keyboard_move_origin.take() {
            if let Some(cluster) = self
                .canvas_state
                .clusters
                .iter_mut()
                .find(|c| c.id == cluster_id)
            {
                cluster.x = origin_x;
                cluster.y = origin_y;
            }
        }
        self.bump_revision(
            prior,
            MutationType::CancelKeyboardMove,
            ConflictOutcome::None,
        );
    }

    pub fn move_window_to_cluster(
        &mut self,
        window_id: WindowId,
        cluster_id: ClusterId,
    ) -> Result<(), String> {
        let prior = self.canvas_state.state_revision;
        let previous_state = self.canvas_state.clone();
        if !self
            .canvas_state
            .clusters
            .iter()
            .any(|c| c.id == cluster_id)
        {
            return Err(
                serde_json::json!({"error":"cluster_not_found","cluster":cluster_id}).to_string(),
            );
        }
        let window = self
            .canvas_state
            .windows
            .iter_mut()
            .find(|w| w.id == window_id)
            .ok_or_else(|| {
                serde_json::json!({"error":"window_not_found","window":window_id}).to_string()
            })?;
        window.cluster_id = Some(cluster_id);
        window.manual_cluster_override = true;
        self.persist_after_mutation(&previous_state);
        self.bump_revision(prior, MutationType::SwayIngest, ConflictOutcome::None);
        Ok(())
    }

    pub fn rename_cluster(&mut self, cluster_id: ClusterId, name: &str) -> Result<(), String> {
        let prior = self.canvas_state.state_revision;
        let previous_state = self.canvas_state.clone();
        let cluster = self
            .canvas_state
            .clusters
            .iter_mut()
            .find(|c| c.id == cluster_id)
            .ok_or_else(|| {
                serde_json::json!({"error":"cluster_not_found","cluster":cluster_id}).to_string()
            })?;
        cluster.name = name.to_owned();
        self.persist_after_mutation(&previous_state);
        self.bump_revision(prior, MutationType::SwayIngest, ConflictOutcome::None);
        Ok(())
    }

    pub fn cycle_cluster(
        &mut self,
        direction: common::contracts::CycleDirection,
    ) -> Result<ClusterId, String> {
        let prior = self.canvas_state.state_revision;
        let previous_state = self.canvas_state.clone();

        // Prune history to only valid clusters
        let valid_ids: HashSet<ClusterId> =
            self.canvas_state.clusters.iter().map(|c| c.id).collect();
        self.cluster_history.retain(|id| valid_ids.contains(id));

        if self.cluster_history.len() <= 1 {
            self.bump_revision(prior, MutationType::CycleCluster, ConflictOutcome::None);
            return Err(
                serde_json::json!({"error":"no_cycle_target","reason":"history_too_short"})
                    .to_string(),
            );
        }

        let current = self.selected_cluster_id.unwrap_or(0);
        let pos = self
            .cluster_history
            .iter()
            .position(|&id| id == current)
            .unwrap_or(0);

        let len = self.cluster_history.len();
        let next_pos = match direction {
            common::contracts::CycleDirection::Forward => (pos + 1) % len,
            common::contracts::CycleDirection::Backward => (pos + len - 1) % len,
        };
        let target = self.cluster_history[next_pos];

        self.selected_cluster_id = Some(target);
        self.canvas_state.zoom = ZoomLevel::Cluster(target);
        self.focus_freeze = FocusFreezeMetadata::default();
        self.persist_after_mutation(&previous_state);
        self.bump_revision(prior, MutationType::CycleCluster, ConflictOutcome::None);
        Ok(target)
    }

    fn update_boot_persisted(&mut self) {
        let mut persisted = PersistedOverviewState::from_canvas(&self.canvas_state);
        persisted.cluster_history = self.cluster_history.clone();
        self.boot_persisted = Some(persisted);
    }

    fn persist_after_mutation(&mut self, previous: &CanvasState) {
        let changed_positions = cluster_position_or_metadata_changed(previous, &self.canvas_state);
        let changed_assignments = window_assignment_changed(previous, &self.canvas_state);
        let changed_viewport = previous.viewport != self.canvas_state.viewport;
        let changed_zoom = previous.zoom != self.canvas_state.zoom;

        if changed_positions || changed_assignments || changed_zoom {
            if let Err(error) = self.persistence.persist_immediate(&self.canvas_state) {
                tracing::warn!(?error, path=?self.persistence.path(), "failed to persist overview state immediately");
            } else {
                self.update_boot_persisted();
            }
            return;
        }

        if changed_viewport {
            self.persistence.persist_debounced(&self.canvas_state);
            if let Err(error) = self.persistence.flush_due() {
                tracing::warn!(?error, path=?self.persistence.path(), "failed to flush debounced overview state");
            }
        }
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

    fn apply_assignment_hints(&mut self) {
        let cluster_by_name: BTreeMap<String, ClusterId> = self
            .canvas_state
            .clusters
            .iter()
            .map(|c| (c.name.to_ascii_lowercase(), c.id))
            .collect();
        let hints = self.assignment_hints.clone();
        for window in &mut self.canvas_state.windows {
            if window.manual_cluster_override || window.transient_for.is_some() {
                continue;
            }
            let app_id_lower = window.app_id.as_deref().map(str::to_ascii_lowercase);
            let class_lower = window.class.as_deref().map(str::to_ascii_lowercase);
            let title_lower = window.title.to_ascii_lowercase();
            for hint in &hints {
                let has_criterion =
                    hint.app_id.is_some() || hint.class.is_some() || hint.title_contains.is_some();
                if !has_criterion {
                    continue;
                }
                let app_match = hint.app_id.as_ref().is_none_or(|h| {
                    app_id_lower.as_deref() == Some(h.to_ascii_lowercase().as_str())
                });
                let class_match = hint.class.as_ref().is_none_or(|h| {
                    class_lower.as_deref() == Some(h.to_ascii_lowercase().as_str())
                });
                let title_match = hint
                    .title_contains
                    .as_ref()
                    .is_none_or(|h| title_lower.contains(h.to_ascii_lowercase().as_str()));
                if app_match && class_match && title_match {
                    let cluster_name_lower = hint.cluster.to_ascii_lowercase();
                    if let Some(&cluster_id) = cluster_by_name.get(&cluster_name_lower) {
                        window.cluster_id = Some(cluster_id);
                    }
                    break;
                }
            }
        }
    }

    fn auto_cluster_by_app_id(&mut self) {
        let mut app_id_to_cluster: BTreeMap<String, ClusterId> = BTreeMap::new();
        for window in &self.canvas_state.windows {
            if let (Some(app_id), Some(cluster_id)) = (&window.app_id, window.cluster_id) {
                app_id_to_cluster
                    .entry(app_id.to_ascii_lowercase())
                    .or_insert(cluster_id);
            }
        }
        for window in &mut self.canvas_state.windows {
            if window.cluster_id.is_some()
                || window.manual_cluster_override
                || window.transient_for.is_some()
            {
                continue;
            }
            if let Some(app_id) = &window.app_id {
                if let Some(&cluster_id) = app_id_to_cluster.get(&app_id.to_ascii_lowercase()) {
                    tracing::debug!(
                        window_id = window.id,
                        app_id = app_id.as_str(),
                        cluster_id,
                        "auto-clustering window by app_id"
                    );
                    window.cluster_id = Some(cluster_id);
                }
            }
        }
    }

    fn anchor_transient_dialogs(&mut self) {
        let window_to_cluster: BTreeMap<WindowId, ClusterId> = self
            .canvas_state
            .windows
            .iter()
            .filter_map(|w| w.cluster_id.map(|c| (w.id, c)))
            .collect();
        for window in &mut self.canvas_state.windows {
            if window.manual_cluster_override {
                continue;
            }
            let Some(parent_id) = window.transient_for else {
                continue;
            };
            let Some(&parent_cluster) = window_to_cluster.get(&parent_id) else {
                continue;
            };
            if window.cluster_id != Some(parent_cluster) {
                tracing::debug!(
                    window_id = window.id,
                    parent_id,
                    from_cluster = ?window.cluster_id,
                    to_cluster = parent_cluster,
                    "anchoring transient dialog to parent cluster"
                );
                window.cluster_id = Some(parent_cluster);
            }
        }
    }
}

fn cluster_position_or_metadata_changed(previous: &CanvasState, next: &CanvasState) -> bool {
    let previous_clusters = previous
        .clusters
        .iter()
        .map(|cluster| (cluster.id, (&cluster.name, cluster.x, cluster.y)))
        .collect::<BTreeMap<_, _>>();
    let next_clusters = next
        .clusters
        .iter()
        .map(|cluster| (cluster.id, (&cluster.name, cluster.x, cluster.y)))
        .collect::<BTreeMap<_, _>>();
    previous_clusters != next_clusters
}

fn window_assignment_changed(previous: &CanvasState, next: &CanvasState) -> bool {
    let previous_assignments = previous
        .windows
        .iter()
        .map(|window| {
            (
                window.id,
                (window.cluster_id, window.manual_cluster_override),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let next_assignments = next
        .windows
        .iter()
        .map(|window| {
            (
                window.id,
                (window.cluster_id, window.manual_cluster_override),
            )
        })
        .collect::<BTreeMap<_, _>>();
    previous_assignments != next_assignments
}

struct SwaySnapshot {
    clusters: Vec<Cluster>,
    windows: Vec<Window>,
    window_geometry: BTreeMap<WindowId, (i32, i32)>,
    output: OutputState,
    outputs: Vec<String>,
    primary_output: Option<String>,
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
    let mut window_geometry = BTreeMap::new();
    collect_windows_from_tree(&tree, None, &mut windows, &mut window_geometry);
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

    let primary_output = outputs
        .iter()
        .find(|output| output.primary)
        .map(|output| output.name.clone());
    let connected_output_names = outputs
        .iter()
        .map(|output| output.name.clone())
        .collect::<Vec<_>>();
    let output = outputs
        .iter()
        .find(|output| output.focused)
        .map(|output| OutputState {
            name: output.name.clone(),
            width: output.rect.width,
            height: output.rect.height,
            scale: output.scale.unwrap_or(1.0),
        })
        .unwrap_or_default();

    Ok(SwaySnapshot {
        clusters,
        windows,
        window_geometry,
        output,
        outputs: connected_output_names,
        primary_output,
    })
}

fn apply_zoom_to_viewport(
    viewport: &mut Viewport,
    factor: f64,
    anchor_canvas_x: f64,
    anchor_canvas_y: f64,
    min_scale: f64,
    max_scale: f64,
) {
    let old_scale = viewport.scale.max(min_scale);
    let new_scale = (old_scale * factor).clamp(min_scale, max_scale);
    if (new_scale - old_scale).abs() < f64::EPSILON {
        return;
    }
    viewport.x = anchor_canvas_x - ((anchor_canvas_x - viewport.x) * (old_scale / new_scale));
    viewport.y = anchor_canvas_y - ((anchor_canvas_y - viewport.y) * (old_scale / new_scale));
    viewport.scale = new_scale;
}

fn collect_windows_from_tree(
    node: &swayipc::Node,
    cluster: Option<ClusterId>,
    out: &mut Vec<Window>,
    geometry_out: &mut BTreeMap<WindowId, (i32, i32)>,
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

        let window_id = node.id as WindowId;
        geometry_out.insert(window_id, (node.rect.width, node.rect.height));
        out.push(Window {
            id: window_id,
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
            manual_position_override: has_overlay_hint || node.fullscreen_mode.unwrap_or(0) > 0,
        });
    }

    for child in &node.nodes {
        collect_windows_from_tree(child, cluster_id, out, geometry_out);
    }
    for child in &node.floating_nodes {
        collect_windows_from_tree(child, cluster_id, out, geometry_out);
    }
}
