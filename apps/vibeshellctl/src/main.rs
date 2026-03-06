use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use clap::{Parser, Subcommand, ValueEnum};
use common::contracts::{
    CanvasState, ClusterId, ContextStripDirection, IpcRequest, IpcResponse, OutputState, Viewport,
    Window, WindowId, ZoomLevel,
};
use serde::Serialize;
use serde_json::json;
use tracing::{info, warn};

use swayipc::{Connection, EventType, Node};

mod state_store;
use state_store::with_state_owner;

#[derive(Debug, Parser)]
#[command(
    name = "vibeshellctl",
    about = "Control vibeshell development components"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Ask Sway to reload its configuration.
    Reload,
    /// Show whether Sway and shell components are running.
    Status,
    /// Restart a specific vibeshell component.
    Restart { component: Component },
    /// Print component logs from a captured nested-session log file.
    Logs { component: Component },
    /// End the current Sway session and return to the display manager login screen.
    Logout,
    /// Print daemon-like continuum state in JSON.
    DumpState {
        /// Pretty-print JSON for humans.
        #[arg(long)]
        pretty: bool,
    },
    /// Send an IPC request to vibeshell and print the JSON response.
    Ipc {
        #[command(subcommand)]
        command: IpcCommands,
    },
}

#[derive(Debug, Subcommand)]
enum IpcCommands {
    /// Read state via GetState IPC.
    GetState {
        /// Pretty-print JSON for humans.
        #[arg(long)]
        pretty: bool,
    },
    /// Activate/select a cluster via SetZoom IPC.
    ActivateCluster { cluster: ClusterId },
    /// Set the focus zoom target to a concrete window id.
    SetFocusZoomTarget { window: WindowId },
    /// Transition one step deeper in zoom mode (overview→cluster→focus).
    ZoomInMode,
    /// Transition one step out in zoom mode (focus→cluster→overview).
    ZoomOutMode,
    /// Cycle to the next window in the context strip while in focus zoom.
    CycleStripForward,
    /// Cycle to the previous window in the context strip while in focus zoom.
    CycleStripBackward,
    /// Cycle to the next window in the context strip while in focus zoom.
    CycleContextStripNext,
    /// Cycle to the previous window in the context strip while in focus zoom.
    CycleContextStripPrevious,
    /// Pan the overview viewport by (dx, dy).
    OverviewPan { dx: f64, dy: f64 },
    /// Zoom the overview viewport by delta at anchor position.
    OverviewZoom {
        delta: f64,
        anchor_x: f64,
        anchor_y: f64,
    },
    /// Create a new cluster with the given name at canvas position (x, y).
    CreateCluster { name: String, x: f64, y: f64 },
    /// Begin dragging a cluster (records drag origin).
    BeginClusterDrag {
        cluster_id: u64,
        pointer_canvas_x: f64,
        pointer_canvas_y: f64,
        base_revision: u64,
    },
    /// Update the dragged cluster's canvas position.
    UpdateClusterDrag { cluster_x: f64, cluster_y: f64 },
    /// Commit the cluster drag, persisting the new position.
    CommitClusterDrag,
    /// Cancel the cluster drag, reverting to the original position.
    CancelClusterDrag,
    /// Select a cluster in the overview without changing zoom level.
    SelectCluster { cluster_id: ClusterId },
    /// Begin keyboard move mode for the given cluster.
    EnterKeyboardMoveMode { cluster_id: ClusterId },
    /// Move the keyboard-moved cluster by (dx, dy) world units.
    KeyboardMoveBy { dx: f64, dy: f64 },
    /// Commit the keyboard move, persisting the new position.
    CommitKeyboardMove,
    /// Cancel the keyboard move, reverting to the original position.
    CancelKeyboardMove,
}

static FOCUS_HANDOFF: OnceLock<Mutex<FocusHandoffController>> = OnceLock::new();

#[derive(Debug, Default)]
struct FocusHandoffController {
    pre_overview_focus: Option<WindowId>,
    frozen_in_overview: bool,
}

#[derive(Debug, Clone, Copy)]
enum FocusPlan {
    None,
    ActivateCluster(ClusterId),
    FocusWindow(WindowId),
}

#[derive(Clone, Debug, ValueEnum)]
enum Component {
    Panel,
    Launcher,
    Notifd,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum IpcRequestType {
    GetState,
    SetFocusZoomTarget,
    ZoomInMode,
    ZoomOutMode,
    CycleContextStrip,
}

#[derive(Debug, Serialize)]
struct DumpState {
    active_zoom: ZoomLevel,
    active_cluster: Option<ClusterId>,
    clusters: Vec<ClusterDump>,
    windows: Vec<Window>,
    output_viewport: OutputViewport,
}

#[derive(Debug, Serialize)]
struct ClusterDump {
    id: ClusterId,
    name: String,
    x: f64,
    y: f64,
    window_ids: Vec<WindowId>,
}

#[derive(Debug, Serialize)]
struct OutputViewport {
    output: OutputState,
    viewport: Viewport,
}

impl Component {
    fn process_name(&self) -> &'static str {
        match self {
            Self::Panel => "panel",
            Self::Launcher => "launcher",
            Self::Notifd => "notifd",
        }
    }

