//! Hotspot (`AP`) and client mode switching.

use serde_json::{Value, json};

use super::types::AP_CONNECTION;
use super::{WifiManager, cmd};
use crate::config::{WIFI_MODE_AP, keys};
use crate::py::PyResult;

/// The hotspot's channels, tried in order.
const CHANNELS: [u32; 3] = [6, 1, 11];

impl WifiManager {
    /// `resetWifiMode()`: bring NetworkManager in line with the configured
    /// mode.
    pub fn reset_wifi_mode(&self) -> PyResult<bool> {
        if !self.networking_available() {
            return Ok(false);
        }
        let success = if self.is_ap_mode() {
            self.start_hotspot()?
        } else {
            let success = self.stop_hotspot()?;
            self.scan_for_networks(1.0, None);
            self.zeroconf_restart()?;
            success
        };
        self.update_gatt_advertisement();
        Ok(success)
    }

    /// The command lines' failure text: stderr, else stdout, else `fallback`.
    fn command_failure(
        done: &Option<crate::py::Completed>,
        channel: u32,
        fallback: &str,
    ) -> String {
        let (stderr, stdout) = match done {
            Some(done) => (
                done.stderr.trim().to_string(),
                done.stdout.trim().to_string(),
            ),
            None => ("command failed".to_string(), String::new()),
        };
        let detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            fallback.to_string()
        };
        format!("channel={channel}: {detail}")
    }

    /// `startHotspot()`: (re)create the access point profile and bring it
    /// up, on channel 6, then 1, then 11.
    pub fn start_hotspot(&self) -> PyResult<bool> {
        if !self.networking_available() {
            self.set_last_health_error("networking_unavailable");
            return Ok(false);
        }
        tracing::info!("Starting hotspot");
        self.invalidate_health_cache();
        let mut last_error = String::new();

        for channel in CHANNELS {
            self.delete_connection_profile(AP_CONNECTION)?;
            tracing::info!("Starting hotspot on channel {channel}");

            let mut add = cmd(&["nmcli", "connection", "add", "type", "wifi", "ifname"]);
            add.push(json!(self.wifi_station_device_name()));
            add.extend(cmd(&["con-name", AP_CONNECTION, "ssid"]));
            add.push(self.config.get(keys::WIFI, keys::WIFI_AP_NAME));
            let added = self.run_command(&add, 10.0);
            if added.as_ref().is_none_or(|done| done.returncode != 0) {
                last_error = Self::command_failure(&added, channel, "connection add failed");
                tracing::error!("Starting hotspot failed: {last_error}");
                self.delete_connection_profile(AP_CONNECTION)?;
                self.host().sleep(1.0);
                continue;
            }

            let channel_text = channel.to_string();
            let mut modify = cmd(&[
                "nmcli",
                "connection",
                "modify",
                AP_CONNECTION,
                "connection.autoconnect",
                "no",
                "802-11-wireless.mode",
                "ap",
                "802-11-wireless.band",
                "bg",
                "802-11-wireless.channel",
                &channel_text,
                "802-11-wireless.ap-isolation",
                "0",
                "802-11-wireless.powersave",
                "2",
                "802-11-wireless-security.key-mgmt",
                "wpa-psk",
                "802-11-wireless-security.auth-alg",
                "open",
                "802-11-wireless-security.psk",
            ]);
            modify.push(self.config.get(keys::WIFI, keys::WIFI_AP_PASSWORD));
            modify.extend(cmd(&[
                "802-11-wireless-security.proto",
                "rsn",
                "802-11-wireless-security.pairwise",
                "ccmp",
                "802-11-wireless-security.group",
                "ccmp",
                "802-11-wireless-security.pmf",
                "1",
                "802-11-wireless-security.wps-method",
                "0",
                "ipv4.method",
                "shared",
                "ipv6.method",
                "ignore",
            ]));
            let modified = self.run_command(&modify, 10.0);
            if modified.as_ref().is_none_or(|done| done.returncode != 0) {
                last_error = Self::command_failure(&modified, channel, "connection modify failed");
                tracing::error!("Starting hotspot failed: {last_error}");
                self.delete_connection_profile(AP_CONNECTION)?;
                self.host().sleep(1.0);
                continue;
            }

            let up = cmd(&["nmcli", "--wait", "35", "connection", "up", AP_CONNECTION]);
            let result = self.run_command(&up, 35.0);
            if result.as_ref().is_some_and(|done| done.returncode == 0) {
                if self.wait_for_hotspot(10.0) {
                    tracing::info!("Hotspot started on channel {channel}");
                    self.set_last_health_error("");
                    self.zeroconf_restart()?;
                    return Ok(true);
                }
                last_error = format!("channel={channel}: hotspot did not become active");
                tracing::error!("Starting hotspot failed: {last_error}");
            } else {
                last_error = Self::command_failure(&result, channel, "unknown error");
                tracing::error!("Starting hotspot failed: {last_error}");
            }
            self.delete_connection_profile(AP_CONNECTION)?;
            self.host().sleep(1.0);
        }

        self.set_last_health_error(format!("hotspot_start_failed: {last_error}"));
        self.zeroconf_restart()?;
        Ok(false)
    }

    /// `stopHotspot()`: take the access point down if it is up.
    pub fn stop_hotspot(&self) -> PyResult<bool> {
        if !self.networking_available() {
            self.set_last_health_error("networking_unavailable");
            return Ok(false);
        }
        let host = self.host();
        for device in host.nmcli().device_status()? {
            if device.device_type != "wifi" || device.connection.as_deref() != Some(AP_CONNECTION) {
                continue;
            }
            tracing::info!("Stopping Hotspot");
            self.invalidate_health_cache();
            if let Err(error) = host.nmcli().connection_down(AP_CONNECTION) {
                tracing::error!("Stopping hotspot failed: {error}");
                self.set_last_health_error(format!("hotspot_stop_failed: {error}"));
                self.zeroconf_restart()?;
                return Ok(false);
            }
            self.zeroconf_restart()?;
            if self.wait_for_hotspot_stopped(10.0) {
                self.set_last_health_error("");
                return Ok(true);
            }
            self.set_last_health_error("hotspot_still_active");
            return Ok(false);
        }
        Ok(true)
    }

    /// `waitForHotspot(timeout)`.
    fn wait_for_hotspot(&self, timeout: f64) -> bool {
        let host = self.host();
        let deadline = host.time() + timeout;
        while host.time() < deadline {
            match self.get_current_config() {
                Ok(config) if config.is_hotspot() => return true,
                Ok(_) => {}
                Err(error) => tracing::info!("Failed to verify hotspot state: {error}"),
            }
            host.sleep(1.0);
        }
        false
    }

    /// `waitForHotspotStopped(timeout)`.
    fn wait_for_hotspot_stopped(&self, timeout: f64) -> bool {
        let host = self.host();
        let deadline = host.time() + timeout;
        while host.time() < deadline {
            match self.get_current_config() {
                Ok(config) if !config.is_hotspot() => return true,
                Ok(_) => {}
                Err(error) => tracing::info!("Failed to verify hotspot stop state: {error}"),
            }
            host.sleep(1.0);
        }
        false
    }

    /// `applyWifiSettings(mode, ap_password)`: `None` keeps the current
    /// value. Persisted only when the radio follows; otherwise the previous
    /// settings come back.
    pub fn apply_wifi_settings(
        &self,
        mode: Option<Value>,
        ap_password: Option<Value>,
    ) -> PyResult<bool> {
        let previous_mode = self.config.get(keys::WIFI, keys::WIFI_MODE);
        let previous_password = self.config.get(keys::WIFI, keys::WIFI_AP_PASSWORD);
        let mode = mode.unwrap_or_else(|| previous_mode.clone());
        let ap_password = ap_password.unwrap_or_else(|| previous_password.clone());

        self.config.set(keys::WIFI, keys::WIFI_MODE, mode.clone());
        self.config
            .set(keys::WIFI, keys::WIFI_AP_PASSWORD, ap_password);
        self.invalidate_health_cache();

        let mut success = self.reset_wifi_mode()?;
        if !success && mode.as_str() == Some(WIFI_MODE_AP) {
            tracing::warn!("Hotspot mode change failed; attempting AP recovery");
            success = self.repair_wifi_connection("apply_ap_mode")?;
        }
        if success {
            self.config.save();
            return Ok(true);
        }

        tracing::warn!("WiFi config change failed; rolling back persisted config");
        self.config.set(keys::WIFI, keys::WIFI_MODE, previous_mode);
        self.config
            .set(keys::WIFI, keys::WIFI_AP_PASSWORD, previous_password);
        self.config.save();
        self.reset_wifi_mode()?;
        Ok(false)
    }
}
