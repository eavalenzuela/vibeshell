//! Smithay protocol handler trait impls + delegate macros.
//!
//! All wired through the central `Vibewm` state. Move/resize grabs are
//! deliberately not implemented for W1b — clients will display, but the
//! "drag from titlebar / resize edge" interactive flows are stubbed. The
//! daemon-driven layout engine (`crates/wm`) handles geometry once W1c
//! bridges the daemon and compositor.

use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::{
    layer_map_for_output, LayerSurface, PopupKind, PopupManager, Space, Window,
};
use smithay::input::pointer::{Focus, GrabStartData as PointerGrabStartData};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_buffer;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::utils::{Rectangle, Serial};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler,
    CompositorState,
};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::selection::data_device::{
    set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
    ServerDndGrabHandler,
};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{
    delegate_compositor, delegate_data_device, delegate_layer_shell, delegate_output,
    delegate_pointer_gestures, delegate_seat, delegate_shm, delegate_xdg_decoration,
    delegate_xdg_shell,
};

use crate::grabs::{handle_resize_commit, MoveSurfaceGrab, ResizeEdge, ResizeSurfaceGrab};
use crate::state::{ClientState, Vibewm};

// --- Compositor ---

impl CompositorHandler for Vibewm {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // XWayland clients carry smithay's `XWaylandClientData` rather than
        // vibewm's own `ClientState`. Check both before bailing.
        #[cfg(feature = "xwayland")]
        {
            use smithay::xwayland::XWaylandClientData;
            if let Some(state) = client.get_data::<XWaylandClientData>() {
                return &state.compositor_state;
            }
        }
        &client
            .get_data::<ClientState>()
            .expect("client without ClientState or XWaylandClientData")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.space.elements().find(|w| {
                w.toplevel()
                    .map(|t| t.wl_surface() == &root)
                    .unwrap_or(false)
            }) {
                window.on_commit();
            }
        }

        handle_xdg_commit(&mut self.popups, &self.space, surface);
        handle_resize_commit(&mut self.space, surface);
        handle_layer_commit(self, surface);

        // A surface commit is damage we should reflect on screen. Under
        // the udev backend, kick a render on each tracked DRM device. The
        // helper is a no-op under the winit backend, so this stays
        // backend-neutral.
        #[cfg(feature = "udev")]
        crate::udev::schedule_render(self);
    }
}

impl BufferHandler for Vibewm {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Vibewm {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(Vibewm);
delegate_shm!(Vibewm);

// --- Seat ---

impl SeatHandler for Vibewm {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        image: smithay::input::pointer::CursorImageStatus,
    ) {
        self.cursor_status = image;
        // The pointer just told us to swap glyph/surface — kick a render so
        // the change is visible without waiting on the next surface commit.
        #[cfg(feature = "udev")]
        crate::udev::schedule_render(self);
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
        self.broadcast_workspace_or_window();
    }
}

delegate_seat!(Vibewm);
delegate_pointer_gestures!(Vibewm);

// --- Data device (clipboard / DnD) ---

impl SelectionHandler for Vibewm {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Vibewm {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

// W1b: drag-and-drop is wired enough for clipboard/selection to function;
// active drag grabs are stubbed (TODO(W1c) full DnD interaction).
impl ClientDndGrabHandler for Vibewm {}
impl ServerDndGrabHandler for Vibewm {}

delegate_data_device!(Vibewm);

// --- Output ---

impl OutputHandler for Vibewm {}
delegate_output!(Vibewm);

// --- Xdg shell ---

impl XdgShellHandler for Vibewm {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Send a sensible initial-configure size so clients don't pick their
        // own. We use the active output's mode minus a margin so the window
        // is visible without overlapping the panel area. Daemon-driven layout
        // (W1c-3 ApplyLayoutOps) replaces this once the daemon ingests the
        // new toplevel.
        let initial_size = self
            .space
            .outputs()
            .next()
            .and_then(|o| o.current_mode())
            .map(|m| (m.size.w, m.size.h))
            .map(|(w, h)| (w.saturating_sub(64).max(640), h.saturating_sub(96).max(480)))
            .unwrap_or((1024, 768));
        surface.with_pending_state(|state| {
            state.size = Some(smithay::utils::Size::from(initial_size));
        });