    fn default_start_command(&self) -> &'static str {
        match self {
            Self::Panel => "cargo run -p panel",
            Self::Launcher => "cargo run -p launcher",
            Self::Notifd => "cargo run -p notifd",
        }
    }

    fn env_command_key(&self) -> &'static str {
        match self {
            Self::Panel => "VIBESHELL_PANEL_CMD",
            Self::Launcher => "VIBESHELL_LAUNCHER_CMD",
            Self::Notifd => "VIBESHELL_NOTIFD_CMD",
        }
    }

    fn log_target(&self) -> &'static str {
        self.process_name()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    common::init_logging("vibeshellctl");

    let cli = Cli::parse();
    match cli.command {
        Commands::Reload => reload()?,
        Commands::Status => status()?,
        Commands::Restart { component } => restart(component)?,
        Commands::Logs { component } => logs(component)?,
        Commands::Logout => logout()?,
        Commands::DumpState { pretty } => dump_state(pretty)?,
        Commands::Ipc { command } => ipc(command)?,
    }

    with_state_owner(|owner| owner.flush_pending_persistence());
    Ok(())
}

fn ipc(command: IpcCommands) -> Result<(), Box<dyn std::error::Error>> {
    let (request, pretty) = match command {
        IpcCommands::GetState { pretty } => (IpcRequest::GetState, pretty),
        IpcCommands::ActivateCluster { cluster } => (
            IpcRequest::SetZoom {
                level: ZoomLevel::Cluster(cluster),
            },
            false,
        ),
        IpcCommands::SetFocusZoomTarget { window } => {
            (IpcRequest::SetFocusZoomTarget { window }, false)
        }
        IpcCommands::ZoomInMode => (IpcRequest::ZoomInMode, false),
        IpcCommands::ZoomOutMode => (IpcRequest::ZoomOutMode, false),
        IpcCommands::CycleStripForward => (IpcRequest::CycleStripForward, false),
        IpcCommands::CycleStripBackward => (IpcRequest::CycleStripBackward, false),
        IpcCommands::CycleContextStripNext => (
            IpcRequest::CycleContextStrip {
                direction: ContextStripDirection::Next,
            },
            false,
        ),
        IpcCommands::CycleContextStripPrevious => (
            IpcRequest::CycleContextStrip {
                direction: ContextStripDirection::Previous,
            },
            false,
        ),
        IpcCommands::OverviewPan { dx, dy } => (
            IpcRequest::OverviewPan {
                dx,
                dy,
                output: None,
            },
            false,
        ),
        IpcCommands::OverviewZoom {
            delta,
            anchor_x,
            anchor_y,
        } => (
            IpcRequest::OverviewZoom {
                delta,
                anchor_canvas_x: anchor_x,
                anchor_canvas_y: anchor_y,
                output: None,
            },
            false,
        ),
        IpcCommands::CreateCluster { name, x, y } => {
            (IpcRequest::CreateCluster { name, x, y }, false)
        }
        IpcCommands::BeginClusterDrag {
            cluster_id,
            pointer_canvas_x,
            pointer_canvas_y,
            base_revision,
        } => (
            IpcRequest::BeginClusterDrag {
                cluster: cluster_id,
                pointer_canvas_x,
                pointer_canvas_y,
                base_revision,
            },
            false,
        ),
        IpcCommands::UpdateClusterDrag {
            cluster_x,
            cluster_y,
        } => (
            IpcRequest::UpdateClusterDrag {
                cluster_x,
                cluster_y,
            },
            false,
        ),
        IpcCommands::CommitClusterDrag => (IpcRequest::CommitClusterDrag, false),
        IpcCommands::CancelClusterDrag => (IpcRequest::CancelClusterDrag, false),
        IpcCommands::SelectCluster { cluster_id } => (
            IpcRequest::SelectCluster {
                cluster: cluster_id,
            },
            false,
        ),
        IpcCommands::EnterKeyboardMoveMode { cluster_id } => (
            IpcRequest::EnterKeyboardMoveMode {
                cluster: cluster_id,
            },
            false,
        ),
        IpcCommands::KeyboardMoveBy { dx, dy } => (IpcRequest::KeyboardMoveBy { dx, dy }, false),
        IpcCommands::CommitKeyboardMove => (IpcRequest::CommitKeyboardMove, false),
        IpcCommands::CancelKeyboardMove => (IpcRequest::CancelKeyboardMove, false),
    };

    let response = dispatch_ipc_request(request)?;
    if pretty {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else {
        println!("{}", serde_json::to_string(&response)?);
    }
    Ok(())
}

fn outputs_linked() -> bool {
    env::var("VIBESHELL_LINK_OUTPUTS")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}
fn needs_sway_ingest(request: &IpcRequest) -> bool {
    matches!(
        request,
        IpcRequest::GetState | IpcRequest::CreateCluster { .. }
    )
}

fn dispatch_ipc_request(request: IpcRequest) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    if needs_sway_ingest(&request) {
        with_state_owner(|owner| owner.ingest_sway_facts())?;
    }

    match request {
        IpcRequest::GetState => {
            let state = with_state_owner(|owner| {
                owner.mutate_get_state();
                owner.state()
            });
            Ok(IpcResponse::State(state))
        }
        IpcRequest::SetZoom {
            level: ZoomLevel::Cluster(cluster_id),
        } => {
            let (result, previous_zoom, next_zoom, state) = with_state_owner(|owner| {
                let previous_zoom = owner.state().zoom;
                let result = owner.activate_cluster(cluster_id);
                let state = owner.state();
                let next_zoom = state.zoom.clone();
                (result, previous_zoom, next_zoom, state)
            });
            match result {
                Ok(()) => {
                    apply_focus_handoff(previous_zoom, next_zoom, &state, Some(cluster_id), None)?;
                    Ok(IpcResponse::Ack)
                }
                Err(message) => Ok(IpcResponse::Error { message }),
            }
        }
        IpcRequest::SetFocusZoomTarget { window } => {
            log_ipc_request(
                IpcRequestType::SetFocusZoomTarget,
                "daemon-control",
                None,
                Some(window),
            );
            let (result, previous_zoom, next_zoom, state, cluster_id) = with_state_owner(|owner| {
                let previous_zoom = owner.state().zoom;
                let result = owner.set_focus_zoom_target(window);
                let state = owner.state();
                let next_zoom = state.zoom.clone();
                let cluster_id = owner.selected_cluster_id();
                (result, previous_zoom, next_zoom, state, cluster_id)
            });
            match result {
                Ok(()) => {
                    apply_focus_handoff(
                        previous_zoom,
                        next_zoom,
                        &state,
                        cluster_id,
                        Some(window),
                    )?;
                    Ok(IpcResponse::Ack)
                }
                Err(message) => Ok(IpcResponse::Error { message }),
            }
        }
        IpcRequest::ZoomInMode => {
            log_ipc_request(IpcRequestType::ZoomInMode, "daemon-control", None, None);
            let (result, previous_zoom, next_zoom, state) = with_state_owner(|owner| {
                let previous_zoom = owner.state().zoom;
                let result = owner.zoom_in_mode();
                let state = owner.state();
                let next_zoom = state.zoom.clone();
                (result, previous_zoom, next_zoom, state)
            });
            match result {
                Ok(()) => {
                    apply_focus_handoff(
                        previous_zoom,
                        next_zoom,
                        &state,
                        state.clusters.first().map(|c| c.id),
                        None,
                    )?;
                    Ok(IpcResponse::Ack)
                }
                Err(message) => Ok(IpcResponse::Error { message }),
            }
        }
        IpcRequest::ZoomOutMode => {
            log_ipc_request(IpcRequestType::ZoomOutMode, "daemon-control", None, None);
            let (result, previous_zoom, next_zoom, state) = with_state_owner(|owner| {
                let previous_zoom = owner.state().zoom;
                let result = owner.zoom_out_mode();
                let state = owner.state();
                let next_zoom = state.zoom.clone();
                (result, previous_zoom, next_zoom, state)
            });
            match result {
                Ok(()) => {
                    if matches!(next_zoom, ZoomLevel::Overview) {
                        send_overview_transition()?;
                    }
                    apply_focus_handoff(
                        previous_zoom,
                        next_zoom,
                        &state,
                        state.clusters.first().map(|c| c.id),
                        None,
                    )?;
                    Ok(IpcResponse::Ack)
                }
                Err(message) => Ok(IpcResponse::Error { message }),
            }
        }
        IpcRequest::CycleStripForward => {
            log_ipc_request(
                IpcRequestType::CycleContextStrip,
                "daemon-control",
                None,
                None,
            );
            match with_state_owner(|owner| owner.cycle_context_strip(ContextStripDirection::Next)) {
                Ok(target) => {
                    focus_window(target)?;
                    Ok(IpcResponse::Ack)
                }
                Err(message) => Ok(IpcResponse::Error { message }),
            }
        }
        IpcRequest::CycleStripBackward => {
            log_ipc_request(
                IpcRequestType::CycleContextStrip,
                "daemon-control",
                None,
                None,
            );
            match with_state_owner(|owner| {
                owner.cycle_context_strip(ContextStripDirection::Previous)
            }) {
                Ok(target) => {
                    focus_window(target)?;
                    Ok(IpcResponse::Ack)
                }
                Err(message) => Ok(IpcResponse::Error { message }),
            }
        }
        IpcRequest::CycleContextStrip { direction } => {
            log_ipc_request(
                IpcRequestType::CycleContextStrip,
                "daemon-control",
                None,
                None,
            );
            match with_state_owner(|owner| owner.cycle_context_strip(direction)) {
                Ok(target) => {
                    focus_window(target)?;
                    Ok(IpcResponse::Ack)
                }
                Err(message) => Ok(IpcResponse::Error { message }),
            }
        }
        IpcRequest::OverviewPan { dx, dy, output } => {
            with_state_owner(|owner| {
                owner.overview_pan(dx, dy, output.as_deref(), outputs_linked())
            });
            Ok(IpcResponse::Ack)
        }
        IpcRequest::OverviewZoom {
            delta,
            anchor_canvas_x,
            anchor_canvas_y,
            output,
        } => {
            with_state_owner(|owner| {
                owner.overview_zoom(
                    delta,
                    anchor_canvas_x,
                    anchor_canvas_y,
                    output.as_deref(),
                    outputs_linked(),
                )
            });
            Ok(IpcResponse::Ack)
        }
        IpcRequest::CreateCluster { name, x, y } => {
            let mut conn = Connection::new()?;
            let escaped = name.replace('"', "\\\"");
            let cmd = format!("workspace \"{escaped}\"; workspace back_and_forth");
            for reply in conn.run_command(&cmd)? {
                if let Err(e) = reply {
                    tracing::warn!(?e, "sway workspace create warning");
                }
            }
            with_state_owner(|owner| owner.ingest_sway_facts())?;
            let result = with_state_owner(|owner| owner.set_cluster_position_by_name(&name, x, y));
            match result {
                Ok(()) => Ok(IpcResponse::Ack),
                Err(msg) => Ok(IpcResponse::Error { message: msg }),
            }
        }
        IpcRequest::BeginClusterDrag { cluster, .. } => {
            with_state_owner(|owner| owner.begin_cluster_drag(cluster));
            Ok(IpcResponse::Ack)
        }
        IpcRequest::UpdateClusterDrag {
            cluster_x,
            cluster_y,
        } => {
            with_state_owner(|owner| owner.update_cluster_drag(cluster_x, cluster_y));
            Ok(IpcResponse::Ack)
        }
        IpcRequest::CommitClusterDrag => {
            with_state_owner(|owner| owner.commit_cluster_drag());
            Ok(IpcResponse::Ack)
        }
        IpcRequest::CancelClusterDrag => {
            with_state_owner(|owner| owner.cancel_cluster_drag());
            Ok(IpcResponse::Ack)
        }
        IpcRequest::SelectCluster { cluster } => {
            let result = with_state_owner(|owner| owner.select_cluster(cluster));
            match result {
                Ok(()) => Ok(IpcResponse::Ack),
                Err(message) => Ok(IpcResponse::Error { message }),
            }
        }
        IpcRequest::EnterKeyboardMoveMode { cluster } => {
            with_state_owner(|owner| owner.enter_keyboard_move_mode(cluster));
            Ok(IpcResponse::Ack)
        }
        IpcRequest::KeyboardMoveBy { dx, dy } => {
            with_state_owner(|owner| owner.keyboard_move_by(dx, dy));
            Ok(IpcResponse::Ack)
        }
        IpcRequest::CommitKeyboardMove => {
            with_state_owner(|owner| owner.commit_keyboard_move());
            Ok(IpcResponse::Ack)
        }
        IpcRequest::CancelKeyboardMove => {
            with_state_owner(|owner| owner.cancel_keyboard_move());
            Ok(IpcResponse::Ack)
        }
        unsupported => Ok(IpcResponse::Error {
            message: json!({
                "error": "unsupported_ipc_request",
                "request": format!("{unsupported:?}"),
            })
            .to_string(),
        }),
    }
}

