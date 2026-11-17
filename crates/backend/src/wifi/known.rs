//! Saved networks. NetworkManager owns them (and their secrets); the config's
//! `KnownWifis` is a legacy list whose passwords are migrated into
//! NetworkManager profiles and then scrubbed.

use serde_json::{Map, Value, json};

use super::types::{AP_CONNECTION, WifiType};
use super::{WifiManager, cmd};
use crate::config::{WIFI_MODE_CLIENT, keys, python_truthy};
use crate::py::{PyError, PyResult, py_str, type_name};

/// `getLegacyKnownWifiCredentials`'s result.
struct LegacyCredentials {
    ssid: Value,
    wifi_type: WifiType,
    password: Value,
    has_secret: bool,
}

/// `key in dict` with a JSON key: lists and dicts are unhashable.
fn dict_has(map: &Map<String, Value>, key: &Value) -> PyResult<bool> {
    match key {
        Value::String(s) => Ok(map.contains_key(s)),
        Value::Array(_) | Value::Object(_) => Err(PyError::type_error(format!(
            "unhashable type: '{}'",
            type_name(key)
        ))),
        _ => Ok(false),
    }
}

impl WifiManager {
    /// `deleteConnectionProfile(name)`.
    pub(crate) fn delete_connection_profile(&self, name: &str) -> PyResult<bool> {
        let host = self.host();
        let mut deleted = false;
        for connection in host.nmcli().connections()? {
            if connection.name == name && connection.conn_type == "wifi" {
                tracing::info!("Deleting Wi-Fi connection profile: {name}");
                host.nmcli().connection_delete(name)?;
                deleted = true;
            }
        }
        Ok(deleted)
    }

    /// `deleteWifi(ssid)`: forget a network, legacy entry and profile. The
    /// hotspot's profile is never deleted.
    pub fn delete_wifi(&self, ssid: &Value) -> PyResult<bool> {
        let mut deleted = false;
        let known = self.config.get(keys::WIFI, keys::WIFI_KNOWN_WIFIS);
        if let Value::Object(map) = &known
            && dict_has(map, ssid)?
        {
            let key = py_str(ssid);
            self.config.update(|tree| {
                if let Some(Value::Object(known)) = tree
                    .get_mut(keys::WIFI)
                    .and_then(|w| w.get_mut(keys::WIFI_KNOWN_WIFIS))
                {
                    known.shift_remove(&key);
                }
            });
            self.config.save();
            deleted = true;
        }

        let host = self.host();
        for connection in host.nmcli().connections()? {
            if ssid.as_str() == Some(connection.name.as_str())
                && connection.conn_type == "wifi"
                && connection.name != AP_CONNECTION
            {
                host.nmcli().connection_delete(&connection.name)?;
                deleted = true;
            }
        }
        Ok(deleted)
    }

    /// `getNetworkManagerWifiConnections()`: saved client profiles by name,
    /// with their security.
    pub fn get_network_manager_wifi_connections(&self) -> Map<String, Value> {
        let mut known = Map::new();
        match self.host().nmcli().connections() {
            Ok(connections) => {
                for connection in connections {
                    if connection.conn_type == "wifi" && connection.name != AP_CONNECTION {
                        let wifi_type = self.saved_wifi_type(&connection.name);
                        known.insert(
                            connection.name.clone(),
                            json!({"ssid": connection.name, "type": wifi_type.value()}),
                        );
                    }
                }
            }
            Err(error) => tracing::warn!("Failed to list known Wi-Fi connections: {error}"),
        }
        known
    }

    /// `getSavedWifiType(ssid)`: from the profile's key management.
    fn saved_wifi_type(&self, ssid: &str) -> WifiType {
        let command = crate::py::argv(&[
            "nmcli",
            "-g",
            "802-11-wireless-security.key-mgmt",
            "connection",
            "show",
            ssid,
        ]);
        match self.host().run(&command, None) {
            Ok(done) => {
                let key_mgmt = done.stdout.trim().to_lowercase();
                if key_mgmt.is_empty() || key_mgmt == "none" {
                    return WifiType::Open;
                }
                if key_mgmt.contains("sae") {
                    return WifiType::PskSae;
                }
                if key_mgmt.contains("wpa-psk") {
                    return WifiType::PreSharedKey;
                }
                if key_mgmt.contains("802-1x") {
                    return WifiType::Enterprise;
                }
            }
            Err(error) => tracing::warn!(
                "Failed to read saved Wi-Fi type for {} ({})",
                self.redact_ssid(ssid),
                error.kind()
            ),
        }
        WifiType::PreSharedKey
    }

