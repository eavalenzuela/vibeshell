//! Vibewm control IPC server.
//!
//! Listens on the vibewm-control socket and dispatches `VibewmRequest`
//! messages against the live `Vibewm` state. The daemon (`vibeshellctl`)
//! runs the client side via `wm::WlrootsBackend`.
//!
//! For W1c-1 most query handlers return stubbed/empty responses — vibewm
//! doesn't model workspaces/window-ids yet (W1c-2). The seam is in place so
//! `WM_BACKEND=wlroots` is dispatchable end-to-end.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};

use common::contracts::{
    Cluster, OutputState, Window as DomainWindow, WindowId, WindowRole, WindowState,
};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use smithay::utils::{Size, SERIAL_COUNTER};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use wm::layout::LayoutOp;
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
            let applied = state.apply_layout_ops(&ops);
            tracing::info!(
                requested = ops.len(),
                applied,
                "vibewm-control: ApplyLayoutOps"
            );
            Some(VibewmResponse::Ack)
        }
        VibewmRequest::FocusWindow { window } => {
            if state.set_keyboard_focus_to(window) {
                state.broadcast_workspace_or_window();
                Some(VibewmResponse::Ack)
            } else {
                Some(VibewmResponse::Error {
                    message: format!("window {window} not found"),
                })
            }
        }
        VibewmRequest::ActivateCluster { cluster } => {
            if state.model.activate_cluster(cluster) {
                state.sync_cluster_visibility();
                state.broadcast_workspace_or_window();
                state.broadcast_cluster_mapped(cluster);
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
            let switched = state.model.activate_cluster(id);
            if switched {
                state.sync_cluster_visibility();
            }
            state.broadcast_workspace_or_window();
            if switched {
                state.broadcast_cluster_mapped(id);
            }
            tracing::info!(
                name,
                cluster_id = id,
                "vibewm-control: CreateNamedWorkspace"
            );
            Some(VibewmResponse::Ack)
        }
        VibewmRequest::BackAndForthWorkspace => {
            if state.model.back_and_forth() {
                state.sync_cluster_visibility();
                state.broadcast_workspace_or_window();
                let active = state.model.active_cluster;
                state.broadcast_cluster_mapped(active);
            }
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
            tracing::info!(
                subscribers = state.event_subscribers.len(),
                "vibewm-control: client subscribed to events"
            );
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

    /// Apply a batch of `LayoutOp`s by repositioning + resizing the
    /// corresponding smithay windows. Position changes are *animated*
    /// (W1c-25-4) via the `window_anims` map; the render loop's
    /// `tick_window_anims` call interpolates between frames. Size changes
    /// fire immediately as xdg_toplevel configures — animating those
    /// would fight the client's redraw cadence and produce flicker.
    /// Returns the number of ops successfully dispatched (skips ops whose
    /// window id isn't registered).
    pub fn apply_layout_ops(&mut self, ops: &[LayoutOp]) -> usize {
        let now = std::time::Instant::now();
        let mut applied = 0;
        for op in ops {
            let Some(window) = self.model.windows.get(&op.window_id).cloned() else {
                continue;
            };
            // Capture current position from the space; fall back to the
            // last-known cache for windows that aren't yet mapped (first
            // map after a fresh new_toplevel).
            let current = self
                .space
                .element_location(&window)
                .map(|loc| (loc.x, loc.y))
                .or_else(|| self.last_known_position.get(&op.window_id).copied())
                .unwrap_or((op.target.x, op.target.y));
            crate::anim::stage(
                &mut self.window_anims,
                op.window_id,
                current,
                (op.target.x, op.target.y),
                now,
                crate::anim::DEFAULT_DURATION,
            );
            // Stamp the target as the canonical "last known" so cluster
            // switches restore at the *destination*, not whatever
            // mid-anim position they happened to be at when an unmap
            // races with a layout change.
            self.last_known_position
                .insert(op.window_id, (op.target.x, op.target.y));
            // Size via xdg_toplevel configure — client redraws at new size on
            // its next commit.
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|s| {
                    s.size = Some(Size::from((op.target.width, op.target.height)));
                });
                toplevel.send_pending_configure();
            }
            applied += 1;
        }
        applied
    }

    /// Drive the in-flight position animations one frame. Called by the
    /// udev render loop. Returns true while at least one animation is
    /// still in progress, so the caller can schedule another render to
    /// continue the animation; false when the map is empty (and the
    /// caller can skip the extra render kick).
    ///
    /// Under the default `winit` backend (no `udev` feature) nothing
    /// drives this — winit windows don't have the same dive→tile
    /// transition; the dev-mode visual is "good enough" without it.
    #[cfg_attr(not(feature = "udev"), allow(dead_code))]
    pub fn tick_window_anims(&mut self, now: std::time::Instant) -> bool {
        if self.window_anims.is_empty() {
            return false;
        }
        let mut completed: Vec<WindowId> = Vec::new();
        // Snapshot the (window, sample) pairs so we can release the
        // borrow on `window_anims` before mutating `space`.
        let samples: Vec<(WindowId, (i32, i32), bool)> = self
            .window_anims
            .iter()
            .map(|(id, anim)| {
                let (pos, done) = anim.sample(now);
                (*id, pos, done)
            })
            .collect();
        for (id, pos, done) in samples {
            if let Some(window) = self.model.windows.get(&id).cloned() {
                self.space.map_element(window, pos, false);
            }
            if done {
                completed.push(id);
            }
        }
        for id in completed {
            self.window_anims.remove(&id);
        }
        !self.window_anims.is_empty()
    }

    /// After a cluster activation, walk the model and ensure exactly the
    /// active cluster's windows are mapped in the smithay `Space`. Inactive
    /// cluster windows get unmapped (kept alive in the registry, but
    /// invisible). Their last position is cached so reactivation restores
    /// them in place.
    pub fn sync_cluster_visibility(&mut self) {
        let active = self.model.active_cluster;
        let active_ids: BTreeSet<WindowId> = self
            .model
            .clusters
            .iter()
            .find(|c| c.id == active)
            .map(|c| c.windows.iter().copied().collect())
            .unwrap_or_default();

        let entries: Vec<(WindowId, smithay::desktop::Window)> = self
            .model
            .windows
            .iter()
            .map(|(id, w)| (*id, w.clone()))
            .collect();

        for (id, window) in entries {
            if active_ids.contains(&id) {
                let pos = self.last_known_position.get(&id).copied().unwrap_or((0, 0));
                self.space.map_element(window, pos, false);
            } else {
                if let Some(loc) = self.space.element_location(&window) {
                    self.last_known_position.insert(id, (loc.x, loc.y));
                }
                self.space.unmap_elem(&window);
            }
        }
    }

    /// Push a `WorkspaceOrWindow` event to all subscribed clients. Drops
    /// disconnected subscribers. Called whenever the model changes
    /// meaningfully — new toplevel, focus change, cluster activation,
    /// toplevel destroyed.
    pub fn broadcast_workspace_or_window(&mut self) {
        self.broadcast_event(VibewmEvent::WorkspaceOrWindow);
    }

    /// Push a `ClusterMapped` event after vibewm finishes (re)mapping a
    /// cluster's windows in `sync_cluster_visibility`. The daemon uses this
    /// to advance its `ZoomTransition` past CompositorRemapping (W1c-25-3+).
    pub fn broadcast_cluster_mapped(&mut self, cluster: common::contracts::ClusterId) {
        let window_count = self
            .model
            .clusters
            .iter()
            .find(|c| c.id == cluster)
            .map(|c| c.windows.len() as u32)
            .unwrap_or(0);
        self.broadcast_event(VibewmEvent::ClusterMapped {
            cluster,
            window_count,
        });
    }

    fn broadcast_event(&mut self, event: VibewmEvent) {
        let line = match serde_json::to_string(&VibewmResponse::Event(event)) {
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