fn focus_window(window_id: WindowId) -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::new()?;
    let command = format!("[con_id={window_id}] focus");
    for reply in connection.run_command(&command)? {
        if let Err(error) = reply {
            return Err(format!("sway rejected focus command `{command}`: {error}").into());
        }
    }
    Ok(())
}

fn activate_cluster(cluster_id: ClusterId) -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::new()?;
    let workspaces = connection.get_workspaces()?;
    let workspace = workspaces
        .into_iter()
        .find(|workspace| workspace.id as ClusterId == cluster_id)
        .ok_or_else(|| format!("cluster {cluster_id} not found"))?;

    let command = if workspace.num >= 0 {
        format!("workspace number {}", workspace.num)
    } else {
        let escaped = workspace.name.replace('"', "\\\"");
        format!("workspace \"{escaped}\"")
    };

    for reply in connection.run_command(&command)? {
        if let Err(error) = reply {
            return Err(format!("sway rejected activation command `{command}`: {error}").into());
        }
    }

    Ok(())
}

fn send_overview_transition() -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::new()?;
    for reply in connection.run_command("workspace back_and_forth")? {
        if let Err(error) = reply {
            return Err(format!(
                "sway rejected overview transition command `workspace back_and_forth`: {error}"
            )
            .into());
        }
    }
    Ok(())
}

