//! vibewm — vibeshell's wlroots-style wayland compositor.
//!
//! Phase 8 W1b: minimal smithay compositor. Boots in a winit window, accepts
//! wayland clients, hosts xdg-shell toplevels and wlr-layer-shell surfaces.
//! Daemon integration, DRM backend, scene-graph effects, gesture input, and
//! parity with `WmBackend` land in W1c+.

use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

mod handlers;
mod input;
mod ipc;
mod model;
mod state;
mod winit;

use state::Vibewm;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    let mut event_loop: EventLoop<Vibewm> = EventLoop::try_new()?;
    let display: Display<Vibewm> = Display::new()?;
    let mut state = Vibewm::new(&mut event_loop, display);

    crate::winit::init_winit(&mut event_loop, &mut state)?;
    crate::ipc::init_ipc(&mut event_loop)?;

    // Children spawned from this process inherit `WAYLAND_DISPLAY`, so e.g.
    // `vibewm -- vibeshell-panel` runs the panel against this compositor.
    // SAFETY: set_var is unsafe in newer Rust editions because env state is
    // process-global; we set it pre-event-loop, before any threads are spawned.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);
    }

    tracing::info!(
        socket = %state.socket_name.to_string_lossy(),
        "vibewm: ready (W1b)",
    );

    let mut args = std::env::args().skip(1);
    if matches!(args.next().as_deref(), Some("--")) {
        let cmd: Vec<String> = args.collect();
        if !cmd.is_empty() {
            spawn_child(&cmd);
        }
    }

    event_loop.run(None, &mut state, |_| {})?;
    Ok(())
}

fn init_logging() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_env("VIBESHELL_LOG")
        .or_else(|_| tracing_subscriber::EnvFilter::try_from_env("RUST_LOG"))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

fn spawn_child(cmd: &[String]) {
    let (program, rest) = cmd.split_first().expect("non-empty command");
    match std::process::Command::new(program).args(rest).spawn() {
        Ok(child) => tracing::info!(program, pid = child.id(), "spawned client"),
        Err(e) => tracing::warn!(?e, program, "failed to spawn client"),
    }
}
