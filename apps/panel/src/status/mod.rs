mod providers;

use providers::{
    AudioProvider, AudioStatus, BatteryProvider, BatteryStatus, BatteryTrend, NetworkProvider,
    NetworkStatus, StatusProvider,
};

#[derive(Clone, Debug)]
pub struct PanelStatus {
    pub audio: String,
    pub network: String,
    pub battery: String,
}

impl Default for PanelStatus {
    fn default() -> Self {
        Self {
            audio: "audio N/A".to_owned(),
            network: "network N/A".to_owned(),
            battery: "battery N/A".to_owned(),
        }
    }
}

pub struct StatusCollector {
    audio: AudioProvider,
    network: NetworkProvider,
    battery: BatteryProvider,
}

impl StatusCollector {
    pub fn new() -> Self {
        Self {
            audio: AudioProvider,
            network: NetworkProvider,
            battery: BatteryProvider,
        }
    }

    pub fn collect(&self) -> PanelStatus {
        PanelStatus {
            audio: self
                .audio
                .poll()
                .map(format_audio)
                .unwrap_or_else(|| "audio N/A".to_owned()),
            network: self
                .network
                .poll()
                .map(format_network)
                .unwrap_or_else(|| "network N/A".to_owned()),
            battery: self
                .battery
                .poll()
                .map(format_battery)
                .unwrap_or_else(|| "battery N/A".to_owned()),
        }
    }
}

fn format_audio(status: AudioStatus) -> String {
    if status.muted {
        return format!("{} muted", audio_icon(true));
    }

    format!("{} {}%", audio_icon(false), status.volume_percent)
}

fn audio_icon(muted: bool) -> &'static str {
    if muted {
        "🔇"
    } else {
        "🔊"
    }
}

fn format_network(status: NetworkStatus) -> String {
    if status.connected {
        format!("📶 connected ({})", status.connection_type)
    } else {
        "📶 disconnected".to_owned()
    }
}

fn format_battery(status: BatteryStatus) -> String {
    let icon = match status.trend {
        BatteryTrend::Charging => "⚡",
        BatteryTrend::Discharging => "🔋",
        BatteryTrend::Full => "🔋",
        BatteryTrend::Unknown => "🔋",
    };

    format!("{icon} {}%", status.percent)
}
