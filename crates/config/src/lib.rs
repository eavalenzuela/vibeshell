use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const DEFAULT_CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(alias = "version")]
    pub config_version: u32,
    pub panel: PanelConfig,
    pub launcher: LauncherConfig,
    #[serde(alias = "notifd")]
    pub notifications: NotificationsConfig,
    pub keybindings: KeybindingsConfig,
    pub commands: CommandsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: DEFAULT_CONFIG_VERSION,
            panel: PanelConfig::default(),
            launcher: LauncherConfig::default(),
            notifications: NotificationsConfig::default(),
            keybindings: KeybindingsConfig::default(),
            commands: CommandsConfig::default(),
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
    pub sway_event_debounce_ms: u64,
    pub network_settings_command: String,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            height: 32,
            margin_start: 12,
            margin_end: 12,
            clock_format: "%H:%M".to_owned(),
            status_poll_interval_ms: 5_000,
            sway_event_debounce_ms: 80,
            network_settings_command: "nm-connection-editor".to_owned(),
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
pub struct NotificationsConfig {
    pub width: i32,
    pub default_timeout_ms: u64,
    pub margin_top: i32,
    pub margin_right: i32,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            width: 360,
            default_timeout_ms: 5_000,
            margin_top: 12,
            margin_right: 12,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KeybindingsConfig {
    pub launcher_toggle: String,
    pub screenshot: String,
    pub volume_up: String,
    pub volume_down: String,
    pub volume_mute_toggle: String,
    pub brightness_up: String,
    pub brightness_down: String,
    pub power_menu: String,
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            launcher_toggle: "Super+d".to_owned(),
            screenshot: "Print".to_owned(),
            volume_up: "XF86AudioRaiseVolume".to_owned(),
            volume_down: "XF86AudioLowerVolume".to_owned(),
            volume_mute_toggle: "XF86AudioMute".to_owned(),
            brightness_up: "XF86MonBrightnessUp".to_owned(),
            brightness_down: "XF86MonBrightnessDown".to_owned(),
            power_menu: "Super+Escape".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CommandsConfig {
    pub volume: VolumeCommands,
    pub brightness: BrightnessCommands,
    pub power: PowerCommands,
}

impl Default for CommandsConfig {
    fn default() -> Self {
        Self {
            volume: VolumeCommands::default(),
            brightness: BrightnessCommands::default(),
            power: PowerCommands::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VolumeCommands {
    pub up: String,
    pub down: String,
    pub toggle_mute: String,
    pub mixer: Option<String>,
}

impl Default for VolumeCommands {
    fn default() -> Self {
        Self {
            up: "wpctl set-volume -l 1.5 @DEFAULT_AUDIO_SINK@ 5%+".to_owned(),
            down: "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-".to_owned(),
            toggle_mute: "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle".to_owned(),
            mixer: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BrightnessCommands {
    pub up: String,
    pub down: String,
}

impl Default for BrightnessCommands {
    fn default() -> Self {
        Self {
            up: "brightnessctl set +5%".to_owned(),
            down: "brightnessctl set 5%-".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PowerCommands {
    pub menu: String,
}

impl Default for PowerCommands {
    fn default() -> Self {
        Self {
            menu: "wlogout".to_owned(),
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
        message: String,
    },
    Validation {
        path: PathBuf,
        messages: Vec<String>,
    },
}

impl fmt::Display for ConfigLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    f,
                    "Could not read config file at {}: {source}",
                    path.display()
                )
            }
            Self::Parse { path, message } => {
                write!(
                    f,
                    "Config file at {} is not valid TOML: {message}",
                    path.display()
                )
            }
            Self::Validation { path, messages } => {
                write!(
                    f,
                    "Config file at {} has invalid values: {}",
                    path.display(),
                    messages.join("; ")
                )
            }
        }
    }
}

impl std::error::Error for ConfigLoadError {}

impl Config {
    pub fn load() -> Result<Self, ConfigLoadError> {
        let path = default_config_path();
        Self::load_from_path(&path)
    }

    pub fn load_from_path(path: &Path) -> Result<Self, ConfigLoadError> {
        match fs::read_to_string(path) {
            Ok(content) => {
                let config = toml::from_str::<Self>(&content).map_err(|source| {
                    let message = source.to_string();
                    ConfigLoadError::Parse {
                        path: path.to_path_buf(),
                        message,
                    }
                })?;

                config.validate(path)?;
                Ok(config)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigLoadError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    fn validate(&self, path: &Path) -> Result<(), ConfigLoadError> {
        let mut issues = Vec::new();

        if self.config_version == 0 {
            issues.push("config_version must be >= 1".to_owned());
        }
        if self.panel.height <= 0 {
            issues.push("panel.height must be greater than 0".to_owned());
        }
        if self.panel.margin_start < 0 || self.panel.margin_end < 0 {
            issues.push("panel margins must be >= 0".to_owned());
        }
        if self.panel.clock_format.trim().is_empty() {
            issues.push("panel.clock_format cannot be empty".to_owned());
        }
        if self.panel.sway_event_debounce_ms < 20 {
            issues.push("panel.sway_event_debounce_ms must be >= 20".to_owned());
        }
        if self.launcher.window_width <= 0 || self.launcher.window_height <= 0 {
            issues.push(
                "launcher.window_width and launcher.window_height must be greater than 0"
                    .to_owned(),
            );
        }
        if self.launcher.max_results == 0 {
            issues.push("launcher.max_results must be at least 1".to_owned());
        }
        if self.notifications.width <= 0 {
            issues.push("notifications.width must be greater than 0".to_owned());
        }
        if self.notifications.margin_top < 0 || self.notifications.margin_right < 0 {
            issues.push("notifications margins must be >= 0".to_owned());
        }

        for (name, value) in [
            ("commands.volume.up", self.commands.volume.up.as_str()),
            ("commands.volume.down", self.commands.volume.down.as_str()),
            (
                "commands.volume.toggle_mute",
                self.commands.volume.toggle_mute.as_str(),
            ),
            (
                "commands.brightness.up",
                self.commands.brightness.up.as_str(),
            ),
            (
                "commands.brightness.down",
                self.commands.brightness.down.as_str(),
            ),
            ("commands.power.menu", self.commands.power.menu.as_str()),
        ] {
            if value.trim().is_empty() {
                issues.push(format!("{name} cannot be empty"));
            }
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(ConfigLoadError::Validation {
                path: path.to_path_buf(),
                messages: issues,
            })
        }
    }
}

pub fn default_config_path() -> PathBuf {
    let relative = Path::new("vibeshell").join("config.toml");

    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join(&relative);
    }

    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join(relative);
    }

    PathBuf::from(".config").join(relative)
}
