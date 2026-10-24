//! Health of the active Wi-Fi path (link, IPv4, gateway, DNS, internet), its
//! short-lived cache, automatic recovery and the repair workflow.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::Value;

use super::radio::{RecoveryEvent, emit_radio_recovery_event};
use super::types::{WifiHealthStatus, WifiSystemConfig, health_error_is_recoverable};
use super::{
    HEALTH_CACHE_TTL, HEALTH_CHECK_INTERVAL, HEALTH_FAILURE_THRESHOLD, RECOVERY_COOLDOWN,
    WifiManager, cmd,
};
use crate::py::{PyError, PyResult, py_repr, py_str};

const DRIVER_RESET_SCRIPT: &str = "/etc/wifi/variscite-wifi.d/iw612-wifi";
const HOTSPOT_ACTIVE_MESSAGE: &str =
    "Hotspot active. Connect your phone or computer to the machine's Wi-Fi network.";
const INTERNET_PROBES: [&str; 2] = [
    "https://meticuloushome.com",
    "https://www.cloudflare.com/cdn-cgi/trace",
];

/// `getHealthCacheSignature(config)`: what must not change for a cached
/// health to still describe the network.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Signature {
    mode: Value,
    connected: bool,
    connection_name: Option<String>,
    ips: Vec<String>,
    gateway: String,
}

#[derive(Debug)]
pub(crate) struct HealthCacheEntry {
    pub health: WifiHealthStatus,
    pub cached_at: f64,
    pub signature: Option<Signature>,
}

/// A repair step (`steps` in `repairWifiConnection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    StartHotspot,
    RestartConnection,
    RestartWifiRadio,
    DriverInBandReset,
    RestartWifiService,
}

impl Step {
    fn action(self) -> &'static str {
        match self {
            Step::StartHotspot => "start_hotspot",
            Step::RestartConnection => "restart_connection",
            Step::RestartWifiRadio => "restart_wifi_radio",
            Step::DriverInBandReset => "driver_in_band_reset",
            Step::RestartWifiService => "restart_wifi_service",
        }
    }
}

impl WifiManager {
    pub fn invalidate_health_cache(&self) {
        *self.health_cache.lock() = None;
    }

    fn store_health_cache(
        &self,
        health: WifiHealthStatus,
        signature: Option<Signature>,
    ) -> WifiHealthStatus {
        let entry = HealthCacheEntry {
            health: health.clone(),
            cached_at: self.host().time(),
            signature,
        };
        *self.health_cache.lock() = Some(Arc::new(entry));
        health
    }

    pub(crate) fn health_cache_signature(
        &self,
        config: Option<&WifiSystemConfig>,
    ) -> Option<Signature> {
        let config = config?;
        Some(Signature {
            mode: self.mode(),
            connected: config.connected,
            connection_name: config.connection_name.clone(),
            ips: config.ips.iter().map(ToString::to_string).collect(),
            gateway: config.gateway.map(|g| g.to_string()).unwrap_or_default(),
        })
    }

    /// The cache entry as one value (`_health_cache`).
    pub(crate) fn health_cache_entry(&self) -> Option<Arc<HealthCacheEntry>> {
        self.health_cache.lock().clone()
    }

    /// `getCachedHealth(config, deep)`: reads the cache exactly once.
    fn cached_health(
        &self,
        config: Option<&WifiSystemConfig>,
        deep: bool,
    ) -> Option<WifiHealthStatus> {
        let entry = self.health_cache_entry()?;
        if self.host().time() - entry.cached_at >= HEALTH_CACHE_TTL {
            return None;
        }
        // A shallow entry never ran the probes, so it cannot answer a deep
        // caller however fresh it is.
        if deep && !entry.health.verified {
            return None;
        }
        let signature = self.health_cache_signature(config);
        if signature.is_some() && signature != entry.signature {
            return None;
        }
        Some(entry.health.clone())
    }

    /// `getHealthStatus(config, force, deep)`.
    ///
    /// The shallow path backs the settings screen and the post-join answer:
    /// it never waits for the probes, it answers from the configuration and
    /// starts a background check.
    pub fn get_health_status(
        &self,
        config: Option<WifiSystemConfig>,
        force: bool,
        deep: bool,
    ) -> PyResult<WifiHealthStatus> {
        if !force && let Some(cached) = self.cached_health(config.as_ref(), deep) {
            return Ok(cached);
        }
        if !deep {
            let health = self.build_health_status(config.clone(), false)?;
            self.refresh_health_in_background(config);
            return Ok(health);
        }
        let _guard = self.health_check_lock.lock();
        if !force && let Some(cached) = self.cached_health(config.as_ref(), true) {
            return Ok(cached);
        }
        self.build_health_status(config, true)
    }