fn apply_focus_handoff(
    previous_zoom: ZoomLevel,
    next_zoom: ZoomLevel,
    state: &CanvasState,
    requested_cluster: Option<ClusterId>,
    requested_window: Option<WindowId>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut controller = FOCUS_HANDOFF
        .get_or_init(|| Mutex::new(FocusHandoffController::default()))
        .lock()
        .expect("focus handoff mutex poisoned");

    if !matches!(previous_zoom, ZoomLevel::Overview) && matches!(next_zoom, ZoomLevel::Overview) {
        controller.pre_overview_focus = query_focused_window_id().ok().flatten();
        controller.frozen_in_overview = true;
        info!(
            previous_zoom = ?previous_zoom,
            next_zoom = ?next_zoom,
            pre_overview_focus = controller.pre_overview_focus,
            "focus handoff entered overview"
        );
        return Ok(());
    }

    if matches!(next_zoom, ZoomLevel::Overview) {
        controller.frozen_in_overview = true;
        info!(
            previous_zoom = ?previous_zoom,
            next_zoom = ?next_zoom,
            "focus handoff skipping focus churn while in overview"
        );
        return Ok(());
    }

    let exiting_overview =
        matches!(previous_zoom, ZoomLevel::Overview) && !matches!(next_zoom, ZoomLevel::Overview);
    if exiting_overview {
        controller.frozen_in_overview = false;
    }

    let plan = if exiting_overview {
        deterministic_focus_plan(
            state,
            requested_cluster,
            requested_window,
            next_zoom.clone(),
            controller.pre_overview_focus,
        )
    } else {
        match next_zoom {
            ZoomLevel::Cluster(cluster_id) => FocusPlan::ActivateCluster(cluster_id),
            ZoomLevel::Focus(window_id) => FocusPlan::FocusWindow(window_id),
            ZoomLevel::Overview => FocusPlan::None,
        }
    };

    info!(
        previous_zoom = ?previous_zoom,
        next_zoom = ?next_zoom,
        requested_cluster,
        requested_window,
        pre_overview_focus = controller.pre_overview_focus,
        ?plan,
        "focus handoff resolved transition"
    );

    match plan {
        FocusPlan::None => {}
        FocusPlan::ActivateCluster(cluster_id) => {
            activate_cluster(cluster_id)?;
        }
        FocusPlan::FocusWindow(window_id) => {
            if let Some(cluster_id) = state
                .windows
                .iter()
                .find(|window| window.id == window_id)
                .and_then(|window| window.cluster_id)
            {
                activate_cluster(cluster_id)?;
            }
            focus_window(window_id)?;
        }
    }

    if exiting_overview {
        controller.pre_overview_focus = None;
    }

    Ok(())
}