    /// `normalizeWifiType(wifi_type)`.
    fn normalize_wifi_type(&self, wifi_type: &Value) -> WifiType {
        if wifi_type.is_null() {
            return WifiType::PreSharedKey;
        }
        match wifi_type.as_str().and_then(WifiType::from_value) {
            Some(t) => t,
            None => {
                tracing::warn!(
                    "Unknown saved Wi-Fi type {}; assuming PSK",
                    py_str(wifi_type)
                );
                WifiType::PreSharedKey
            }
        }
    }

    /// `getLegacyKnownWifiCredentials(ssid, entry)`.
    fn legacy_credentials(&self, ssid: &str, entry: &Value) -> LegacyCredentials {
        match entry {
            Value::String(password) => LegacyCredentials {
                ssid: json!(ssid),
                wifi_type: WifiType::PreSharedKey,
                password: json!(password),
                has_secret: true,
            },
            Value::Object(map) => {
                let wifi_type = self.normalize_wifi_type(map.get("type").unwrap_or(&json!("PSK")));
                let password = map.get("password").cloned().unwrap_or(Value::Null);
                LegacyCredentials {
                    ssid: map.get("ssid").cloned().unwrap_or_else(|| json!(ssid)),
                    wifi_type,
                    has_secret: !password.is_null(),
                    password,
                }
            }
            _ => LegacyCredentials {
                ssid: json!(ssid),
                wifi_type: WifiType::PreSharedKey,
                password: Value::Null,
                has_secret: false,
            },
        }
    }

    /// `createNetworkManagerWifiProfile(ssid, wifi_type, password)`: a
    /// profile that never autoconnects (the auto-connect thread decides).
    fn create_network_manager_wifi_profile(
        &self,
        ssid: &Value,
        wifi_type: WifiType,
        password: &Value,
    ) -> PyResult<bool> {
        let ssid_text = py_str(ssid);
        if matches!(wifi_type, WifiType::PreSharedKey | WifiType::PskSae)
            && !python_truthy(password)
        {
            tracing::warn!(
                "Cannot migrate Wi-Fi profile {}: password is not available",
                self.redact_ssid(&ssid_text)
            );
            return Ok(false);
        }
        let key_mgmt = match wifi_type {
            WifiType::Open => "none",
            WifiType::PreSharedKey => "wpa-psk",
            WifiType::PskSae => "sae",
            other => {
                tracing::warn!("Cannot migrate unsupported Wi-Fi type: {}", other.py_name());
                return Ok(false);
            }
        };

        let mut add = cmd(&[
            "nmcli",
            "connection",
            "add",
            "type",
            "wifi",
            "ifname",
            "*",
            "con-name",
        ]);
        add.push(ssid.clone());
        add.push(json!("ssid"));
        add.push(ssid.clone());
        let added = self.run_command(&add, 10.0);
        if added.as_ref().is_none_or(|done| done.returncode != 0) {
            tracing::warn!(
                "Failed to create migrated Wi-Fi profile {ssid_text}: {}",
                output_detail(&added)
            );
            return Ok(false);
        }

        let mut settings = cmd(&["nmcli", "connection", "modify"]);
        settings.push(ssid.clone());
        settings.extend(cmd(&[
            "connection.autoconnect",
            "no",
            "802-11-wireless-security.key-mgmt",
            key_mgmt,
        ]));
        if python_truthy(password) {
            settings.push(json!("802-11-wireless-security.psk"));
            settings.push(password.clone());
        }
        let modified = self.run_command(&settings, 10.0);
        if modified.as_ref().is_none_or(|done| done.returncode != 0) {
            tracing::warn!(
                "Failed to configure migrated Wi-Fi profile {ssid_text}: {}",
                output_detail(&modified)
            );
            if let Value::String(name) = ssid {
                self.delete_connection_profile(name)?;
            } else {
                self.host().nmcli().connections()?;
            }
            return Ok(false);
        }

        tracing::info!(
            "Migrated Wi-Fi profile {} into NetworkManager",
            self.redact_ssid(&ssid_text)
        );
        Ok(true)
    }

