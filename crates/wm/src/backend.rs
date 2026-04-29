//! `WmBackend` trait: the seam every WM backend implements.
//!
//! The daemon (`apps/vibeshellctl`) talks to whichever backend is active
//! through this trait. Today only `sway::SwayBackend` exists; a `WlrootsBackend`
//! lands in W1b.

use std::sync::mpsc::Receiver;

use common::contracts::{ClusterId, WindowId};

use crate::facts::WmFacts;
use crate::layout::LayoutOp;

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("WM backend unavailable: {0}")]
    Unavailable(String),
    #[error("WM backend rejected command `{command}`: {reason}")]
    CommandRejected { command: String, reason: String },
    #[error("WM backend `{0}` is not yet implemented")]
    NotImplemented(String),
    #[error("WM backend error: {0}")]
    Other(String),
}

/// Coalesced event signal from the WM. Backends collapse low-level events
/// (workspace add/move/focus, window new/close/title-change) into a single
/// "something changed, re-ingest" pulse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WmSignal {
    WorkspaceOrWindow,
}

/// Tree-of-windows + workspace + output snapshot, plus the few imperative
/// operations the daemon needs to drive the WM.
pub trait WmBackend: Send {
    /// Pull a fresh snapshot. Called every time the daemon wants to ingest WM
    /// state into its `StateOwner`.
    fn snapshot(&mut self) -> Result<WmFacts, BackendError>;

    /// Apply a batch of `LayoutOp`s in one round-trip. Returns Ok if the
    /// batch is accepted; per-op rejections are logged as warnings.
    fn apply_layout_ops(&mut self, ops: &[LayoutOp]) -> Result<(), BackendError>;

    /// Focus a specific window.
    fn focus_window(&mut self, window: WindowId) -> Result<(), BackendError>;

    /// Switch to a workspace whose internal id matches `cluster`.
    fn activate_cluster(&mut self, cluster: ClusterId) -> Result<(), BackendError>;

    /// Create a workspace by name (selects + back-and-forths so it exists for
    /// ingest without staying focused). Returns when the workspace is gone.
    fn create_named_workspace(&mut self, name: &str) -> Result<(), BackendError>;

    /// Switch back to the previously-focused workspace.
    fn back_and_forth_workspace(&mut self) -> Result<(), BackendError>;

    /// Tell the WM to exit the current session.
    fn exit_session(&mut self) -> Result<(), BackendError>;

    /// Tell the WM to reload its own configuration. Sway runs `reload`;
    /// future backends interpret this as their config-reload action.
    fn reload_wm_config(&mut self) -> Result<(), BackendError>;

    /// Currently-focused window id, or `None` if no client is focused.
    fn focused_window(&mut self) -> Result<Option<WindowId>, BackendError>;

    /// Return true if the WM is reachable at all.
    fn is_alive(&mut self) -> bool;

    /// Spawn a background thread that pumps coalesced workspace/window events
    /// onto a channel. The thread lives as long as the WM connection holds.
    fn spawn_event_stream(&self) -> Result<Receiver<WmSignal>, BackendError>;
}
