use serde::{Deserialize, Serialize};

pub type WindowId = u64;
pub type ClusterId = u64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CanvasState {
    pub zoom: ZoomLevel,
    pub viewport: Viewport,
    pub clusters: Vec<Cluster>,
    pub windows: Vec<Window>,
    pub output: OutputState,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
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
}

impl Default for Cluster {
    fn default() -> Self {
        Self {
            id: 0,
            name: "Cluster".to_owned(),
            x: 0.0,
            y: 0.0,
            enabled: true,
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
    Pan {
        dx: f64,
        dy: f64,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    Ack,
    State(CanvasState),
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_state_fixture() {
        let fixture = CanvasState {
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
    fn old_minimal_state_payload_uses_defaults() {
        let parsed: CanvasState = serde_json::from_str("{}").expect("parse minimal state");
        assert_eq!(parsed, CanvasState::default());
    }
}
