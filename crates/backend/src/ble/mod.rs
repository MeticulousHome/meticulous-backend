//! `ble_gatt.py`: the BLE GATT server phones use to put the machine on Wi-Fi
//! (Improv Wi-Fi), plus a machine identity characteristic and the
//! advertisement whose manufacturer data carries the network state.
//!
//! The Python builds it on `bless` (BlueZ over D-Bus); [`bluez`] exports
//! the same objects with `zbus`. What the server does with reads and writes
//! is here, independent of D-Bus.

pub mod bluez;
pub mod improv;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tokio::sync::Notify;

use crate::config::{ConfigStore, WIFI_MODE_AP, keys};
use crate::device_auth::{CodeSource, DeviceAuth};
use crate::hostname::HostnameManager;
use crate::notifications::{Notification, NotificationManager, responses};
use crate::py::{PyError, PyResult, bytes_repr, repr_str};
use crate::wifi::{ScanEntry, WifiManager, WifiSystemConfig, psk_credentials};
use improv::{Callbacks, ImprovProtocol, Payload, RPC_RESULT_UUID, State};

/// Seconds after boot before the server starts: the variscite Wi-Fi service
/// needs that long to bring Bluetooth back.
pub const MIN_BOOT_TIME: f64 = 60.0;
pub const MACHINE_IDENT_UUID: &str = "7f01d7b8-121e-11ef-a097-b3b1396fea81";
pub const ADVERTISEMENT_UPDATE_RETRY_DELAYS: [u64; 3] = [1, 3, 8];
/// One notification per machine: a client identifying again replaces it.
pub const IDENTIFY_NOTIFICATION_ID: &str = "ble-improv-identify";
/// Longest client name shown; BlueZ passes on whatever the phone sends.
pub(crate) const MAX_CLIENT_NAME_CHARS: usize = 48;
/// An SSID is at most 32 bytes, so at most 32 characters.
const MAX_SSID_CHARS: usize = 32;
/// How long the Dial asks before a Wi-Fi request counts as rejected.
pub const WIFI_APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the join may spend making the writer the sole connected
/// central (listing the centrals, disconnecting the others, waiting for
/// BlueZ to list them gone) before it sends the result without a pairing
/// code; and how long the check after sending may take.
const SOLE_CENTRAL_TIMEOUT: Duration = Duration::from_secs(4);
/// How long to wait before asking BlueZ again while a central it was told
/// to disconnect is still listed.
const SOLE_CENTRAL_SETTLE: Duration = Duration::from_millis(200);
/// What a disconnect answers for a central already gone (it left on its
/// own after it was listed, or BlueZ dropped its object): not a failure,
/// the next listing decides.
const CENTRAL_ALREADY_GONE: [&str; 2] = [
    "org.bluez.Error.NotConnected",
    "org.freedesktop.DBus.Error.UnknownObject",
];

/// `text` without control characters and without `<`, `>` and `&` (which
/// the Dial could read as markup), trimmed and at most `max` characters, an
/// ellipsis ending a shortened one; `None` when nothing printable is left.
pub(crate) fn display_text(text: &str, max: usize) -> Option<String> {
    let text: String = text
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | '&'))
        .collect();
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= max {
        return Some(text.to_string());
    }
    let shown: String = text.chars().take(max.saturating_sub(1)).collect();
    Some(shown.trim_end().to_string() + "\u{2026}")
}

/// The BLE client behind a write, as BlueZ's `Device1` describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BleClient {
    /// `Device1.Name`: the name the phone gives itself, when it gave one.
    pub name: Option<String>,
    /// Its `Device1` object path, to tell it from other connected centrals.
    /// The path carries the Bluetooth address: never log it.
    pub device: Option<String>,
}

impl BleClient {
    /// The name, without control characters, trimmed and capped; `None`
    /// when nothing printable is left.
    fn display_name(&self) -> Option<String> {
        display_text(self.name.as_deref()?, MAX_CLIENT_NAME_CHARS)
    }
}

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
    notifications: Arc<NotificationManager>,
    /// Mints the pairing code the Wi-Fi result carries.
    auth: Arc<DeviceAuth>,
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
    /// Where notifications that come after a write (the Dial's answer, the
    /// join) are sent from; set by `start`.
    runtime: OnceLock<tokio::runtime::Handle>,
    approval_timeout_ms: AtomicU64,
    /// [`SOLE_CENTRAL_TIMEOUT`].
    sole_central_timeout_ms: AtomicU64,
    /// Every notification sent after a write, in order.
    #[cfg(test)]
    pushed: Mutex<Vec<(String, Vec<u8>)>>,
    /// The tasks that send them, to wait for.
    #[cfg(test)]
    sends: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// BlueZ's connected centrals, as a test sets them up; `None` asks
    /// BlueZ, which tests have not set up.
    #[cfg(test)]
    centrals: Mutex<Option<FakeCentrals>>,
}