    /// `refreshHealthInBackground(config)`.
    pub(crate) fn refresh_health_in_background(&self, config: Option<WifiSystemConfig>) {
        if self.health_check_in_progress.load(Ordering::Relaxed) {
            return;
        }
        let Some(me) = self.arc() else { return };
        self.host().spawn(
            "WifiHealthRefresh",
            Box::new(move || {
                me.health_check_in_progress.store(true, Ordering::Relaxed);
                let result = me.get_health_status(config, true, true);
                me.health_check_in_progress.store(false, Ordering::Relaxed);
                result.map(|_| ())
            }),
        );
    }

    /// `buildHealthStatus(config, deep)`.
    pub(crate) fn build_health_status(
        &self,
        config: Option<WifiSystemConfig>,
        deep: bool,
    ) -> PyResult<WifiHealthStatus> {
        let config = match config {
            Some(config) => config,
            None => self.get_current_config()?,
        };
        let mode = self.mode();
        let signature = self.health_cache_signature(Some(&config));
        let has_ipv4 = config.has_ipv4();
        let ap_active = config.is_hotspot();
        let (recovery_action, recovery_result, stored_error) = {
            let state = self.state.lock();
            (
                state.last_recovery_action.clone(),
                state.last_recovery_result.clone(),
                state.last_health_error.clone(),
            )
        };
        let base = |degraded: bool, last_error: String| WifiHealthStatus {
            mode: mode.clone(),
            link_connected: config.connected,
            has_ipv4,
            gateway_reachable: false,
            dns_resolves: false,
            internet_reachable: false,
            ap_active,
            degraded,
            last_error,
            last_recovery_action: recovery_action.clone(),
            last_recovery_result: recovery_result.clone(),
            message: String::new(),
            verified: true,
        };

        if mode.as_str() == Some(crate::config::WIFI_MODE_AP) {
            let degraded = !ap_active;
            let mut last_error = stored_error;
            if degraded && last_error.is_empty() {
                last_error = "hotspot_not_active".into();
            }
            let mut health = base(degraded, last_error);
            if ap_active {
                health.message = HOTSPOT_ACTIVE_MESSAGE.into();
            }
            return Ok(self.store_health_cache(health, signature));
        }

        if config.connected && has_ipv4 && !deep {
            let mut health = base(false, String::new());
            health.verified = false;
            return Ok(self.store_health_cache(health, signature));
        }

        let (mut gateway, mut dns, mut internet) = (false, false, false);
        if config.connected && has_ipv4 {
            gateway = self.gateway_reachable(config.gateway);
            dns = self.dns_resolves();
            internet = self.internet_reachable();
        }
        let degraded = !(config.connected && has_ipv4 && gateway && dns && internet);
        let mut last_error = stored_error;
        if degraded && last_error.is_empty() {
            last_error = if !config.connected {
                "wifi_not_connected"
            } else if !has_ipv4 {
                "missing_ipv4"
            } else if !gateway {
                "gateway_unreachable"
            } else if !dns {
                "dns_unreachable"
            } else {
                "internet_unreachable"
            }
            .into();
        }
        let mut health = base(degraded, last_error);
        health.gateway_reachable = gateway;
        health.dns_resolves = dns;
        health.internet_reachable = internet;
        Ok(self.store_health_cache(health, signature))
    }

    /// `gatewayReachable(gateway)`: one ping, or assume yes without `ping`.
    fn gateway_reachable(&self, gateway: Option<IpAddr>) -> bool {
        let Some(gateway) = gateway else {
            return false;
        };
        if !self.host().which("ping") {
            tracing::info!("Skipping gateway ping health check: ping command not found");
            return true;
        }
        let command = cmd(&["ping", "-c", "1", "-W", "2", &gateway.to_string()]);
        self.run_command(&command, 4.0)
            .is_some_and(|done| done.returncode == 0)
    }

    /// `dnsResolves()`.
    fn dns_resolves(&self) -> bool {
        match self.host().getaddrinfo("meticuloushome.com", 443) {
            Ok(()) => true,
            Err(error) => {
                tracing::info!("DNS health check failed: {error}");
                false
            }
        }
    }

