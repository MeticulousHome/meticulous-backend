//! Joining a network (`connectToWifi`) from the app, the Dial, BLE
//! provisioning and the auto-connect thread.

use serde_json::{Map, Value, json};

use super::WifiManager;
use super::types::{JOIN_FAILED, ScanEntry, WifiType, classify_connection_error};
use crate::config::keys;
use crate::py::{PyError, PyResult, py_str, type_name};

/// `WifiWpaPskCredentials(ssid=..., password=...).to_dict()`.
pub fn psk_credentials(ssid: &str, password: &str) -> Value {
    json!({"type": "PSK", "security": "", "ssid": ssid, "password": password})
}

/// A value handed to `subprocess.run` as an argument.
fn cli_arg(value: &Value) -> PyResult<&str> {
    value.as_str().ok_or_else(|| PyError::not_a_path(value))
}

/// `connection_name == ssid`: never true for an SSID that is not a string.
fn is_connection(name: &Option<String>, ssid: &Value) -> bool {
    ssid.as_str().is_some_and(|s| name.as_deref() == Some(s))
}

fn is_psk(wifi_type: &Value) -> bool {
    matches!(wifi_type.as_str(), Some("PSK" | "SAE"))
}

impl WifiManager {
    /// `connectToWifi(credentials, source)`: `Ok(false)` with
    /// `last_connection_error` set when the join fails; `Err` where the
    /// Python raises.
    pub fn connect_to_wifi(&self, credentials: &Value, source: &str) -> PyResult<bool> {
        self.clear_last_connection_error();
        if !self.networking_available() {
            self.set_last_connection_error(
                "networking_unavailable",
                "Wi-Fi hardware is not available.",
            );
            return Ok(false);
        }
        let mut credentials: Map<String, Value> = match credentials {
            Value::Null => {
                self.set_last_connection_error(
                    "missing_credentials",
                    "Wi-Fi credentials were not provided.",
                );
                return Ok(false);
            }
            Value::Object(map) => map.clone(),
            other => {
                return Err(PyError::attribute_error(format!(
                    "'{}' object has no attribute 'to_dict'",
                    type_name(other)
                )));
            }
        };

        let mut wifi_type = credentials.get("type").cloned().unwrap_or(Value::Null);
        if wifi_type.is_null() {
            wifi_type = json!(WifiType::PreSharedKey.value());
            credentials.insert("type".into(), wifi_type.clone());
        }
        let ssid = match credentials.get("ssid") {
            None | Some(Value::Null) => {
                self.set_last_connection_error(
                    "missing_ssid",
                    "Wi-Fi network name was not provided.",
                );
                return Ok(false);
            }
            Some(ssid) => ssid.clone(),
        };
        let ssid_text = py_str(&ssid);
        let redacted = self.redact_ssid(&ssid_text);
        let is_auto_connect = source == "auto";
        if is_auto_connect && self.is_auto_connect_suppressed() {
            tracing::info!(
                "Skipping auto-connect to {redacted}; manual WiFi connect is in progress"
            );
            return Ok(false);
        }
        if !is_auto_connect {
            self.suppress_auto_connect(90.0, &format!("manual connect to {redacted}"));
        }
        tracing::info!("Connecting to wifi: {redacted}");

        let current = self.get_current_config()?;
        if current.connected && is_connection(&current.connection_name, &ssid) {
            tracing::info!("Already connected");
            if !is_auto_connect {
                self.persist_client_mode_after_manual_connect(&ssid_text);
                self.suppress_auto_connect(
                    45.0,
                    &format!("manual connect to {redacted} completed"),
                );
            }
            self.zeroconf_restart()?;
            self.update_gatt_advertisement();
            return Ok(true);
        }

        if !self.is_wifi_device_ready() {
            self.set_last_connection_error(
                "wifi_device_unavailable",
                "Wi-Fi radio is not ready. Please wait a moment and try again.",
            );
            self.update_gatt_advertisement();
            return Ok(false);
        }

        let password = credentials.get("password").cloned().unwrap_or(Value::Null);
        let has_known_profile = self.has_known_wifi_connection(&ssid)?;
        let cached: Vec<ScanEntry> = self
            .get_available_networks(true, false)
            .into_iter()
            .filter(|n| ssid.as_str() == Some(n.ssid.as_str()))
            .collect();
        let networks = if (password.is_null() && has_known_profile) || !cached.is_empty() {
            cached
        } else {
            self.scan_for_networks(8.0, Some(&ssid))
        };
        tracing::info!("Found {} candidate Wi-Fi networks", networks.len());

        if wifi_type.as_str() == Some("PSK")
            && networks
                .iter()
                .any(|n| n.security.to_uppercase().contains("WPA3"))
        {
            wifi_type = json!(WifiType::PskSae.value());
            credentials.insert("type".into(), wifi_type.clone());
            tracing::info!("Network supports WPA3, switching to SAE");
        }

        tracing::info!("Connecting through NetworkManager");
        let attempt = self.issue_connect(&ssid, &password, &wifi_type, has_known_profile);
        let mut needs_fix = false;
        match attempt {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {}
            Err(error) => {
                if error
                    .message
                    .contains("802-11-wireless-security.key-mgmt: property is missing")
                {
                    needs_fix = true;
                } else {
                    self.report_connect_exception(&error, &wifi_type);
                    return Ok(false);
                }
            }
        }
        if needs_fix && let Err(error) = self.fix_wifi_connection(&ssid, &wifi_type) {
            self.report_connect_exception(&error, &wifi_type);
            return Ok(false);
        }

        tracing::info!("Connection command completed, checking active connection");
        if self.wait_for_connection(&ssid, 8.0)? {
            tracing::info!("Successfully connected");
            if !is_auto_connect {
                self.suppress_auto_connect(
                    45.0,
                    &format!("manual connect to {redacted} completed"),
                );
                self.persist_client_mode_after_manual_connect(&ssid_text);
            }
            self.zeroconf_restart()?;
            self.invalidate_health_cache();
            self.remember_wifi(&credentials);
            let current = self.get_current_config()?;
            self.refresh_health_in_background(Some(current));
            self.update_gatt_advertisement();
            return Ok(true);
        }

        let current = self.get_current_config()?;
        if current.connected && !is_connection(&current.connection_name, &ssid) {
            let other = current
                .connection_name
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "another network".into());
            self.set_last_connection_error(
                "connection_preempted",
                &format!("Connected to {other} instead of {ssid_text}."),
            );
            if !is_auto_connect {
                self.suppress_auto_connect(
                    30.0,
                    &format!("manual connect to {redacted} was preempted"),
                );
            }
        } else if is_psk(&wifi_type) {
            self.set_last_connection_error("wifi_join_failed", JOIN_FAILED);
        } else {
            self.set_last_connection_error(
                "connection_timeout",
                "Could not verify the Wi-Fi connection. Please try again.",
            );
        }
        self.update_gatt_advertisement();
        Ok(false)
    }

    /// The `try:` block of `connectToWifi`: `Ok(Some(result))` when it
    /// returns early, `Ok(None)` to go on and verify the connection.
    fn issue_connect(
        &self,
        ssid: &Value,
        password: &Value,
        wifi_type: &Value,
        has_known_profile: bool,
    ) -> PyResult<Option<bool>> {
        let host = self.host();
        match wifi_type.as_str() {
            Some("OPEN") => host.nmcli().device_wifi_connect(cli_arg(ssid)?, None)?,
            Some("PSK" | "SAE") => {
                if password.is_null() && has_known_profile {
                    tracing::info!(
                        "Connecting to known Wi-Fi profile: {}",
                        self.redact_ssid(&py_str(ssid))
                    );
                    host.nmcli().connection_up(cli_arg(ssid)?, Some(12))?;
                } else if password.is_null() {
                    self.set_last_connection_error(
                        "missing_credentials",
                        "Wi-Fi password was not provided.",
                    );
                    self.update_gatt_advertisement();
                    return Ok(Some(false));
                } else {
                    host.nmcli()
                        .device_wifi_connect(cli_arg(ssid)?, Some(cli_arg(password)?))?;
                }
            }
            Some("802.1X") => {
                tracing::error!("Enterprise wifi not yet implemented");
                self.set_last_connection_error(
                    "unsupported_security",
                    "Enterprise Wi-Fi networks are not supported yet.",
                );
                return Ok(Some(false));
            }
            _ => {}
        }
        Ok(None)
    }

    fn report_connect_exception(&self, error: &PyError, wifi_type: &Value) {
        let (code, message) = classify_connection_error(&error.message, is_psk(wifi_type));
        self.set_last_connection_error(code, &message);
        tracing::error!("Failed to connect to wifi ({})", error.kind);
        self.update_gatt_advertisement();
    }

    /// `fixWifiConnection(ssid, wifi_type)`: a profile NetworkManager saved
    /// without key management.
    fn fix_wifi_connection(&self, ssid: &Value, wifi_type: &Value) -> PyResult<()> {
        tracing::info!(
            "Fixing wifi connection for {}",
            self.redact_ssid(&py_str(ssid))
        );
        let key_mgmt = match wifi_type.as_str() {
            Some("OPEN") => "none",
            Some("PSK") => "wpa-psk",
            Some("802.1X") => "802-1x",
            Some("WEP") => "wep",
            Some("SAE") => "sae",
            _ => {
                return Err(PyError::value_error(format!(
                    "Unknown WifiType: {}",
                    py_str(wifi_type)
                )));
            }
        };
        let name = cli_arg(ssid)?;
        let host = self.host();
        host.nmcli().connection_modify(
            name,
            &[
                ("802-11-wireless.ssid", name),
                ("802-11-wireless-security.key-mgmt", key_mgmt),
            ],
        )?;
        host.nmcli().connection_up(name, Some(10))
    }

    /// `waitForConnection(ssid, timeout)`.
    fn wait_for_connection(&self, ssid: &Value, timeout: f64) -> PyResult<bool> {
        let host = self.host();
        let deadline = host.time() + timeout;
        while host.time() < deadline {
            let current = self.get_current_config()?;
            if current.connected && is_connection(&current.connection_name, ssid) {
                return Ok(true);
            }
            host.sleep(0.5);
        }
        Ok(false)
    }

    /// `rememberWifi(credentials)`: NetworkManager keeps the profile, so the
    /// legacy entry goes.
    fn remember_wifi(&self, credentials: &Map<String, Value>) {
        let Some(ssid) = credentials
            .get("ssid")
            .filter(|s| crate::config::python_truthy(s))
        else {
            return;
        };
        let Some(ssid) = ssid.as_str() else { return };
        let known = self.config.get(keys::WIFI, keys::WIFI_KNOWN_WIFIS);
        if known.get(ssid).is_none() {
            return;
        }
        self.config.update(|tree| {
            if let Some(Value::Object(known)) = tree
                .get_mut(keys::WIFI)
                .and_then(|w| w.get_mut(keys::WIFI_KNOWN_WIFIS))
            {
                known.shift_remove(ssid);
            }
        });
        self.config.save();
    }
}