        let window = Window::new_wayland_window(surface);
        let id = self.model.register_window(window.clone());
        tracing::info!(
            window_id = id,
            initial_w = initial_size.0,
            initial_h = initial_size.1,
            "vibewm: new toplevel"
        );
        self.space.map_element(window, (0, 0), false);
        self.last_known_position.insert(id, (0, 0));
        self.broadcast_workspace_or_window();
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat = match Seat::<Self>::from_resource(&seat) {
            Some(s) => s,
            None => return,
        };
        let wl_surface = surface.wl_surface();
        let Some(start_data) = check_pointer_grab(&seat, wl_surface, serial) else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| {
                w.toplevel()
                    .map(|t| t.wl_surface() == wl_surface)
                    .unwrap_or(false)
            })
            .cloned()
        else {
            return;
        };
        let Some(initial_window_location) = self.space.element_location(&window) else {
            return;
        };
        let Some(pointer) = seat.get_pointer() else {
            return;
        };
        pointer.set_grab(
            self,
            MoveSurfaceGrab {
                start_data,
                window,
                initial_window_location,
            },
            serial,
            Focus::Clear,
        );
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let seat = match Seat::<Self>::from_resource(&seat) {
            Some(s) => s,
            None => return,
        };
        let wl_surface = surface.wl_surface();
        let Some(start_data) = check_pointer_grab(&seat, wl_surface, serial) else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| {
                w.toplevel()
                    .map(|t| t.wl_surface() == wl_surface)
                    .unwrap_or(false)
            })
            .cloned()
        else {
            return;
        };
        let Some(initial_window_location) = self.space.element_location(&window) else {
            return;
        };
        let initial_window_size = window.geometry().size;
        let Some(pointer) = seat.get_pointer() else {
            return;
        };

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
        });
        surface.send_pending_configure();

        let resize_edges: ResizeEdge = edges.into();
        let grab = ResizeSurfaceGrab::start(
            start_data,
            window,
            resize_edges,
            Rectangle::new(initial_window_location, initial_window_size),
        );
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // TODO(W1c): popup grabs.
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let target = surface.wl_surface();
        if let Some(id) = self.model.window_id_for_surface(target) {
            tracing::info!(window_id = id, "vibewm: toplevel destroyed");
            self.last_known_position.remove(&id);
            self.model.unregister_window(id);
            self.broadcast_workspace_or_window();
        }
    }
}

delegate_xdg_shell!(Vibewm);

/// Verify the pointer is currently grabbing on this surface — otherwise the
/// move/resize_request is a stale (or malicious) ask we should ignore.
fn check_pointer_grab(
    seat: &Seat<Vibewm>,
    surface: &WlSurface,
    serial: Serial,
) -> Option<PointerGrabStartData<Vibewm>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }
    Some(start_data)
}

// --- Xdg decoration ---

// Force server-side decorations on every toplevel. vibewm doesn't have a
// titlebar yet, but advertising SSD is what stops Gtk and other clients from
// drawing CSDs awkwardly on top of vibewm's own window. Once vibewm has a
// proper window-frame (W1d+), this becomes the integration point for
// per-window decoration choices.
impl XdgDecorationHandler for Vibewm {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_pending_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_pending_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_pending_configure();
    }
}

delegate_xdg_decoration!(Vibewm);

fn handle_xdg_commit(popups: &mut PopupManager, space: &Space<Window>, surface: &WlSurface) {
    if let Some(window) = space
        .elements()
        .find(|w| {
            w.toplevel()
                .map(|t| t.wl_surface() == surface)
                .unwrap_or(false)
        })
        .cloned()
    {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .expect("XdgToplevelSurfaceData missing")
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        if !initial_configure_sent {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_configure();
            }
        }
    }

    popups.commit(surface);
    if let Some(popup) = popups.find_popup(surface) {
        match popup {
            PopupKind::Xdg(ref xdg) => {
                if !xdg.is_initial_configure_sent() {
                    let _ = xdg.send_configure();
                }
            }
            PopupKind::InputMethod(_) => {}
        }
    }
}