    /// `internetReachable()`: `urlopen` raises for 4xx and 5xx, so only a
    /// success or a redirect chain ending in one counts.
    fn internet_reachable(&self) -> bool {
        let host = self.host();
        for probe in INTERNET_PROBES {
            match host.http_status(probe, 2.0, "meticulous-wifi-health/1.0") {
                Ok(status) if status < 400 => return true,
                Ok(status) => {
                    tracing::info!("Internet health check failed for {probe}: HTTP Error {status}")
                }
                Err(error) => tracing::info!("Internet health check failed for {probe}: {error}"),
            }
        }
        false
    }

    /// `maybeRecoverCurrentConnection(config)`: called every auto-connect
    /// pass while connected; acts at most once a minute and repairs after
    /// three degraded checks, five minutes apart at least.
    pub(crate) fn maybe_recover_current_connection(
        &self,
        config: Option<WifiSystemConfig>,
    ) -> PyResult<bool> {
        if self.is_ap_mode() {
            return Ok(false);
        }
        let now = self.host().time();
        {
            let mut state = self.state.lock();
            if now - state.last_health_check < HEALTH_CHECK_INTERVAL {
                return Ok(false);
            }
            state.last_health_check = now;
        }

        let health = self.get_health_status(config, true, true)?;
        if !health.degraded {
            let mut state = self.state.lock();
            state.health_failures = 0;
            state.last_health_error.clear();
            state.last_recovery_result = "not_needed".into();
            return Ok(false);
        }

        let failures = {
            let mut state = self.state.lock();
            state.health_failures += 1;
            state.last_health_error = health.last_error.clone();
            state.health_failures
        };
        tracing::warn!(
            "WiFi health degraded ({failures}/{HEALTH_FAILURE_THRESHOLD}): {}",
            py_repr(&health.to_json())
        );

        if !health_error_is_recoverable(&health.last_error) {
            {
                let mut state = self.state.lock();
                state.last_recovery_action = "health_check".into();
                state.last_recovery_result = "not_recoverable".into();
            }
            tracing::warn!(
                "WiFi health issue is not recoverable by driver reset: {}",
                health.last_error
            );
            return Ok(false);
        }
        if failures < HEALTH_FAILURE_THRESHOLD {
            return Ok(false);
        }
        if now - self.state.lock().last_recovery_attempt < RECOVERY_COOLDOWN {
            tracing::warn!("WiFi recovery skipped due to cooldown");
            return Ok(false);
        }
        self.repair_wifi_connection(&health.last_error)
    }

    fn record(
        &self,
        event: &str,
        operation_id: &str,
        started: f64,
        reason: &str,
        fields: RecoveryEvent<'_>,
    ) {
        let elapsed_ms = ((self.host().monotonic() - started) * 1000.0) as i64;
        let mode = self.mode();
        let mode = match &mode {
            Value::Null => None,
            other if !crate::config::python_truthy(other) => Some("unknown".to_string()),
            other => Some(py_str(other)),
        };
        emit_radio_recovery_event(
            event,
            operation_id,
            elapsed_ms,
            &RecoveryEvent {
                reason: Some(reason),
                mode,
                ..fields
            },
        );
    }

    /// `repairWifiConnection(reason)`: escalate from restarting the
    /// connection (or the hotspot) to resetting the shared Wi-Fi/Bluetooth
    /// radio, checking health after each step.
    pub fn repair_wifi_connection(&self, reason: &str) -> PyResult<bool> {
        let operation_id: String = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let started = self.host().monotonic();

        if !self.networking_available() {
            {
                let mut state = self.state.lock();
                state.last_recovery_result = "failed".into();
                state.last_health_error = "networking_unavailable".into();
            }
            self.record(
                "rejected",
                &operation_id,
                started,
                reason,
                RecoveryEvent {
                    result: Some("networking_unavailable"),
                    ..RecoveryEvent::default()
                },
            );
            return Ok(false);
        }

        if self.repair_in_progress.load(Ordering::Relaxed) {
            tracing::warn!("WiFi repair requested while another repair is in progress");
            self.state.lock().last_recovery_result = "in_progress".into();
            self.record(
                "rejected",
                &operation_id,
                started,
                reason,
                RecoveryEvent {
                    result: Some("already_in_progress"),
                    ..RecoveryEvent::default()
                },
            );
            return Ok(false);
        }

        let current = self.get_current_config()?;
        if !self.is_ap_mode()
            && !current.connected
            && self.get_network_manager_wifi_connections().is_empty()
        {
            let now = self.host().time();
            {
                let mut state = self.state.lock();
                state.last_recovery_attempt = now;
                state.last_recovery_action = "health_check".into();
                state.last_recovery_result = "not_recoverable".into();
                state.last_health_error = "no_saved_wifi_connection".into();
            }
            self.invalidate_health_cache();
            tracing::warn!(
                "WiFi repair skipped because there are no saved client connections. \
                 Refusing to reset the shared WiFi/Bluetooth radio."
            );
            return Ok(false);
        }

        self.repair_in_progress.store(true, Ordering::Relaxed);
        self.suppress_auto_connect(180.0, &format!("wifi repair in progress: {reason}"));
        let result = self.run_repair(reason, &operation_id, started, current);
        self.repair_in_progress.store(false, Ordering::Relaxed);
        self.suppress_auto_connect(15.0, "wifi repair finished");
        result
    }

