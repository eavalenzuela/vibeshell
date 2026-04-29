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
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_buffer;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::utils::Serial;
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
    delegate_seat, delegate_shm, delegate_xdg_decoration, delegate_xdg_shell,
};

use crate::state::{ClientState, Vibewm};

// --- Compositor ---

impl CompositorHandler for Vibewm {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("client without ClientState")
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
        handle_layer_commit(self, surface);
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
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
        self.broadcast_workspace_or_window();
    }
}

delegate_seat!(Vibewm);

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

    fn move_request(&mut self, _surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // TODO(W1c): wire interactive move grabs through `crates/wm`'s drag flow.
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
        // TODO(W1c): wire interactive resize grabs.
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
        let mut map = layer_map_for_output(&output);
        match map.map_layer(&LayerSurface::new(surface, namespace.clone())) {
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
}
