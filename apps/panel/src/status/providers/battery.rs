use std::fs;
use std::path::Path;

use super::StatusProvider;

#[derive(Clone, Debug)]
pub enum BatteryTrend {
    Charging,
    Discharging,
    Full,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct BatteryStatus {
    pub percent: u8,
    pub trend: BatteryTrend,
}

pub struct BatteryProvider;

impl StatusProvider for BatteryProvider {
    type Output = BatteryStatus;

    fn poll(&self) -> Option<Self::Output> {
        let entries = fs::read_dir("/sys/class/power_supply").ok()?;

        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if !name.starts_with("BAT") {
                continue;
            }

            let percent = read_trimmed(path.join("capacity"))?.parse::<u8>().ok()?;
            let trend = read_trimmed(path.join("status"))
                .map(|status| parse_status(&status))
                .unwrap_or(BatteryTrend::Unknown);

            return Some(BatteryStatus { percent, trend });
        }

        None
    }
}

fn parse_status(raw: &str) -> BatteryTrend {
    match raw.trim().to_lowercase().as_str() {
        "charging" => BatteryTrend::Charging,
        "discharging" => BatteryTrend::Discharging,
        "full" => BatteryTrend::Full,
        _ => BatteryTrend::Unknown,
    }
}

fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    Some(raw.trim().to_owned())
}
