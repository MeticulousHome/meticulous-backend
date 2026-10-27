//! The Python tests' `FakeNetworkManager`, `FakeClock` and recorded
//! `NamedThread`s, as a [`WifiHost`]: NetworkManager with one station
//! device, saved connections, a scan list, the `nmcli`/`ping` subprocesses,
//! DNS and the internet probe, a clock that advances when slept on, and
//! threads that are recorded instead of run.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;

use super::host::{Task, WifiHost};
use super::nmcli::Nmcli;
use super::types::{AP_CONNECTION, Connection, Device, DeviceDetails, ScanEntry};
use crate::http_client::HttpError;
use crate::py::{Completed, PyError, PyResult, RunError};
use crate::zeroconf::Announcer;

pub const STATION: &str = "wlan0";
pub const MAC: &str = "AA:BB:CC:DD:EE:01";
pub const HOSTNAME: &str = "meticulousAromaticbean-1234";
pub const AP_NAME: &str = "MeticulousAromaticbean";
pub const AP_PASSWORD: &str = "espresso-1234";
pub const HOME_ROUTE: &str = "dst = 192.168.1.0/24, nh = 0.0.0.0, mt = 600";

/// A call on the `nmcli` package, as the Python fake records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    DeviceWifi(Option<bool>),
    WifiConnect(String, Option<String>),
    Delete(String),
    Up(String, Option<u32>),
    Down(String),
    Modify(String, Vec<(String, String)>),
}

pub struct FakeState {
    pub calls: Vec<Call>,
    pub commands: Vec<Vec<String>>,
    pub devices: Vec<(Device, DeviceDetails)>,
    pub connections: Vec<Connection>,
    pub scan: Vec<ScanEntry>,
    pub saved_key_mgmt: HashMap<String, String>,
    pub ping_returncodes: VecDeque<i32>,
    pub failing_command_prefix: Option<(Vec<String>, String)>,
    pub join_error: Option<PyError>,
    pub join_redirect: Option<String>,
    pub up_error: Option<PyError>,
    pub down_error: Option<PyError>,
    pub scan_error: Option<PyError>,
    pub show_all_error: Option<PyError>,
    pub threads: Vec<String>,
    pub dns_lookups: Vec<(String, u16)>,
    pub internet_calls: usize,
    pub internet_ok: bool,
    pub now: f64,
    /// Panics once slept past, to end an endless loop under test.
    pub stop_after: Option<f64>,
    /// Real time a scan takes, for the shared cold-start scan.
    pub scan_delay: std::time::Duration,
}

pub struct FakeNetwork {
    pub state: Mutex<FakeState>,
    pub device_wifi_calls: AtomicUsize,
}

impl FakeNetwork {
    pub fn new() -> Arc<FakeNetwork> {
        let network = Arc::new(FakeNetwork {
            state: Mutex::new(FakeState {
                calls: Vec::new(),
                commands: Vec::new(),
                devices: Vec::new(),
                connections: Vec::new(),
                scan: Vec::new(),
                saved_key_mgmt: HashMap::new(),
                ping_returncodes: VecDeque::new(),
                failing_command_prefix: None,
                join_error: None,
                join_redirect: None,
                up_error: None,
                down_error: None,
                scan_error: None,
                show_all_error: None,
                threads: Vec::new(),
                dns_lookups: Vec::new(),
                internet_calls: 0,
                internet_ok: true,
                now: 1_800_000_000.0,
                stop_after: None,
                scan_delay: std::time::Duration::ZERO,
            }),
            device_wifi_calls: AtomicUsize::new(0),
        });
        network.disconnect_station("disconnected");
        network
    }

    pub fn calls(&self) -> Vec<Call> {
        self.state.lock().calls.clone()
    }

    pub fn commands(&self) -> Vec<Vec<String>> {
        self.state.lock().commands.clone()
    }

    pub fn threads(&self) -> Vec<String> {
        self.state.lock().threads.clone()
    }

    /// `commands_except_type_lookups()`.
    pub fn commands_except_type_lookups(&self) -> Vec<Vec<String>> {
        self.commands()
            .into_iter()
            .filter(|c| !(c.len() >= 2 && c[0] == "nmcli" && c[1] == "-g"))
            .collect()
    }