// --- Layer shell ---

impl WlrLayerShellHandler for Vibewm {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.space.outputs().next().cloned());
        let Some(output) = output else {
            tracing::warn!(
                namespace,
                "layer surface created with no output available; closing"
            );
            surface.send_close();
            return;
        };
        let output_name = output.name();
        let layer_surface = LayerSurface::new(surface.clone(), namespace.clone());
        let mut map = layer_map_for_output(&output);
        match map.map_layer(&layer_surface) {
            Ok(()) => tracing::info!(
                namespace,
                output = %output_name,
                ?layer,
                "vibewm: layer surface mapped"
            ),
            Err(e) => {
                tracing::warn!(?e, namespace, output = %output_name, "vibewm: layer surface map failed")
            }
        }
        // smithay's `LayerMap::arrange` populates the pending size but
        // refuses to send the *initial* configure (per spec: initial
        // configure must follow the client's initial commit). The
        // `new_layer_surface` callback fires exactly once, on that initial
        // commit, so this is where we send it. Without this the client
        // never receives a size and stays stuck never attaching a buffer.
        drop(map);
        surface.send_configure();
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let outputs: Vec<_> = self.space.outputs().cloned().collect();
        for output in outputs {
            let mut map = layer_map_for_output(&output);
            let layer = map
                .layers()
                .find(|l| l.layer_surface() == &surface)
                .cloned();
            if let Some(layer) = layer {
                let namespace = layer.namespace().to_owned();
                let output_name = output.name();
                map.unmap_layer(&layer);
                tracing::info!(
                    namespace,
                    output = %output_name,
                    "vibewm: layer surface destroyed"
                );
            }
        }
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        let _ = self.popups.track_popup(PopupKind::Xdg(popup));
    }
}

delegate_layer_shell!(Vibewm);

fn handle_layer_commit(state: &mut Vibewm, surface: &WlSurface) {
    let outputs: Vec<_> = state.space.outputs().cloned().collect();
    for output in outputs {
        let mut map = layer_map_for_output(&output);
        let needs_arrange = map
            .layer_for_surface(surface, smithay::desktop::WindowSurfaceType::TOPLEVEL)
            .is_some();
        if needs_arrange {
            map.arrange();
        }
    }

    // If this layer surface requested keyboard focus (Exclusive or
    // OnDemand), grab it now. Without this an `exclusive` overlay like
    // the launcher renders fine but never receives key events. Pick the
    // topmost layer that wants focus across all outputs (Overlay > Top).
    use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer as LayerEnum};
    let mut best: Option<(u8, WlSurface)> = None;
    for output in state.space.outputs().cloned().collect::<Vec<_>>() {
        let map = layer_map_for_output(&output);
        for layer in map.layers() {
            let cached = layer.cached_state();
            let wants_focus = matches!(
                cached.keyboard_interactivity,
                KeyboardInteractivity::Exclusive | KeyboardInteractivity::OnDemand
            );
            if !wants_focus {
                continue;
            }
            let rank = match layer.layer() {
                LayerEnum::Overlay => 3,
                LayerEnum::Top => 2,
                LayerEnum::Bottom => 1,
                LayerEnum::Background => 0,
            };
            let candidate = layer.wl_surface().clone();
            if best.as_ref().is_none_or(|(r, _)| rank > *r) {
                best = Some((rank, candidate));
            }
        }
    }
    if let Some((_, focus_surface)) = best {
        if let Some(keyboard) = state.seat.get_keyboard() {
            let current = keyboard.current_focus();
            if current.as_ref() != Some(&focus_surface) {
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                keyboard.set_focus(state, Some(focus_surface), serial);
            }
        }
    }
}

