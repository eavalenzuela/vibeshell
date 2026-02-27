use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use clap::{Parser, Subcommand, ValueEnum};
use common::contracts::{
    CanvasState, Cluster, ClusterId, ContextStripDirection, IpcRequest, IpcResponse, OutputState,
    Viewport, Window, WindowId, WindowRole, WindowState, ZoomLevel,
};
use common::model::{CanvasModel, OpenAssignPolicy};
use serde::Serialize;
use serde_json::json;
use tracing::{info, warn};

use swayipc::{Connection, EventType};

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
}

#[derive(Clone, Debug, ValueEnum)]
enum Component {
    Panel,
    Launcher,
    Notifd,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ModelMutation {
    SeedClusterFromWorkspace,
    UpsertWindowFromTree,
    SelectActiveCluster,
    SyncZoom,
    UpdateOutputViewport,
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

    fn from_log_target(&self) -> &'static str {
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
    };

    let response = dispatch_ipc_request(request)?;
    if pretty {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else {
        println!("{}", serde_json::to_string(&response)?);
    }
    Ok(())
}

fn dispatch_ipc_request(request: IpcRequest) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    match request {
        IpcRequest::GetState => {
            let state = build_canvas_state_from_sway()?;
            Ok(IpcResponse::State(state))
        }
        IpcRequest::SetZoom {
            level: ZoomLevel::Cluster(cluster_id),
        } => match activate_cluster(cluster_id) {
            Ok(()) => Ok(IpcResponse::Ack),
            Err(error) => Ok(IpcResponse::Error {
                message: error.to_string(),
            }),
        },
        IpcRequest::SetFocusZoomTarget { window } => {
            log_ipc_request(
                IpcRequestType::SetFocusZoomTarget,
                "daemon-control",
                None,
                Some(window),
            );
            match set_focus_zoom_target(window) {
                Ok(()) => Ok(IpcResponse::Ack),
                Err(error) => Ok(IpcResponse::Error { message: error }),
            }
        }
        IpcRequest::ZoomInMode => {
            log_ipc_request(IpcRequestType::ZoomInMode, "daemon-control", None, None);
            match zoom_in_mode() {
                Ok(()) => Ok(IpcResponse::Ack),
                Err(error) => Ok(IpcResponse::Error { message: error }),
            }
        }
        IpcRequest::ZoomOutMode => {
            log_ipc_request(IpcRequestType::ZoomOutMode, "daemon-control", None, None);
            match zoom_out_mode() {
                Ok(()) => Ok(IpcResponse::Ack),
                Err(error) => Ok(IpcResponse::Error { message: error }),
            }
        }
        IpcRequest::CycleStripForward => {
            log_ipc_request(
                IpcRequestType::CycleContextStrip,
                "daemon-control",
                None,
                None,
            );
            match cycle_context_strip(ContextStripDirection::Next) {
                Ok(()) => Ok(IpcResponse::Ack),
                Err(error) => Ok(IpcResponse::Error { message: error }),
            }
        }
        IpcRequest::CycleStripBackward => {
            log_ipc_request(
                IpcRequestType::CycleContextStrip,
                "daemon-control",
                None,
                None,
            );
            match cycle_context_strip(ContextStripDirection::Previous) {
                Ok(()) => Ok(IpcResponse::Ack),
                Err(error) => Ok(IpcResponse::Error { message: error }),
            }
        }
        IpcRequest::CycleContextStrip { direction } => {
            log_ipc_request(
                IpcRequestType::CycleContextStrip,
                "daemon-control",
                None,
                None,
            );
            match cycle_context_strip(direction) {
                Ok(()) => Ok(IpcResponse::Ack),
                Err(error) => Ok(IpcResponse::Error { message: error }),
            }
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

fn zoom_in_mode() -> Result<(), String> {
    let state = build_canvas_state_from_sway().map_err(|error| error.to_string())?;
    match state.zoom {
        ZoomLevel::Overview => {
            let cluster_id = state
                .clusters
                .first()
                .map(|cluster| cluster.id)
                .ok_or_else(|| {
                    json!({"error":"invalid_state","reason":"no_clusters_available"}).to_string()
                })?;
            activate_cluster(cluster_id).map_err(|error| error.to_string())
        }
        ZoomLevel::Cluster(cluster_id) => {
            let cluster = state
                .clusters
                .iter()
                .find(|cluster| cluster.id == cluster_id)
                .ok_or_else(|| {
                    json!({"error":"invalid_state","reason":"active_cluster_missing"}).to_string()
                })?;

            let window_id = cluster
                .last_focus
                .or_else(|| cluster.windows.first().copied())
                .ok_or_else(|| {
                    json!({"error":"unsupported_state_combination","reason":"cluster_has_no_windows"})
                        .to_string()
                })?;

            focus_window(window_id).map_err(|error| error.to_string())
        }
        ZoomLevel::Focus(_) => Err(
            json!({"error":"unsupported_state_combination","reason":"already_in_focus_zoom"})
                .to_string(),
        ),
    }
}

fn zoom_out_mode() -> Result<(), String> {
    let state = build_canvas_state_from_sway().map_err(|error| error.to_string())?;
    match state.zoom {
        ZoomLevel::Focus(window_id) => {
            let cluster_id = state
                .clusters
                .iter()
                .find(|cluster| cluster.windows.contains(&window_id))
                .map(|cluster| cluster.id)
                .ok_or_else(|| {
                    json!({"error":"invalid_state","reason":"focused_window_cluster_missing"})
                        .to_string()
                })?;
            activate_cluster(cluster_id).map_err(|error| error.to_string())
        }
        ZoomLevel::Cluster(_) => {
            let mut connection = Connection::new().map_err(|error| error.to_string())?;
            for reply in connection
                .run_command("workspace back_and_forth")
                .map_err(|error| error.to_string())?
            {
                if let Err(error) = reply {
                    return Err(format!("sway rejected overview transition command `workspace back_and_forth`: {error}"));
                }
            }
            Ok(())
        }
        ZoomLevel::Overview => Err(
            json!({"error":"unsupported_state_combination","reason":"already_in_overview_zoom"})
                .to_string(),
        ),
    }
}

fn set_focus_zoom_target(window_id: WindowId) -> Result<(), String> {
    let state = build_canvas_state_from_sway().map_err(|error| error.to_string())?;
    let active_cluster = state
        .clusters
        .iter()
        .find(|cluster| cluster.windows.contains(&window_id))
        .map(|cluster| cluster.id)
        .or_else(|| state.clusters.first().map(|cluster| cluster.id))
        .ok_or_else(|| {
            json!({"error":"invalid_state","reason":"no_clusters_available"}).to_string()
        })?;

    let mut model = CanvasModel::new(state.clone(), active_cluster).map_err(|error| {
        json!({"error":"model_error","reason": format!("{error:?}")}).to_string()
    })?;
    model.on_focus_change(window_id).map_err(|error| {
        json!({
            "error":"invalid_focus_target",
            "reason": format!("{error:?}"),
            "window": window_id
        })
        .to_string()
    })?;

    let target_cluster = model
        .state()
        .windows
        .iter()
        .find(|window| window.id == window_id)
        .and_then(|window| window.cluster_id)
        .ok_or_else(|| {
            json!({
                "error":"invalid_state",
                "reason":"window_missing_cluster",
                "window": window_id
            })
            .to_string()
        })?;

    activate_cluster(target_cluster).map_err(|error| error.to_string())?;
    focus_window(window_id).map_err(|error| error.to_string())?;
    Ok(())
}

fn cycle_context_strip(direction: ContextStripDirection) -> Result<(), String> {
    let state = build_canvas_state_from_sway().map_err(|error| error.to_string())?;
    let focused = match state.zoom {
        ZoomLevel::Focus(window_id) => window_id,
        _ => {
            return Err(
                json!({"error":"unsupported_state_combination","reason":"requires_focus_zoom"})
                    .to_string(),
            )
        }
    };

    let cluster = state
        .clusters
        .iter()
        .find(|cluster| cluster.windows.contains(&focused))
        .ok_or_else(|| {
            json!({
                "error":"invalid_state",
                "reason":"focused_window_cluster_missing",
                "window": focused
            })
            .to_string()
        })?;

    if cluster.windows.len() < 2 {
        return Err(
            json!({"error":"unsupported_state_combination","reason":"context_strip_requires_multiple_windows"})
                .to_string(),
        );
    }

    let mut order = if cluster.recency.is_empty() {
        cluster.windows.clone()
    } else {
        let mut merged = cluster.recency.clone();
        for window_id in &cluster.windows {
            if !merged.contains(window_id) {
                merged.push(*window_id);
            }
        }
        merged
    };
    order.retain(|window_id| *window_id != focused);
    if order.is_empty() {
        return Err(
            json!({"error":"unsupported_state_combination","reason":"context_strip_empty"})
                .to_string(),
        );
    }

    let target = match direction {
        ContextStripDirection::Next => order[0],
        ContextStripDirection::Previous => *order.last().expect("order non-empty"),
    };

    focus_window(target).map_err(|error| error.to_string())
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

    let target = component.from_log_target();
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
    let state = build_canvas_state_from_sway()?;

    let active_cluster = state
        .windows
        .iter()
        .find(|window| matches!(state.zoom, ZoomLevel::Focus(id) if id == window.id))
        .and_then(|window| window.cluster_id)
        .or_else(|| state.clusters.first().map(|cluster| cluster.id));
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

fn build_canvas_state_from_sway() -> Result<CanvasState, Box<dyn std::error::Error>> {
    let mut connection = Connection::new()?;
    let tree = connection.get_tree()?;
    let outputs = connection.get_outputs()?;
    let workspaces = connection.get_workspaces()?;

    let mut clusters: BTreeMap<ClusterId, Cluster> = BTreeMap::new();
    for workspace in workspaces {
        if workspace.num < 0 {
            continue;
        }

        let cluster_id = workspace.id as ClusterId;
        log_model_mutation(
            ModelMutation::SeedClusterFromWorkspace,
            Some(cluster_id),
            None,
            "workspace->cluster",
        );
        clusters.insert(
            cluster_id,
            Cluster {
                id: cluster_id,
                name: workspace.name,
                x: workspace.rect.x as f64,
                y: workspace.rect.y as f64,
                enabled: workspace.visible,
                windows: Vec::new(),
                last_focus: None,
                recency: Vec::new(),
            },
        );
    }

    let mut windows = Vec::new();
    collect_windows_from_tree(&tree, None, &mut windows);
    windows.sort_by_key(|window| window.id);

    for window in &windows {
        log_model_mutation(
            ModelMutation::UpsertWindowFromTree,
            window.cluster_id,
            Some(window.id),
            "tree-node",
        );
    }

    let focused_window_id = windows
        .iter()
        .find(|window| {
            matches!(window.state, WindowState::Fullscreen) || window.title.contains("[focused]")
        })
        .map(|window| window.id);

    let fallback_cluster = clusters.keys().next().copied();
    let mut model = CanvasModel::new(
        CanvasState {
            zoom: ZoomLevel::Overview,
            viewport: Viewport::default(),
            clusters: clusters.values().cloned().collect(),
            windows: Vec::new(),
            output: OutputState::default(),
        },
        fallback_cluster.unwrap_or_default(),
    )
    .map_err(|error| format!("unable to initialize daemon model from sway snapshot: {error:?}"))?;

    for mut window in windows {
        let assigned_cluster = window.cluster_id.or(fallback_cluster);
        if let Some(cluster_id) = assigned_cluster {
            window.cluster_id = Some(cluster_id);
            let _ = model.on_window_open(window, OpenAssignPolicy::FallbackCluster(cluster_id));
        }
    }

    if let Some(window_id) = focused_window_id {
        let _ = model.on_focus_change(window_id);
    }

    let active_cluster = Some(model.active_cluster());

    log_model_mutation(
        ModelMutation::SelectActiveCluster,
        active_cluster,
        focused_window_id,
        "model-focus",
    );

    let active_zoom = if let Some(window_id) = focused_window_id {
        ZoomLevel::Focus(window_id)
    } else if let Some(cluster_id) = active_cluster {
        ZoomLevel::Cluster(cluster_id)
    } else {
        ZoomLevel::Overview
    };

    log_model_mutation(
        ModelMutation::SyncZoom,
        active_cluster,
        focused_window_id,
        "derived",
    );

    let output = outputs
        .into_iter()
        .find(|output| output.focused)
        .or_else(|| {
            connection
                .get_outputs()
                .ok()
                .and_then(|all| all.into_iter().next())
        })
        .map(|output| OutputState {
            name: output.name,
            width: output.rect.width,
            height: output.rect.height,
            scale: output.scale.unwrap_or(1.0),
        })
        .unwrap_or_default();

    let viewport = Viewport {
        x: 0.0,
        y: 0.0,
        scale: 1.0,
    };

    log_model_mutation(
        ModelMutation::UpdateOutputViewport,
        active_cluster,
        focused_window_id,
        "output+viewport",
    );

    let mut state = model.state().clone();
    state.zoom = active_zoom;
    state.viewport = viewport;
    state.output = output;
    Ok(state)
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
        let focused_suffix = if node.focused { " [focused]" } else { "" };

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

        let exclusion_reason = if node.fullscreen_mode.unwrap_or(0) > 0 {
            Some("fullscreen_temporary_override")
        } else if transient_for.is_some() {
            Some("transient_dialog_attached_to_parent")
        } else if has_overlay_hint {
            Some("overlay_or_popup")
        } else {
            None
        };

        if let Some(reason) = exclusion_reason {
            info!(
                window_id = node.id,
                reason, "window excluded from cluster reflow/slot geometry policy"
            );
        }

        let role = if has_overlay_hint {
            WindowRole::Utility
        } else if node.floating.is_some() || transient_for.is_some() {
            WindowRole::Dialog
        } else {
            WindowRole::Normal
        };

        out.push(Window {
            id: node.id as WindowId,
            title: format!("{}{}", title, focused_suffix),
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
            manual_position_override: exclusion_reason.is_some(),
        });
    }

    for child in &node.nodes {
        collect_windows_from_tree(child, cluster_id, out);
    }
    for child in &node.floating_nodes {
        collect_windows_from_tree(child, cluster_id, out);
    }
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

fn log_model_mutation(
    mutation: ModelMutation,
    cluster_id: Option<ClusterId>,
    window_id: Option<WindowId>,
    source: &str,
) {
    info!(
        module = "daemon_model",
        event_type = ?mutation,
        cluster_id,
        window_id,
        source,
        "structured daemon model mutation"
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
        IpcResponse::Ack => "ack",
        IpcResponse::State(_) => "state",
        IpcResponse::Error { .. } => "error",
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
            zoom: ZoomLevel::Cluster(7),
            viewport: Viewport::default(),
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
}