    fn run_repair(
        &self,
        reason: &str,
        operation_id: &str,
        started: f64,
        current: WifiSystemConfig,
    ) -> PyResult<bool> {
        let now = self.host().time();
        self.state.lock().last_recovery_attempt = now;
        tracing::warn!("Starting WiFi repair workflow. reason={reason}");
        self.invalidate_health_cache();
        self.record(
            "started",
            operation_id,
            started,
            reason,
            RecoveryEvent {
                connected: Some(current.connected),
                has_ipv4: Some(current.has_ipv4()),
                ..RecoveryEvent::default()
            },
        );

        let initial = self.get_health_status(Some(current.clone()), true, true)?;
        if self.health_has_no_connected.load(Ordering::Relaxed) {
            // PARITY: `initial_health.connected` raises; the repair never
            // gets further in Python.
            return Err(no_connected_attribute());
        }
        self.record(
            "health_assessed",
            operation_id,
            started,
            reason,
            RecoveryEvent {
                connected: Some(initial.link_connected),
                has_ipv4: Some(initial.has_ipv4),
                result: Some(non_empty_or(&initial.last_error, "healthy")),
                ..RecoveryEvent::default()
            },
        );
        if !initial.degraded {
            {
                let mut state = self.state.lock();
                state.last_recovery_action = "health_check".into();
                state.last_recovery_result = "not_needed".into();
            }
            self.record(
                "completed",
                operation_id,
                started,
                reason,
                RecoveryEvent {
                    result: Some("not_needed"),
                    ..RecoveryEvent::default()
                },
            );
            return Ok(true);
        }
        if !health_error_is_recoverable(&initial.last_error) {
            {
                let mut state = self.state.lock();
                state.last_recovery_action = "health_check".into();
                state.last_recovery_result = "not_recoverable".into();
                state.last_health_error = initial.last_error.clone();
            }
            tracing::warn!(
                "WiFi repair skipped for non-recoverable health issue: {}",
                initial.last_error
            );
            self.record(
                "completed",
                operation_id,
                started,
                reason,
                RecoveryEvent {
                    result: Some("not_recoverable"),
                    ..RecoveryEvent::default()
                },
            );
            return Ok(false);
        }

        let ap = self.is_ap_mode();
        let steps = if ap {
            [
                Step::StartHotspot,
                Step::RestartWifiRadio,
                Step::DriverInBandReset,
                Step::RestartWifiService,
            ]
        } else {
            [
                Step::RestartConnection,
                Step::RestartWifiRadio,
                Step::DriverInBandReset,
                Step::RestartWifiService,
            ]
        };
        for step in steps {
            let action = step.action();
            {
                let mut state = self.state.lock();
                state.last_recovery_action = action.into();
                state.last_recovery_result = "in_progress".into();
            }
            self.record(
                "step_started",
                operation_id,
                started,
                reason,
                RecoveryEvent {
                    action: Some(action),
                    ..RecoveryEvent::default()
                },
            );
            let outcome = self.run_step(step, &current).and_then(|()| {
                if self.is_ap_mode() && step != Step::StartHotspot {
                    self.start_hotspot().map(|_| ())
                } else {
                    Ok(())
                }
            });
            if let Err(error) = outcome {
                tracing::warn!("WiFi repair step failed: {action}: {error}");
                self.record(
                    "step_exception",
                    operation_id,
                    started,
                    reason,
                    RecoveryEvent {
                        action: Some(action),
                        error_type: Some(error.kind),
                        ..RecoveryEvent::default()
                    },
                );
            }

            self.host().sleep(5.0);
            let health = self.get_health_status(None, true, true)?;
            self.record(
                "step_assessed",
                operation_id,
                started,
                reason,
                RecoveryEvent {
                    action: Some(action),
                    connected: Some(health.link_connected),
                    has_ipv4: Some(health.has_ipv4),
                    result: Some(non_empty_or(&health.last_error, "healthy")),
                    ..RecoveryEvent::default()
                },
            );
            if !health.degraded {
                tracing::warn!("WiFi repair succeeded with {action}");
                {
                    let mut state = self.state.lock();
                    state.health_failures = 0;
                    state.last_health_error.clear();
                    state.last_recovery_result = "recovered".into();
                }
                self.zeroconf_restart()?;
                self.update_gatt_advertisement();
                self.record(
                    "completed",
                    operation_id,
                    started,
                    reason,
                    RecoveryEvent {
                        action: Some(action),
                        result: Some("recovered"),
                        ..RecoveryEvent::default()
                    },
                );
                return Ok(true);
            }
            if !health_error_is_recoverable(&health.last_error) {
                {
                    let mut state = self.state.lock();
                    state.last_recovery_result = "not_recoverable".into();
                    state.last_health_error = health.last_error.clone();
                }
                tracing::warn!(
                    "Stopping WiFi repair after {action}; issue is not recoverable by driver reset: {}",
                    health.last_error
                );
                self.record(
                    "completed",
                    operation_id,
                    started,
                    reason,
                    RecoveryEvent {
                        action: Some(action),
                        result: Some("not_recoverable"),
                        ..RecoveryEvent::default()
                    },
                );
                return Ok(false);
            }
        }

        self.state.lock().last_recovery_result = "failed".into();
        let last_error = self.get_health_status(None, true, true)?.last_error;
        self.set_last_health_error(last_error);
        self.update_gatt_advertisement();
        self.record(
            "completed",
            operation_id,
            started,
            reason,
            RecoveryEvent {
                result: Some("failed"),
                ..RecoveryEvent::default()
            },
        );
        Ok(false)
    }