// --- XWayland ---
//
// X11 client integration: an `X11Wm` (the X11-side window manager) is owned
// by `Vibewm::xwm` once `xwayland::start_xwayland` finishes its handshake.
// Smithay routes X server events through `XwmHandler` so we can map X11
// surfaces into the same `Space<Window>` as xdg toplevels.
//
// Override-redirect windows (X tooltips/popups that bypass the WM) are mapped
// at their requested location and never have grab/configure logic; they're
// effectively passthrough.

#[cfg(feature = "xwayland")]
mod xwm_handler {
    use smithay::delegate_xwayland_shell;
    use smithay::desktop::Window;
    use smithay::input::pointer::{Focus, GrabStartData as PointerGrabStartData};
    use smithay::utils::{Logical, Rectangle};
    use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
    use smithay::xwayland::xwm::{Reorder, ResizeEdge as X11ResizeEdge, XwmId};
    use smithay::xwayland::{X11Surface, X11Wm, XwmHandler};

    use crate::grabs::{MoveSurfaceGrab, ResizeEdge as ResizeBits, ResizeSurfaceGrab};
    use crate::state::Vibewm;

    impl XwmHandler for Vibewm {
        fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
            self.xwm
                .as_mut()
                .expect("xwm_state called before X11Wm attached")
        }

        fn new_window(&mut self, _xwm: XwmId, window: X11Surface) {
            tracing::info!(?window, "vibewm: new X11 window");
        }

        fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {
            // Override-redirect surfaces (X tooltips, popups, etc.) ride
            // outside the WM model. We don't track them in `model` — they
            // appear in the Space when smithay maps them via the surface
            // commit path.
        }

        fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
            // Tell the X11 server to render the window. After this, the X11
            // surface emits `map_window_notify` once mapped on screen.
            if let Err(e) = window.set_mapped(true) {
                tracing::warn!(?e, "vibewm: X11 set_mapped(true) failed");
                return;
            }
            let win = Window::new_x11_window(window);
            let id = self.model.register_window(win.clone());
            self.space.map_element(win, (0, 0), false);
            self.last_known_position.insert(id, (0, 0));
            self.broadcast_workspace_or_window();
            tracing::info!(window_id = id, "vibewm: X11 window mapped");
        }

        fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
            let geometry = window.geometry();
            let win = Window::new_x11_window(window);
            self.space
                .map_element(win, (geometry.loc.x, geometry.loc.y), false);
        }

        fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
            // Find and unmap from the space; remove from the model so the
            // daemon's next snapshot drops it.
            let target_window = self
                .space
                .elements()
                .find(|w| matches!(w.x11_surface(), Some(s) if s == &window))
                .cloned();
            if let Some(win) = target_window {
                self.space.unmap_elem(&win);
                if let Some((id, _)) = self
                    .model
                    .windows
                    .iter()
                    .find(|(_, w)| w.x11_surface().map(|s| s == &window).unwrap_or(false))
                {
                    let id = *id;
                    self.last_known_position.remove(&id);
                    self.model.unregister_window(id);
                }
            }
            self.broadcast_workspace_or_window();
        }

        fn destroyed_window(&mut self, _xwm: XwmId, _window: X11Surface) {
            // unmapped_window already pruned the model + space. Nothing to do
            // here for now.
        }

        fn configure_request(
            &mut self,
            _xwm: XwmId,
            window: X11Surface,
            x: Option<i32>,
            y: Option<i32>,
            w: Option<u32>,
            h: Option<u32>,
            _reorder: Option<Reorder>,
        ) {
            // Honor what the client asked for. Daemon's layout engine will
            // override on the next ApplyLayoutOps tick if it wants.
            let mut geo = window.geometry();
            if let Some(x) = x {
                geo.loc.x = x;
            }
            if let Some(y) = y {
                geo.loc.y = y;
            }
            if let Some(w) = w {
                geo.size.w = w as i32;
            }
            if let Some(h) = h {
                geo.size.h = h as i32;
            }
            let _ = window.configure(Some(geo));
        }

        fn configure_notify(
            &mut self,
            _xwm: XwmId,
            window: X11Surface,
            geometry: Rectangle<i32, Logical>,
            _above: Option<u32>,
        ) {
            // Server notified us the window's geometry changed. Reposition it
            // in the space if we have a tracked Window for this surface.
            let target = self
                .space
                .elements()
                .find(|w| matches!(w.x11_surface(), Some(s) if s == &window))
                .cloned();
            if let Some(win) = target {
                self.space
                    .map_element(win, (geometry.loc.x, geometry.loc.y), false);
            }
        }

        fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
            // X11 doesn't carry a wayland `PointerGrabStartData`, so we
            // synthesize one from (the seat's current pointer location,
            // the X11 surface's associated wl_surface). The wayland-side
            // `MoveSurfaceGrab` then takes it from there exactly as if a
            // wayland client had asked for an interactive move.
            let Some(target_window) = self
                .space
                .elements()
                .find(|w| matches!(w.x11_surface(), Some(s) if s == &window))
                .cloned()
            else {
                return;
            };
            let Some(initial_window_location) = self.space.element_location(&target_window) else {
                return;
            };
            let Some(pointer) = self.seat.get_pointer() else {
                return;
            };
            let Some(wl_surface) = window.wl_surface() else {
                return;
            };

            let location = pointer.current_location();
            let start_data = PointerGrabStartData {
                focus: Some((wl_surface, (0.0, 0.0).into())),
                button: 0x110, // BTN_LEFT — buttons in xwm are X11 codes; map
                // generously to "primary mouse" since the
                // grab only checks `current_pressed.contains`
                // for release detection.
                location,
            };
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            pointer.set_grab(
                self,
                MoveSurfaceGrab {
                    start_data,
                    window: target_window,
                    initial_window_location,
                },
                serial,
                Focus::Clear,
            );
        }

        fn resize_request(
            &mut self,
            _xwm: XwmId,
            window: X11Surface,
            _button: u32,
            resize_edge: X11ResizeEdge,
        ) {
            let Some(target_window) = self
                .space
                .elements()
                .find(|w| matches!(w.x11_surface(), Some(s) if s == &window))
                .cloned()
            else {
                return;
            };
            let Some(initial_window_location) = self.space.element_location(&target_window) else {
                return;
            };
            let initial_window_size = target_window.geometry().size;
            let Some(pointer) = self.seat.get_pointer() else {
                return;
            };
            let Some(wl_surface) = window.wl_surface() else {
                return;
            };

            let location = pointer.current_location();
            let start_data = PointerGrabStartData {
                focus: Some((wl_surface, (0.0, 0.0).into())),
                button: 0x110,
                location,
            };

            // Translate xwm's ResizeEdge into our ResizeBits flags. The two
            // enums encode the same eight directions; map by name.
            let edges = match resize_edge {
                X11ResizeEdge::Top => ResizeBits::TOP,
                X11ResizeEdge::Bottom => ResizeBits::BOTTOM,
                X11ResizeEdge::Left => ResizeBits::LEFT,
                X11ResizeEdge::Right => ResizeBits::RIGHT,
                X11ResizeEdge::TopLeft => ResizeBits::TOP_LEFT,
                X11ResizeEdge::TopRight => ResizeBits::TOP_RIGHT,
                X11ResizeEdge::BottomLeft => ResizeBits::BOTTOM_LEFT,
                X11ResizeEdge::BottomRight => ResizeBits::BOTTOM_RIGHT,
            };
            let initial_rect = Rectangle::new(initial_window_location, initial_window_size);
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            let grab = ResizeSurfaceGrab::start(start_data, target_window, edges, initial_rect);
            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
    }

    impl XWaylandShellHandler for Vibewm {
        fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
            &mut self.xwayland_shell_state
        }
    }

    delegate_xwayland_shell!(Vibewm);
}