fn deterministic_focus_plan(
    state: &CanvasState,
    requested_cluster: Option<ClusterId>,
    requested_window: Option<WindowId>,
    next_zoom: ZoomLevel,
    pre_overview_focus: Option<WindowId>,
) -> FocusPlan {
    if let Some(window_id) = requested_window {
        return FocusPlan::FocusWindow(window_id);
    }

    if let ZoomLevel::Focus(window_id) = next_zoom {
        return FocusPlan::FocusWindow(window_id);
    }

    let target_cluster = match next_zoom {
        ZoomLevel::Cluster(cluster_id) => Some(cluster_id),
        ZoomLevel::Focus(window_id) => state
            .windows
            .iter()
            .find(|window| window.id == window_id)
            .and_then(|window| window.cluster_id),
        ZoomLevel::Overview => requested_cluster,
    }
    .or(requested_cluster);

    if let Some(window_id) = pre_overview_focus {
        let belongs_to_target = state
            .windows
            .iter()
            .find(|window| window.id == window_id)
            .and_then(|window| window.cluster_id)
            .zip(target_cluster)
            .map(|(cluster_id, target)| cluster_id == target)
            .unwrap_or(false);
        if belongs_to_target {
            return FocusPlan::FocusWindow(window_id);
        }
    }

    if let Some(cluster_id) = target_cluster {
        if let Some(cluster) = state
            .clusters
            .iter()
            .find(|cluster| cluster.id == cluster_id)
        {
            if let Some(window_id) = cluster
                .last_focus
                .or_else(|| cluster.windows.first().copied())
            {
                return FocusPlan::FocusWindow(window_id);
            }
            return FocusPlan::ActivateCluster(cluster_id);
        }
    }

    FocusPlan::None
}

