//! `ble_gatt.py`: the BLE GATT server phones use to put the machine on Wi-Fi
//! (Improv Wi-Fi), plus a machine identity characteristic and the
//! advertisement whose manufacturer data carries the network state.
//!
//! The Python builds it on `bless` (BlueZ over D-Bus); [`bluez`] exports
//! the same objects with `zbus`. What the server does with reads and writes
//! is here, independent of D-Bus.

pub mod bluez;
pub mod improv;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tokio::sync::Notify;

use crate::config::{ConfigStore, WIFI_MODE_AP, keys};
use crate::hostname::HostnameManager;
use crate::py::{PyError, PyResult, bytes_repr, repr_str};
use crate::wifi::{ScanEntry, WifiManager, WifiSystemConfig, psk_credentials};
use improv::{Callbacks, ImprovProtocol, Payload, State};

/// Seconds after boot before the server starts: the variscite Wi-Fi service
/// needs that long to bring Bluetooth back.
pub const MIN_BOOT_TIME: f64 = 60.0;
pub const MACHINE_IDENT_UUID: &str = "7f01d7b8-121e-11ef-a097-b3b1396fea81";
pub const ADVERTISEMENT_UPDATE_RETRY_DELAYS: [u64; 3] = [1, 3, 8];

/// What the server needs from the Wi-Fi manager.
pub trait ProvisioningWifi: Send + Sync {
    /// `WifiManager.connectToWifi(credentials)`.
    fn connect(&self, credentials: &Value) -> PyResult<bool>;
    /// `WifiManager.getCurrentConfig()`.
    fn current_config(&self) -> PyResult<WifiSystemConfig>;
    /// `WifiManager.scanForNetworks()`.
    fn scan(&self) -> Vec<ScanEntry>;
}

impl ProvisioningWifi for WifiManager {
    fn connect(&self, credentials: &Value) -> PyResult<bool> {
        self.connect_to_wifi(credentials, "manual")
    }

    fn current_config(&self) -> PyResult<WifiSystemConfig> {
        self.get_current_config()
    }

    fn scan(&self) -> Vec<ScanEntry> {
        self.scan_for_networks(10.0, None)
    }
}

/// `GATTServer`.
pub struct GattServer {
    me: Weak<GattServer>,
    wifi: RwLock<Arc<dyn ProvisioningWifi>>,
    hostname: Arc<HostnameManager>,
    config: Arc<ConfigStore>,
    port: u16,
    improv: Mutex<ImprovProtocol>,
    gatt_name: Mutex<String>,
    manufacturer_data: Mutex<Vec<u8>>,
    running: AtomicBool,
    /// `bless_gatt_server is not None`: set once the boot wait is over.
    created: AtomicBool,
    update_trigger: Notify,
    stop_trigger: Notify,
    app: OnceLock<bluez::BluezApp>,
    /// `app.subscribed_characteristics`.
    subscribed: Mutex<Vec<String>>,
}

impl GattServer {
    pub fn new(
        wifi: Arc<dyn ProvisioningWifi>,
        hostname: Arc<HostnameManager>,
        config: Arc<ConfigStore>,
        port: u16,
    ) -> Arc<GattServer> {
        Arc::new_cyclic(|me| GattServer {
            me: me.clone(),
            wifi: RwLock::new(wifi),
            hostname,
            config,
            port,
            improv: Mutex::new(ImprovProtocol::new(false, 250)),
            gatt_name: Mutex::new("MeticulousEspresso".into()),
            manufacturer_data: Mutex::new(Vec::new()),
            running: AtomicBool::new(false),
            created: AtomicBool::new(false),
            update_trigger: Notify::new(),
            stop_trigger: Notify::new(),
            app: OnceLock::new(),
            subscribed: Mutex::new(Vec::new()),
        })
    }

    fn wifi(&self) -> Arc<dyn ProvisioningWifi> {
        self.wifi.read().clone()
    }

    pub fn name(&self) -> String {
        self.gatt_name.lock().clone()
    }

