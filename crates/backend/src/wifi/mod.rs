//! `wifi.py`: the `WifiManager`. NetworkManager (through `nmcli`) runs the
//! radio either as the machine's own hotspot (`AP` mode) or as a client of
//! the user's network (`CLIENT` mode); this service switches between them,
//! joins and forgets networks, keeps a scan cache for the pickers, reports
//! whether the connection actually works, and repairs it.
//!
//! The Python keeps all of this in class attributes shared by the HTTP
//! executor threads, the auto-connect thread and short-lived refresh
//! threads; here it is one service with short locks. Every call that touches
//! NetworkManager blocks, so async callers use `spawn_blocking`.
//!
//! Python exceptions that reach a client keep their class name and text as a
//! [`PyError`]; methods that can raise in Python return `PyResult`.

mod connect;
#[cfg(test)]
pub(crate) mod fake;
mod health;
pub mod host;
mod hotspot;
mod known;
pub mod nmcli;
pub mod radio;
pub mod redact;
mod scan;
pub mod types;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::{Mutex, RwLock};
use serde_json::{Value, json};

use crate::config::{ConfigStore, WIFI_MODE_AP, keys};
use crate::hostname::HostnameManager;
use crate::py::{PyError, PyResult, py_str};
use crate::timezone::TimezoneManager;
use crate::zeroconf::Announcer;

pub use connect::psk_credentials;
pub use host::{SystemWifiHost, WifiHost};
pub use types::{ScanEntry, WifiSystemConfig, WifiType, health_error_message};

use health::HealthCacheEntry;
use redact::SsidRedactor;

const HEALTH_CHECK_INTERVAL: f64 = 60.0;
const RECOVERY_COOLDOWN: f64 = 300.0;
const HEALTH_FAILURE_THRESHOLD: u32 = 3;
const HEALTH_CACHE_TTL: f64 = 10.0;
const MACHINE_HOSTNAMES: [&str; 2] = ["imx8mn-var-som", "meticulous"];

/// Builds the zeroconf announcer the first time `init` runs.
pub type AnnouncerFactory =
    Arc<dyn Fn(Weak<WifiManager>) -> PyResult<Arc<dyn Announcer>> + Send + Sync>;
type Hook = Arc<dyn Fn() + Send + Sync>;
type Probe = Arc<dyn Fn() -> bool + Send + Sync>;

/// The class attributes of `WifiManager` that change at run time.
#[derive(Debug, Clone)]
struct State {
    known_wifis: Vec<ScanEntry>,
    scan_cache: Vec<ScanEntry>,
    scan_cache_time: f64,
    last_health_error: String,
    last_recovery_action: String,
    last_recovery_result: String,
    health_failures: u32,
    last_recovery_attempt: f64,
    last_health_check: f64,
    last_connection_error_code: String,
    last_connection_error_message: String,
    auto_connect_suppressed_until: f64,
    last_auto_connect_suppressed_log: f64,
}

impl Default for State {
    fn default() -> Self {
        State {
            known_wifis: Vec::new(),
            scan_cache: Vec::new(),
            scan_cache_time: 0.0,
            last_health_error: String::new(),
            last_recovery_action: String::new(),
            last_recovery_result: "not_needed".into(),
            health_failures: 0,
            last_recovery_attempt: 0.0,
            last_health_check: 0.0,
            last_connection_error_code: String::new(),
            last_connection_error_message: String::new(),
            auto_connect_suppressed_until: 0.0,
            last_auto_connect_suppressed_log: 0.0,
        }
    }
}

pub struct WifiManager {
    me: Weak<WifiManager>,
    config: Arc<ConfigStore>,
    hostname: Arc<HostnameManager>,
    timezone: Arc<TimezoneManager>,
    host: RwLock<Arc<dyn WifiHost>>,
    redactor: SsidRedactor,
    /// `ZEROCONF_OVERWRITE`: "192.168.2.123/24,MyHostname" replaces what
    /// NetworkManager reports.
    zeroconf_overwrite: RwLock<String>,
    networking_available: AtomicBool,
    state: Mutex<State>,
    /// (health, cached_at, signature), replaced as one value so a reader
    /// never sees half of two builds.
    health_cache: Mutex<Option<Arc<HealthCacheEntry>>>,
    health_check_lock: Mutex<()>,
    health_check_in_progress: AtomicBool,
    scan_lock: Mutex<()>,
    scan_in_progress: AtomicBool,
    repair_in_progress: AtomicBool,
    auto_connect_started: AtomicBool,
    zeroconf: Mutex<Option<Arc<dyn Announcer>>>,
    announcer_factory: RwLock<AnnouncerFactory>,
    gatt_hook: RwLock<Option<Hook>>,
    manufacturing: RwLock<Probe>,
    /// PARITY: `repairWifiConnection` reads `.connected` on a
    /// `WifiHealthStatus`, which only has `link_connected`, so every repair
    /// that gets past the guards raises `AttributeError`. Tests turn this off
    /// to exercise the repair steps the Python intends.
    health_has_no_connected: AtomicBool,
}