fn query_focused_window_id() -> Result<Option<WindowId>, Box<dyn std::error::Error>> {
    let mut connection = Connection::new()?;
    let tree = connection.get_tree()?;
    Ok(find_focused_window_id(&tree))
}

fn find_focused_window_id(node: &Node) -> Option<WindowId> {
    if node.focused && node.pid.is_some() {
        return Some(node.id as WindowId);
    }

    for child in &node.nodes {
        if let Some(window_id) = find_focused_window_id(child) {
            return Some(window_id);
        }
    }

    for child in &node.floating_nodes {
        if let Some(window_id) = find_focused_window_id(child) {
            return Some(window_id);
        }
    }

    None
}

fn reload() -> Result<(), Box<dyn std::error::Error>> {
    let request = IpcRequest::ReloadConfig;
    let sway_command = match request {
        IpcRequest::ReloadConfig => "reload",
        _ => return Err("unexpected IPC request for reload command".into()),
    };

    let mut connection = Connection::new()?;
    let replies = connection.run_command(sway_command)?;

    for reply in replies {
        if let Err(error) = reply {
            return Err(format!("sway rejected reload command: {error}").into());
        }
    }

    for component in [Component::Panel, Component::Launcher, Component::Notifd] {
        send_reload_signal(component.process_name())?;
    }

    println!("reload requested (sway + vibeshell components)");
    Ok(())
}

fn send_reload_signal(process_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("pkill")
        .args(["-HUP", "-x", process_name])
        .status()?;

    if !status.success() {
        return Err(format!("failed to send SIGHUP to {process_name}").into());
    }

    info!(process_name, "requested config reload");
    Ok(())
}

fn status() -> Result<(), Box<dyn std::error::Error>> {
    let sway_running = Connection::new().is_ok();
    println!("sway: {}", running_label(sway_running));

    for component in [Component::Panel, Component::Launcher, Component::Notifd] {
        let running = is_running(component.process_name())?;
        println!("{}: {}", component.process_name(), running_label(running));
    }

    Ok(())
}

fn restart(component: Component) -> Result<(), Box<dyn std::error::Error>> {
    let process_name = component.process_name();

    if is_running(process_name)? {
        let status = Command::new("pkill").args(["-x", process_name]).status()?;
        if !status.success() {
            return Err(format!("failed to stop {process_name}").into());
        }
        println!("stopped {process_name}");
    }

    let cmd = env::var(component.env_command_key())
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| component.default_start_command().to_owned());

    let child = Command::new("setsid")
        .args(["bash", "-lc", &cmd])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    println!("started {process_name} (pid {}) via `{cmd}`", child.id());
    Ok(())
}

fn logout() -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::new()?;
    let replies = connection.run_command("exit")?;

    for reply in replies {
        if let Err(error) = reply {
            return Err(format!("sway rejected exit command: {error}").into());
        }
    }

    println!("logout requested");
    Ok(())
}

fn logs(component: Component) -> Result<(), Box<dyn std::error::Error>> {
    let log_path = env::var("VIBESHELL_LOG_FILE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/vibeshell-nested.log"));

    let file = fs::File::open(&log_path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to open {} ({error}). Capture logs first, e.g. `VIBESHELL_LOG=debug ./scripts/run-in-nested-sway 2>&1 | tee {}`",
                log_path.display(),
                log_path.display(),
            ),
        )
    })?;

    let target = component.log_target();
    let mut matched = 0usize;
    for line in io::BufReader::new(file).lines() {
        let line = line?;
        if line.contains(target) {
            println!("{line}");
            matched += 1;
        }
    }

    if matched == 0 {
        println!(
            "no log lines matched component `{}` in {}",
            component.process_name(),
            log_path.display()
        );
    }

    Ok(())
}