    /// `GATTServer.__init__` (the Python builds it on first use, from
    /// `main`) and `start()`.
    pub fn start(self: &Arc<Self>, wifi: &Arc<WifiManager>) {
        let name = self.hostname.generate_device_name();
        *self.gatt_name.lock() = name.clone();
        tracing::info!("BLE init called. Name={name}");
        let data = self.build_manufacturer_data().unwrap_or_else(|error| {
            tracing::warn!("Could not read the network state for the advertisement: {error}");
            Vec::new()
        });
        *self.manufacturer_data.lock() = data;

        tracing::info!("Starting BLE GATT Server");
        if self.running.swap(true, Ordering::Relaxed) {
            tracing::info!("BLE GATT Server is already running");
            return;
        }
        let runtime = tokio::runtime::Handle::current();
        let weak = Arc::downgrade(self);
        let hook_runtime = runtime.clone();
        wifi.set_gatt_hook(Arc::new(move || {
            if let Some(server) = weak.upgrade() {
                hook_runtime.spawn(async move { server.update_advertisement().await });
            }
        }));
        let server = self.clone();
        runtime.spawn(async move { server.update_data_loop().await });
        let server = self.clone();
        runtime.spawn(async move {
            if let Err(error) = server.clone().gatt_server_loop().await {
                // The loop's exception handler.
                tracing::warn!("Got exception {error}");
                tracing::error!("GATT Server crashed! {error}");
                server.stop();
            }
        });
    }

    /// The radio is off while the machine is in standby. Nothing to do
    /// when the GATT server never reached BlueZ.
    pub async fn set_radio_powered(&self, powered: bool) {
        let Some(app) = self.app.get() else {
            return;
        };
        if let Err(error) = app.set_powered(powered).await {
            tracing::warn!(
                "Could not power the Bluetooth radio {}: {error}",
                if powered { "on" } else { "off" }
            );
        }
    }

    /// `stop()`.
    pub fn stop(&self) {
        tracing::info!("Stopping BLE GATT Server");
        self.stop_trigger.notify_one();
    }

    /// `update_advertisement()`: rebuild the manufacturer data unless a
    /// client is connected.
    pub async fn update_advertisement(self: Arc<Self>) {
        let has_clients = self.check_client_connections();
        tracing::info!(
            "Checking client connections: {}",
            if has_clients { "True" } else { "False" }
        );
        if has_clients {
            tracing::info!("Skipping BLE advertisement update - clients connected");
            return;
        }
        tracing::info!("Updating BLE advertisement");
        let me = self.clone();
        let data = tokio::task::spawn_blocking(move || me.build_manufacturer_data()).await;
        match data {
            Ok(Ok(data)) => {
                *self.manufacturer_data.lock() = data;
                self.update_trigger.notify_one();
            }
            Ok(Err(error)) => {
                tracing::error!("Error updating advertisement: {error}");
                tracing::error!("Advertisement update failed");
            }
            Err(error) => tracing::error!("Error updating advertisement: {error}"),
        }
    }

    /// `_build_manufacturer_data()`: the mode (2 hotspot, 1 connected, 0
    /// neither) then each IPv4 address, at most 27 bytes.
    pub fn build_manufacturer_data(&self) -> PyResult<Vec<u8>> {
        let config = self.wifi().current_config()?;
        let mut data = Vec::new();
        if self.config.get(keys::WIFI, keys::WIFI_MODE).as_str() == Some(WIFI_MODE_AP) {
            data.push(0x02);
        } else if config.connected {
            data.push(0x01);
        } else {
            data.push(0x00);
        }
        for ip in &config.ips {
            if let std::net::IpAddr::V4(v4) = ip.addr {
                data.extend_from_slice(&v4.octets());
            }
        }
        data.truncate(27);
        Ok(data)
    }

    /// `check_client_connections()`: a client counts once it subscribed to a
    /// characteristic.
    fn check_client_connections(&self) -> bool {
        if !self.created.load(Ordering::Relaxed) {
            tracing::info!("GATT Server not initialized yet");
            return false;
        }
        let connected = !self.subscribed.lock().is_empty();
        if connected {
            tracing::info!("BLE Client Connected! Active connection detected");
            // PARITY: bless has no `_subscribed_clients`, so the count is
            // never logged.
            tracing::info!("No BLE clients currently connected");
        }
        connected
    }

