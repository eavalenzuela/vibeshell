use serde::{Deserialize, Serialize};

pub type WindowId = u64;
pub type ClusterId = u64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CanvasState {
    pub state_revision: u64,
    pub zoom: ZoomLevel,
    pub viewport: Viewport,
    pub clusters: Vec<Cluster>,
    pub windows: Vec<Window>,
    pub output: OutputState,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            state_revision: 0,
            zoom: ZoomLevel::default(),
            viewport: Viewport::default(),
            clusters: Vec::new(),
            windows: Vec::new(),
            output: OutputState::default(),
        }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

impl Default for Window {
    fn default() -> Self {
        Self {
            id: 0,
            title: String::new(),
            app_id: None,
            class: None,
            role: WindowRole::default(),
            state: WindowState::default(),
            cluster_id: None,
            transient_for: None,
            manual_cluster_override: false,
            manual_position_override: false,
        }
    }
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
        pointer_canvas_x: f64,
        pointer_canvas_y: f64,
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
    KeyboardMoveBy {
        dx: f64,
        dy: f64,
    },
    CommitKeyboardMove,
    CancelKeyboardMove,
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

#[cfg(test)]
mod tests {
    use super::*;

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
                pointer_canvas_x: 130.0,
                pointer_canvas_y: 460.0,
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
    fn old_minimal_state_payload_uses_defaults() {
        let parsed: CanvasState = serde_json::from_str("{}").expect("parse minimal state");
        assert_eq!(parsed, CanvasState::default());
    }
}
