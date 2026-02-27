use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const DEFAULT_CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub panel: PanelConfig,
    pub launcher: LauncherConfig,
    pub notifd: NotifdConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: DEFAULT_CONFIG_VERSION,
            panel: PanelConfig::default(),
            launcher: LauncherConfig::default(),
            notifd: NotifdConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PanelConfig {
    pub height: i32,
    pub margin_start: i32,
    pub margin_end: i32,
    pub clock_format: String,
    pub status_poll_interval_ms: u64,
    pub audio_toggle_command: String,
    pub audio_mixer_command: Option<String>,
    pub network_settings_command: String,
    pub power_menu_command: String,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            height: 32,
            margin_start: 12,
            margin_end: 12,
            clock_format: "%H:%M".to_owned(),
            status_poll_interval_ms: 5_000,
            audio_toggle_command: "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle".to_owned(),
            audio_mixer_command: None,
            network_settings_command: "nm-connection-editor".to_owned(),
            power_menu_command: "wlogout".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LauncherConfig {
    pub window_width: i32,
    pub window_height: i32,
    pub max_results: usize,
    pub terminal_command: String,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            window_width: 640,
            window_height: 420,
            max_results: 10,
            terminal_command: "foot".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NotifdConfig {
    pub width: i32,
    pub default_timeout_ms: u64,
    pub margin_top: i32,
    pub margin_right: i32,
}

impl Default for NotifdConfig {
    fn default() -> Self {
        Self {
            width: 360,
            default_timeout_ms: 5_000,
            margin_top: 12,
            margin_right: 12,
        }
    }
}

#[derive(Debug)]
pub enum ConfigLoadError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl fmt::Display for ConfigLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "failed to read config file {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(
                    f,
                    "failed to parse config file {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for ConfigLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, ConfigLoadError> {
        let path = config_path();
        Self::load_from_path(&path)
    }

    fn load_from_path(path: &Path) -> Result<Self, ConfigLoadError> {
        match fs::read_to_string(path) {
            Ok(content) => {
                toml::from_str::<Self>(&content).map_err(|source| ConfigLoadError::Parse {
                    path: path.to_path_buf(),
                    source,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigLoadError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }
}

fn config_path() -> PathBuf {
    let relative = Path::new("vibeshell").join("config.toml");

    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join(&relative);
    }

    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join(relative);
    }

    PathBuf::from(".config").join(relative)
}