    /// `_update_data_loop()`.
    async fn update_data_loop(self: Arc<Self>) {
        loop {
            self.update_trigger.notified().await;
            let Some(app) = self.app.get() else {
                tracing::info!("BLE GATT Server not initialized yet");
                continue;
            };
            let data = self.manufacturer_data.lock().clone();
            tracing::info!("Updating BLE announcement: {}", bytes_repr(&data));
            let name = self.name();
            update_with_retries(app, &name, &data, &ADVERTISEMENT_UPDATE_RETRY_DELAYS).await;
        }
    }

    /// `_ble_gatt_server_loop()`.
    async fn gatt_server_loop(self: Arc<Self>) -> Result<(), String> {
        // FIXME (Python): drop once SDIO power sequencing replaces the
        // variscite-wifi service.
        let uptime_missing = crate::py::round_half_even(MIN_BOOT_TIME - seconds_since_boot());
        if uptime_missing > 0 {
            tracing::info!(
                "GattServer started to fast after system boot. Waiting {uptime_missing} seconds"
            );
            tokio::time::sleep(Duration::from_secs(uptime_missing as u64)).await;
        }
        self.created.store(true, Ordering::Relaxed);

        let name = self.name();
        let app = match bluez::BluezApp::setup(&name, self.me.clone()).await {
            Ok(app) => app,
            Err(bluez::SetupError::NoBus) => {
                tracing::warn!("Could not initialize the BLE gatt interface. Bailing out!");
                return Ok(());
            }
            Err(bluez::SetupError::Failed(error)) => return Err(error),
        };
        let app = self.app.get_or_init(|| app);
        app.start().await?;

        tracing::info!("GATT Server started");
        self.update_trigger.notify_one();
        self.stop_trigger.notified().await;
        tracing::debug!("GATT loop exiting");
        app.stop().await?;
        tracing::info!("GATT Server stopped");
        Ok(())
    }

    /// `updateAuthentication()`. PARITY: the Dial never answers the
    /// authorization notification, so every Improv read authorizes, even
    /// after provisioning (STATUS then reads AUTHORIZED).
    fn update_authentication(&self, improv: &mut ImprovProtocol) {
        improv.state = State::AwaitingAuthorization;
        improv.state = State::Authorized;
    }

