//! Scanning and the scan cache the network pickers read.
//!
//! `/wifi/list` answers from the cache so the Dial's five-second poll never
//! waits on a scan; only a cold cache blocks, on one scan shared by every
//! caller.

use std::sync::atomic::Ordering;

use serde_json::Value;

use super::WifiManager;
use super::types::ScanEntry;

impl WifiManager {
    /// `scanForNetworks(timeout, target_network_ssid)`.
    ///
    /// With a target, scans until it shows up. PARITY: when it never does,
    /// the last unfiltered scan is returned.
    pub fn scan_for_networks(&self, timeout: f64, target: Option<&Value>) -> Vec<ScanEntry> {
        if !self.networking_available() {
            return Vec::new();
        }
        let target = target.filter(|t| t.as_str() != Some(""));
        if !self.is_wifi_device_ready() {
            let state = self.wifi_device_state();
            self.set_last_health_error("wifi_device_unavailable");
            tracing::warn!(
                "WiFi device is not ready for scanning: state={}",
                state.as_deref().unwrap_or("None")
            );
            return Vec::new();
        }

        let host = self.host();
        let deadline = host.time() + timeout;
        let mut retries = 0u32;
        let mut last_forced_rescan = 0.0;
        let mut wifis: Vec<ScanEntry> = Vec::new();
        while host.time() < deadline {
            if retries < 3 {
                tracing::info!(
                    "Requesting scan results: Time left: {}s",
                    pyfmt::repr_f64(deadline - host.time())
                );
            } else if retries == 3 {
                tracing::info!("Scans returning very fast, stopping logging");
            }

            let force_rescan = retries == 0 || host.time() - last_forced_rescan > 5.0;
            if force_rescan {
                last_forced_rescan = host.time();
            }
            wifis = match host.nmcli().device_wifi(Some(force_rescan)) {
                Ok(wifis) => wifis,
                Err(error) => {
                    tracing::info!(
                        "Failed to scan for wifis: {error}, retrying if timeout is not reached"
                    );
                    Vec::new()
                }
            };

            if target.is_none() && !wifis.is_empty() {
                self.store_scan(&wifis);
            }
            let filtered: Vec<ScanEntry> = match target {
                Some(target) => wifis
                    .iter()
                    .filter(|w| target.as_str() == Some(w.ssid.as_str()))
                    .cloned()
                    .collect(),
                None => wifis.clone(),
            };
            if !filtered.is_empty() {
                wifis = filtered;
                break;
            }
            retries += 1;
            host.sleep((deadline - host.time()).clamp(0.0, 0.5));
        }

        tracing::info!("Scanning finished after {retries}");
        if target.is_none() {
            self.store_scan(&wifis);
        }
        wifis
    }

    fn store_scan(&self, wifis: &[ScanEntry]) {
        let now = self.host().time();
        let mut state = self.state.lock();
        state.known_wifis = wifis.to_vec();
        state.scan_cache = wifis.to_vec();
        state.scan_cache_time = now;
    }

    /// `_scan_cache or _known_wifis`.
    fn cached_scan(&self) -> Vec<ScanEntry> {
        let state = self.state.lock();
        if state.scan_cache.is_empty() {
            state.known_wifis.clone()
        } else {
            state.scan_cache.clone()
        }
    }

    /// `_scan_cache`.
    #[cfg(test)]
    pub fn scan_cache(&self) -> Vec<ScanEntry> {
        self.state.lock().scan_cache.clone()
    }

    #[cfg(test)]
    pub(crate) fn set_scan_cache(&self, entries: Vec<ScanEntry>) {
        self.state.lock().scan_cache = entries;
    }

    /// `refreshScanCacheInBackground()`.
    fn refresh_scan_cache_in_background(&self) {
        if self.scan_in_progress.load(Ordering::Relaxed) {
            return;
        }
        let Some(me) = self.arc() else { return };
        self.host().spawn(
            "WifiScanRefresh",
            Box::new(move || {
                {
                    let _guard = me.scan_lock.lock();
                    if me.scan_in_progress.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    me.scan_in_progress.store(true, Ordering::Relaxed);
                }
                me.scan_for_networks(10.0, None);
                me.scan_in_progress.store(false, Ordering::Relaxed);
                Ok(())
            }),
        );
    }

    /// `ensureScanCache(timeout)`: the cache, or one bounded scan that every
    /// concurrent cold-start caller waits on.
    fn ensure_scan_cache(&self, timeout: f64) -> Vec<ScanEntry> {
        let cached = self.cached_scan();
        if !cached.is_empty() {
            return cached;
        }
        let _guard = self.scan_lock.lock();
        let cached = self.cached_scan();
        if !cached.is_empty() {
            return cached;
        }
        self.scan_in_progress.store(true, Ordering::Relaxed);
        let scanned = self.scan_for_networks(timeout, None);
        self.scan_in_progress.store(false, Ordering::Relaxed);
        scanned
    }

    /// `getAvailableNetworks(refresh, block_if_empty)`.
    pub fn get_available_networks(&self, refresh: bool, block_if_empty: bool) -> Vec<ScanEntry> {
        let cached = self.cached_scan();
        if cached.is_empty() && refresh && block_if_empty {
            return self.ensure_scan_cache(8.0);
        }
        if refresh {
            self.refresh_scan_cache_in_background();
        }
        cached
    }
}