/// BlueZ as a test sets it up: what each listing of the connected
/// centrals answers, which `Disconnect` calls fail, which call answers
/// late; and what the server asked of it.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub(crate) struct FakeCentrals {
    /// What each listing answers, in order; the last one repeats.
    pub listings: Vec<Result<Vec<String>, String>>,
    /// The D-Bus error `Disconnect` answers for a central, by path.
    pub refusals: Vec<(String, String)>,
    /// The BlueZ call (listings and disconnects, counted from 0 in the
    /// order made) that answers only after this long.
    pub held: Option<(usize, Duration)>,
    /// What the server did, in order: `list`, `disconnect <path>`,
    /// `notify <characteristic>`; and `answered <n>` once the held call
    /// `n` answers.
    pub calls: Vec<String>,
    bluez_calls: usize,
    listed: usize,
}

#[cfg(test)]
impl FakeCentrals {
    /// BlueZ answers each listing in turn, then the last one again, and
    /// disconnects whoever it is asked to.
    pub fn new(listings: Vec<Result<Vec<String>, String>>) -> FakeCentrals {
        assert!(!listings.is_empty(), "at least one listing");
        FakeCentrals {
            listings,
            ..FakeCentrals::default()
        }
    }

    /// Records `call`; its number and how long it is held.
    fn begin(&mut self, call: String) -> (usize, Option<Duration>) {
        let number = self.bluez_calls;
        self.bluez_calls += 1;
        self.calls.push(call);
        let held = self
            .held
            .filter(|(held, _)| *held == number)
            .map(|(_, delay)| delay);
        (number, held)
    }

    fn list(&mut self) -> Result<Vec<String>, String> {
        let index = self.listed.min(self.listings.len() - 1);
        self.listed += 1;
        self.listings[index].clone()
    }

