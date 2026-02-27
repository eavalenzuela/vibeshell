use std::process::Command;

use super::StatusProvider;

#[derive(Clone, Debug)]
pub struct NetworkStatus {
    pub connected: bool,
    pub connection_type: String,
}

pub struct NetworkProvider;

impl StatusProvider for NetworkProvider {
    type Output = NetworkStatus;

    fn poll(&self) -> Option<Self::Output> {
        let output = Command::new("nmcli")
            .args(["-t", "-f", "TYPE,STATE", "device", "status"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8(output.stdout).ok()?;
        parse_nmcli_device_status(&stdout)
    }
}

fn parse_nmcli_device_status(raw: &str) -> Option<NetworkStatus> {
    let mut any_device = false;

    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        any_device = true;
        let mut parts = line.split(':');
        let device_type = parts.next()?.trim();
        let state = parts.next()?.trim();

        let is_network_type = matches!(device_type, "wifi" | "ethernet");
        if !is_network_type {
            continue;
        }

        if state.eq_ignore_ascii_case("connected") {
            return Some(NetworkStatus {
                connected: true,
                connection_type: device_type.to_owned(),
            });
        }
    }

    if any_device {
        return Some(NetworkStatus {
            connected: false,
            connection_type: "offline".to_owned(),
        });
    }

    None
}