    fn run_step(&self, step: Step, current: &WifiSystemConfig) -> PyResult<()> {
        match step {
            Step::StartHotspot => self.start_hotspot().map(|_| ()),
            Step::RestartConnection => self.restart_active_connection(current),
            Step::RestartWifiRadio => {
                self.restart_wifi_radio();
                Ok(())
            }
            Step::DriverInBandReset => {
                self.driver_in_band_reset();
                Ok(())
            }
            Step::RestartWifiService => {
                self.restart_wifi_service();
                Ok(())
            }
        }
    }

    /// `restartActiveConnection(config)`.
    fn restart_active_connection(&self, config: &WifiSystemConfig) -> PyResult<()> {
        let Some(name) = config.connection_name.as_deref().filter(|n| !n.is_empty()) else {
            tracing::warn!("No active WiFi connection to restart");
            return Ok(());
        };
        tracing::warn!(
            "Restarting active WiFi connection: {}",
            self.redact_ssid(name)
        );
        let host = self.host();
        if let Err(error) = host.nmcli().connection_down(name) {
            tracing::warn!("Failed to bring WiFi connection down: {error}");
        }
        host.sleep(2.0);
        host.nmcli().connection_up(name, Some(15))
    }

    /// `restartWifiRadio()`.
    fn restart_wifi_radio(&self) {
        tracing::warn!("Restarting WiFi radio");
        self.run_command(&cmd(&["nmcli", "radio", "wifi", "off"]), 10.0);
        self.host().sleep(3.0);
        self.run_command(&cmd(&["nmcli", "radio", "wifi", "on"]), 10.0);
    }

    /// `driverInBandReset()`.
    fn driver_in_band_reset(&self) {
        if !self.host().path_exists(DRIVER_RESET_SCRIPT) {
            tracing::warn!("WiFi driver reset script not found: {DRIVER_RESET_SCRIPT}");
            return;
        }
        tracing::warn!("Running WiFi driver in-band reset");
        self.run_command(&cmd(&[DRIVER_RESET_SCRIPT, "in-band-reset"]), 15.0);
    }

    /// `restartWifiService()`.
    fn restart_wifi_service(&self) {
        tracing::warn!("Restarting variscite WiFi service");
        self.run_command(
            &cmd(&["systemctl", "restart", "variscite-wifi.service"]),
            30.0,
        );
    }
}

fn non_empty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() { fallback } else { value }
}

/// The `AttributeError` every repair hits in Python.
pub(crate) fn no_connected_attribute() -> PyError {
    PyError::attribute_error("'WifiHealthStatus' object has no attribute 'connected'")
}
