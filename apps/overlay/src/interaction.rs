use std::process::Command;

use common::contracts::ClusterId;

#[derive(Debug, Clone, Copy)]
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
        pointer_canvas_x: f64,
        pointer_canvas_y: f64,
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
}

pub fn dispatch_ipc_mutation(mutation: IpcMutation) {
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
            pointer_canvas_x,
            pointer_canvas_y,
        } => {
            command.args([
                "update-cluster-drag",
                &pointer_canvas_x.to_string(),
                &pointer_canvas_y.to_string(),
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
    }

    if let Err(error) = command.status() {
        tracing::warn!(?error, ?mutation, "failed to execute IPC mutation");
    }
}
