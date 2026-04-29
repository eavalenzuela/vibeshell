//! Sway impl of `wm::WmBackend`.
//!
//! Owns a `swayipc::Connection`. Translates `swayipc` tree/workspaces/outputs
//! into the backend-neutral `WmFacts` shape the daemon ingests.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use common::contracts::{
    Cluster, ClusterId, OutputState, Window, WindowId, WindowRole, WindowState,
};
use swayipc::{Connection, EventType};
use wm::backend::{BackendError, WmBackend, WmSignal};
use wm::facts::WmFacts;
use wm::layout::LayoutOp;

pub struct SwayBackend {
    connection: Connection,
}

impl SwayBackend {
    pub fn connect() -> Result<Self, BackendError> {
        let connection = Connection::new()
            .map_err(|e| BackendError::Unavailable(format!("sway IPC unreachable: {e}")))?;
        Ok(Self { connection })
    }

    fn run(&mut self, command: &str) -> Result<(), BackendError> {
        let replies = self
            .connection
            .run_command(command)
            .map_err(|e| BackendError::Other(format!("run_command(`{command}`): {e}")))?;
        for reply in replies {
            if let Err(e) = reply {
                return Err(BackendError::CommandRejected {
                    command: command.to_owned(),
                    reason: e.to_string(),
                });
            }
        }
        Ok(())
    }
}

impl WmBackend for SwayBackend {
    fn snapshot(&mut self) -> Result<WmFacts, BackendError> {
        sway_snapshot(&mut self.connection)
    }

    fn apply_layout_ops(&mut self, ops: &[LayoutOp]) -> Result<(), BackendError> {
        let Some(batch) = wm::layout::LayoutEngine::apply(ops) else {
            return Ok(());
        };
        let replies = self
            .connection
            .run_command(&batch)
            .map_err(|e| BackendError::Other(format!("apply_layout_ops: {e}")))?;
        for reply in replies {
            if let Err(e) = reply {
                tracing::warn!(?e, "sway rejected layout op in batch");
            }
        }
        Ok(())
    }

    fn focus_window(&mut self, window: WindowId) -> Result<(), BackendError> {
        self.run(&format!("[con_id={window}] focus"))
    }

    fn activate_cluster(&mut self, cluster: ClusterId) -> Result<(), BackendError> {
        let workspaces = self
            .connection
            .get_workspaces()
            .map_err(|e| BackendError::Other(format!("get_workspaces: {e}")))?;
        let workspace = workspaces
            .into_iter()
            .find(|workspace| workspace.id as ClusterId == cluster)
            .ok_or_else(|| BackendError::Other(format!("cluster {cluster} not found")))?;
        let command = if workspace.num >= 0 {
            format!("workspace number {}", workspace.num)
        } else {
            let escaped = workspace.name.replace('"', "\\\"");
            format!("workspace \"{escaped}\"")
        };
        self.run(&command)
    }

    fn create_named_workspace(&mut self, name: &str) -> Result<(), BackendError> {
        let escaped = name.replace('"', "\\\"");
        self.run(&format!("workspace \"{escaped}\""))
    }

    fn back_and_forth_workspace(&mut self) -> Result<(), BackendError> {
        self.run("workspace back_and_forth")
    }

    fn exit_session(&mut self) -> Result<(), BackendError> {
        self.run("exit")
    }

    fn reload_wm_config(&mut self) -> Result<(), BackendError> {
        self.run("reload")
    }

    fn focused_window(&mut self) -> Result<Option<WindowId>, BackendError> {
        let tree = self
            .connection
            .get_tree()
            .map_err(|e| BackendError::Other(format!("get_tree: {e}")))?;
        Ok(find_focused_window_id(&tree))
    }

    fn is_alive(&mut self) -> bool {
        self.connection.get_version().is_ok()
    }