    fn set_station(state: &mut FakeState, device: Device, details: DeviceDetails) {
        match state.devices.iter_mut().find(|(d, _)| d.device == STATION) {
            Some(entry) => *entry = (device, details),
            None => state.devices.push((device, details)),
        }
    }

    fn active_connection(state: &FakeState) -> Option<String> {
        state
            .devices
            .iter()
            .find(|(d, _)| d.device == STATION)
            .and_then(|(d, _)| d.connection.clone())
    }

    fn connect_locked(
        state: &mut FakeState,
        name: &str,
        ip4: Option<&str>,
        gateway: Option<&str>,
        ip6: Option<&str>,
    ) {
        let mut details = DeviceDetails::from_pairs(&[
            ("GENERAL.DEVICE", Some(STATION)),
            ("GENERAL.HWADDR", Some(MAC)),
            ("GENERAL.CONNECTION", Some(name)),
            ("IP4.ADDRESS[1]", ip4),
            ("IP4.GATEWAY", gateway),
            ("IP4.ROUTE[1]", gateway.map(|_| HOME_ROUTE)),
            ("IP4.DNS[1]", gateway),
        ]);
        if let Some(ip6) = ip6 {
            details.insert("IP6.ADDRESS[1]", Some(ip6.to_string()));
        }
        let device = Device {
            device: STATION.into(),
            device_type: "wifi".into(),
            state: "connected".into(),
            connection: Some(name.into()),
        };
        Self::set_station(state, device, details);
    }

    fn disconnect_locked(state: &mut FakeState, device_state: &str) {
        let details = DeviceDetails::from_pairs(&[
            ("GENERAL.DEVICE", Some(STATION)),
            ("GENERAL.HWADDR", Some(MAC)),
            ("GENERAL.CONNECTION", None),
        ]);
        let device = Device {
            device: STATION.into(),
            device_type: "wifi".into(),
            state: device_state.into(),
            connection: None,
        };
        Self::set_station(state, device, details);
    }

    pub fn connect_station(
        &self,
        name: &str,
        ip4: Option<&str>,
        gateway: Option<&str>,
        ip6: Option<&str>,
    ) {
        Self::connect_locked(&mut self.state.lock(), name, ip4, gateway, ip6);
    }

    /// `connect_station(name)` with the defaults.
    pub fn connect_home_station(&self, name: &str) {
        self.connect_station(name, Some("192.168.1.42/24"), Some("192.168.1.1"), None);
    }

    pub fn disconnect_station(&self, device_state: &str) {
        Self::disconnect_locked(&mut self.state.lock(), device_state);
    }

    pub fn add_connection(&self, name: &str, conn_type: &str) {
        Self::add_connection_locked(&mut self.state.lock(), name, conn_type);
    }

    fn add_connection_locked(state: &mut FakeState, name: &str, conn_type: &str) {
        state.connections.push(Connection {
            name: name.into(),
            uuid: format!("uuid-{name}"),
            conn_type: conn_type.into(),
            device: "--".into(),
        });
    }

    pub fn activate_hotspot(&self) {
        Self::connect_locked(
            &mut self.state.lock(),
            AP_CONNECTION,
            Some("10.42.0.1/24"),
            None,
            None,
        );
    }

    fn join_locked(state: &mut FakeState, ssid: &str) {
        let target = state
            .join_redirect
            .clone()
            .unwrap_or_else(|| ssid.to_string());
        Self::connect_locked(
            state,
            &target,
            Some("192.168.1.42/24"),
            Some("192.168.1.1"),
            None,
        );
    }
}

/// `scan_entry(ssid, signal, security, in_use, bssid)`.
pub fn scan_entry(ssid: &str, signal: i64, security: &str, in_use: bool, bssid: &str) -> ScanEntry {
    ScanEntry {
        in_use,
        ssid: ssid.into(),
        bssid: bssid.into(),
        mode: "Infra".into(),
        chan: 6,
        freq: 2437,
        rate: 130,
        signal,
        security: security.into(),
    }
}

/// `scan_entry(ssid, signal)` with WPA2, not in use.
pub fn wpa2(ssid: &str, signal: i64) -> ScanEntry {
    scan_entry(ssid, signal, "WPA2", false, "00:11:22:33:44:55")
}

impl Nmcli for FakeNetwork {
    fn device_status(&self) -> PyResult<Vec<Device>> {
        Ok(self
            .state
            .lock()
            .devices
            .iter()
            .map(|(d, _)| d.clone())
            .collect())
    }

