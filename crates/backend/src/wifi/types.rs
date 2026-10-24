//! The data `wifi.py` passes around: NetworkManager's devices, connections
//! and scan results (the `nmcli` package's dataclasses), the current network
//! configuration and the health report the settings screen shows.

use std::net::IpAddr;

use serde_json::{Value, json};

use crate::py::{PyError, PyResult};

/// The hotspot's NetworkManager connection name (`WifiManager._conname`).
pub const AP_CONNECTION: &str = "meticulousLocalAP";

/// `WifiType`: the security of a network, as the API and the config spell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiType {
    Open,
    PreSharedKey,
    PskSae,
    Enterprise,
    Wep,
}

impl WifiType {
    pub fn value(self) -> &'static str {
        match self {
            WifiType::Open => "OPEN",
            WifiType::PreSharedKey => "PSK",
            WifiType::PskSae => "SAE",
            WifiType::Enterprise => "802.1X",
            WifiType::Wep => "WEP",
        }
    }

    /// `f"{wifi_type}"` (Python 3.11 formats a str enum by member name).
    pub fn py_name(self) -> &'static str {
        match self {
            WifiType::Open => "WifiType.Open",
            WifiType::PreSharedKey => "WifiType.PreSharedKey",
            WifiType::PskSae => "WifiType.PSK_SAE",
            WifiType::Enterprise => "WifiType.Enterprise",
            WifiType::Wep => "WifiType.WEP",
        }
    }

    /// `WifiType(value)`.
    pub fn from_value(value: &str) -> Option<WifiType> {
        match value {
            "OPEN" => Some(WifiType::Open),
            "PSK" => Some(WifiType::PreSharedKey),
            "SAE" => Some(WifiType::PskSae),
            "802.1X" => Some(WifiType::Enterprise),
            "WEP" => Some(WifiType::Wep),
            _ => None,
        }
    }

    /// `WifiType.from_nmcli_security`: `None` (and an error report) for a
    /// security string nobody planned for.
    pub fn from_nmcli_security(security: &str) -> Option<WifiType> {
        let security = security.trim().to_uppercase();
        if security.is_empty() {
            Some(WifiType::Open)
        } else if security.contains("802.1X") {
            Some(WifiType::Enterprise)
        } else if security.contains("WPA3") {
            Some(WifiType::PskSae)
        } else if security.contains("WPA") {
            Some(WifiType::PreSharedKey)
        } else if security.contains("WEP") {
            // WEP is not supported; the type is still reported.
            Some(WifiType::Wep)
        } else {
            // Python also sends this message to Sentry at level "error".
            tracing::error!("Unknown wifi security type: {security}");
            None
        }
    }
}

/// `nmcli.Device`: one row of `nmcli device status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub device: String,
    pub device_type: String,
    pub state: String,
    pub connection: Option<String>,
}

/// `nmcli.Connection`: one row of `nmcli connection`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub name: String,
    pub uuid: String,
    pub conn_type: String,
    pub device: String,
}

/// `nmcli.DeviceWifi`: one access point from a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanEntry {
    pub in_use: bool,
    pub ssid: String,
    pub bssid: String,
    pub mode: String,
    pub chan: i64,
    pub freq: i64,
    pub rate: i64,
    pub signal: i64,
    pub security: String,
}

/// `nmcli.DeviceDetails`: `nmcli device show` as an ordered dict, `None`
/// for `--`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceDetails(pub Vec<(String, Option<String>)>);

impl DeviceDetails {
    /// `details[key] = value`: replaces in place, appends when new.
    pub fn insert(&mut self, key: &str, value: Option<String>) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    /// `details.get(key)`, flattening a `None` value.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_deref())
    }

    #[cfg(test)]
    pub fn from_pairs(pairs: &[(&str, Option<&str>)]) -> DeviceDetails {
        let mut details = DeviceDetails::default();
        for (k, v) in pairs {
            details.insert(k, v.map(str::to_string));
        }
        details
    }
}

/// `netaddr.IPNetwork`: an interface address with its prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpNet {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl IpNet {
    /// `IPNetwork(text)`.
    pub fn parse(text: &str) -> PyResult<IpNet> {
        let invalid = || PyError::new("AddrFormatError", format!("invalid IPNetwork {text}"));
        let (addr, prefix) = match text.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (text, None),
        };
        let addr: IpAddr = addr.trim().parse().map_err(|_| invalid())?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.trim().parse::<u8>().map_err(|_| invalid())?,
            None => max,
        };
        if prefix > max {
            return Err(invalid());
        }
        Ok(IpNet { addr, prefix })
    }

    /// `IPNetwork.first` as an address: the network address.
    pub fn first(&self) -> IpAddr {
        match self.addr {
            IpAddr::V4(v4) => {
                let bits = u32::from(v4);
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - u32::from(self.prefix))
                };
                IpAddr::V4((bits & mask).into())
            }
            IpAddr::V6(v6) => {
                let bits = u128::from(v6);
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - u32::from(self.prefix))
                };
                IpAddr::V6((bits & mask).into())
            }
        }
    }

    pub fn is_ipv4(&self) -> bool {
        self.addr.is_ipv4()
    }
}

