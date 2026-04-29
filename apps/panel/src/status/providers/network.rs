use std::sync::Mutex;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

use super::StatusProvider;

#[derive(Clone, Debug)]
pub struct NetworkStatus {
    pub connected: bool,
    pub connection_type: String,
}

const NM_BUS: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const ACTIVE_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";

/// `Connectivity` value where the host has full Internet reachability per
/// NetworkManager's own probing. Lower values (Limited / Portal / None /
/// Unknown) all map to "not connected" for the panel.
const NM_CONNECTIVITY_FULL: u32 = 4;

pub struct NetworkProvider {
    /// Cached system-bus connection. `None` until the first successful
    /// connect; reset to `None` on read failure so the next `poll()` retries.
    connection: Mutex<Option<Connection>>,
}

impl NetworkProvider {
    pub fn new() -> Self {
        Self {
            connection: Mutex::new(None),
        }
    }

    fn with_connection<T>(&self, f: impl FnOnce(&Connection) -> zbus::Result<T>) -> Option<T> {
        let mut guard = self.connection.lock().ok()?;
        if guard.is_none() {
            *guard = Connection::system().ok();
        }
        let conn = guard.as_ref()?;
        match f(conn) {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::debug!(
                    ?error,
                    "NetworkManager DBus read failed; resetting connection"
                );
                *guard = None;
                None
            }
        }
    }
}

impl StatusProvider for NetworkProvider {
    type Output = NetworkStatus;

    fn poll(&self) -> Option<Self::Output> {
        self.with_connection(|conn| {
            let nm = Proxy::new(conn, NM_BUS, NM_PATH, NM_IFACE)?;
            let connectivity: u32 = nm.get_property("Connectivity")?;
            let primary: OwnedObjectPath = nm.get_property("PrimaryConnection")?;

            let primary_str = primary.as_str();
            // "/" means "no primary connection" — NetworkManager's idle sentinel.
            if primary_str == "/" {
                return Ok(NetworkStatus {
                    connected: false,
                    connection_type: "offline".to_owned(),
                });
            }

            let active = Proxy::new(conn, NM_BUS, primary_str, ACTIVE_IFACE)?;
            let conn_type: String = active.get_property("Type")?;

            Ok(NetworkStatus {
                connected: connectivity == NM_CONNECTIVITY_FULL,
                connection_type: friendly_type(&conn_type),
            })
        })
    }
}

/// Map NetworkManager's wire-format types to the short labels the old
/// `nmcli` parser produced, so panel formatting stays unchanged.
fn friendly_type(nm_type: &str) -> String {
    match nm_type {
        "802-11-wireless" => "wifi".to_owned(),
        "802-3-ethernet" => "ethernet".to_owned(),
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::friendly_type;

    #[test]
    fn maps_known_nm_types() {
        assert_eq!(friendly_type("802-11-wireless"), "wifi");
        assert_eq!(friendly_type("802-3-ethernet"), "ethernet");
    }

    #[test]
    fn passes_through_unknown_types() {
        assert_eq!(friendly_type("vpn"), "vpn");
        assert_eq!(friendly_type("bluetooth"), "bluetooth");
    }
}