    /// `migrateKnownWifiSecretsToNetworkManager()`.
    pub(crate) fn migrate_known_wifi_secrets(&self) -> PyResult<()> {
        let known = self.config.get(keys::WIFI, keys::WIFI_KNOWN_WIFIS);
        if !python_truthy(&known) {
            return Ok(());
        }
        let Value::Object(known) = known else {
            return Err(PyError::attribute_error(format!(
                "'{}' object has no attribute 'items'",
                type_name(&known)
            )));
        };

        let network_manager = self.get_network_manager_wifi_connections();
        let mut scrubbed = Map::new();
        let mut changed = false;
        for (ssid, entry) in &known {
            let credentials = self.legacy_credentials(ssid, entry);
            let saved_ssid = credentials.saved_ssid_key();
            let wifi_type = credentials.wifi_type;

            if dict_has(&network_manager, &credentials.ssid)? {
                let replacement = json!({
                    "ssid": credentials.ssid,
                    "type": network_manager[&saved_ssid]["type"],
                });
                if &replacement != entry {
                    changed = true;
                }
                scrubbed.insert(saved_ssid, replacement);
                continue;
            }

            if credentials.has_secret || wifi_type == WifiType::Open {
                let migrated = self.create_network_manager_wifi_profile(
                    &credentials.ssid,
                    wifi_type,
                    &credentials.password,
                )?;
                if migrated {
                    scrubbed.insert(
                        saved_ssid,
                        json!({"ssid": credentials.ssid, "type": wifi_type.value()}),
                    );
                    changed = true;
                    continue;
                }
                scrubbed.insert(ssid.clone(), entry.clone());
                continue;
            }

            let replacement = json!({"ssid": credentials.ssid, "type": wifi_type.value()});
            if &replacement != entry {
                changed = true;
            }
            scrubbed.insert(ssid.clone(), replacement);
        }

        if changed {
            self.config
                .set(keys::WIFI, keys::WIFI_KNOWN_WIFIS, Value::Object(scrubbed));
            self.config.save();
        }
        Ok(())
    }

    /// `getKnownWifis()`.
    pub fn get_known_wifis(&self) -> PyResult<Map<String, Value>> {
        self.migrate_known_wifi_secrets()?;
        Ok(self.get_network_manager_wifi_connections())
    }

    /// `hasKnownWifiConnection(ssid)`.
    pub(crate) fn has_known_wifi_connection(&self, ssid: &Value) -> PyResult<bool> {
        dict_has(&self.get_network_manager_wifi_connections(), ssid)
    }

    /// `persistClientModeAfterManualConnect(ssid)`.
    pub(crate) fn persist_client_mode_after_manual_connect(&self, ssid: &str) {
        if self.mode().as_str() == Some(WIFI_MODE_CLIENT) {
            return;
        }
        tracing::info!("Persisting Wi-Fi client mode after successful manual connection to {ssid}");
        self.config
            .set(keys::WIFI, keys::WIFI_MODE, json!(WIFI_MODE_CLIENT));
        self.config.save();
        self.invalidate_health_cache();
    }
}

impl LegacyCredentials {
    /// The dict key the Python would use for `saved_ssid`.
    fn saved_ssid_key(&self) -> String {
        py_str(&self.ssid)
    }
}

/// `stderr.strip() or stdout.strip()` of a finished command.
fn output_detail(done: &Option<crate::py::Completed>) -> String {
    match done {
        Some(done) => {
            let stderr = done.stderr.trim();
            if stderr.is_empty() {
                done.stdout.trim().to_string()
            } else {
                stderr.to_string()
            }
        }
        None => String::new(),
    }
}
