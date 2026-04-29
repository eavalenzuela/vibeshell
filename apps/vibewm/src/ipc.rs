//! Vibewm control IPC server.
//!
//! Listens on the vibewm-control socket and dispatches `VibewmRequest`
//! messages against the live `Vibewm` state. The daemon (`vibeshellctl`)
//! runs the client side via `wm::WlrootsBackend`.
//!
//! For W1c-1 most query handlers return stubbed/empty responses — vibewm
//! doesn't model workspaces/window-ids yet (W1c-2). The seam is in place so
//! `WM_BACKEND=wlroots` is dispatchable end-to-end.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};

use common::contracts::{
    Cluster, OutputState, Window as DomainWindow, WindowId, WindowRole, WindowState,
};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
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
            // W1c-2: model is in place but actual smithay move/resize is W1c-3.
            tracing::info!(count = ops.len(), "vibewm-control: ApplyLayoutOps (stub)");
            Some(VibewmResponse::Ack)
        }
        VibewmRequest::FocusWindow { window } => match state.set_keyboard_focus_to(window) {
            true => Some(VibewmResponse::Ack),
            false => Some(VibewmResponse::Error {
                message: format!("window {window} not found"),
            }),
        },
        VibewmRequest::ActivateCluster { cluster } => {
            if state.model.activate_cluster(cluster) {
                Some(VibewmResponse::Ack)
            } else {
                Some(VibewmResponse::Error {
                    message: format!("cluster {cluster} not found"),
                })
            }
        }
        VibewmRequest::CreateNamedWorkspace { name } => {
            // Sway-compat: `workspace "play"` switches to the workspace,
            // creating it if it didn't exist. Mirror that here.
            let id = state
                .model
                .find_cluster_by_name(&name)
                .unwrap_or_else(|| state.model.create_cluster(name.clone()));
            state.model.activate_cluster(id);
            tracing::info!(
                name,
                cluster_id = id,
                "vibewm-control: CreateNamedWorkspace"
            );
            Some(VibewmResponse::Ack)
        }
        VibewmRequest::BackAndForthWorkspace => {
            state.model.back_and_forth();
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
        VibewmRequest::FocusedWindow => Some(VibewmResponse::FocusedWindow {
            window: state.focused_window_id(),
        }),
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
    /// Build a `WmFacts` snapshot of vibewm's current state.
    ///
    /// Walks the model registry, pulls live title/app_id off each toplevel,
    /// merges geometry from the smithay `Space`, and reports the winit
    /// output. `prune_dead` runs first so closed-but-not-yet-cleaned-up
    /// surfaces don't leak into snapshots.
    pub fn snapshot_facts(&mut self) -> WmFacts {
        self.model.prune_dead();

        let clusters: Vec<Cluster> = self
            .model
            .clusters
            .iter()
            .map(|c| Cluster {
                id: c.id,
                name: c.name.clone(),
                // Spatial overview-canvas coords don't exist in vibewm yet
                // (W1c-3+). Daemon's persisted state owns them.
                x: 0.0,
                y: 0.0,
                enabled: c.id == self.model.active_cluster,
                windows: c.windows.clone(),
                last_focus: c.windows.last().copied(),
                recency: c.windows.clone(),
            })
            .collect();

        let mut windows = Vec::with_capacity(self.model.windows.len());
        let mut window_geometry: BTreeMap<WindowId, (i32, i32)> = BTreeMap::new();
        for (&id, win) in &self.model.windows {
            let toplevel = match win.toplevel() {
                Some(t) => t,
                None => continue,
            };
            let surface = toplevel.wl_surface();
            let (title, app_id) = with_states(surface, |states| {
                let data = states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .map(|d| d.lock().expect("XdgToplevelSurfaceData mutex poisoned"));
                match data {
                    Some(d) => (d.title.clone().unwrap_or_default(), d.app_id.clone()),
                    None => (String::new(), None),
                }
            });
            let cluster_id = self
                .model
                .clusters
                .iter()
                .find(|c| c.windows.contains(&id))
                .map(|c| c.id);
            if let Some(geo) = self.space.element_geometry(win) {
                window_geometry.insert(id, (geo.size.w, geo.size.h));
            }
            windows.push(DomainWindow {
                id,
                title,
                app_id,
                class: None,
                role: WindowRole::Normal,
                state: WindowState::Tiled,
                cluster_id,
                transient_for: None,
                manual_cluster_override: false,
                manual_position_override: false,
            });
        }

        let (output_state, output_names, primary_output) = self
            .space
            .outputs()
            .next()
            .cloned()
            .map(|output| {
                let mode = output.current_mode();
                let scale = output.current_scale().fractional_scale();
                let state = OutputState {
                    name: output.name(),
                    width: mode.map(|m| m.size.w).unwrap_or(0),
                    height: mode.map(|m| m.size.h).unwrap_or(0),
                    scale,
                };
                let names = self.space.outputs().map(|o| o.name()).collect();
                (state, names, Some(output.name()))
            })
            .unwrap_or_else(|| (OutputState::default(), Vec::new(), None));

        WmFacts {
            clusters,
            windows,
            window_geometry,
            output: output_state,
            outputs: output_names,
            primary_output,
        }
    }

    /// Map the seat's current keyboard focus to a `WindowId` via the model.
    pub fn focused_window_id(&self) -> Option<WindowId> {
        let keyboard = self.seat.get_keyboard()?;
        let surface = keyboard.current_focus()?;
        self.model.window_id_for_surface(&surface)
    }

    /// Tell the seat to focus the toplevel registered under `window`.
    /// Returns false if the window id is unknown or has no toplevel.
    pub fn set_keyboard_focus_to(&mut self, window: WindowId) -> bool {
        let Some(win) = self.model.windows.get(&window).cloned() else {
            return false;
        };
        let Some(toplevel) = win.toplevel() else {
            return false;
        };
        let target = toplevel.wl_surface().clone();
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Some(target), serial);
            true
        } else {
            false
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
