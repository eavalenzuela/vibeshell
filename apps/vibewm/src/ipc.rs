//! Vibewm control IPC server.
//!
//! Listens on the vibewm-control socket and dispatches `VibewmRequest`
//! messages against the live `Vibewm` state. The daemon (`vibeshellctl`)
//! runs the client side via `wm::WlrootsBackend`.
//!
//! For W1c-1 most query handlers return stubbed/empty responses — vibewm
//! doesn't model workspaces/window-ids yet (W1c-2). The seam is in place so
//! `WM_BACKEND=wlroots` is dispatchable end-to-end.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use wm::vibewm_ipc::{vibewm_socket_path, VibewmEvent, VibewmRequest, VibewmResponse};
use wm::WmFacts;

use crate::state::Vibewm;

pub fn init_ipc(event_loop: &mut EventLoop<Vibewm>) -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = vibewm_socket_path();
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;
    tracing::info!(path = %socket_path.display(), "vibewm-control listening");

    event_loop.handle().insert_source(
        Generic::new(listener, Interest::READ, Mode::Level),
        |_, listener, state| {
            loop {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        if let Err(e) = handle_connection(stream, state) {
                            tracing::warn!(?e, "vibewm-control: connection handler error");
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        tracing::warn!(?e, "vibewm-control: accept error");
                        break;
                    }
                }
            }
            Ok(PostAction::Continue)
        },
    )?;

    Ok(())
}

fn handle_connection(
    mut stream: UnixStream,
    state: &mut Vibewm,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim().is_empty() {
        return Ok(());
    }
    let request: VibewmRequest = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => {
            let resp = VibewmResponse::Error {
                message: format!("parse request: {e}"),
            };
            write_response(&mut stream, &resp)?;
            return Ok(());
        }
    };

    let response = dispatch_request(state, request, stream.try_clone()?);
    if let Some(resp) = response {
        write_response(&mut stream, &resp)?;
    }
    // Subscribe-mode hands the stream off to `state.event_subscribers`; the
    // connection stays open until vibewm tears it down on shutdown or until
    // the client disconnects.
    Ok(())
}

fn write_response(stream: &mut UnixStream, response: &VibewmResponse) -> std::io::Result<()> {
    let line = serde_json::to_string(response).map_err(std::io::Error::other)?;
    writeln!(stream, "{line}")?;
    stream.flush()
}

/// Returns `Some(response)` if the connection expects an immediate reply +
/// close, or `None` if the connection has been retained by the server (i.e.
/// Subscribe handed it to `state.event_subscribers`).
fn dispatch_request(
    state: &mut Vibewm,
    request: VibewmRequest,
    stream: UnixStream,
) -> Option<VibewmResponse> {
    match request {
        VibewmRequest::Ping => Some(VibewmResponse::Pong),
        VibewmRequest::Snapshot => Some(VibewmResponse::Snapshot(state.snapshot_facts())),
        VibewmRequest::ApplyLayoutOps { ops } => {
            // W1c-1 stub: log, no actual move/resize yet (W1c-3).
            tracing::info!(count = ops.len(), "vibewm-control: ApplyLayoutOps (stub)");
            Some(VibewmResponse::Ack)
        }
        VibewmRequest::FocusWindow { window } => {
            tracing::info!(window, "vibewm-control: FocusWindow (stub)");
            Some(VibewmResponse::Ack)
        }
        VibewmRequest::ActivateCluster { cluster } => {
            tracing::info!(cluster, "vibewm-control: ActivateCluster (stub)");
            Some(VibewmResponse::Ack)
        }
        VibewmRequest::CreateNamedWorkspace { name } => {
            tracing::info!(name, "vibewm-control: CreateNamedWorkspace (stub)");
            Some(VibewmResponse::Ack)
        }
        VibewmRequest::BackAndForthWorkspace => {
            tracing::info!("vibewm-control: BackAndForthWorkspace (stub)");
            Some(VibewmResponse::Ack)
        }
        VibewmRequest::ExitSession => {
            tracing::info!("vibewm-control: ExitSession");
            state.loop_signal.stop();
            Some(VibewmResponse::Ack)
        }
        VibewmRequest::ReloadWmConfig => {
            tracing::info!("vibewm-control: ReloadWmConfig (stub)");
            Some(VibewmResponse::Ack)
        }
        VibewmRequest::FocusedWindow => {
            // W1c-1 stub: no window-id model yet.
            Some(VibewmResponse::FocusedWindow { window: None })
        }
        VibewmRequest::Subscribe => {
            // Send the initial Subscribed reply on `stream`, then hand the
            // (cloned) stream to the subscribers list. Returning None tells
            // the caller to skip the auto-reply path.
            let mut hand = match stream.try_clone() {
                Ok(s) => s,
                Err(e) => {
                    return Some(VibewmResponse::Error {
                        message: format!("subscribe clone: {e}"),
                    });
                }
            };
            if let Err(e) = write_response(&mut hand, &VibewmResponse::Subscribed) {
                return Some(VibewmResponse::Error {
                    message: format!("subscribe initial reply: {e}"),
                });
            }
            // Long-lived subscribers don't honor the per-request read/write
            // timeout — drop both so push events can take their time.
            let _ = stream.set_read_timeout(None);
            let _ = stream.set_write_timeout(None);
            state.event_subscribers.push(stream);
            None
        }
    }
}

impl Vibewm {
    /// Build a `WmFacts` snapshot from current compositor state. W1c-1 stub:
    /// returns mostly empty data. W1c-2 will fill in clusters/windows/output
    /// as vibewm grows a real workspace + window-id model.
    pub fn snapshot_facts(&self) -> WmFacts {
        WmFacts {
            clusters: Vec::new(),
            windows: Vec::new(),
            window_geometry: Default::default(),
            output: Default::default(),
            outputs: self.space.outputs().map(|o| o.name()).collect(),
            primary_output: None,
        }
    }

    /// Push a `WorkspaceOrWindow` event to all subscribed clients. Drops
    /// disconnected subscribers. Called from W1c-3 onwards when smithay
    /// events drive workspace/window changes.
    #[allow(dead_code)]
    pub fn broadcast_workspace_or_window(&mut self) {
        let event = VibewmResponse::Event(VibewmEvent::WorkspaceOrWindow);
        let line = match serde_json::to_string(&event) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(?e, "broadcast: serialize event failed");
                return;
            }
        };
        self.event_subscribers
            .retain_mut(|stream| writeln!(stream, "{line}").is_ok() && stream.flush().is_ok());
    }
}
