use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::time::Duration;

use common::contracts::{daemon_socket_path, ClusterId, IpcRequest, IpcResponse};

#[derive(Debug, Clone)]
pub enum IpcMutation {
    SelectCluster {
        cluster: ClusterId,
    },
    BeginClusterDrag {
        cluster: ClusterId,
        pointer_canvas_x: f64,
        pointer_canvas_y: f64,
        base_revision: u64,
    },
    UpdateClusterDrag {
        cluster_x: f64,
        cluster_y: f64,
    },
    CommitClusterDrag,
    CancelClusterDrag,
    EnterKeyboardMoveMode {
        cluster: ClusterId,
    },
    KeyboardMoveBy {
        dx: f64,
        dy: f64,
    },
    CommitKeyboardMove,
    CancelKeyboardMove,
    OverviewPan {
        dx: f64,
        dy: f64,
        output: Option<String>,
    },
    OverviewZoom {
        delta: f64,
        anchor_x: f64,
        anchor_y: f64,
        output: Option<String>,
    },
    CreateCluster {
        name: String,
        x: f64,
        y: f64,
    },
}

fn build_mutation_command(mutation: &IpcMutation) -> Command {
    let mut command = Command::new("vibeshellctl");
    command.arg("ipc");

    match mutation {
        IpcMutation::SelectCluster { cluster } => {
            command.args(["select-cluster", &cluster.to_string()]);
        }
        IpcMutation::BeginClusterDrag {
            cluster,
            pointer_canvas_x,
            pointer_canvas_y,
            base_revision,
        } => {
            command.args([
                "begin-cluster-drag",
                &cluster.to_string(),
                &pointer_canvas_x.to_string(),
                &pointer_canvas_y.to_string(),
                &base_revision.to_string(),
            ]);
        }
        IpcMutation::UpdateClusterDrag {
            cluster_x,
            cluster_y,
        } => {
            command.args([
                "update-cluster-drag",
                &cluster_x.to_string(),
                &cluster_y.to_string(),
            ]);
        }
        IpcMutation::CommitClusterDrag => {
            command.arg("commit-cluster-drag");
        }
        IpcMutation::CancelClusterDrag => {
            command.arg("cancel-cluster-drag");
        }
        IpcMutation::EnterKeyboardMoveMode { cluster } => {
            command.args(["enter-keyboard-move-mode", &cluster.to_string()]);
        }
        IpcMutation::KeyboardMoveBy { dx, dy } => {
            command.args(["keyboard-move-by", &dx.to_string(), &dy.to_string()]);
        }
        IpcMutation::CommitKeyboardMove => {
            command.arg("commit-keyboard-move");
        }
        IpcMutation::CancelKeyboardMove => {
            command.arg("cancel-keyboard-move");
        }
        IpcMutation::OverviewPan { dx, dy, ref output } => {
            command.args(["overview-pan", &dx.to_string(), &dy.to_string()]);
            if let Some(output) = output {
                command.args(["--output", output]);
            }
        }
        IpcMutation::OverviewZoom {
            delta,
            anchor_x,
            anchor_y,
            ref output,
        } => {
            command.args([
                "overview-zoom",
                &delta.to_string(),
                &anchor_x.to_string(),
                &anchor_y.to_string(),
            ]);
            if let Some(output) = output {
                command.args(["--output", output]);
            }
        }
        IpcMutation::CreateCluster { name, x, y } => {
            command.args([
                "create-cluster",
                name.as_str(),
                &x.to_string(),
                &y.to_string(),
            ]);
        }
    }

    command
}

pub fn try_dispatch_via_socket(request: &IpcRequest) -> Option<IpcResponse> {
    let socket_path = daemon_socket_path();
    let stream = UnixStream::connect(&socket_path).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .ok()?;
    let json = serde_json::to_string(request).ok()?;
    let mut writer = stream.try_clone().ok()?;
    writeln!(writer, "{json}").ok()?;
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    serde_json::from_str(line.trim()).ok()
}

fn mutation_to_ipc_request(mutation: &IpcMutation) -> IpcRequest {
    match mutation {
        IpcMutation::SelectCluster { cluster } => IpcRequest::SelectCluster { cluster: *cluster },
        IpcMutation::BeginClusterDrag {
            cluster,
            pointer_canvas_x,
            pointer_canvas_y,
            base_revision,
        } => IpcRequest::BeginClusterDrag {
            cluster: *cluster,
            pointer_canvas_x: *pointer_canvas_x,
            pointer_canvas_y: *pointer_canvas_y,
            base_revision: *base_revision,
        },
        IpcMutation::UpdateClusterDrag {
            cluster_x,
            cluster_y,
        } => IpcRequest::UpdateClusterDrag {
            cluster_x: *cluster_x,
            cluster_y: *cluster_y,
        },
        IpcMutation::CommitClusterDrag => IpcRequest::CommitClusterDrag,
        IpcMutation::CancelClusterDrag => IpcRequest::CancelClusterDrag,
        IpcMutation::EnterKeyboardMoveMode { cluster } => {
            IpcRequest::EnterKeyboardMoveMode { cluster: *cluster }
        }
        IpcMutation::KeyboardMoveBy { dx, dy } => IpcRequest::KeyboardMoveBy { dx: *dx, dy: *dy },
        IpcMutation::CommitKeyboardMove => IpcRequest::CommitKeyboardMove,
        IpcMutation::CancelKeyboardMove => IpcRequest::CancelKeyboardMove,
        IpcMutation::OverviewPan { dx, dy, output } => IpcRequest::OverviewPan {
            dx: *dx,
            dy: *dy,
            output: output.clone(),
        },
        IpcMutation::OverviewZoom {
            delta,
            anchor_x,
            anchor_y,
            output,
        } => IpcRequest::OverviewZoom {
            delta: *delta,
            anchor_canvas_x: *anchor_x,
            anchor_canvas_y: *anchor_y,
            output: output.clone(),
        },
        IpcMutation::CreateCluster { name, x, y } => IpcRequest::CreateCluster {
            name: name.clone(),
            x: *x,
            y: *y,
        },
    }
}

pub fn dispatch_ipc_mutation(mutation: IpcMutation) {
    let request = mutation_to_ipc_request(&mutation);
    if try_dispatch_via_socket(&request).is_some() {
        return;
    }
    let debug_str = format!("{mutation:?}");
    let mut command = build_mutation_command(&mutation);
    if let Err(error) = command.status() {
        tracing::warn!(
            ?error,
            mutation = debug_str,
            "failed to execute IPC mutation"
        );
    }
}

pub fn dispatch_ipc_mutation_detached(mutation: IpcMutation) {
    let request = mutation_to_ipc_request(&mutation);
    if try_dispatch_via_socket(&request).is_some() {
        return;
    }
    let debug_str = format!("{mutation:?}");
    let mut command = build_mutation_command(&mutation);
    if let Err(error) = command.spawn() {
        tracing::warn!(?error, mutation = debug_str, "failed to spawn IPC mutation");
    }
}
