//! Panel display state — backend-neutral.
//!
//! These types describe what the panel renders (workspace strip, focused
//! window title). Lived in `crates/sway` historically because their initial
//! source was sway IPC; W1c-5 relocated them so the panel can pull them
//! from the daemon (which already owns a backend-neutral snapshot).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceState {
    /// Stable cluster id from the daemon's snapshot (was sway workspace id
    /// in the pre-W1c-5 implementation).
    pub id: i64,
    /// Numeric prefix if the workspace name is a digit (sway convention),
    /// otherwise `None`.
    pub num: Option<i32>,
    pub name: String,
    pub output: String,
    pub focused: bool,
    pub visible: bool,
    pub urgent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PanelState {
    pub workspaces: Vec<WorkspaceState>,
    pub focused_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelUpdate {
    Snapshot(PanelState),
}