    /// `machine_ident_read_request()`: name, hostname, colour, versions,
    /// connection, addresses, comma separated.
    pub fn machine_ident(&self) -> PyResult<String> {
        let config = self.wifi().current_config()?;
        let mut response = self.hostname.generate_device_name() + ",";
        response += &(config.hostname.clone() + ",");
        response += "black,";
        response += "v10.1.0,";
        response += "103,";
        response += if config.connected { "1," } else { "0," };
        // PARITY: `str + None` raises while no connection is active.
        let Some(connection_name) = &config.connection_name else {
            return Err(PyError::type_error(
                "can only concatenate str (not \"NoneType\") to str",
            ));
        };
        response += &(connection_name.clone() + ",");
        response += &config
            .ips
            .iter()
            .map(|ip| ip.addr.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Ok(response)
    }

    fn char_name(uuid: &str) -> String {
        if let Some(name) = improv::uuid_name(uuid) {
            name.to_string()
        } else if uuid == MACHINE_IDENT_UUID {
            "MACHINE_IDENT".to_string()
        } else {
            uuid.to_string()
        }
    }

    /// `read_request(characteristic)`: every characteristic is in the Improv
    /// service.
    pub fn read_request(&self, uuid: &str) -> PyResult<Vec<u8>> {
        let char_name = Self::char_name(uuid);
        let value = if uuid == MACHINE_IDENT_UUID {
            self.machine_ident()?.into_bytes()
        } else {
            let mut improv = self.improv.lock();
            self.update_authentication(&mut improv);
            improv.handle_read(uuid).first()?
        };
        tracing::info!(
            "BLE READ  {char_name} -> {} bytes: {}",
            value.len(),
            hex::encode(&value)
        );
        Ok(value)
    }

    /// `write_request(characteristic, value)`: the notifications to send,
    /// in order, as (characteristic UUID, value).
    pub fn write_request(&self, uuid: &str, value: &[u8]) -> PyResult<Vec<(String, Vec<u8>)>> {
        let char_name = improv::uuid_name(uuid).unwrap_or(uuid);
        tracing::info!("BLE WRITE {char_name} <- {} bytes", value.len());
        let mut callbacks = WifiCallbacks { server: self };
        let answer = self
            .improv
            .lock()
            .handle_write(uuid, value, &mut callbacks)?;
        let Some((target_uuid, target_values)) = answer else {
            return Ok(Vec::new());
        };
        let target_name = improv::uuid_name(target_uuid).unwrap_or(target_uuid);
        let packets = match target_values {
            Payload::Bytes(bytes) => vec![bytes],
            Payload::Packets(packets) => packets,
        };
        Ok(packets
            .into_iter()
            .map(|packet| {
                tracing::info!("BLE RESP  {target_name} -> {} bytes", packet.len());
                (target_uuid.to_string(), packet)
            })
            .collect())
    }

    /// `wifi_connect(ssid, passwd)`: join with the provisioned credentials
    /// and answer with the backend's URLs.
    pub fn wifi_connect(&self, ssid: &[u8], password: &[u8]) -> Option<Vec<String>> {
        let Ok(ssid) = std::str::from_utf8(ssid) else {
            tracing::error!("Failed to decode SSID (UnicodeDecodeError)");
            return None;
        };
        let Ok(password) = std::str::from_utf8(password) else {
            tracing::error!("Failed to decode password (UnicodeDecodeError)");
            return None;
        };
        let metadata = format!(
            "ssid_bytes={}, password_bytes={}",
            ssid.len(),
            password.len()
        );
        tracing::info!("Connecting to Wi-Fi ({metadata})");
        let wifi = self.wifi();
        let result = wifi
            .connect(&psk_credentials(ssid, password))
            .and_then(|joined| {
                if !joined {
                    return Ok(None);
                }
                let config = wifi.current_config()?;
                Ok(Some(
                    config
                        .ips
                        .iter()
                        .map(|ip| match ip.addr {
                            std::net::IpAddr::V6(v6) => format!("http://[{v6}]:{}", self.port),
                            std::net::IpAddr::V4(v4) => format!("http://{v4}:{}", self.port),
                        })
                        .collect::<Vec<_>>(),
                ))
            });
        match result {
            Ok(Some(urls)) => {
                tracing::debug!(
                    "Backend redirect IP/URL: [{}]",
                    urls.iter()
                        .map(|u| repr_str(u))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                Some(urls)
            }
            Ok(None) => None,
            Err(error) => {
                tracing::error!("Failed to connect to Wi-Fi ({}; {metadata})", error.kind);
                None
            }
        }
    }

    /// `get_wifi_networks()`.
    pub fn get_wifi_networks(&self) -> Vec<String> {
        self.wifi().scan().into_iter().map(|n| n.ssid).collect()
    }

    /// `app.StartNotify`: bless calls it with no characteristic.
    pub(crate) fn start_notify(&self, uuid: &str) {
        tracing::info!("BLE client subscribed to notifications (char=None)");
        self.subscribed.lock().push(uuid.to_string());
    }

    /// `app.StopNotify`; an unsubscribed characteristic is an error, as
    /// `list.remove` makes it.
    pub(crate) fn stop_notify(&self, uuid: &str) -> Result<(), String> {
        tracing::info!("BLE client unsubscribed from notifications (char=None)");
        let mut subscribed = self.subscribed.lock();
        match subscribed.iter().position(|u| u == uuid) {
            Some(index) => {
                subscribed.remove(index);
                Ok(())
            }
            None => Err("list.remove(x): x not in list".into()),
        }
    }
}

struct WifiCallbacks<'a> {
    server: &'a GattServer,
}

impl Callbacks for WifiCallbacks<'_> {
    fn wifi_connect(&mut self, ssid: &[u8], password: &[u8]) -> Option<Vec<String>> {
        self.server.wifi_connect(ssid, password)
    }