impl std::fmt::Display for IpNet {
    /// `str(IPNetwork)`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

/// `netaddr.IPAddress(text)`.
pub fn parse_ip(text: &str) -> PyResult<IpAddr> {
    text.trim().parse().map_err(|_| {
        PyError::new(
            "AddrFormatError",
            format!("failed to detect a valid IP address from '{text}'"),
        )
    })
}

/// `WifiSystemConfig`: what NetworkManager says about the Wi-Fi device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiSystemConfig {
    pub connected: bool,
    pub connection_name: Option<String>,
    pub gateway: Option<IpAddr>,
    pub routes: Vec<String>,
    pub ips: Vec<IpNet>,
    pub dns: Vec<IpAddr>,
    /// `""` until a device reports one; `None` when the connected device
    /// shows `--`.
    pub mac: Option<String>,
    pub hostname: String,
    pub domains: Vec<String>,
}

impl WifiSystemConfig {
    pub fn disconnected(hostname: String) -> WifiSystemConfig {
        WifiSystemConfig {
            connected: false,
            connection_name: None,
            gateway: None,
            routes: Vec::new(),
            ips: Vec::new(),
            dns: Vec::new(),
            mac: Some(String::new()),
            hostname,
            domains: Vec::new(),
        }
    }

    /// `to_json()`: the `status` object of `/wifi/config`.
    pub fn to_json(&self) -> Value {
        json!({
            "connected": self.connected,
            "connection_name": self.connection_name,
            "gateway": self.gateway.map(|g| g.to_string()).unwrap_or_default(),
            "routes": self.routes,
            "ips": self.ips.iter().map(|ip| ip.addr.to_string()).collect::<Vec<_>>(),
            "dns": self.dns.iter().map(|d| d.to_string()).collect::<Vec<_>>(),
            "mac": self.mac,
            "hostname": self.hostname,
        })
    }

    pub fn is_hotspot(&self) -> bool {
        self.connection_name.as_deref() == Some(AP_CONNECTION)
    }

    pub fn has_ipv4(&self) -> bool {
        self.ips.iter().any(IpNet::is_ipv4)
    }
}

/// `WifiHealthStatus`: whether the active Wi-Fi path is usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiHealthStatus {
    pub mode: Value,
    pub link_connected: bool,
    pub has_ipv4: bool,
    pub gateway_reachable: bool,
    pub dns_resolves: bool,
    pub internet_reachable: bool,
    pub ap_active: bool,
    pub degraded: bool,
    pub last_error: String,
    pub last_recovery_action: String,
    pub last_recovery_result: String,
    pub message: String,
    /// False when the reachability probes were skipped: the gateway, DNS and
    /// internet fields are then unknown, not unreachable.
    pub verified: bool,
}

impl WifiHealthStatus {
    pub fn to_json(&self) -> Value {
        let message = if self.message.is_empty() {
            health_error_message(&self.last_error).to_string()
        } else {
            self.message.clone()
        };
        json!({
            "mode": self.mode,
            "link_connected": self.link_connected,
            "has_ipv4": self.has_ipv4,
            "gateway_reachable": self.gateway_reachable,
            "dns_resolves": self.dns_resolves,
            "internet_reachable": self.internet_reachable,
            "ap_active": self.ap_active,
            "degraded": self.degraded,
            "verified": self.verified,
            "last_error": self.last_error,
            "message": message,
            "last_recovery_action": self.last_recovery_action,
            "last_recovery_result": self.last_recovery_result,
        })
    }
}

/// `getHealthErrorMessage(error)`: what the settings screen tells the user.
pub fn health_error_message(error: &str) -> &'static str {
    if error.is_empty() {
        return "";
    }
    if error.starts_with("hotspot_start_failed") {
        return "The machine could not start its Wi-Fi hotspot. Please try again. \
                If this keeps happening, restart the machine and try again.";
    }
    if error.starts_with("hotspot_stop_failed") {
        return "The machine could not leave hotspot mode. Please try again or \
                restart the machine.";
    }
    match error {
        "internet_unreachable" => {
            "Connected to Wi-Fi, but this network does not appear to have \
             internet access. Check the router, modem, or phone hotspot data connection."
        }
        "dns_unreachable" => {
            "Connected to Wi-Fi, but DNS is not working. Check the router's \
             internet or DNS settings, or try another network."
        }
        "gateway_unreachable" => {
            "Connected to Wi-Fi, but the router is not responding. Move closer \
             or restart the router."
        }
        "missing_ipv4" => {
            "Connected to Wi-Fi, but the router did not assign an IP address. \
             Check DHCP settings on the router."
        }
        "wifi_not_connected" => "The machine is not connected to Wi-Fi.",
        "no_saved_wifi_connection" => {
            "There is no saved Wi-Fi connection to repair. \
             Choose a Wi-Fi network on the machine or connect it from \
             the Meticulous app."
        }
        "wifi_device_unavailable" => "Wi-Fi is still starting. Wait a moment and try again.",
        "hotspot_not_active" => "The machine could not start its Wi-Fi hotspot. Please try again.",
        "networking_unavailable" => "Wi-Fi hardware is not available.",
        _ => "Wi-Fi could not be verified. Please check the network and try again.",
    }
}