    fn device_show(&self, ifname: &str) -> PyResult<DeviceDetails> {
        self.state
            .lock()
            .devices
            .iter()
            .find(|(d, _)| d.device == ifname)
            .map(|(_, details)| details.clone())
            .ok_or_else(|| PyError::key_error(ifname))
    }

    fn device_show_all(&self) -> PyResult<Vec<DeviceDetails>> {
        let state = self.state.lock();
        if let Some(error) = state.show_all_error.clone() {
            return Err(error);
        }
        Ok(state.devices.iter().map(|(_, d)| d.clone()).collect())
    }

    fn device_wifi(&self, rescan: Option<bool>) -> PyResult<Vec<ScanEntry>> {
        self.device_wifi_calls.fetch_add(1, Ordering::SeqCst);
        let delay = {
            let mut state = self.state.lock();
            state.calls.push(Call::DeviceWifi(rescan));
            if let Some(error) = state.scan_error.clone() {
                return Err(error);
            }
            state.scan_delay
        };
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        Ok(self.state.lock().scan.clone())
    }

    fn device_wifi_connect(&self, ssid: &str, password: Option<&str>) -> PyResult<()> {
        let mut state = self.state.lock();
        state
            .calls
            .push(Call::WifiConnect(ssid.into(), password.map(str::to_string)));
        if let Some(error) = state.join_error.clone() {
            return Err(error);
        }
        Self::join_locked(&mut state, ssid);
        Ok(())
    }

    fn connections(&self) -> PyResult<Vec<Connection>> {
        Ok(self.state.lock().connections.clone())
    }

    fn connection_up(&self, name: &str, wait: Option<u32>) -> PyResult<()> {
        let mut state = self.state.lock();
        state.calls.push(Call::Up(name.into(), wait));
        if let Some(error) = state.up_error.clone() {
            return Err(error);
        }
        Self::join_locked(&mut state, name);
        Ok(())
    }

    fn connection_down(&self, name: &str) -> PyResult<()> {
        let mut state = self.state.lock();
        state.calls.push(Call::Down(name.into()));
        if let Some(error) = state.down_error.clone() {
            return Err(error);
        }
        if Self::active_connection(&state).as_deref() == Some(name) {
            Self::disconnect_locked(&mut state, "disconnected");
        }
        Ok(())
    }

    fn connection_delete(&self, name: &str) -> PyResult<()> {
        let mut state = self.state.lock();
        state.calls.push(Call::Delete(name.into()));
        state.connections.retain(|c| c.name != name);
        if Self::active_connection(&state).as_deref() == Some(name) {
            Self::disconnect_locked(&mut state, "disconnected");
        }
        Ok(())
    }

    fn connection_modify(&self, name: &str, options: &[(&str, &str)]) -> PyResult<()> {
        self.state.lock().calls.push(Call::Modify(
            name.into(),
            options
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ));
        Ok(())
    }
}

impl WifiHost for FakeNetwork {
    fn nmcli(&self) -> &dyn Nmcli {
        self
    }

    fn run(&self, argv: &[String], _timeout: Option<f64>) -> Result<Completed, RunError> {
        let mut state = self.state.lock();
        state.commands.push(argv.to_vec());
        let mut done = Completed::default();
        let failing = state.failing_command_prefix.clone();
        if let Some((prefix, stderr)) = failing.filter(|(p, _)| argv.starts_with(p)) {
            let _ = prefix;
            done.returncode = 1;
            done.stderr = stderr;
        } else if argv.len() >= 3
            && argv[..3] == ["nmcli", "-g", "802-11-wireless-security.key-mgmt"]
        {
            let name = argv.last().cloned().unwrap_or_default();
            let key_mgmt = state
                .saved_key_mgmt
                .get(&name)
                .cloned()
                .unwrap_or_else(|| "wpa-psk".into());
            done.stdout = format!("{key_mgmt}\n");
        } else if argv.first().map(String::as_str) == Some("ping") {
            done.returncode = state.ping_returncodes.pop_front().unwrap_or(0);
        } else if argv.len() >= 3 && argv[..3] == ["nmcli", "connection", "add"] {
            if let Some(i) = argv.iter().position(|a| a == "con-name") {
                let name = argv[i + 1].clone();
                Self::add_connection_locked(&mut state, &name, "wifi");
            }
        } else if argv == ["nmcli", "--wait", "35", "connection", "up", AP_CONNECTION] {
            Self::connect_locked(&mut state, AP_CONNECTION, Some("10.42.0.1/24"), None, None);
        }
        Ok(done)
    }