    fn wifi_networks(&mut self) -> Option<Vec<String>> {
        Some(self.server.get_wifi_networks())
    }
}

/// `time.time() - psutil.boot_time()`.
fn seconds_since_boot() -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let boot = std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|stat| {
            stat.lines()
                .find_map(|l| l.strip_prefix("btime ").map(str::trim).map(str::to_string))
        })
        .and_then(|b| b.parse::<f64>().ok());
    match boot {
        Some(boot) => now - boot,
        None => MIN_BOOT_TIME,
    }
}

/// An advertisement as `BlueZLEAdvertisement` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advertisement {
    pub path: String,
    pub local_name: String,
    pub service_uuids: Vec<String>,
    /// Company id 0xFFFF.
    pub manufacturer_data: Option<Vec<u8>>,
}

/// The `LEAdvertisingManager1` side of `_replace_ble_advertisement`.
pub(crate) trait AdvertisingBackend: Sync {
    fn base_path(&self) -> String;
    fn take_last(&self) -> impl std::future::Future<Output = Option<String>> + Send;
    fn push(&self, path: String) -> impl std::future::Future<Output = ()> + Send;
    fn unregister(
        &self,
        path: &str,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;
    fn register(&self, path: &str) -> impl std::future::Future<Output = Result<(), String>> + Send;
    fn export(
        &self,
        ad: Advertisement,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;
    fn unexport(&self, path: &str) -> impl std::future::Future<Output = Result<(), String>> + Send;
}

/// `_advertisement_is_missing(error)`.
fn advertisement_is_missing(error: &str) -> bool {
    let message = error.to_lowercase();
    message.contains("does not exist") || message.contains("unknown object")
}

/// `_replace_ble_advertisement()`: swap the registered advertisement for one
/// with the current manufacturer data (and, PARITY, no service UUID).
pub(crate) async fn replace_advertisement<B: AdvertisingBackend>(
    backend: &B,
    name: &str,
    data: &[u8],
) -> Result<(), String> {
    if let Some(previous) = backend.take_last().await {
        if let Err(error) = backend.unregister(&previous).await {
            if advertisement_is_missing(&error) {
                tracing::warn!("Previous BLE advertisement no longer exists; rebuilding it");
            } else {
                backend.push(previous).await;
                return Err(error);
            }
        }
        if let Err(error) = backend.unexport(&previous).await {
            tracing::warn!("Could not unexport previous BLE advertisement: {error}");
        }
    }
    let advertisement = Advertisement {
        path: format!("{}/advertisement0", backend.base_path()),
        local_name: name.to_string(),
        service_uuids: Vec::new(),
        manufacturer_data: Some(data.to_vec()),
    };
    let path = advertisement.path.clone();
    backend.export(advertisement).await?;
    if let Err(error) = backend.register(&path).await {
        if let Err(cleanup) = backend.unexport(&path).await {
            tracing::warn!("Could not clean up failed BLE advertisement: {cleanup}");
        }
        return Err(error);
    }
    backend.push(path).await;
    Ok(())
}

/// The retry loop in `_update_data_loop`: a failed update never stops the
/// server.
pub(crate) async fn update_with_retries<B: AdvertisingBackend>(
    backend: &B,
    name: &str,
    data: &[u8],
    delays: &[u64],
) {
    let total = delays.len() + 1;
    for attempt in 0..total {
        match replace_advertisement(backend, name, data).await {
            Ok(()) => break,
            Err(error) => {
                let Some(&delay) = delays.get(attempt) else {
                    tracing::error!(
                        "BLE advertisement update failed after retries; GATT server remains running: {error}"
                    );
                    break;
                };
                tracing::warn!(
                    "BLE advertisement update failed; retrying in {delay}s ({}/{total}): {error}",
                    attempt + 1
                );
                tokio::time::sleep(Duration::from_secs(delay)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests;
