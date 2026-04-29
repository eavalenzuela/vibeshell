//! XWayland integration: spawns the X server, attaches an `X11Wm`, and bridges
//! X11 windows into vibewm's model + space.
//!
//! Adapted from smithay's anvil example. Without this, X11-only clients
//! (electron apps, IDEs, lots of legacy tooling) can't run under vibewm.
//!
//! Compiled out under `--no-default-features` (e.g. for headless CI builds
//! where pulling Xwayland would be wasteful).

use std::process::Stdio;

use smithay::reexports::calloop::EventLoop;
use smithay::xwayland::{X11Wm, XWayland, XWaylandEvent};
use tracing::{info, warn};

use crate::state::Vibewm;

/// Spawn `XWayland`, register its calloop event source, and on the `Ready`
/// event bind an `X11Wm` to vibewm. Best-effort: failures degrade gracefully
/// to "no X11 client support this run" rather than aborting the compositor.
pub fn start_xwayland(event_loop: &mut EventLoop<Vibewm>, state: &Vibewm) {
    let display_handle = state.display_handle.clone();

    let (xwayland, client) = match XWayland::spawn(
        &display_handle,
        None,
        std::iter::empty::<(String, String)>(),
        true,
        Stdio::null(),
        Stdio::null(),
        |_| (),
    ) {
        Ok(pair) => pair,
        Err(e) => {
            warn!(
                ?e,
                "vibewm: failed to spawn XWayland; X11 clients won't work this session"
            );
            return;
        }
    };

    let _dh_for_event = display_handle.clone();
    let result = event_loop
        .handle()
        .insert_source(xwayland, move |event, _, data| match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => match X11Wm::start_wm(data.loop_handle.clone(), x11_socket, client.clone()) {
                Ok(wm) => {
                    info!(
                        display_number,
                        "vibewm: XWayland ready, X11Wm attached (DISPLAY=:{display_number})"
                    );
                    data.xwm = Some(wm);
                    data.xdisplay = Some(display_number);
                }
                Err(e) => {
                    warn!(?e, "vibewm: failed to attach X11Wm");
                }
            },
            XWaylandEvent::Error => {
                warn!("vibewm: XWayland exited with an error");
            }
        });

    if let Err(e) = result {
        warn!(
            ?e,
            "vibewm: failed to insert XWayland source into event loop"
        );
    }
}
