use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub type WindowId = u64;
pub type ClusterId = u64;

/// Full canvas snapshot returned by `GetState`. The daemon (`vibeshellctl`) owns
/// the authoritative copy; clients (overlay, panel, launcher) receive clones via
/// JSON IPC and must not assume their copy stays fresh.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CanvasState {
    /// Monotonic counter bumped on every state mutation in the daemon. Used by
    /// clients to detect concurrent edits when issuing optimistic-CAS mutations
    /// (currently only observed by drag flows; see `IpcRequest::BeginClusterDrag`).
    pub state_revision: u64,
    pub zoom: ZoomLevel,
    /// Fallback viewport used when `output_viewports` has no entry for the
    /// output being rendered. Single-monitor setups use this exclusively.
    pub viewport: Viewport,
    /// Per-output overview viewport keyed by Sway output name (e.g. `"DP-1"`).
    /// Populated once the overlay has panned/zoomed on that output.
    pub output_viewports: HashMap<String, Viewport>,
    pub clusters: Vec<Cluster>,
    pub windows: Vec<Window>,
    pub output: OutputState,
}

impl CanvasState {
    /// Returns the viewport for the named output, falling back to the global
    /// `viewport` when no per-output override exists or when `output` is `None`.
    pub fn viewport_for_output(&self, output: Option<&str>) -> Viewport {
        output
            .and_then(|name| self.output_viewports.get(name).cloned())
            .unwrap_or_else(|| self.viewport.clone())
    }
}

/// A group of windows with a world-space position on the overview canvas.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Cluster {
    pub id: ClusterId,
    pub name: String,
    /// World-canvas coordinates (not screen-space). The origin is the overview
    /// center; positive Y is down. Rendering applies `Viewport` to map these
    /// into pixels.
    pub x: f64,
    pub y: f64,
    /// Whether the cluster's underlying Sway workspace is currently visible.
    /// Mirrors Sway's `workspace.visible` flag at ingest time.
    pub enabled: bool,
    /// Stable insertion order of windows in this cluster. Round-trips through
    /// serialization preserve order (see `round_trip` tests).
    pub windows: Vec<WindowId>,
    /// Last window focused inside the cluster. Used when re-entering the
    /// cluster from Overview to restore focus without a fresh selection.
    pub last_focus: Option<WindowId>,
    /// MRU-ordered window ids (most recent first). Used for within-cluster
    /// cycling and focus restoration.
    pub recency: Vec<WindowId>,
}

