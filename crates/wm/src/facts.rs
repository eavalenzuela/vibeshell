//! Backend-neutral snapshot types.
//!
//! `WmFacts` is the per-tick view of the WM the daemon ingests. The Sway
//! backend translates `swayipc` tree/workspaces/outputs into these; future
//! backends produce the same shape from their own state.

use std::collections::BTreeMap;

use common::contracts::{Cluster, OutputState, Window, WindowId};

#[derive(Debug, Clone, PartialEq)]
pub struct WmFacts {
    pub clusters: Vec<Cluster>,
    pub windows: Vec<Window>,
    pub window_geometry: BTreeMap<WindowId, (i32, i32)>,
    pub output: OutputState,
    pub outputs: Vec<String>,
    pub primary_output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterFact {
    pub id: u64,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowFact {
    pub id: WindowId,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputFact {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub focused: bool,
    pub primary: bool,
}