/// `healthErrorIsRecoverable(error)`: what a driver reset can fix.
pub fn health_error_is_recoverable(error: &str) -> bool {
    matches!(
        error,
        "wifi_not_connected" | "missing_ipv4" | "gateway_unreachable" | "hotspot_not_active"
    )
}

/// `classifyConnectionError(error, auth_expected)`: a code and the message
/// the app shows for a failed join.
pub fn classify_connection_error(error: &str, auth_expected: bool) -> (&'static str, String) {
    let lower = error.to_lowercase();
    const AUTH: [&str; 7] = [
        "secrets were required",
        "no secrets were provided",
        "invalid secrets",
        "wrong password",
        "bad password",
        "authentication",
        "802-11-wireless-security.psk",
    ];
    const AUTH_ACTIVATION: [&str; 7] = [
        "activation failed",
        "connection activation failed",
        "supplicant-disconnect",
        "no secrets",
        "4way_handshake",
        "disconnected during association",
        "asking for new key",
    ];
    const NOT_FOUND: [&str; 3] = [
        "no network with ssid",
        "wi-fi network could not be found",
        "network could not be found",
    ];
    if AUTH.iter().any(|m| lower.contains(m)) {
        return (
            "invalid_credentials",
            "Incorrect Wi-Fi password. Please check it and try again.".into(),
        );
    }
    if auth_expected && AUTH_ACTIVATION.iter().any(|m| lower.contains(m)) {
        return ("wifi_join_failed", JOIN_FAILED.into());
    }
    if NOT_FOUND.iter().any(|m| lower.contains(m)) {
        return (
            "network_not_found",
            "Wi-Fi network was not found. Move closer and try again.".into(),
        );
    }
    (
        "connection_failed",
        format!("Could not connect to Wi-Fi: {error}"),
    )
}

pub const JOIN_FAILED: &str = "Could not join this Wi-Fi network. Check the password and try \
                               again, or move closer to the router.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_strings_map_to_types() {
        assert_eq!(WifiType::from_nmcli_security(""), Some(WifiType::Open));
        assert_eq!(
            WifiType::from_nmcli_security("WPA2 802.1X"),
            Some(WifiType::Enterprise)
        );
        assert_eq!(
            WifiType::from_nmcli_security("WPA2 WPA3"),
            Some(WifiType::PskSae)
        );
        assert_eq!(
            WifiType::from_nmcli_security(" wpa2 "),
            Some(WifiType::PreSharedKey)
        );
        assert_eq!(WifiType::from_nmcli_security("WEP"), Some(WifiType::Wep));
        assert_eq!(WifiType::from_nmcli_security("XYZ"), None);
    }

    #[test]
    fn networks_parse_like_netaddr() {
        let net = IpNet::parse("192.168.1.42/24").expect("valid");
        assert_eq!(net.addr.to_string(), "192.168.1.42");
        assert_eq!(net.first().to_string(), "192.168.1.0");
        assert_eq!(net.to_string(), "192.168.1.42/24");
        assert_eq!(
            IpNet::parse("fd00::42/64").expect("v6").first().to_string(),
            "fd00::"
        );
        assert_eq!(IpNet::parse("10.0.0.1").expect("bare").prefix, 32);
        assert!(IpNet::parse("nope/24").is_err());
    }

    /// The messages `classifyConnectionError` produces for the nmcli package's
    /// exception texts.
    #[test]
    fn connection_errors_are_classified() {
        assert_eq!(
            classify_connection_error(
                "Error: Connection activation failed: Secrets were required, but not provided.",
                true
            )
            .0,
            "invalid_credentials"
        );
        assert_eq!(
            classify_connection_error("Connection activation failed", true).0,
            "wifi_join_failed"
        );
        assert_eq!(
            classify_connection_error("Connection activation failed", false).0,
            "connection_failed"
        );
        assert_eq!(
            classify_connection_error("Error: No network with SSID 'HomeNet' found.", true).0,
            "network_not_found"
        );
        assert_eq!(
            classify_connection_error("boom", false).1,
            "Could not connect to Wi-Fi: boom"
        );
    }
}
