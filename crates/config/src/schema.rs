use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContinuumSchema {
    pub clusters_enabled: bool,
    pub zoom_step_sizes: ZoomStepSizes,
    pub strip_placement: StripPlacement,
    pub auto_cluster: bool,
    pub assignment_hints: Vec<AssignmentHint>,
}

impl Default for ContinuumSchema {
    fn default() -> Self {
        Self {
            clusters_enabled: true,
            zoom_step_sizes: ZoomStepSizes::default(),
            strip_placement: StripPlacement::default(),
            auto_cluster: false,
            assignment_hints: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AssignmentHint {
    pub app_id: Option<String>,
    pub class: Option<String>,
    pub title_contains: Option<String>,
    pub cluster: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoomStepSizes {
    pub overview_to_cluster: f64,
    pub cluster_to_focus: f64,
    pub keyboard_pan: f64,
}

impl Default for ZoomStepSizes {
    fn default() -> Self {
        Self {
            overview_to_cluster: 0.15,
            cluster_to_focus: 0.25,
            keyboard_pan: 120.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StripPlacement {
    Left,
    #[default]
    Bottom,
    Right,
    Hidden,
}
