//! Vibewm compositor state.
//!
//! Holds smithay's per-protocol state objects, the desktop `Space`, and the
//! seat/keyboard/pointer handles. One `Vibewm` lives in the calloop event loop
//! for the duration of the compositor process.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use smithay::desktop::{PopupManager, Space, Window, WindowSurfaceType};
use smithay::input::{Seat, SeatState};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{Logical, Point};
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::socket::ListeningSocketSource;
#[cfg(feature = "xwayland")]
use smithay::wayland::xwayland_shell::XWaylandShellState;
#[cfg(feature = "xwayland")]
use smithay::xwayland::X11Wm;
use wm::layout::WindowId;

use crate::model::VibewmModel;

pub struct Vibewm {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, Vibewm>,
    pub loop_signal: LoopSignal,

    pub space: Space<Window>,
    pub popups: PopupManager,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    /// Held to keep the xdg-decoration global alive. The handler is a stateless
    /// "always-SSD" responder so we never actually read this field back.
    #[allow(dead_code)]
    pub xdg_decoration_state: XdgDecorationState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    /// Held to keep the xdg-output global alive for the compositor's lifetime.
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Vibewm>,
    pub data_device_state: DataDeviceState,
    pub seat: Seat<Self>,

    /// Long-lived subscriber connections for the vibewm-control IPC. Each
    /// stream blocks-on-read on the client side and gets pushed JSON-line
    /// `VibewmResponse::Event(...)` messages when state changes.
    pub event_subscribers: Vec<UnixStream>,

    /// Workspace + window-id registry. The daemon snapshots this to build a
    /// `WmFacts`; vibewm itself uses it to route IPC commands to smithay
    /// handles.
    pub model: VibewmModel,

    /// Last known logical position for each window, captured before unmapping
    /// on cluster switch so we can re-map at the same spot on reactivation.
    pub last_known_position: HashMap<WindowId, (i32, i32)>,

    /// Holds the xwayland_shell_v1 protocol global so X11 clients can
    /// associate their wl_surface with their X11 window. Initialized
    /// regardless of whether XWayland is actually spawned yet.
    #[cfg(feature = "xwayland")]
    pub xwayland_shell_state: XWaylandShellState,

    /// The XWayland-side window manager, populated after `XWayland::spawn`
    /// emits its `Ready` event in `start_xwayland`. None until then.
    #[cfg(feature = "xwayland")]
    pub xwm: Option<X11Wm>,

    /// X11 display number XWayland is serving on (e.g. `Some(0)` for `:0`).
    /// Used to derive the `DISPLAY` env var for X11 child processes.
    #[cfg(feature = "xwayland")]
    pub xdisplay: Option<u32>,

    /// DRM/KMS backend state. `Some` after `udev::run_udev` has initialized
    /// the seat + first DRM device. Stays `None` under `VIBEWM_BACKEND=winit`.
    #[cfg(feature = "udev")]
    pub udev: Option<crate::udev::UdevState>,
}

impl Vibewm {
    pub fn new(event_loop: &mut EventLoop<'static, Self>, display: Display<Self>) -> Self {
        let start_time = std::time::Instant::now();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "winit");
        seat.add_keyboard(Default::default(), 200, 25)
            .expect("add_keyboard failed");
        seat.add_pointer();

        #[cfg(feature = "xwayland")]
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&dh);

        let socket_name = Self::init_wayland_listener(display, event_loop);
        let loop_handle = event_loop.handle();
        let loop_signal = event_loop.get_signal();

        Self {
            start_time,
            socket_name,
            display_handle: dh,
            loop_handle,
            loop_signal,
            space: Space::default(),
            popups: PopupManager::default(),
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            layer_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            seat,
            event_subscribers: Vec::new(),
            model: VibewmModel::new(),
            last_known_position: HashMap::new(),
            #[cfg(feature = "xwayland")]
            xwayland_shell_state,
            #[cfg(feature = "xwayland")]
            xwm: None,
            #[cfg(feature = "xwayland")]
            xdisplay: None,
            #[cfg(feature = "udev")]
            udev: None,
        }
    }

    fn init_wayland_listener(
        display: Display<Self>,
        event_loop: &mut EventLoop<'static, Self>,
    ) -> OsString {
        let listening_socket =
            ListeningSocketSource::new_auto().expect("ListeningSocketSource::new_auto failed");
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                    .expect("insert_client failed");
            })
            .expect("failed to register wayland listening socket source");

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    tracing::debug!("wayland display source: dispatch_clients fired");
                    // SAFETY: the display is owned by the calloop event loop for
                    // the lifetime of this source and never dropped while in use.
                    // Smithay's Generic source exposes `&mut Display` only via
                    // `get_mut()` which is `unsafe` by design.
                    #[allow(unsafe_code)]
                    unsafe {
                        display.get_mut().dispatch_clients(state).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .expect("failed to register wayland display dispatch source");

        socket_name
    }

    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, surface_pos)| (surface, (surface_pos + location).to_f64()))
            })
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
