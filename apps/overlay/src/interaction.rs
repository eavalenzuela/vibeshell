use std::process::Command;

use common::contracts::ClusterId;

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
    },
    OverviewZoom {
        delta: f64,
        anchor_x: f64,
        anchor_y: f64,
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
        IpcMutation::OverviewPan { dx, dy } => {
            command.args(["overview-pan", &dx.to_string(), &dy.to_string()]);
        }
        IpcMutation::OverviewZoom {
            delta,
            anchor_x,
            anchor_y,
        } => {
            command.args([
                "overview-zoom",
                &delta.to_string(),
                &anchor_x.to_string(),
                &anchor_y.to_string(),
            ]);
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

pub fn dispatch_ipc_mutation(mutation: IpcMutation) {
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
    let debug_str = format!("{mutation:?}");
    let mut command = build_mutation_command(&mutation);
    if let Err(error) = command.spawn() {
        tracing::warn!(?error, mutation = debug_str, "failed to spawn IPC mutation");
    }
}