    fn disconnect(&mut self, device: &str) -> Result<(), String> {
        match self.refusals.iter().find(|(path, _)| path == device) {
            Some((_, error)) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

/// A pairing code the Wi-Fi result carries, and the central it is for.
struct SentCode {
    code: String,
    writer: String,
}

/// Why the Wi-Fi result goes out without a pairing code.
enum NotSole {
    /// The writer is no longer connected.
    WriterGone,
    /// Centrals still listed after they were disconnected.
    StillConnected(usize),
    /// BlueZ failed or did not answer in time.
    Bluez(String),
}

impl GattServer {
    pub fn new(
        wifi: Arc<dyn ProvisioningWifi>,
        hostname: Arc<HostnameManager>,
        config: Arc<ConfigStore>,
        notifications: Arc<NotificationManager>,
        auth: Arc<DeviceAuth>,
        port: u16,
    ) -> Arc<GattServer> {
        Arc::new_cyclic(|me| GattServer {
            me: me.clone(),
            wifi: RwLock::new(wifi),
            hostname,
            config,
            notifications,
            auth,
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
            runtime: OnceLock::new(),
            approval_timeout_ms: AtomicU64::new(WIFI_APPROVAL_TIMEOUT.as_millis() as u64),
            sole_central_timeout_ms: AtomicU64::new(SOLE_CENTRAL_TIMEOUT.as_millis() as u64),
            #[cfg(test)]
            pushed: Mutex::new(Vec::new()),
            #[cfg(test)]
            sends: Mutex::new(Vec::new()),
            #[cfg(test)]
            centrals: Mutex::new(None),
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
        let _ = self.runtime.set(runtime.clone());
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

    /// `updateAuthentication()`. PARITY: every Improv read authorizes, even
    /// after provisioning (STATUS then reads AUTHORIZED); but not while a
    /// Wi-Fi request waits for the Dial or joins, which the Python never did.
    fn update_authentication(&self, improv: &mut ImprovProtocol) {
        if improv.request_in_flight() {
            return;
        }
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
    /// in order, as (characteristic UUID, value). `client` is who wrote it,
    /// when BlueZ said.
    pub fn write_request(
        &self,
        uuid: &str,
        value: &[u8],
        client: Option<&BleClient>,
    ) -> PyResult<Vec<(String, Vec<u8>)>> {
        let char_name = improv::uuid_name(uuid).unwrap_or(uuid);
        tracing::info!("BLE WRITE {char_name} <- {} bytes", value.len());
        let mut callbacks = WifiCallbacks {
            server: self,
            client,
        };
        let answer = self
            .improv
            .lock()
            .handle_write(uuid, value, &mut callbacks)?;
        Ok(answer
            .map(|answer| packets(vec![answer]))
            .unwrap_or_default())
    }

    /// A `WIFI_SETTINGS` with credentials: ask on the Dial, and carry the
    /// request on from a thread of its own once someone answers or
    /// [`WIFI_APPROVAL_TIMEOUT`] passes. The Improv lock is held by the
    /// caller, so this only starts the wait.
    fn request_wifi_authorization(
        &self,
        client: Option<BleClient>,
        ssid: Vec<u8>,
        password: Vec<u8>,
    ) {
        let client_name = client.as_ref().and_then(BleClient::display_name);
        tracing::info!(
            "BLE client asked to join a Wi-Fi network; asking on the Dial (name={})",
            client_name.as_deref().unwrap_or("unknown")
        );
        let device = client_name
            .clone()
            .unwrap_or_else(|| "A device".to_string());
        let network = display_text(&String::from_utf8_lossy(&ssid), MAX_SSID_CHARS)
            .unwrap_or_else(|| "a hidden network".to_string());
        let (answered, answer) = std::sync::mpsc::channel::<()>();
        let notification = Notification::with_responses(
            format!("{device} is requesting connection to {network}"),
            &[responses::CONNECT, responses::REJECT],
        )
        // PARITY: acknowledging runs the callback twice; a second send is
        // never read.
        .callback(Arc::new(move || {
            let _ = answered.send(());
        }))
        .private()
        // Connect also lets the phone use the machine (the result carries a
        // pairing code), so only someone at the machine may press it.
        .answer_locally();
        let request = WifiRequest {
            notification: notification.id.clone(),
            central: client.and_then(|client| client.device),
            client_name,
            device,
            network,
            ssid,
            password,
        };
        self.notifications.add(notification);
        let Some(server) = self.me.upgrade() else {
            return;
        };
        std::thread::spawn(move || server.await_wifi_answer(request, answer));
    }

    /// The rest of a Wi-Fi request: the Dial's answer, then the join.
    fn await_wifi_answer(&self, request: WifiRequest, answer: std::sync::mpsc::Receiver<()>) {
        let timeout = Duration::from_millis(self.approval_timeout_ms.load(Ordering::Relaxed));
        let approved = match answer.recv_timeout(timeout) {
            Ok(()) => {
                let response = self
                    .notifications
                    .all()
                    .into_iter()
                    .find(|n| n.id == request.notification)
                    .and_then(|n| n.response);
                let approved = response.as_deref() == Some(responses::CONNECT);
                tracing::info!(
                    "Wi-Fi request {} on the Dial",
                    if approved { "approved" } else { "rejected" }
                );
                approved
            }
            Err(_) => {
                tracing::info!(
                    "Wi-Fi request not answered within {} s; rejecting it",
                    timeout.as_secs()
                );
                // The Dial drops a notification only when it is answered:
                // replace the question, under the same id, with the outcome.
                let mut expired = Notification::new(format!(
                    "{} was not allowed to connect to {}: nobody answered in time",
                    request.device, request.network
                ))
                .private();
                expired.id = request.notification.clone();
                self.notifications.add(expired);
                false
            }
        };
        let answered = self.improv.lock().answered(approved);
        self.send_to_client(packets(answered), None);
        if !approved {
            return;
        }
        let urls = self.wifi_connect(&request.ssid, &request.password);
        // The phone that set the machine up may use it: the result carries
        // a pairing code under the first URL. BlueZ notifies every connected
        // central that subscribed, not only the writer, so the other
        // centrals are disconnected first and the code goes out only once
        // the writer is the sole one; otherwise the app falls back to
        // asking for access. A central that connects again before BlueZ
        // sends the result gets it too: once the result is sent, the code
        // is revoked if anyone but the writer is connected.
        let mut sent_code = None;
        let pair_url = urls
            .as_ref()
            .and_then(|urls| urls.first())
            .and_then(|base| {
                let writer = self.make_writer_sole_central(request.central.as_deref())?;
                let code = self
                    .auth
                    .mint_code(CodeSource::Improv, request.client_name.clone());
                let pair_url = format!("{base}/#pair={code}");
                sent_code = Some(SentCode { code, writer });
                Some(pair_url)
            });
        match self.improv.lock().joined(urls, pair_url) {
            Ok(outcome) => self.send_to_client(packets(outcome), sent_code),
            Err(error) => {
                tracing::error!("Could not build the Wi-Fi RPC result: {error}");
                if let Some(sent) = sent_code {
                    self.revoke_code(&sent.code, "the Wi-Fi result was not built");
                }
            }
        }
    }

    /// `writer` (a `Device1` path) once it is, or has been made, the only
    /// connected central: every other connected central is disconnected
    /// (never the writer), then BlueZ is asked again until it lists the
    /// writer alone, disconnecting again a central listed again. An unknown
    /// or disconnected writer, a disconnect BlueZ refuses, a central still
    /// connected, or BlueZ not answering, all within
    /// [`SOLE_CENTRAL_TIMEOUT`], gives `None`. Only counts and reasons are
    /// logged: a path carries the address.
    fn make_writer_sole_central(&self, writer: Option<&str>) -> Option<String> {
        let Some(writer) = writer else {
            tracing::info!("BLE writer unknown; the Wi-Fi result carries no pairing code");
            return None;
        };
        let deadline = Instant::now() + self.sole_central_timeout();
        match self.disconnect_other_centrals(writer, deadline) {
            Ok(()) => return Some(writer.to_string()),
            Err(NotSole::WriterGone) => tracing::info!(
                "BLE writer no longer connected; the Wi-Fi result carries no pairing code"
            ),
            Err(NotSole::StillConnected(others)) => tracing::info!(
                "{others} other BLE central(s) still connected after disconnecting them; \
                 the Wi-Fi result carries no pairing code"
            ),
            Err(NotSole::Bluez(error)) => tracing::warn!(
                "Could not make the BLE writer the sole central ({error}); \
                 the Wi-Fi result carries no pairing code"
            ),
        }
        None
    }

    /// Disconnect every connected central but `writer`, again while one is
    /// still or again listed, until BlueZ lists `writer` alone, all before
    /// `deadline`.
    fn disconnect_other_centrals(&self, writer: &str, deadline: Instant) -> Result<(), NotSole> {
        let mut first = true;
        loop {
            let connected = self.connected_centrals(deadline).map_err(NotSole::Bluez)?;
            if !connected.iter().any(|path| path == writer) {
                return Err(NotSole::WriterGone);
            }
            let others: Vec<String> = connected
                .into_iter()
                .filter(|path| path != writer)
                .collect();
            if others.is_empty() {
                return Ok(());
            }
            if first {
                tracing::info!(
                    "Disconnecting {} other BLE central(s) so the Wi-Fi result can carry a \
                     pairing code",
                    others.len()
                );
            } else {
                tracing::info!(
                    "{} other BLE central(s) still or again connected; disconnecting again",
                    others.len()
                );
            }
            for other in &others {
                self.disconnect_central(other, deadline)
                    .map_err(|error| NotSole::Bluez(format!("disconnecting a central: {error}")))?;
            }
            if Instant::now() + SOLE_CENTRAL_SETTLE >= deadline {
                return Err(NotSole::StillConnected(others.len()));
            }
            first = false;
            std::thread::sleep(SOLE_CENTRAL_SETTLE);
        }
    }

    /// [`SOLE_CENTRAL_TIMEOUT`], or what a test set.
    fn sole_central_timeout(&self) -> Duration {
        Duration::from_millis(self.sole_central_timeout_ms.load(Ordering::Relaxed))
    }

    /// The `Device1` paths of the connected BLE centrals, as BlueZ lists
    /// them.
    fn connected_centrals(&self, deadline: Instant) -> Result<Vec<String>, String> {
        self.on_bluez(
            deadline,
            |server| async move { server.list_centrals().await },
        )
    }

    /// Disconnect the central at `device`; one already gone counts as
    /// disconnected.
    fn disconnect_central(&self, device: &str, deadline: Instant) -> Result<(), String> {
        let path = device.to_string();
        let result = self.on_bluez(deadline, |server| async move {
            server.disconnect_device(&path).await
        });
        match result {
            Err(error) if CENTRAL_ALREADY_GONE.contains(&error.as_str()) => {
                tracing::info!("A BLE central was already gone when disconnecting it ({error})");
                Ok(())
            }
            result => result,
        }
    }

    /// BlueZ's listing of the connected BLE centrals, unbounded.
    async fn list_centrals(&self) -> Result<Vec<String>, String> {
        #[cfg(test)]
        if let Some(answer) = self.fake_bluez("list".into(), FakeCentrals::list).await {
            return answer;
        }
        self.bluez_app()?.connected_devices().await
    }

    /// BlueZ disconnecting the central at `device`, unbounded.
    async fn disconnect_device(&self, device: &str) -> Result<(), String> {
        #[cfg(test)]
        if let Some(answer) = self
            .fake_bluez(format!("disconnect {device}"), |fake| {
                fake.disconnect(device)
            })
            .await
        {
            return answer;
        }
        self.bluez_app()?.disconnect_device(device).await
    }

    /// A test's BlueZ answering `call`; `None` without one.
    #[cfg(test)]
    async fn fake_bluez<T>(
        &self,
        call: String,
        answer: impl FnOnce(&mut FakeCentrals) -> T,
    ) -> Option<T> {
        let begun = self.centrals.lock().as_mut().map(|fake| fake.begin(call));
        let (number, held) = begun?;
        if let Some(delay) = held {
            tokio::time::sleep(delay).await;
        }
        let mut fake = self.centrals.lock();
        let fake = fake.as_mut()?;
        if held.is_some() {
            fake.calls.push(format!("answered {number}"));
        }
        Some(answer(fake))
    }

    /// Run a BlueZ call on the runtime and wait for it until `deadline`.
    /// It runs on the request's own thread, so it can block on the runtime;
    /// the call is given up at `deadline` too, so one BlueZ never answers
    /// does not outlive the wait.
    fn on_bluez<T, F, Fut>(&self, deadline: Instant, call: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(Arc<GattServer>) -> Fut,
        Fut: std::future::Future<Output = Result<T, String>> + Send + 'static,
    {
        let (Some(runtime), Some(server)) = (self.runtime.get(), self.me.upgrade()) else {
            return Err("the GATT server is not running".into());
        };
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(bluez_timed_out)?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let answer = call(server);
        runtime.spawn(async move {
            let _ = sender.send(by_deadline(deadline, answer).await);
        });
        receiver
            .recv_timeout(remaining)
            .map_err(|_| bluez_timed_out())?
    }

    fn bluez_app(&self) -> Result<&bluez::BluezApp, String> {
        self.app
            .get()
            .ok_or_else(|| "BlueZ is not set up".to_string())
    }

    /// Notify the client outside a write: the Dial's answer and the join.
    /// `code` is the pairing code the packets carry: once BlueZ took them
    /// it is checked that nobody else may have it.
    fn send_to_client(&self, packets: Vec<(String, Vec<u8>)>, code: Option<SentCode>) {
        #[cfg(test)]
        let sent = packets.clone();
        if let (Some(runtime), Some(server)) = (self.runtime.get(), self.me.upgrade()) {
            let task = runtime.spawn(async move {
                let mut result_sent = true;
                for (uuid, packet) in packets {
                    let notified = server.notify(&uuid, packet).await;
                    if let Err(error) = &notified {
                        tracing::warn!("Updating characteristic return status=False ({error})");
                    }
                    if uuid == RPC_RESULT_UUID {
                        result_sent &= notified.is_ok();
                    }
                }
                if let Some(code) = code {
                    server.check_code_receivers(code, result_sent).await;
                }
            });
            #[cfg(test)]
            self.sends.lock().push(task);
            #[cfg(not(test))]
            drop(task);
        } else if let Some(code) = code {
            self.revoke_code(&code.code, "the Wi-Fi result could not be sent");
        }
        #[cfg(test)]
        self.pushed.lock().extend(sent);
    }

    /// Set the characteristic's value; BlueZ notifies every subscribed
    /// central.
    async fn notify(&self, uuid: &str, packet: Vec<u8>) -> Result<(), String> {
        #[cfg(test)]
        if let Some(fake) = self.centrals.lock().as_mut() {
            let name = improv::uuid_name(uuid).unwrap_or(uuid);
            fake.calls.push(format!("notify {name}"));
            return Ok(());
        }
        let Some(app) = self.app.get() else {
            return Ok(());
        };
        app.update_value(uuid, packet).await
    }

    /// After BlueZ took the result carrying `code`: BlueZ sends it to every
    /// central connected and subscribed, so one that connected (or came
    /// back) after the last listing, before the send, may have it too. The
    /// code is revoked when anyone but the writer is connected now, when
    /// BlueZ cannot say, and when the result did not go out. Only reasons
    /// are logged.
    async fn check_code_receivers(&self, code: SentCode, result_sent: bool) {
        if !result_sent {
            self.revoke_code(&code.code, "the Wi-Fi result was not sent");
            return;
        }
        let deadline = Instant::now() + self.sole_central_timeout();
        match by_deadline(deadline, self.list_centrals()).await {
            Ok(connected) => {
                let others = connected
                    .iter()
                    .filter(|path| **path != code.writer)
                    .count();
                if others > 0 {
                    self.revoke_code(
                        &code.code,
                        &format!("{others} other BLE central(s) connected when it was sent"),
                    );
                }
            }
            Err(error) => self.revoke_code(
                &code.code,
                &format!("could not list the connected BLE centrals after sending it ({error})"),
            ),
        }
    }

    /// Withdraw the pairing code the Wi-Fi result carried.
    fn revoke_code(&self, code: &str, reason: &str) {
        let live = self.auth.take_code(code).is_some();
        tracing::warn!(
            "Revoked the pairing code of the Wi-Fi result: {reason}{}",
            if live {
                ""
            } else {
                " (it was already used or expired)"
            }
        );
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
                            // No port: the backend no longer listens on it
                            // directly, nginx proxies every request.
                            std::net::IpAddr::V4(v4) => format!("http://{v4}"),
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

    /// Improv `IDENTIFY`: the Dial shows which phone is talking to this
    /// machine over BLE.
    pub fn identify(&self, client: Option<&BleClient>) {
        let name = client.and_then(BleClient::display_name);
        tracing::info!(
            "BLE client asked the machine to identify itself (name={})",
            name.as_deref().unwrap_or("unknown")
        );
        let message = match &name {
            Some(name) => {
                format!("\u{201c}{name}\u{201d} is connected to this machine over Bluetooth")
            }
            None => "A device is connected to this machine over Bluetooth".to_string(),
        };
        let mut notification = Notification::new(message);
        notification.id = IDENTIFY_NOTIFICATION_ID.to_string();
        self.notifications.add(notification);
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

fn bluez_timed_out() -> String {
    "BlueZ did not answer in time".to_string()
}

/// `call`, given up at `deadline`.
async fn by_deadline<T>(
    deadline: Instant,
    call: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(bluez_timed_out)?;
    tokio::time::timeout(remaining, call)
        .await
        .map_err(|_| bluez_timed_out())?
}

struct WifiCallbacks<'a> {
    server: &'a GattServer,
    client: Option<&'a BleClient>,
}

/// A Wi-Fi request waiting for the Dial.
struct WifiRequest {
    /// The id of the notification that asks.
    notification: String,
    /// The writer's `Device1` path, when BlueZ gave it.
    central: Option<String>,
    /// The phone's name, when it gave a printable one.
    client_name: Option<String>,
    /// How the Dial names the phone.
    device: String,
    network: String,
    ssid: Vec<u8>,
    password: Vec<u8>,
}

/// Characteristic values as the packets to notify, in order.
fn packets(values: Vec<(&'static str, Payload)>) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for (uuid, value) in values {
        let name = improv::uuid_name(uuid).unwrap_or(uuid);
        let packets = match value {
            Payload::Bytes(bytes) => vec![bytes],
            Payload::Packets(packets) => packets,
        };
        for packet in packets {
            tracing::info!("BLE RESP  {name} -> {} bytes", packet.len());
            out.push((uuid.to_string(), packet));
        }
    }
    out
}

impl Callbacks for WifiCallbacks<'_> {
    fn request_authorization(&mut self, ssid: &[u8], password: &[u8]) {
        self.server.request_wifi_authorization(
            self.client.cloned(),
            ssid.to_vec(),
            password.to_vec(),
        );
    }

    fn wifi_networks(&mut self) -> Option<Vec<String>> {
        Some(self.server.get_wifi_networks())
    }

    fn identify(&mut self) {
        self.server.identify(self.client);
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