impl WifiManager {
    pub fn new(
        config: Arc<ConfigStore>,
        hostname: Arc<HostnameManager>,
        timezone: Arc<TimezoneManager>,
        host: Arc<dyn WifiHost>,
        redaction_key: std::path::PathBuf,
        announcer_factory: AnnouncerFactory,
    ) -> Arc<WifiManager> {
        Arc::new_cyclic(|me| WifiManager {
            me: me.clone(),
            config,
            hostname,
            timezone,
            host: RwLock::new(host),
            redactor: SsidRedactor::new(redaction_key),
            zeroconf_overwrite: RwLock::new(
                std::env::var("ZEROCONF_OVERWRITE").unwrap_or_default(),
            ),
            networking_available: AtomicBool::new(true),
            state: Mutex::new(State::default()),
            health_cache: Mutex::new(None),
            health_check_lock: Mutex::new(()),
            health_check_in_progress: AtomicBool::new(false),
            scan_lock: Mutex::new(()),
            scan_in_progress: AtomicBool::new(false),
            repair_in_progress: AtomicBool::new(false),
            auto_connect_started: AtomicBool::new(false),
            zeroconf: Mutex::new(None),
            announcer_factory: RwLock::new(announcer_factory),
            gatt_hook: RwLock::new(None),
            manufacturing: RwLock::new(Arc::new(|| false)),
            health_has_no_connected: AtomicBool::new(true),
        })
    }

    pub(crate) fn host(&self) -> Arc<dyn WifiHost> {
        self.host.read().clone()
    }

    #[cfg(test)]
    pub fn set_host(&self, host: Arc<dyn WifiHost>) {
        *self.host.write() = host;
    }

    #[cfg(test)]
    pub fn set_zeroconf(&self, announcer: Option<Arc<dyn Announcer>>) {
        *self.zeroconf.lock() = announcer;
    }

    #[cfg(test)]
    pub fn set_announcer_factory(&self, factory: AnnouncerFactory) {
        *self.announcer_factory.write() = factory;
    }

    #[cfg(test)]
    pub(crate) fn set_zeroconf_overwrite(&self, value: &str) {
        *self.zeroconf_overwrite.write() = value.to_string();
    }

