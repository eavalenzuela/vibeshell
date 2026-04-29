//! Wire protocol for the daemon ↔ vibewm control channel.
//!
//! Vibewm listens on a unix socket and accepts JSON-line framed requests.
//! Each connection serves either a single request/response (`one_shot`) or a
//! long-lived event subscription (started by sending [`VibewmRequest::Subscribe`]).
//!
//! `WlrootsBackend` (in `crates/wm/src/wlroots_backend.rs`) is the canonical
//! client; vibewm itself contains the server. Keep the protocol additive — the
//! daemon and compositor are versioned together within the same workspace, so
//! we don't need full backwards-compat, but breaking changes still need a
//! visible struct field rename so the JSON parse fails loudly.

use std::path::PathBuf;

use common::contracts::{ClusterId, WindowId};
use serde::{Deserialize, Serialize};

use crate::facts::WmFacts;
use crate::layout::LayoutOp;

/// Default socket path: `$VIBEWM_SOCKET` if set, otherwise
/// `$XDG_RUNTIME_DIR/vibewm-control.sock`, falling back to `/tmp/...` if
/// `$XDG_RUNTIME_DIR` is also unset.
pub fn vibewm_socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("VIBEWM_SOCKET") {
        return PathBuf::from(custom);
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/tmp".to_owned());
    PathBuf::from(runtime).join("vibewm-control.sock")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum VibewmRequest {
    /// Pull a fresh `WmFacts` snapshot.
    Snapshot,
    /// Apply a batch of layout ops (move/resize windows).
    ApplyLayoutOps { ops: Vec<LayoutOp> },
    /// Focus a specific window.
    FocusWindow { window: WindowId },
    /// Switch to the workspace with this cluster id.
    ActivateCluster { cluster: ClusterId },
    /// Create a workspace by name. Like sway: switches to it so it's live for ingest.
    CreateNamedWorkspace { name: String },
    /// Switch to the previously focused workspace.
    BackAndForthWorkspace,
    /// Tell the compositor to exit (cleanly stops the calloop event loop).
    ExitSession,
    /// Reload compositor-side config (e.g. keybindings, output layout).
    ReloadWmConfig,
    /// Return the currently-focused window id, if any.
    FocusedWindow,
    /// Take over this connection as a long-lived event subscription. Server
    /// replies once with `Subscribed`, then pushes `Event { ... }` lines.
    Subscribe,
    /// Liveness probe.
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum VibewmResponse {
    Ack,
    Pong,
    Error { message: String },
    Snapshot(WmFacts),
    FocusedWindow { window: Option<WindowId> },
    Subscribed,
    Event(VibewmEvent),
}

/// Events vibewm pushes to subscribed clients.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum VibewmEvent {
    /// A workspace or window changed (created, destroyed, focused, retitled).
    /// Clients re-snapshot in response. Mirrors `WmSignal::WorkspaceOrWindow`.
    WorkspaceOrWindow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip_through_json() {
        let req = VibewmRequest::CreateNamedWorkspace {
            name: "play".into(),
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: VibewmRequest = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, VibewmRequest::CreateNamedWorkspace { name } if name == "play"));
    }

    #[test]
    fn response_round_trip_through_json() {
        let resp = VibewmResponse::FocusedWindow { window: Some(42) };
        let s = serde_json::to_string(&resp).unwrap();
        let back: VibewmResponse = serde_json::from_str(&s).unwrap();
        assert!(matches!(
            back,
            VibewmResponse::FocusedWindow { window: Some(42) }
        ));
    }

    #[test]
    fn socket_path_honors_env_override() {
        // SAFETY: pre-test, single-threaded.
        std::env::set_var("VIBEWM_SOCKET", "/tmp/test-vibewm.sock");
        assert_eq!(vibewm_socket_path(), PathBuf::from("/tmp/test-vibewm.sock"));
        std::env::remove_var("VIBEWM_SOCKET");
    }
}