    fn spawn_event_stream(&self) -> Result<Receiver<WmSignal>, BackendError> {
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let connection = match Connection::new() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(?e, "sway event stream: failed to connect");
                    return;
                }
            };
            let events = match connection.subscribe([EventType::Workspace, EventType::Window]) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(?e, "sway event stream: failed to subscribe");
                    return;
                }
            };
            for event in events {
                if let Err(e) = event {
                    tracing::warn!(?e, "sway event stream: error");
                    break;
                }
                if tx.send(WmSignal::WorkspaceOrWindow).is_err() {
                    break;
                }
            }
        });

        Ok(rx)
    }
}

/// Build a backend-neutral `WmFacts` snapshot from a live sway connection.
///
/// Replaces the previous `sway_snapshot()` that lived in
/// `apps/vibeshellctl/src/state_store.rs`.
fn sway_snapshot(connection: &mut Connection) -> Result<WmFacts, BackendError> {
    let tree = connection
        .get_tree()
        .map_err(|e| BackendError::Other(format!("get_tree: {e}")))?;
    let workspaces = connection
        .get_workspaces()
        .map_err(|e| BackendError::Other(format!("get_workspaces: {e}")))?;
    let outputs = connection
        .get_outputs()
        .map_err(|e| BackendError::Other(format!("get_outputs: {e}")))?;

    let mut clusters = Vec::new();
    for workspace in workspaces
        .into_iter()
        .filter(|w| !w.name.starts_with("__i3"))
    {
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
    collect_windows_from_tree(&tree, None, false, &mut windows, &mut window_geometry);
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

    Ok(WmFacts {
        clusters,
        windows,
        window_geometry,
        output,
        outputs: connected_output_names,
        primary_output,
    })
}

fn collect_windows_from_tree(
    node: &swayipc::Node,
    cluster: Option<ClusterId>,
    in_scratchpad_workspace: bool,
    out: &mut Vec<Window>,
    geometry_out: &mut BTreeMap<WindowId, (i32, i32)>,
) {
    let under_scratch_ws = in_scratchpad_workspace
        || (matches!(node.node_type, swayipc::NodeType::Workspace)
            && node.name.as_deref().is_some_and(|n| n.starts_with("__i3")));
    let cluster_id = if matches!(node.node_type, swayipc::NodeType::Workspace) {
        if under_scratch_ws {
            None
        } else {
            Some(node.id as ClusterId)
        }
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

        let is_scratchpad = under_scratch_ws
            || matches!(
                node.scratchpad_state,
                Some(swayipc::ScratchpadState::Fresh) | Some(swayipc::ScratchpadState::Changed)
            );

        let role = if is_scratchpad {
            WindowRole::Scratchpad
        } else if has_overlay_hint {
            WindowRole::Utility
        } else if node.floating.is_some() || transient_for.is_some() {
            WindowRole::Dialog
        } else {
            WindowRole::Normal
        };

        let effective_cluster_id = if is_scratchpad { None } else { cluster_id };

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
            } else if is_scratchpad {
                WindowState::MinimizedLike
            } else if node.floating.is_some() {
                WindowState::Floating
            } else {
                WindowState::Tiled
            },
            cluster_id: effective_cluster_id,
            transient_for,
            manual_cluster_override: false,
            manual_position_override: is_scratchpad
                || has_overlay_hint
                || node.fullscreen_mode.unwrap_or(0) > 0,
        });
    }

    for child in &node.nodes {
        collect_windows_from_tree(child, cluster_id, under_scratch_ws, out, geometry_out);
    }
    for child in &node.floating_nodes {
        collect_windows_from_tree(child, cluster_id, under_scratch_ws, out, geometry_out);
    }
}

fn find_focused_window_id(node: &swayipc::Node) -> Option<WindowId> {
    if node.focused && node.pid.is_some() {
        return Some(node.id as WindowId);
    }
    for child in &node.nodes {
        if let Some(id) = find_focused_window_id(child) {
            return Some(id);
        }
    }
    for child in &node.floating_nodes {
        if let Some(id) = find_focused_window_id(child) {
            return Some(id);
        }
    }
    None
}