    #[cfg(test)]
    pub(crate) fn set_repair_in_progress(&self, value: bool) {
        self.repair_in_progress.store(value, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn set_health_has_no_connected(&self, value: bool) {
        self.health_has_no_connected.store(value, Ordering::Relaxed);
    }

    /// Where `update_gatt_advertisement` goes once the BLE server runs.
    pub fn set_gatt_hook(&self, hook: Hook) {
        *self.gatt_hook.write() = Some(hook);
    }

    /// `Machine.enable_manufacturing`, read by the auto-connect thread. The
    /// machine service sets this when it starts.
    #[allow(dead_code, reason = "wired by the machine service, ported separately")]
    pub fn set_manufacturing_probe(&self, probe: Probe) {
        *self.manufacturing.write() = probe;
    }

    fn arc(&self) -> Option<Arc<WifiManager>> {
        self.me.upgrade()
    }

    /// `redact_ssid(ssid)`.
    pub fn redact_ssid(&self, ssid: &str) -> String {
        self.redactor.redact(ssid)
    }

    /// `networking_available()`.
    pub fn networking_available(&self) -> bool {
        self.networking_available.load(Ordering::Relaxed)
    }

    pub fn set_networking_available(&self, available: bool) {
        self.networking_available
            .store(available, Ordering::Relaxed);
    }

    fn mode(&self) -> Value {
        self.config.get(keys::WIFI, keys::WIFI_MODE)
    }

    fn is_ap_mode(&self) -> bool {
        self.mode().as_str() == Some(WIFI_MODE_AP)
    }

    /// `WifiManager._last_health_error`.
    pub fn last_health_error(&self) -> String {
        self.state.lock().last_health_error.clone()
    }

    pub(crate) fn set_last_health_error(&self, error: impl Into<String>) {
        self.state.lock().last_health_error = error.into();
    }

    /// `getLastHealthErrorMessage()`.
    pub fn last_health_error_message(&self) -> &'static str {
        health_error_message(&self.last_health_error())
    }

    // ---- connection errors and auto-connect suppression ----

    fn clear_last_connection_error(&self) {
        let mut state = self.state.lock();
        state.last_connection_error_code.clear();
        state.last_connection_error_message.clear();
    }

    fn set_last_connection_error(&self, code: &str, message: &str) {
        {
            let mut state = self.state.lock();
            state.last_connection_error_code = code.to_string();
            state.last_connection_error_message = message.to_string();
        }
        tracing::warn!("WiFi connection failed: {code}: {message}");
    }

    /// `getLastConnectionError()`: (code, message) with the generic fallbacks.
    pub fn last_connection_error(&self) -> (String, String) {
        let state = self.state.lock();
        let code = if state.last_connection_error_code.is_empty() {
            "connection_failed".to_string()
        } else {
            state.last_connection_error_code.clone()
        };
        let message = if state.last_connection_error_message.is_empty() {
            "Could not connect to Wi-Fi.".to_string()
        } else {
            state.last_connection_error_message.clone()
        };
        (code, message)
    }

    fn suppress_auto_connect(&self, seconds: f64, reason: &str) {
        let until = self.host().time() + seconds;
        self.state.lock().auto_connect_suppressed_until = until;
        tracing::info!(
            "Suppressing WiFi auto-connect for {}s: {reason}",
            seconds as i64
        );
    }

    pub fn is_auto_connect_suppressed(&self) -> bool {
        self.host().time() < self.state.lock().auto_connect_suppressed_until
    }

    /// `getAutoConnectSuppressionRemaining()`.
    pub fn auto_connect_suppression_remaining(&self) -> i64 {
        let until = self.state.lock().auto_connect_suppressed_until;
        ((until - self.host().time()) as i64).max(0)
    }

    // ---- the Wi-Fi device ----

    /// `getWifiDeviceState()`.
    fn wifi_device_state(&self) -> Option<String> {
        match self.host().nmcli().device_status() {
            Ok(devices) => devices
                .into_iter()
                .find(|d| d.device_type == "wifi")
                .map(|d| d.state),
            Err(error) => {
                tracing::warn!("Failed to read WiFi device state: {error}");
                None
            }
        }
    }

    /// `isWifiDeviceReady()`.
    fn is_wifi_device_ready(&self) -> bool {
        matches!(
            self.wifi_device_state().as_deref(),
            Some("connected" | "connecting" | "disconnected")
        )
    }

    /// `getWifiStationDeviceName()`.
    fn wifi_station_device_name(&self) -> String {
        match self.host().nmcli().device_status() {
            Ok(devices) => {
                if let Some(device) = devices.into_iter().find(|d| {
                    d.device_type == "wifi"
                        && !d.device.starts_with("p2p-")
                        && d.state != "unmanaged"
                }) {
                    return device.device;
                }
            }
            Err(error) => tracing::warn!("Failed to read WiFi device name: {error}"),
        }
        "wlan0".into()
    }

    /// `runCommand(command, timeout)`: `None` when the command could not run
    /// at all. Arguments come from the settings as well as literals; a
    /// setting that is not a string fails like `subprocess.run` does.
    fn run_command(&self, command: &[Value], timeout: f64) -> Option<crate::py::Completed> {
        let metadata = command_metadata(command);
        let mut argv = Vec::with_capacity(command.len());
        for part in command {
            match part {
                Value::String(s) => argv.push(s.clone()),
                _ => {
                    tracing::warn!("Failed to run {metadata} (TypeError)");
                    return None;
                }
            }
        }
        match self.host().run(&argv, Some(timeout)) {
            Ok(done) => Some(done),
            Err(error) => {
                tracing::warn!("Failed to run {metadata} ({})", error.kind());
                None
            }
        }
    }

    // ---- collaborators ----

    /// `WifiManager._zeroconf.restart()`, which raises before `init` created
    /// the announcer.
    fn zeroconf_restart(&self) -> PyResult<()> {
        let announcer = self.zeroconf.lock().clone();
        match announcer {
            Some(announcer) => announcer.restart(),
            None => Err(PyError::attribute_error(
                "'NoneType' object has no attribute 'restart'",
            )),
        }
    }

    /// `update_gatt_advertisement()`.
    fn update_gatt_advertisement(&self) {
        let hook = self.gatt_hook.read().clone();
        match hook {
            Some(hook) => hook(),
            None => tracing::warn!("Cannot update GATT advertisement - server or loop not ready"),
        }
    }

    // ---- start-up ----

    /// `WifiManager.init()`.
    pub fn init(&self) -> PyResult<()> {
        tracing::info!("Wifi initializing");
        if !self.zeroconf_overwrite.read().is_empty() {
            tracing::info!("Overwriting network configuration due to ZEROCONF_OVERWRITE");
        }
        if let Err(error) = self.host().nmcli().device_show_all() {
            tracing::warn!("Networking unavailable! {error}");
            self.set_networking_available(false);
        }

        let config = self.get_current_config()?;
        // Only rename a machine (or a container of one) whose name is still
        // the image default or ours, so a lookup table change takes effect.
        tracing::info!("Current hostname is '{}'", config.hostname);
        let hostname_override = self.config.get(keys::USER, keys::HOSTNAME_OVERRIDE);
        if MACHINE_HOSTNAMES
            .iter()
            .any(|prefix| config.hostname.starts_with(prefix))
            && hostname_override.is_null()
        {
            let new_hostname = self.hostname.generate_hostname();
            if config.hostname != new_hostname {
                tracing::info!("Changing hostname new = {new_hostname}");
                self.hostname.set_hostname(&new_hostname)?;
            }
        } else if !hostname_override.is_null() && hostname_override.as_str() != Some("none") {
            let wanted = py_str(&hostname_override);
            tracing::info!("Hostname override is set to {wanted}");
            if config.hostname != wanted {
                tracing::info!("Setting hostname to override: {wanted}");
                self.hostname.set_hostname(&wanted)?;
            }
        }

        let ap_name: String = self
            .hostname
            .generate_device_name()
            .chars()
            .take(31)
            .collect();
        self.config
            .set(keys::WIFI, keys::WIFI_AP_NAME, json!(ap_name));
        self.config.save();

        if self.zeroconf.lock().is_none() {
            tracing::info!("Creating Zeroconf Object");
            let factory = self.announcer_factory.read().clone();
            let announcer = factory(self.me.clone())?;
            *self.zeroconf.lock() = Some(announcer);
        }

        if self.networking_available() {
            self.migrate_known_wifi_secrets()?;
            if self.is_ap_mode() {
                self.start_hotspot()?;
            } else {
                self.stop_hotspot()?;
            }
            if let Some(me) = self.arc() {
                self.auto_connect_started.store(true, Ordering::Relaxed);
                self.host()
                    .spawn("WifiAutoConnect", Box::new(move || me.try_auto_connect()));
            }
        }

        let announcer = self.zeroconf.lock().clone();
        if let Some(announcer) = announcer {
            announcer.start()?;
        }
        Ok(())
    }

    /// Only `init`'s NetworkManager probe: whether `nmcli device show`
    /// works decides `networking_available()`.
    pub fn probe_networking(&self) {
        if let Err(error) = self.host().nmcli().device_show_all() {
            tracing::warn!("Networking unavailable! {error}");
            self.set_networking_available(false);
        }
    }

    /// `start_wifi_manager_in_background()` (backend.py).
    pub fn start_in_background(self: &Arc<Self>) {
        tracing::info!("Starting Wi-Fi initialization in background");
        let me = self.clone();
        self.host().spawn("WifiInit", Box::new(move || me.init()));
    }

    /// `tryAutoConnect()`: every ten seconds, outside hotspot mode, keep the
    /// current connection healthy or join a known network. Returns only when
    /// an iteration raises, which in Python ends the thread.
    fn try_auto_connect(&self) -> PyResult<()> {
        tracing::info!("Starting Networking background Thread");
        loop {
            self.host().sleep(10.0);
            self.auto_connect_iteration()?;
        }
    }

    /// One pass of `tryAutoConnect`'s loop.
    pub(crate) fn auto_connect_iteration(&self) -> PyResult<()> {
        let manufacturing = (self.manufacturing.read().clone())();
        if self.is_ap_mode() && !manufacturing {
            return Ok(());
        }

        if self.repair_in_progress.load(Ordering::Relaxed) || self.is_auto_connect_suppressed() {
            let now = self.host().time();
            let last = self.state.lock().last_auto_connect_suppressed_log;
            if now - last > 30.0 {
                let remaining = self.auto_connect_suppression_remaining();
                tracing::info!(
                    "Skipping WiFi auto-connect while manual connect or repair is in progress ({remaining}s remaining)"
                );
                self.state.lock().last_auto_connect_suppressed_log = now;
            }
            return Ok(());
        }

        let current = self.get_current_config()?;
        if current.connected {
            self.maybe_recover_current_connection(Some(current))?;
            self.timezone.tz_background_update();
            return Ok(());
        }

        let networks = self.scan_for_networks(10.0, None);
        let known = self.get_known_wifis()?;
        for network in networks {
            // The factory's test network.
            if manufacturing && network.ssid == "MeticulousEPW" {
                let credentials = psk_credentials(&network.ssid, "23456789");
                if self.connect_to_wifi(&credentials, "auto")? {
                    break;
                }
            }
            if known.contains_key(&network.ssid) {
                tracing::info!(
                    "Found known WIFI {}. Connecting",
                    self.redact_ssid(&network.ssid)
                );
                let wifi_type = WifiType::from_nmcli_security(&network.security);
                let credentials = json!({
                    "ssid": network.ssid,
                    "type": wifi_type.map(WifiType::value),
                });
                if self.connect_to_wifi(&credentials, "auto")? {
                    break;
                }
            }
        }
        Ok(())
    }

    // ---- the current configuration ----

    /// `mockCurrentConfig()`: the address and hostname from
    /// `ZEROCONF_OVERWRITE`.
    fn mock_current_config(&self) -> PyResult<WifiSystemConfig> {
        let overwrite = self.zeroconf_overwrite.read().clone();
        let mut parts = overwrite.split(',');
        let network = types::IpNet::parse(parts.next().unwrap_or(""))?;
        let hostname = parts
            .next()
            .ok_or_else(|| PyError::new("IndexError", "list index out of range"))?;
        Ok(WifiSystemConfig {
            connected: true,
            connection_name: Some("MeticulousMockConnection".into()),
            gateway: Some(network.first()),
            routes: Vec::new(),
            dns: vec!["8.8.8.8".parse().expect("static address")],
            ips: vec![network],
            mac: Some("AA:BB:CC:FF:FF:FF".into()),
            hostname: hostname.to_string(),
            domains: Vec::new(),
        })
    }

    /// `getCurrentConfig()`: what NetworkManager reports for the Wi-Fi
    /// devices.
    pub fn get_current_config(&self) -> PyResult<WifiSystemConfig> {
        if !self.zeroconf_overwrite.read().is_empty() {
            return self.mock_current_config();
        }
        let host = self.host();
        let mut config = WifiSystemConfig::disconnected(host.gethostname());
        if !self.networking_available() {
            return Ok(config);
        }
        for device in host.nmcli().device_status()? {
            if device.device_type != "wifi" {
                continue;
            }
            let details = host.nmcli().device_show(&device.device)?;
            if device.state == "connected" {
                config.connected = true;
                for (key, value) in &details.0 {
                    if key.contains("IP4.ADDRESS") || key.contains("IP6.ADDRESS") {
                        if let Some(v) = value {
                            config.ips.push(types::IpNet::parse(v)?);
                        }
                    } else if key.contains("IP4.ROUTE") || key.contains("IP6.ROUTE") {
                        if let Some(v) = value {
                            config.routes.push(v.clone());
                        }
                    } else if key.contains("IP4.DNS") || key.contains("IP6.DNS") {
                        if let Some(v) = value {
                            config.dns.push(types::parse_ip(v)?);
                        }
                    } else if key.contains("IP4.DOMAIN") {
                        if let Some(v) = value.as_deref().filter(|v| *v != "domain_not_set.invalid")
                        {
                            config.domains.push(v.to_string());
                        }
                    } else if key == "GENERAL.HWADDR" {
                        config.mac = value.clone();
                    } else if key == "GENERAL.CONNECTION" {
                        config.connection_name = value.clone();
                    } else if key == "IP4.GATEWAY"
                        && let Some(v) = value
                    {
                        config.gateway = Some(types::parse_ip(v)?);
                    }
                }
            } else if config.mac.as_deref() == Some("")
                && let Some(mac) = details.get("GENERAL.HWADDR").filter(|m| !m.is_empty())
            {
                config.mac = Some(mac.to_string());
            }
        }
        Ok(config)
    }
}

/// `sensitive_logging.command_metadata(command)`.
fn command_metadata(command: &[Value]) -> String {
    let Some(first) = command.first() else {
        return "<empty command>".into();
    };
    let executable = py_str(first);
    let base = executable.rsplit('/').next().unwrap_or(&executable);
    format!("{base} ({} args)", command.len().saturating_sub(1))
}

/// A command line of literals, for `run_command`.
fn cmd(parts: &[&str]) -> Vec<Value> {
    parts.iter().map(|p| Value::String(p.to_string())).collect()
}

#[cfg(test)]
mod tests;