fn dump_state(pretty: bool) -> Result<(), Box<dyn std::error::Error>> {
    log_ipc_request(IpcRequestType::GetState, "daemon-snapshot", None, None);
    let events = ingest_sway_event_metadata()?;
    with_state_owner(|owner| owner.ingest_sway_facts())?;
    let state = with_state_owner(|owner| {
        owner.mutate_get_state();
        owner.state()
    });

    let active_cluster = with_state_owner(|owner| owner.selected_cluster_id());
    let active_zoom = state.zoom.clone();
    let output = state.output.clone();
    let viewport = state.viewport.clone();

    let mut state = state;
    state.clusters.sort_by_key(|cluster| cluster.id);
    state.windows.sort_by_key(|window| window.id);

    let mut clusters_with_windows: Vec<ClusterDump> = state
        .clusters
        .iter()
        .map(|cluster| {
            let mut window_ids = cluster.recency.clone();
            for &window_id in &cluster.windows {
                if !window_ids.contains(&window_id) {
                    window_ids.push(window_id);
                }
            }

            ClusterDump {
                id: cluster.id,
                name: cluster.name.clone(),
                x: cluster.x,
                y: cluster.y,
                window_ids,
            }
        })
        .collect();
    clusters_with_windows.sort_by_key(|cluster| cluster.id);

    let dump = DumpState {
        active_zoom,
        active_cluster,
        clusters: clusters_with_windows,
        windows: state.windows.clone(),
        output_viewport: OutputViewport { output, viewport },
    };

    let response = IpcResponse::State(state);
    log_ipc_response(
        &response,
        "daemon-snapshot",
        events.windows,
        events.workspaces,
    );

    let rendered = render_dump_state_json(&dump, pretty)?;
    println!("{rendered}");

    Ok(())
}

fn render_dump_state_json(dump: &DumpState, pretty: bool) -> Result<String, serde_json::Error> {
    if pretty {
        serde_json::to_string_pretty(dump)
    } else {
        serde_json::to_string(dump)
    }
}

#[derive(Default)]
struct SwayEventIngestSummary {
    windows: usize,
    workspaces: usize,
}

fn ingest_sway_event_metadata() -> Result<SwayEventIngestSummary, Box<dyn std::error::Error>> {
    let mut summary = SwayEventIngestSummary::default();

    if let Ok(connection) = Connection::new() {
        if let Ok(mut events) = connection.subscribe([EventType::Window, EventType::Workspace]) {
            for event in (&mut events).take(2) {
                match event {
                    Ok(evt) => {
                        let event_type = evt.event_type();
                        let event_name = format!("{:?}", event_type).to_lowercase();
                        match event_type {
                            EventType::Window => summary.windows += 1,
                            EventType::Workspace => summary.workspaces += 1,
                            _ => {}
                        }
                        log_sway_ingest("sway", &event_name, None, None);
                    }
                    Err(error) => {
                        warn!(
                            ?error,
                            "failed to parse sway event while ingesting metadata"
                        );
                    }
                }
            }
        } else {
            warn!("unable to subscribe for sway ingest metadata during dump-state");
        }
    } else {
        warn!("unable to connect for sway ingest metadata during dump-state");
    }

    Ok(summary)
}

fn log_sway_ingest(
    module: &str,
    event_type: &str,
    cluster_id: Option<ClusterId>,
    window_id: Option<WindowId>,
) {
    info!(
        module,
        event_type, cluster_id, window_id, "structured daemon ingest event"
    );
}

fn log_ipc_request(
    request: IpcRequestType,
    module: &str,
    cluster_id: Option<ClusterId>,
    window_id: Option<WindowId>,
) {
    info!(
        module,
        event_type = ?request,
        cluster_id,
        window_id,
        "structured daemon ipc request"
    );
}

fn log_ipc_response(response: &IpcResponse, module: &str, windows: usize, workspaces: usize) {
    let event_type = match response {
        IpcResponse::Ack | IpcResponse::ClusterDragAck { .. } => "ack",
        IpcResponse::State(_) => "state",
        IpcResponse::Error { .. } | IpcResponse::ClusterDragError { .. } => "error",
    };

    info!(
        module,
        event_type, windows, workspaces, "structured daemon ipc response"
    );
}

