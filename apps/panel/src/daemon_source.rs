//! Pull `PanelState` snapshots from the vibeshell daemon.
//!
//! Replaces the W1c-4 panel's `sway::SwayClient` listener thread. The daemon
//! already holds a backend-neutral snapshot (assembled from sway under
//! `WM_BACKEND=sway` or from vibewm under `WM_BACKEND=wlroots`); the panel
//! just polls it and projects onto `PanelState`.
//!
//! The daemon doesn't currently expose a push-style subscribe channel, so
//! this is a poll loop. Cadence is configurable via `panel.sway_event_debounce_ms`
//! (kept under that name in config for back-compat — it's the inter-tick
//! delay regardless of backend).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use common::contracts::{daemon_socket_path, CanvasState, IpcRequest, IpcResponse};
use common::panel::{PanelState, PanelUpdate, WorkspaceState};

const CONNECT_BACKOFF_INITIAL: Duration = Duration::from_millis(500);
const CONNECT_BACKOFF_MAX: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Spawn a thread that polls the daemon every `poll_interval` and pushes
/// `PanelUpdate` snapshots onto `tx`. Also spawns a parallel thread that
/// subscribes to vibewm's event stream (under WM_BACKEND=wlroots) and signals
/// the poll thread to do an immediate fetch on each `WorkspaceOrWindow` event,
/// so cluster switches and new windows propagate within event RTT instead of
/// waiting for the next poll tick. The poll thread reconnects with
/// exponential backoff on socket errors.
pub fn spawn_daemon_listener(tx: Sender<PanelUpdate>, poll_interval: Duration) {
    let (wakeup_tx, wakeup_rx) = mpsc::channel::<()>();

    // Event-subscribe thread: pulses `wakeup_tx` whenever vibewm reports a
    // workspace/window change. Silently no-ops if vibewm isn't running (sway
    // mode), in which case the poll thread's tick is the sole driver.
    {
        let wakeup_tx = wakeup_tx.clone();
        thread::spawn(move || {
            use wm::WmBackend;
            let backend = match wm::WlrootsBackend::connect() {
                Ok(b) => b,
                Err(_) => return,
            };
            let stream = match backend.spawn_event_stream() {
                Ok(s) => s,
                Err(_) => return,
            };
            while stream.recv().is_ok() {
                if wakeup_tx.send(()).is_err() {
                    break;
                }
            }
        });
    }

    // Poll thread: fetch on each tick OR each wakeup, whichever fires first.
    thread::spawn(move || {
        let mut backoff = CONNECT_BACKOFF_INITIAL;
        let mut last_state: Option<PanelState> = None;
        loop {
            match snapshot_panel_state() {
                Ok(state) => {
                    backoff = CONNECT_BACKOFF_INITIAL;
                    if last_state.as_ref() != Some(&state) {
                        if tx.send(PanelUpdate::Snapshot(state.clone())).is_err() {
                            break;
                        }
                        last_state = Some(state);
                    }
                    // Sleep up to `poll_interval`, but wake immediately on
                    // any pulse from the event-subscribe thread.
                    match wakeup_rx.recv_timeout(poll_interval) {
                        Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        ?e,
                        retry_ms = backoff.as_millis(),
                        "panel: daemon snapshot failed; will retry"
                    );
                    thread::sleep(backoff);
                    backoff = (backoff * 2).min(CONNECT_BACKOFF_MAX);
                }
            }
        }
    });
}

fn snapshot_panel_state() -> Result<PanelState, Box<dyn std::error::Error>> {
    let response = daemon_request(IpcRequest::GetState)?;
    let canvas = match response {
        IpcResponse::State(state) => state,
        other => {
            return Err(
                format!("daemon returned unexpected response to GetState: {other:?}").into(),
            )
        }
    };
    Ok(canvas_to_panel_state(&canvas))
}

fn daemon_request(request: IpcRequest) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    let socket_path = daemon_socket_path();
    let stream = UnixStream::connect(&socket_path)
        .map_err(|e| format!("connect {}: {e}", socket_path.display()))?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;

    let json = serde_json::to_string(&request)?;
    let mut writer = stream.try_clone()?;
    writeln!(writer, "{json}")?;
    writer.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.is_empty() {
        return Err("daemon closed connection without responding".into());
    }
    let response: IpcResponse = serde_json::from_str(line.trim())?;
    Ok(response)
}

/// Project the daemon's `CanvasState` onto a `PanelState`. Cluster name → workspace
/// num if numeric (sway convention); focused/visible from cluster.enabled +
/// active selection; urgent is unavailable through the daemon today, so always
/// false. Focused title comes from the focused window's title.
fn canvas_to_panel_state(canvas: &CanvasState) -> PanelState {
    let focused_title = canvas
        .windows
        .iter()
        .find(|w| canvas.clusters.iter().any(|c| c.last_focus == Some(w.id)))
        .map(|w| w.title.clone())
        .filter(|t| !t.is_empty());

    let workspaces = canvas
        .clusters
        .iter()
        .map(|cluster| {
            let num = cluster.name.parse::<i32>().ok().filter(|n| *n > 0);
            WorkspaceState {
                id: cluster.id as i64,
                num,
                name: cluster.name.clone(),
                output: canvas.output.name.clone(),
                focused: cluster.enabled,
                visible: cluster.enabled,
                urgent: false,
            }
        })
        .collect();

    PanelState {
        workspaces,
        focused_title,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::contracts::{Cluster, OutputState, Window};

    fn fixture_canvas() -> CanvasState {
        CanvasState {
            clusters: vec![
                Cluster {
                    id: 1,
                    name: "1".into(),
                    enabled: false,
                    windows: vec![],
                    last_focus: None,
                    ..Default::default()
                },
                Cluster {
                    id: 2,
                    name: "play".into(),
                    enabled: true,
                    windows: vec![10],
                    last_focus: Some(10),
                    ..Default::default()
                },
            ],
            windows: vec![Window {
                id: 10,
                title: "Editor — main.rs".into(),
                ..Default::default()
            }],
            output: OutputState {
                name: "winit".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn projects_clusters_to_workspaces() {
        let panel = canvas_to_panel_state(&fixture_canvas());
        assert_eq!(panel.workspaces.len(), 2);
        assert_eq!(panel.workspaces[0].name, "1");
        assert_eq!(panel.workspaces[0].num, Some(1));
        assert!(!panel.workspaces[0].focused);
        assert_eq!(panel.workspaces[1].name, "play");
        assert_eq!(panel.workspaces[1].num, None);
        assert!(panel.workspaces[1].focused);
    }

    #[test]
    fn picks_focused_title_from_active_cluster() {
        let panel = canvas_to_panel_state(&fixture_canvas());
        assert_eq!(panel.focused_title.as_deref(), Some("Editor — main.rs"));
    }

    #[test]
    fn empty_canvas_yields_empty_panel() {
        let panel = canvas_to_panel_state(&CanvasState::default());
        assert!(panel.workspaces.is_empty());
        assert!(panel.focused_title.is_none());
    }
}