impl Default for Cluster {
    fn default() -> Self {
        Self {
            id: 0,
            name: "Cluster".to_owned(),
            x: 0.0,
            y: 0.0,
            enabled: true,
            windows: Vec::new(),
            last_focus: None,
            recency: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Window {
    pub id: WindowId,
    pub title: String,
    pub app_id: Option<String>,
    pub class: Option<String>,
    pub role: WindowRole,
    pub state: WindowState,
    /// Cluster the window belongs to, or `None` if it has not been assigned
    /// (freshly-mapped windows, transient dialogs without a parent cluster).
    pub cluster_id: Option<ClusterId>,
    pub transient_for: Option<WindowId>,
    /// Set when the user explicitly moved the window into this cluster
    /// (`MoveWindowToCluster`). Suppresses subsequent auto-cluster-by-app-id
    /// reassignment.
    pub manual_cluster_override: bool,
    /// Set when Sway reports geometry diverging from the cluster's intended
    /// layout by >10px, or when the window is fullscreen/has overlay hints.
    /// Signals the layout engine to leave the window alone.
    pub manual_position_override: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OutputState {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
}

impl Default for OutputState {
    fn default() -> Self {
        Self {
            name: "default".to_owned(),
            width: 1920,
            height: 1080,
            scale: 1.0,
        }
    }
}

/// The three navigation modes of the Continuum WM zoom hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", content = "id")]
pub enum ZoomLevel {
    /// All clusters visible on the canvas; pan/zoom free-form.
    #[default]
    Overview,
    /// Zoomed into a single cluster; windows tiled within it.
    Cluster(ClusterId),
    /// Zoomed into a single window within its cluster.
    Focus(WindowId),
}

/// Pan/zoom state for the overview canvas. `(x, y)` is the world-space point at
/// the viewport center; `scale` is pixels-per-world-unit (1.0 = identity).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindowRole {
    #[default]
    Normal,
    Dialog,
    Utility,
    Scratchpad,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    #[default]
    Tiled,
    Floating,
    Fullscreen,
    /// Window is hidden but still present in Sway (e.g. on an inactive
    /// workspace or marked as scratchpad). Sway has no true "minimized" state,
    /// hence the "-like" suffix.
    MinimizedLike,
}

/// All client→daemon requests. Serialized as tagged JSON (`{"type": "set_zoom", ...}`).
///
/// The daemon processes requests serially on a single state-owning thread;
/// see `apps/vibeshellctl/src/main.rs::handle_ipc_request`. Most variants
/// map 1:1 to a `vibeshellctl` CLI subcommand.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcRequest {
    SetZoom {
        level: ZoomLevel,
    },
    SetFocusZoomTarget {
        window: WindowId,
    },
    ZoomInMode,
    ZoomOutMode,
    CycleStripForward,
    CycleStripBackward,
    CycleContextStrip {
        direction: ContextStripDirection,
    },
    Pan {
        dx: f64,
        dy: f64,
    },
    /// Mark a cluster as selected without changing zoom level. Used by overlay
    /// hover/keyboard navigation to tee up subsequent `EnterKeyboardMoveModeSelected`
    /// or other selection-dependent commands.
    SelectCluster {
        cluster: ClusterId,
    },
    /// Start a pointer-drag session on a cluster. In-flight drags are protected
    /// from concurrent `GetState` ingest by `drag_origin` excluding the dragged
    /// cluster from Sway-fact merges (see `merge_into_live_canvas_excluding`).
    BeginClusterDrag {
        cluster: ClusterId,
        pointer_canvas_x: f64,
        pointer_canvas_y: f64,
    },
    /// Move the currently-dragging cluster to the given world-space coords.
    /// Overlay throttles these to ~30 Hz and dispatches them detached.
    UpdateClusterDrag {
        cluster_x: f64,
        cluster_y: f64,
    },
    /// End the drag, persisting the cluster's final position.
    CommitClusterDrag,
    /// Abort the drag, restoring the cluster to its pre-drag coordinates.
    CancelClusterDrag,
    /// Pan the overview on the given output (or the global viewport when
    /// `output` is `None`). Coordinates are world-space deltas.
    OverviewPan {
        dx: f64,
        dy: f64,
        output: Option<String>,
    },
    /// Zoom the overview on the given output around a world-space anchor.
    /// `delta` is a signed scale step (positive = zoom in).
    OverviewZoom {
        delta: f64,
        anchor_canvas_x: f64,
        anchor_canvas_y: f64,
        output: Option<String>,
    },
    /// Enter keyboard-move mode for `cluster`, recording its current position
    /// as the restore point if the user cancels.
    EnterKeyboardMoveMode {
        cluster: ClusterId,
    },
    /// Enter keyboard-move mode for whichever cluster is currently selected
    /// (via `SelectCluster`). No-op if nothing is selected.
    EnterKeyboardMoveModeSelected,
    /// Nudge the keyboard-move cluster by `(dx, dy)` world-space units.
    KeyboardMoveBy {
        dx: f64,
        dy: f64,
    },
    /// Exit keyboard-move mode, persisting the cluster's new position.
    CommitKeyboardMove,
    /// Exit keyboard-move mode, restoring the cluster to its entry position.
    CancelKeyboardMove,
    /// Switch focus to the next/previous cluster in MRU order (Mod+Tab style).
    CycleCluster {
        direction: CycleDirection,
    },
    /// Create a new cluster at the given world-space position. The daemon
    /// batches the matching Sway workspace creation in a single `run_command`
    /// call (Phase 6 Work F).
    CreateCluster {
        name: String,
        x: f64,
        y: f64,
    },
    /// Explicitly move a window into a cluster. Sets `manual_cluster_override`
    /// so auto-cluster-by-app-id won't reassign it later.
    MoveWindowToCluster {
        window: WindowId,
        cluster: ClusterId,
    },
    RenameCluster {
        cluster: ClusterId,
        name: String,
    },
    /// Fetch a full `CanvasState` snapshot. Overlay polls this at ~1200 ms.
    GetState,
    /// Ask the daemon to re-read `~/.config/vibeshell/config.toml`.
    /// Standalone apps reload via SIGHUP; this request routes through the
    /// daemon specifically.
    ReloadConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextStripDirection {
    Next,
    Previous,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CycleDirection {
    Forward,
    Backward,
}

/// All daemon→client responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    /// Request accepted. Used for all mutations that don't return data.
    Ack,
    /// Full canvas state, returned by `GetState`.
    State(CanvasState),
    /// Mutation failed; `message` is a human-readable reason. Some errors are
    /// structured JSON (see `state_store.rs` handlers).
    Error { message: String },
}

/// Path to the daemon's Unix socket. Defaults to
/// `$XDG_RUNTIME_DIR/vibeshell-daemon.sock`, falling back to `/tmp` when
/// `XDG_RUNTIME_DIR` is unset.
pub fn daemon_socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned());
    PathBuf::from(runtime_dir).join("vibeshell-daemon.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_cluster(id: ClusterId, window_ids: &[WindowId]) -> Cluster {
        Cluster {
            id,
            name: format!("Cluster {id}"),
            x: id as f64 * 10.0,
            y: id as f64 * -5.0,
            enabled: true,
            windows: window_ids.to_vec(),
            last_focus: window_ids.first().copied(),
            recency: window_ids.to_vec(),
        }
    }

    fn fixture_window(id: WindowId, cluster_id: ClusterId) -> Window {
        Window {
            id,
            title: format!("Window {id}"),
            app_id: Some("fixture-app".into()),
            class: Some("fixture-class".into()),
            role: WindowRole::Normal,
            state: WindowState::Tiled,
            cluster_id: Some(cluster_id),
            transient_for: None,
            manual_cluster_override: false,
            manual_position_override: false,
        }
    }

    #[test]
    fn round_trip_state_fixture() {
        let fixture = CanvasState {
            state_revision: 9,
            zoom: ZoomLevel::Cluster(7),
            viewport: Viewport {
                x: 42.0,
                y: -13.0,
                scale: 1.15,
            },
            output_viewports: HashMap::from([(
                "DP-1".to_owned(),
                Viewport {
                    x: 42.0,
                    y: -13.0,
                    scale: 1.15,
                },
            )]),
            clusters: vec![Cluster {
                id: 7,
                name: "Work".into(),
                x: 1.0,
                y: 2.0,
                enabled: true,
                windows: vec![100],
                last_focus: Some(100),
                recency: vec![100],
            }],
            windows: vec![Window {
                id: 100,
                title: "Terminal".into(),
                app_id: Some("foot".into()),
                class: Some("foot".into()),
                role: WindowRole::Normal,
                state: WindowState::Tiled,
                cluster_id: Some(7),
                transient_for: None,
                manual_cluster_override: true,
                manual_position_override: false,
            }],
            output: OutputState::default(),
        };

        let json = serde_json::to_string_pretty(&fixture).expect("serialize fixture");
        let parsed: CanvasState = serde_json::from_str(&json).expect("parse fixture");
        assert_eq!(parsed, fixture);
    }

    #[test]
    fn round_trip_ipc_fixture() {
        let fixture = IpcRequest::MoveWindowToCluster {
            window: 100,
            cluster: 7,
        };

        let json = serde_json::to_string(&fixture).expect("serialize request");
        let parsed: IpcRequest = serde_json::from_str(&json).expect("parse request");
        assert_eq!(parsed, fixture);
    }

    #[test]
    fn round_trip_ipc_set_focus_zoom_target() {
        let fixture = IpcRequest::SetFocusZoomTarget { window: 100 };

        let json = serde_json::to_string(&fixture).expect("serialize request");
        let parsed: IpcRequest = serde_json::from_str(&json).expect("parse request");
        assert_eq!(parsed, fixture);
    }

    #[test]
    fn round_trip_ipc_cycle_context_strip() {
        let fixture = IpcRequest::CycleContextStrip {
            direction: ContextStripDirection::Previous,
        };

        let json = serde_json::to_string(&fixture).expect("serialize request");
        let parsed: IpcRequest = serde_json::from_str(&json).expect("parse request");
        assert_eq!(parsed, fixture);
    }

    #[test]
    fn round_trip_ipc_phase3_requests() {
        for fixture in [
            IpcRequest::ZoomInMode,
            IpcRequest::ZoomOutMode,
            IpcRequest::CycleStripForward,
            IpcRequest::CycleStripBackward,
        ] {
            let json = serde_json::to_string(&fixture).expect("serialize request");
            let parsed: IpcRequest = serde_json::from_str(&json).expect("parse request");
            assert_eq!(parsed, fixture);
        }
    }

    #[test]
    fn round_trip_ipc_overview_interaction_requests() {
        for fixture in [
            IpcRequest::SelectCluster { cluster: 7 },
            IpcRequest::BeginClusterDrag {
                cluster: 7,
                pointer_canvas_x: 123.0,
                pointer_canvas_y: 456.0,
            },
            IpcRequest::UpdateClusterDrag {
                cluster_x: 130.0,
                cluster_y: 460.0,
            },
            IpcRequest::CommitClusterDrag,
            IpcRequest::CancelClusterDrag,
            IpcRequest::OverviewPan {
                dx: 15.0,
                dy: -2.5,
                output: Some("HDMI-A-1".to_owned()),
            },
            IpcRequest::OverviewZoom {
                delta: -1.0,
                anchor_canvas_x: 12.5,
                anchor_canvas_y: 20.0,
                output: Some("DP-1".to_owned()),
            },
            IpcRequest::EnterKeyboardMoveMode { cluster: 7 },
            IpcRequest::EnterKeyboardMoveModeSelected,
            IpcRequest::KeyboardMoveBy {
                dx: -20.0,
                dy: 40.0,
            },
            IpcRequest::CommitKeyboardMove,
            IpcRequest::CancelKeyboardMove,
        ] {
            let json = serde_json::to_string(&fixture).expect("serialize request");
            let parsed: IpcRequest = serde_json::from_str(&json).expect("parse request");
            assert_eq!(parsed, fixture);
        }
    }

    #[test]
    fn round_trip_ipc_cycle_cluster() {
        for fixture in [
            IpcRequest::CycleCluster {
                direction: CycleDirection::Forward,
            },
            IpcRequest::CycleCluster {
                direction: CycleDirection::Backward,
            },
        ] {
            let json = serde_json::to_string(&fixture).expect("serialize request");
            let parsed: IpcRequest = serde_json::from_str(&json).expect("parse request");
            assert_eq!(parsed, fixture);
        }
    }

    #[test]
    fn old_minimal_state_payload_uses_defaults() {
        let parsed: CanvasState = serde_json::from_str("{}").expect("parse minimal state");
        assert_eq!(parsed, CanvasState::default());
    }

    #[test]
    fn transition_fixtures_keep_window_order_after_20_round_trips() {
        let fixtures = [
            (1_u64, vec![101]),
            (2_u64, vec![201, 202]),
            (3_u64, vec![301, 302, 303, 304]),
        ];

        for (cluster_id, window_ids) in fixtures {
            let mut state = CanvasState {
                state_revision: 42,
                zoom: ZoomLevel::Cluster(cluster_id),
                viewport: Viewport::default(),
                output_viewports: HashMap::new(),
                clusters: vec![fixture_cluster(cluster_id, &window_ids)],
                windows: window_ids
                    .iter()
                    .map(|window_id| fixture_window(*window_id, cluster_id))
                    .collect(),
                output: OutputState::default(),
            };

            for _ in 0..20 {
                let json = serde_json::to_string(&state).expect("serialize state fixture");
                state = serde_json::from_str(&json).expect("deserialize state fixture");
            }

            assert_eq!(state.clusters[0].windows, window_ids);
            assert_eq!(state.clusters[0].recency, window_ids);
            assert_eq!(state.zoom, ZoomLevel::Cluster(cluster_id));
        }
    }
}
