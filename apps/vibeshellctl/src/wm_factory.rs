//! WM backend factory.
//!
//! Reads `WM_BACKEND` (default `sway`) and constructs the corresponding
//! `wm::WmBackend`. CLI commands construct one per invocation; the daemon
//! constructs one at startup and reuses it.

use wm::{BackendError, WmBackend};

pub fn connect_default() -> Result<Box<dyn WmBackend>, BackendError> {
    let kind = std::env::var("WM_BACKEND").unwrap_or_else(|_| "sway".to_owned());
    match kind.as_str() {
        "sway" => Ok(Box::new(sway::SwayBackend::connect()?)),
        "wlroots" => Err(BackendError::NotImplemented(
            "wlroots: `apps/vibewm` runs but daemon control-plane bridge is Phase 8 W1c".into(),
        )),
        other => Err(BackendError::Other(format!(
            "unknown WM_BACKEND `{other}` (expected `sway` or `wlroots`)"
        ))),
    }
}