fn is_running(process_name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let status = Command::new("pgrep")
        .args(["-x", process_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(status.success())
}

fn running_label(running: bool) -> &'static str {
    if running {
        "running"
    } else {
        "stopped"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::contracts::{Cluster, WindowRole, WindowState};

    fn fixture_canvas_state(window_ids: &[WindowId]) -> CanvasState {
        let cluster_id = 7;
        CanvasState {
            state_revision: 1,
            zoom: ZoomLevel::Overview,
            viewport: Viewport::default(),
            output_viewports: std::collections::HashMap::new(),
            clusters: vec![Cluster {
                id: cluster_id,
                name: "fixture".into(),
                x: 10.0,
                y: 20.0,
                enabled: true,
                windows: window_ids.to_vec(),
                last_focus: window_ids.last().copied(),
                recency: window_ids.iter().rev().copied().collect(),
            }],
            windows: window_ids
                .iter()
                .map(|window_id| Window {
                    id: *window_id,
                    title: format!("Window {window_id}"),
                    app_id: Some("fixture".into()),
                    class: Some("fixture".into()),
                    role: WindowRole::Normal,
                    state: WindowState::Tiled,
                    cluster_id: Some(cluster_id),
                    transient_for: None,
                    manual_cluster_override: false,
                    manual_position_override: false,
                })
                .collect(),
            output: OutputState::default(),
        }
    }

    fn fixture_dump_state() -> DumpState {
        DumpState {
            active_zoom: ZoomLevel::Cluster(7),
            active_cluster: Some(7),
            clusters: vec![ClusterDump {
                id: 7,
                name: "Work".into(),
                x: 100.0,
                y: 200.0,
                window_ids: vec![101],
            }],
            windows: vec![Window {
                id: 101,
                title: "Terminal".into(),
                app_id: Some("foot".into()),
                class: Some("foot".into()),
                role: WindowRole::Normal,
                state: WindowState::Tiled,
                cluster_id: Some(7),
                transient_for: None,
                manual_cluster_override: false,
                manual_position_override: false,
            }],
            output_viewport: OutputViewport {
                output: OutputState::default(),
                viewport: Viewport::default(),
            },
        }
    }

    #[test]
    fn dump_state_smoke_idle_machine_mode() {
        let dump = DumpState {
            active_zoom: ZoomLevel::Overview,
            active_cluster: None,
            clusters: Vec::new(),
            windows: Vec::new(),
            output_viewport: OutputViewport {
                output: OutputState::default(),
                viewport: Viewport::default(),
            },
        };

        let json = render_dump_state_json(&dump, false).expect("serialize dump state");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse dump-state json");

        assert!(parsed.get("active_zoom").is_some());
        assert!(parsed.get("clusters").is_some());
        assert!(parsed.get("windows").is_some());
        assert!(parsed.get("output_viewport").is_some());
    }

    #[test]
    fn dump_state_smoke_after_synthetic_message() {
        let canvas_state = CanvasState {
            state_revision: 0,
            zoom: ZoomLevel::Cluster(7),
            viewport: Viewport::default(),
            output_viewports: std::collections::HashMap::new(),
            clusters: vec![Cluster {
                id: 7,
                name: "Work".into(),
                x: 1.0,
                y: 2.0,
                enabled: true,
                windows: vec![101],
                last_focus: Some(101),
                recency: vec![101],
            }],
            windows: vec![Window {
                id: 101,
                title: "Terminal".into(),
                app_id: Some("foot".into()),
                class: Some("foot".into()),
                role: WindowRole::Normal,
                state: WindowState::Tiled,
                cluster_id: Some(7),
                transient_for: None,
                manual_cluster_override: false,
                manual_position_override: false,
            }],
            output: OutputState::default(),
        };

        let response = match IpcRequest::GetState {
            IpcRequest::GetState => IpcResponse::State(canvas_state.clone()),
            _ => IpcResponse::Ack,
        };
        match response {
            IpcResponse::State(state) => {
                assert_eq!(state.clusters.len(), 1);
                assert_eq!(state.windows.len(), 1);
            }
            other => panic!("expected state response, got {other:?}"),
        }

        let json =
            render_dump_state_json(&fixture_dump_state(), true).expect("serialize pretty dump");
        assert!(json.contains("\n"));
        assert!(json.contains("\"active_cluster\""));
    }

    #[test]
    fn deterministic_focus_plan_prefers_stable_last_focus_across_20_cycles() {
        let fixtures = [vec![101], vec![201, 202], vec![301, 302, 303, 304]];

        for window_ids in fixtures {
            let state = fixture_canvas_state(&window_ids);
            let expected_focus = *window_ids.last().expect("fixture has windows");

            for _ in 0..20 {
                let plan =
                    deterministic_focus_plan(&state, Some(7), None, ZoomLevel::Cluster(7), None);
                assert!(
                    matches!(plan, FocusPlan::FocusWindow(window_id) if window_id == expected_focus)
                );

                let focus_plan = deterministic_focus_plan(
                    &state,
                    Some(7),
                    None,
                    ZoomLevel::Focus(expected_focus),
                    None,
                );
                assert!(
                    matches!(focus_plan, FocusPlan::FocusWindow(window_id) if window_id == expected_focus)
                );
            }
        }
    }

    #[test]
    fn deterministic_focus_plan_restores_pre_overview_focus_when_valid_over_20_cycles() {
        let fixtures = [vec![401], vec![501, 502], vec![601, 602, 603, 604]];

        for window_ids in fixtures {
            let state = fixture_canvas_state(&window_ids);
            let pre_overview_focus = window_ids[0];

            for _ in 0..20 {
                let plan = deterministic_focus_plan(
                    &state,
                    Some(7),
                    None,
                    ZoomLevel::Cluster(7),
                    Some(pre_overview_focus),
                );
                assert!(
                    matches!(plan, FocusPlan::FocusWindow(window_id) if window_id == pre_overview_focus)
                );
            }
        }
    }
}