    fn which(&self, _name: &str) -> bool {
        true
    }

    fn path_exists(&self, _path: &str) -> bool {
        false
    }

    fn getaddrinfo(&self, host: &str, port: u16) -> Result<(), String> {
        self.state.lock().dns_lookups.push((host.into(), port));
        Ok(())
    }

    fn http_status(&self, _url: &str, _timeout: f64, _user_agent: &str) -> Result<u16, HttpError> {
        let mut state = self.state.lock();
        state.internet_calls += 1;
        if state.internet_ok {
            Ok(200)
        } else {
            Err(HttpError::Failed("unreachable".into()))
        }
    }

    fn gethostname(&self) -> String {
        HOSTNAME.into()
    }

    fn time(&self) -> f64 {
        self.state.lock().now
    }

    fn monotonic(&self) -> f64 {
        self.state.lock().now
    }

    fn sleep(&self, seconds: f64) {
        let mut state = self.state.lock();
        state.now += seconds;
        if state.stop_after.is_some_and(|limit| state.now > limit) {
            panic!("fake clock reached its stop");
        }
    }

    fn spawn(&self, name: &str, _task: Task) {
        // Background refreshes are recorded, not run, so answers stay
        // deterministic.
        self.state.lock().threads.push(name.into());
    }
}

/// A `MagicMock` zeroconf: counts calls.
#[derive(Default)]
pub struct FakeAnnouncer {
    pub starts: AtomicUsize,
    pub stops: AtomicUsize,
    pub restarts: AtomicUsize,
}

impl Announcer for FakeAnnouncer {
    fn start(&self) -> PyResult<()> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&self) -> PyResult<()> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn restart(&self) -> PyResult<()> {
        self.restarts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A counter for `update_gatt_advertisement` and config saves.
#[derive(Default)]
pub struct Counter(pub AtomicUsize);

impl Counter {
    pub fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }

    pub fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// The fixture of `WifiApiTestCase.setUp`, on an `App`.
pub struct WifiFixture {
    pub network: Arc<FakeNetwork>,
    pub zeroconf: Arc<FakeAnnouncer>,
    pub gatt: Arc<Counter>,
    pub saves: Arc<Counter>,
}

impl WifiFixture {
    pub fn install(app: &crate::app::App) -> WifiFixture {
        use crate::config::{WIFI_MODE_CLIENT, keys};
        use serde_json::json;
        let network = FakeNetwork::new();
        let zeroconf = Arc::new(FakeAnnouncer::default());
        let gatt = Arc::new(Counter::default());
        let saves = Arc::new(Counter::default());
        app.wifi.set_host(network.clone());
        app.wifi.set_zeroconf(Some(zeroconf.clone()));
        let hook = gatt.clone();
        app.wifi.set_gatt_hook(Arc::new(move || hook.bump()));
        app.config
            .set(keys::WIFI, keys::WIFI_MODE, json!(WIFI_MODE_CLIENT));
        app.config
            .set(keys::WIFI, keys::WIFI_AP_NAME, json!(AP_NAME));
        app.config
            .set(keys::WIFI, keys::WIFI_AP_PASSWORD, json!(AP_PASSWORD));
        app.config
            .set(keys::WIFI, keys::WIFI_KNOWN_WIFIS, json!({}));
        let counter = saves.clone();
        app.config.on_save(move || counter.bump());
        WifiFixture {
            network,
            zeroconf,
            gatt,
            saves,
        }
    }

    pub fn restarts(&self) -> usize {
        self.zeroconf.restarts.load(Ordering::SeqCst)
    }

    /// `connect_home()`.
    pub fn connect_home(&self) {
        self.network.add_connection("HomeNet", "wifi");
        self.network.connect_home_station("HomeNet");
    }

    /// `start_in_hotspot_mode()`.
    pub fn start_in_hotspot_mode(&self, app: &crate::app::App) {
        use crate::config::{WIFI_MODE_AP, keys};
        app.config
            .set(keys::WIFI, keys::WIFI_MODE, serde_json::json!(WIFI_MODE_AP));
        self.network.add_connection(AP_CONNECTION, "wifi");
        self.network.activate_hotspot();
    }
}
