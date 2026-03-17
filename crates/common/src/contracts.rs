use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub type WindowId = u64;
pub type ClusterId = u64;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CanvasState {
    pub state_revision: u64,
    pub zoom: ZoomLevel,
    pub viewport: Viewport,
    pub output_viewports: HashMap<String, Viewport>,
    pub clusters: Vec<Cluster>,
    pub windows: Vec<Window>,
    pub output: OutputState,
}

impl CanvasState {
    pub fn viewport_for_output(&self, output: Option<&str>) -> Viewport {
        output
            .and_then(|name| self.output_viewports.get(name).cloned())
            .unwrap_or_else(|| self.viewport.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Cluster {
    pub id: ClusterId,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub enabled: bool,
    pub windows: Vec<WindowId>,
    pub last_focus: Option<WindowId>,
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
    pub cluster_id: Option<ClusterId>,
    pub transient_for: Option<WindowId>,
    pub manual_cluster_override: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", content = "id")]
pub enum ZoomLevel {
    #[default]
    Overview,
    Cluster(ClusterId),
    Focus(WindowId),
}

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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    #[default]
    Tiled,
    Floating,
    Fullscreen,
    MinimizedLike,
}

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
    SelectCluster {
        cluster: ClusterId,
    },
    BeginClusterDrag {
        cluster: ClusterId,
        pointer_canvas_x: f64,
        pointer_canvas_y: f64,
        base_revision: u64,
    },
    UpdateClusterDrag {
        cluster_x: f64,
        cluster_y: f64,
    },
    CommitClusterDrag,
    CancelClusterDrag,
    OverviewPan {
        dx: f64,
        dy: f64,
        output: Option<String>,
    },
    OverviewZoom {
        delta: f64,
        anchor_canvas_x: f64,
        anchor_canvas_y: f64,
        output: Option<String>,
    },
    EnterKeyboardMoveMode {
        cluster: ClusterId,
    },
    EnterKeyboardMoveModeSelected,
    KeyboardMoveBy {
        dx: f64,
        dy: f64,
    },
    CommitKeyboardMove,
    CancelKeyboardMove,
    CycleCluster {
        direction: CycleDirection,
    },
    CreateCluster {
        name: String,
        x: f64,
        y: f64,
    },
    MoveWindowToCluster {
        window: WindowId,
        cluster: ClusterId,
    },
    RenameCluster {
        cluster: ClusterId,
        name: String,
    },
    GetState,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    Ack,
    ClusterDragAck {
        state_revision: u64,
    },
    State(CanvasState),
    ClusterDragError {
        message: String,
        state_revision: u64,
    },
    Error {
        message: String,
    },
}

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
                base_revision: 8,
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
    fn round_trip_ipc_overview_interaction_responses() {
        for fixture in [
            IpcResponse::ClusterDragAck { state_revision: 11 },
            IpcResponse::ClusterDragError {
                message: "stale base revision".to_owned(),
                state_revision: 12,
            },
        ] {
            let json = serde_json::to_string(&fixture).expect("serialize response");
            let parsed: IpcResponse = serde_json::from_str(&json).expect("parse response");
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
