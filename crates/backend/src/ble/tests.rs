//! tests/test_ble_gatt_wifi.py, plus the Improv read/write handling and the
//! advertisement data, without D-Bus: the Wi-Fi manager is replaced at the
//! `ProvisioningWifi` boundary and BlueZ's advertising manager by a fake.

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Value, json};
use tracing_subscriber::layer::SubscriberExt;

use super::improv::{ERROR_UUID, RPC_COMMAND_UUID, RPC_RESULT_UUID, STATUS_UUID, checksum};
use super::*;
use crate::config::keys;
use crate::py::PyError;
use crate::wifi::types::IpNet;

pub(crate) struct FakeWifi {
    connect_result: Mutex<Result<bool, PyError>>,
    config: Mutex<WifiSystemConfig>,
    credentials: Mutex<Vec<Value>>,
    scan: Vec<ScanEntry>,
}

impl FakeWifi {
    pub(crate) fn new() -> Arc<FakeWifi> {
        Arc::new(FakeWifi {
            connect_result: Mutex::new(Ok(true)),
            config: Mutex::new(network_config(true, "MyWiFi", &["192.168.1.100/24"])),
            credentials: Mutex::new(Vec::new()),
            scan: vec![
                crate::wifi::fake::wpa2("HomeNet", 70),
                crate::wifi::fake::wpa2("", 10),
            ],
        })
    }

    fn last(&self) -> Value {
        self.credentials
            .lock()
            .last()
            .cloned()
            .unwrap_or(Value::Null)
    }
}

impl ProvisioningWifi for FakeWifi {
    fn connect(&self, credentials: &Value) -> PyResult<bool> {
        self.credentials.lock().push(credentials.clone());
        self.connect_result.lock().clone()
    }

    fn current_config(&self) -> PyResult<WifiSystemConfig> {
        Ok(self.config.lock().clone())
    }

    fn scan(&self) -> Vec<ScanEntry> {
        self.scan.clone()
    }
}

fn network_config(connected: bool, name: &str, ips: &[&str]) -> WifiSystemConfig {
    WifiSystemConfig {
        connected,
        connection_name: Some(name.into()),
        gateway: None,
        routes: Vec::new(),
        ips: ips.iter().map(|ip| IpNet::parse(ip).expect("ip")).collect(),
        dns: Vec::new(),
        mac: Some(String::new()),
        hostname: "meticulous".into(),
        domains: Vec::new(),
    }
}

fn server() -> (
    tempfile::TempDir,
    Arc<crate::app::App>,
    Arc<GattServer>,
    Arc<FakeWifi>,
) {
    let (dir, app) = crate::testing::app();
    let wifi = FakeWifi::new();
    let server = GattServer::new(
        wifi.clone(),
        app.hostname.clone(),
        app.config.clone(),
        app.notifications.clone(),
        app.auth.clone(),
        8080,
    );
    (dir, app, server, wifi)
}

/// Records every log line emitted while `f` runs on this thread.
fn logged<T>(f: impl FnOnce() -> T) -> (T, String) {
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<String>>>);
    struct Visitor(String);
    impl tracing::field::Visit for Visitor {
        fn record_debug(&mut self, _field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push_str(&format!("{value:?} "));
        }
    }
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut visitor = Visitor(String::new());
            event.record(&mut visitor);
            self.0.lock().push(visitor.0);
        }
    }
    let capture = Capture::default();
    crate::logging::pin_callsite_interest();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let result = tracing::subscriber::with_default(subscriber, f);
    let lines = capture.0.lock().join("\n");
    (result, lines)
}

// ---- wifi_connect: UTF-8 decoding ----

/// test_ble_gatt_wifi.py::TestWifiConnectUTF8::test_ascii_ssid_and_password
#[test]
fn ascii_ssid_and_password() {
    let (_dir, _app, server, wifi) = server();
    let result = server
        .wifi_connect(b"MyNetwork", b"password")
        .expect("urls");
    assert!(result.iter().any(|url| url.contains("192.168.1.100")));
    assert_eq!(wifi.last()["ssid"], json!("MyNetwork"));
    assert_eq!(wifi.last()["password"], json!("password"));
    // `WifiWpaPskCredentials(...).to_dict()`.
    assert_eq!(
        wifi.last(),
        json!({"type": "PSK", "security": "", "ssid": "MyNetwork", "password": "password"})
    );
}

/// test_ble_gatt_wifi.py::TestWifiConnectUTF8 (umlauts, Chinese, Japanese,
/// Korean, emoji, Arabic, mixed-script password)
#[test]
fn utf8_names_and_passwords() {
    let (_dir, _app, server, wifi) = server();
    for (ssid, password) in [
        ("\u{dc}n\u{ef}c\u{f6}d\u{e9}", "P\u{e4}ssw\u{f6}rd"),
        ("\u{6211}\u{7684}\u{7f51}\u{7edc}", "\u{5bc6}\u{7801}123"),
        ("\u{6771}\u{4eac}WiFi", "pass"),
        ("\u{c640}\u{c774}\u{d30c}\u{c774}", "pass"),
        ("\u{2615}\u{1f3e0}Net", "pass"),
        ("\u{634}\u{628}\u{643}\u{629}", "pass"),
        ("Net", "p@ss\u{5bc6}\u{7801}w\u{f6}rd!"),
    ] {
        let result = server.wifi_connect(ssid.as_bytes(), password.as_bytes());
        assert!(result.is_some(), "{ssid}");
        assert_eq!(wifi.last()["ssid"], json!(ssid));
        assert_eq!(wifi.last()["password"], json!(password));
    }
}

/// test_ble_gatt_wifi.py::TestWifiConnectInvalidEncoding
#[test]
fn invalid_utf8_is_refused_before_connecting() {
    let (_dir, _app, server, wifi) = server();
    for (ssid, password) in [
        (vec![0x80, 0x81, 0x82], b"pass".to_vec()),
        (b"Net".to_vec(), vec![0xFE, 0xFF]),
        ([b"Net".as_slice(), &[0xE4]].concat(), b"pass".to_vec()),
        (b"caf\xe9".to_vec(), b"pass".to_vec()),
        (vec![0xC0, 0xAF], b"pass".to_vec()),
    ] {
        assert_eq!(server.wifi_connect(&ssid, &password), None, "{ssid:?}");
    }
    assert!(wifi.credentials.lock().is_empty());
}

/// test_ble_gatt_wifi.py::TestWifiConnectEdgeCases
#[test]
fn edge_cases() {
    let (_dir, _app, server, wifi) = server();
    assert!(server.wifi_connect(b"", b"pass").is_some());
    assert_eq!(wifi.last()["ssid"], json!(""));
    assert!(server.wifi_connect(b"OpenNet", b"").is_some());
    assert_eq!(wifi.last()["password"], json!(""));
    assert!(server.wifi_connect(b"My Home Network", b"pass").is_some());
    assert_eq!(wifi.last()["ssid"], json!("My Home Network"));
    assert!(server.wifi_connect(b"Net", b"p@$$w0rd!#%^&*()").is_some());
    assert_eq!(wifi.last()["password"], json!("p@$$w0rd!#%^&*()"));

    *wifi.connect_result.lock() = Ok(false);
    assert_eq!(server.wifi_connect(b"Net", b"pass"), None);
    *wifi.connect_result.lock() = Err(PyError::exception("NetworkManager error"));
    assert_eq!(server.wifi_connect(b"Net", b"pass"), None);
}

/// test_ble_gatt_wifi.py::TestWifiConnectEdgeCases::test_ipv6_address_in_result
#[test]
fn ipv6_address_in_result() {
    let (_dir, _app, server, wifi) = server();
    *wifi.config.lock() = network_config(true, "Net", &["192.168.1.100/24", "fe80::1/64"]);
    let result = server.wifi_connect(b"Net", b"pass").expect("urls");
    assert_eq!(
        result,
        // IPv4 without the port (nginx serves it on 80); IPv6 keeps it.
        vec!["http://192.168.1.100", "http://[fe80::1]:8080"]
    );
}

// ---- advertisement update recovery ----

#[derive(Default)]
struct FakeAdvertising {
    ads: Mutex<Vec<String>>,
    register_failures: Mutex<Vec<String>>,
    registered: Mutex<Vec<String>>,
    unregistered: Mutex<Vec<String>>,
    exported: Mutex<Vec<Advertisement>>,
    unexported: Mutex<Vec<String>>,
}

impl AdvertisingBackend for FakeAdvertising {
    fn base_path(&self) -> String {
        "/org/bluez/MeticulousEspresso".into()
    }

    async fn take_last(&self) -> Option<String> {
        self.ads.lock().pop()
    }

    async fn push(&self, path: String) {
        self.ads.lock().push(path);
    }

    async fn unregister(&self, path: &str) -> Result<(), String> {
        self.unregistered.lock().push(path.into());
        Ok(())
    }

    async fn register(&self, path: &str) -> Result<(), String> {
        self.registered.lock().push(path.into());
        let mut failures = self.register_failures.lock();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.remove(0))
        }
    }

    async fn export(&self, ad: Advertisement) -> Result<(), String> {
        self.exported.lock().push(ad);
        Ok(())
    }

    async fn unexport(&self, path: &str) -> Result<(), String> {
        self.unexported.lock().push(path.into());
        Ok(())
    }
}

/// test_ble_gatt_wifi.py::TestAdvertisementUpdateRecovery::test_transient_registration_failure_does_not_stop_gatt
#[tokio::test]
async fn transient_registration_failure_does_not_stop_gatt() {
    let backend = FakeAdvertising::default();
    backend
        .register_failures
        .lock()
        .push("Failed to register advertisement".into());

    update_with_retries(&backend, "MeticulousEspresso", &[0x01], &[0]).await;

    assert_eq!(backend.registered.lock().len(), 2);
    assert_eq!(
        *backend.ads.lock(),
        vec!["/org/bluez/MeticulousEspresso/advertisement0".to_string()]
    );
}

/// test_ble_gatt_wifi.py::TestAdvertisementUpdateRecovery::test_failed_registration_is_cleaned_up_for_retry
#[tokio::test]
async fn failed_registration_is_cleaned_up_for_retry() {
    let backend = FakeAdvertising::default();
    let old = "/org/bluez/MeticulousEspresso/advertisement1".to_string();
    let new = "/org/bluez/MeticulousEspresso/advertisement0".to_string();
    backend.ads.lock().push(old.clone());
    backend
        .register_failures
        .lock()
        .push("Failed to register advertisement".into());

    let first = replace_advertisement(&backend, "MeticulousEspresso", &[0x02]).await;
    assert_eq!(first, Err("Failed to register advertisement".into()));
    assert!(backend.ads.lock().is_empty());

    replace_advertisement(&backend, "MeticulousEspresso", &[0x02])
        .await
        .expect("recovered");

    assert_eq!(*backend.ads.lock(), vec![new.clone()]);
    assert_eq!(*backend.unregistered.lock(), vec![old.clone()]);
    assert_eq!(backend.registered.lock().len(), 2);
    assert_eq!(*backend.unexported.lock(), vec![old, new.clone()]);
    // PARITY: the replacement carries the manufacturer data and no service
    // UUID.
    let ad = backend.exported.lock()[1].clone();
    assert_eq!(
        ad,
        Advertisement {
            path: new,
            local_name: "MeticulousEspresso".into(),
            service_uuids: Vec::new(),
            manufacturer_data: Some(vec![0x02]),
        }
    );
}

/// A previous advertisement BlueZ already forgot is rebuilt, not retried.
#[tokio::test]
async fn missing_previous_advertisement_is_rebuilt() {
    struct Missing(FakeAdvertising);
    impl AdvertisingBackend for Missing {
        fn base_path(&self) -> String {
            self.0.base_path()
        }
        async fn take_last(&self) -> Option<String> {
            self.0.take_last().await
        }
        async fn push(&self, path: String) {
            self.0.push(path).await
        }
        async fn unregister(&self, _path: &str) -> Result<(), String> {
            Err("org.bluez.Error.DoesNotExist: Does Not Exist".into())
        }
        async fn register(&self, path: &str) -> Result<(), String> {
            self.0.register(path).await
        }
        async fn export(&self, ad: Advertisement) -> Result<(), String> {
            self.0.export(ad).await
        }
        async fn unexport(&self, path: &str) -> Result<(), String> {
            self.0.unexport(path).await
        }
    }
    let backend = Missing(FakeAdvertising::default());
    backend.0.ads.lock().push("/old".into());
    replace_advertisement(&backend, "M", &[0])
        .await
        .expect("rebuilt");
    assert_eq!(backend.0.ads.lock().len(), 1);
}

// ---- credentials never reach the logs ----

/// test_ble_gatt_wifi.py::TestCredentialLogRedaction::test_wifi_connect_does_not_log_ssid_or_password
#[test]
fn wifi_connect_does_not_log_ssid_or_password() {
    let (_dir, _app, server, _wifi) = server();
    let ssid = "SENTINEL-PRIVATE-NETWORK";
    let password = "SENTINEL-SECRET-PASSWORD";

    let (result, logged) = logged(|| server.wifi_connect(ssid.as_bytes(), password.as_bytes()));

    assert!(result.is_some());
    assert!(!logged.contains(ssid));
    assert!(!logged.contains(password));
    assert!(logged.contains("ssid_bytes="));
    assert!(logged.contains("password_bytes="));
}

/// test_ble_gatt_wifi.py::TestCredentialLogRedaction::test_wifi_connect_exception_does_not_log_exception_credentials
#[test]
fn wifi_connect_exception_does_not_log_exception_credentials() {
    let (_dir, _app, server, wifi) = server();
    let password = "SENTINEL-EXCEPTION-SECRET";
    *wifi.connect_result.lock() = Err(PyError::new(
        "RuntimeError",
        format!("NetworkManager rejected {password}"),
    ));

    let (result, logged) = logged(|| server.wifi_connect(b"PrivateNetwork", password.as_bytes()));

    assert_eq!(result, None);
    assert!(!logged.contains(password));
    assert!(logged.contains("RuntimeError"));
}

/// test_ble_gatt_wifi.py::TestCredentialLogRedaction::test_ble_write_does_not_log_raw_payload
#[test]
fn ble_write_does_not_log_raw_payload() {
    let (_dir, _app, server, _wifi) = server();
    let payload = b"SENTINEL-RAW-BLE-CREDENTIAL-PACKET";

    let (_result, logged) = logged(|| server.write_request(RPC_COMMAND_UUID, payload, None));

    assert!(!logged.contains("SENTINEL-RAW-BLE-CREDENTIAL-PACKET"));
    assert!(!logged.contains(&hex::encode(payload)));
    assert!(logged.contains(&format!("{} bytes", payload.len())));
}

// ---- Improv over the characteristics ----

fn wifi_settings(ssid: &[u8], password: &[u8]) -> Vec<u8> {
    let mut data = vec![
        0x01,
        (ssid.len() + password.len() + 2) as u8,
        ssid.len() as u8,
    ];
    data.extend_from_slice(ssid);
    data.push(password.len() as u8);
    data.extend_from_slice(password);
    data.push(checksum(&data));
    data
}

/// The Dial's question for the pending Wi-Fi request.
fn wifi_question(app: &crate::app::App) -> crate::notifications::Notification {
    let pending = app.notifications.unacknowledged();
    assert_eq!(pending.len(), 1, "one question on the Dial");
    pending[0].clone()
}

/// Waits until `count` notifications were sent after the write.
fn pushed_after_write(server: &GattServer, count: usize) -> Vec<(String, Vec<u8>)> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let pushed = server.pushed.lock().clone();
        if pushed.len() >= count || std::time::Instant::now() > deadline {
            return pushed;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn status(state: u8) -> (String, Vec<u8>) {
    (STATUS_UUID.to_string(), vec![state])
}

fn error(code: u8) -> (String, Vec<u8>) {
    (ERROR_UUID.to_string(), vec![code])
}

/// `Device1` paths of the provisioning phone and of another central.
const PHONE: &str = "/org/bluez/hci0/dev_00_00_00_00_00_01";
const OTHER: &str = "/org/bluez/hci0/dev_00_00_00_00_00_02";

/// Where the server runs its BlueZ calls and sends, as `start` sets it up.
static RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> = std::sync::LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
});

/// BlueZ answers as `fake` says, and making the writer the sole central
/// (and the check after the send) is given up after `timeout`.
fn fake_bluez(server: &GattServer, fake: FakeCentrals, timeout: Duration) {
    *server.centrals.lock() = Some(fake);
    let _ = server.runtime.set(RUNTIME.handle().clone());
    server
        .sole_central_timeout_ms
        .store(timeout.as_millis() as u64, Ordering::Relaxed);
}

/// The RPC result a later read returns after joining with `base` only.
fn url_only_result(base: &str) -> Vec<u8> {
    let mut packet = vec![0x01, (base.len() + 1) as u8, base.len() as u8];
    packet.extend_from_slice(base.as_bytes());
    packet.push(checksum(&packet));
    packet
}

/// WIFI_SETTINGS joins nothing until someone presses Connect on the Dial,
/// which asks "<phone> is requesting connection to <SSID>"; then the app
/// gets AUTHORIZED, PROVISIONING, PROVISIONED and the URL packets.
#[test]
fn approved_wifi_request_provisions_and_notifies_the_rpc_result() {
    let (_dir, app, server, wifi) = server();
    let phone = BleClient {
        name: Some("Ana's iPhone".into()),
        device: Some(PHONE.into()),
    };
    // The phone is the only central connected.
    fake_bluez(
        &server,
        FakeCentrals::new(vec![Ok(vec![PHONE.into()])]),
        SOLE_CENTRAL_TIMEOUT,
    );

    let notifications = server
        .write_request(
            RPC_COMMAND_UUID,
            &wifi_settings(b"HomeNet", b"hunter22"),
            Some(&phone),
        )
        .expect("no exception");

    // AUTHORIZATION_REQUIRED while the Dial asks; nothing joined yet, and a
    // read does not authorize the request.
    assert_eq!(notifications, vec![status(0x01)]);
    assert_eq!(server.read_request(STATUS_UUID).expect("read"), vec![0x01]);
    assert!(wifi.credentials.lock().is_empty());
    let question = wifi_question(&app);
    assert_eq!(
        question.message,
        "Ana's iPhone is requesting connection to HomeNet"
    );
    assert_eq!(question.responses, ["Connect", "Reject"]);
    // Connect lets the phone use the machine: only the Dial may press it.
    assert!(question.local_answer);

    assert!(
        app.notifications
            .acknowledge(&question.id, Some("Connect".into()))
    );
    let pushed = pushed_after_write(&server, 4);

    assert_eq!(wifi.last()["ssid"], json!("HomeNet"));
    assert_eq!(pushed[..3], [status(0x02), status(0x03), status(0x04)]);
    // The result: the URL, then the pair URL with a code for this phone
    // (device authorization contract, section 8).
    let (uuid, packet) = &pushed[3];
    assert_eq!(uuid, RPC_RESULT_UUID);
    let base = "http://192.168.1.100";
    let pair_url_length = packet[2 + 1 + base.len()];
    let pair_url =
        std::str::from_utf8(&packet[2 + 1 + base.len() + 1..][..usize::from(pair_url_length)])
            .expect("utf8");
    let code = pair_url
        .strip_prefix(&format!("{base}/#pair="))
        .expect("pair URL");
    let mut expected = vec![0x01, 0];
    for entry in [base, pair_url] {
        expected.push(entry.len() as u8);
        expected.extend_from_slice(entry.as_bytes());
    }
    expected[1] = (expected.len() - 2) as u8;
    expected.push(checksum(&expected));
    assert_eq!(packet, &expected);
    let pairing = app.auth.take_code(code).expect("a live code");
    assert_eq!(pairing.source, crate::device_auth::CodeSource::Improv);
    assert_eq!(pairing.name.as_deref(), Some("Ana's iPhone"));

    // PARITY: once the request is done, reading STATUS re-authorizes, so it
    // says AUTHORIZED (2) even after provisioning.
    assert_eq!(server.read_request(STATUS_UUID).expect("read"), vec![0x02]);
    // A later read (any BLE client in range) gets the URL only.
    assert_eq!(
        server.read_request(RPC_RESULT_UUID).expect("read"),
        url_only_result(base)
    );
}

/// A Wi-Fi request from `writer`, approved and joined while BlueZ answers
/// as the fake says.
struct Joined {
    /// The RPC result notified.
    packet: Vec<u8>,
    /// The pairing codes live once everything was sent and checked.
    codes: usize,
    /// What the server asked of BlueZ, in order, but the STATUS
    /// notifications (`FakeCentrals::calls`).
    calls: Vec<String>,
    server: Arc<GattServer>,
}

const BASE: &str = "http://192.168.1.100";

fn has_pair_url(packet: &[u8]) -> bool {
    String::from_utf8_lossy(packet).contains(&format!("{BASE}/#pair="))
}

fn joined(writer: Option<&str>, fake: FakeCentrals, timeout: Duration) -> Joined {
    let (_dir, app, server, _wifi) = server();
    fake_bluez(&server, fake, timeout);
    let phone = BleClient {
        name: Some("Ana's iPhone".into()),
        device: writer.map(str::to_string),
    };
    server
        .write_request(
            RPC_COMMAND_UUID,
            &wifi_settings(b"HomeNet", b"x"),
            Some(&phone),
        )
        .expect("no exception");
    app.notifications
        .acknowledge(&wifi_question(&app).id, Some("Connect".into()));
    let pushed = pushed_after_write(&server, 4);
    assert_eq!(pushed[3].0, RPC_RESULT_UUID);
    // Every send, and the check after the result, is over.
    let sends = std::mem::take(&mut *server.sends.lock());
    for send in sends {
        RUNTIME.block_on(send).expect("the send task");
    }
    Joined {
        packet: pushed[3].1.clone(),
        codes: app.auth.live_codes(),
        calls: bluez_calls(&server),
        server,
    }
}

fn bluez_calls(server: &GattServer) -> Vec<String> {
    server
        .centrals
        .lock()
        .as_ref()
        .expect("a fake BlueZ")
        .calls
        .iter()
        .filter(|call| *call != "notify STATUS_UUID")
        .cloned()
        .collect()
}

fn listings(listings: &[&[&str]]) -> Vec<Result<Vec<String>, String>> {
    listings
        .iter()
        .map(|listed| Ok(listed.iter().map(|path| path.to_string()).collect()))
        .collect()
}

fn calls(calls: &[&str]) -> Vec<String> {
    calls.iter().map(|call| call.to_string()).collect()
}

/// BlueZ notifies the result to every connected central that subscribed
/// to RPC_RESULT, not only the phone that wrote WIFI_SETTINGS, and a code
/// in it lets whoever exchanges it first use the machine. A bonded tablet
/// left connected in the background kept every phone from getting a code,
/// so the server disconnects the other centrals (never the writer) and
/// the pairing URL, and the code, go out once BlueZ lists the writer alone
/// (device authorization contract, section 8). A central that left on its
/// own before its disconnect (BlueZ answers `NotConnected` or
/// `UnknownObject`) is not a failure, and one that is back while the
/// server waits for BlueZ is disconnected again. Expected values: the
/// order of BlueZ calls `make_writer_sole_central` and
/// `check_code_receivers` make, and the live codes
/// (`DeviceAuth::live_codes`).
#[test]
fn pairing_code_goes_out_once_the_writer_is_the_sole_central() {
    let timeout = Duration::from_secs(2);
    for (case, fake, expected) in [
        (
            "the writer alone: nobody is disconnected",
            FakeCentrals::new(listings(&[&[PHONE]])),
            calls(&["list", "notify RPC_RESULT_UUID", "list"]),
        ),
        (
            "another central: disconnected, then the code goes out",
            FakeCentrals::new(listings(&[&[PHONE, OTHER], &[PHONE]])),
            calls(&[
                "list",
                &format!("disconnect {OTHER}"),
                "list",
                "notify RPC_RESULT_UUID",
                "list",
            ]),
        ),
        (
            "another central back during the wait: disconnected again",
            FakeCentrals::new(listings(&[&[PHONE, OTHER], &[PHONE, OTHER], &[PHONE]])),
            calls(&[
                "list",
                &format!("disconnect {OTHER}"),
                "list",
                &format!("disconnect {OTHER}"),
                "list",
                "notify RPC_RESULT_UUID",
                "list",
            ]),
        ),
        (
            "another central already disconnected (NotConnected)",
            FakeCentrals {
                refusals: vec![(OTHER.into(), "org.bluez.Error.NotConnected".into())],
                ..FakeCentrals::new(listings(&[&[PHONE, OTHER], &[PHONE]]))
            },
            calls(&[
                "list",
                &format!("disconnect {OTHER}"),
                "list",
                "notify RPC_RESULT_UUID",
                "list",
            ]),
        ),
        (
            "another central already gone from BlueZ (UnknownObject)",
            FakeCentrals {
                refusals: vec![(
                    OTHER.into(),
                    "org.freedesktop.DBus.Error.UnknownObject".into(),
                )],
                ..FakeCentrals::new(listings(&[&[PHONE, OTHER], &[PHONE]]))
            },
            calls(&[
                "list",
                &format!("disconnect {OTHER}"),
                "list",
                "notify RPC_RESULT_UUID",
                "list",
            ]),
        ),
    ] {
        let joined = joined(Some(PHONE), fake, timeout);
        assert!(has_pair_url(&joined.packet), "{case}");
        assert_eq!(joined.codes, 1, "{case}");
        assert_eq!(joined.calls, expected, "{case}");
    }
}

/// Without a writer BlueZ lists alone the result is the URL-only one, no
/// code is minted and the app asks for access instead (device
/// authorization contract, section 8): the writer unknown, gone from the
/// start or gone after the others were disconnected, a disconnect BlueZ
/// refuses, a listing BlueZ fails. Expected values: the URL-only layout
/// the stored copy already has, the BlueZ calls `make_writer_sole_central`
/// makes, and no live code.
#[test]
fn no_pairing_code_unless_the_writer_is_made_the_sole_central() {
    let timeout = Duration::from_secs(2);
    for (case, writer, fake, expected) in [
        (
            "the writer unknown",
            None,
            FakeCentrals::new(listings(&[&[PHONE, OTHER]])),
            calls(&["notify RPC_RESULT_UUID"]),
        ),
        (
            "the writer disconnected",
            Some(PHONE),
            FakeCentrals::new(listings(&[&[OTHER]])),
            calls(&["list", "notify RPC_RESULT_UUID"]),
        ),
        (
            "the writer gone once the others were disconnected",
            Some(PHONE),
            FakeCentrals::new(listings(&[&[PHONE, OTHER], &[]])),
            calls(&[
                "list",
                &format!("disconnect {OTHER}"),
                "list",
                "notify RPC_RESULT_UUID",
            ]),
        ),
        (
            "a disconnect BlueZ refuses",
            Some(PHONE),
            FakeCentrals {
                refusals: vec![(OTHER.into(), "org.bluez.Error.Failed".into())],
                ..FakeCentrals::new(listings(&[&[PHONE, OTHER], &[PHONE]]))
            },
            calls(&[
                "list",
                &format!("disconnect {OTHER}"),
                "notify RPC_RESULT_UUID",
            ]),
        ),
        (
            "the BlueZ listing fails",
            Some(PHONE),
            FakeCentrals::new(vec![Err("org.bluez.Error.Failed".into())]),
            calls(&["list", "notify RPC_RESULT_UUID"]),
        ),
    ] {
        let joined = joined(writer, fake, timeout);
        assert_eq!(joined.packet, url_only_result(BASE), "{case}");
        assert_eq!(joined.codes, 0, "{case}");
        assert_eq!(joined.calls, expected, "{case}");
    }
}

/// A central that stays connected (a bonded tablet that reconnects as
/// soon as it is dropped) is disconnected again on every listing until
/// [`SOLE_CENTRAL_TIMEOUT`] runs out; then the result is the URL-only one
/// and no code is minted. Expected values: `disconnect_other_centrals`
/// (disconnect, wait [`SOLE_CENTRAL_SETTLE`], list again) and no live
/// code.
#[test]
fn a_central_that_stays_connected_is_disconnected_again_until_the_deadline() {
    let joined = joined(
        Some(PHONE),
        FakeCentrals::new(listings(&[&[PHONE, OTHER]])),
        Duration::from_millis(700),
    );
    assert_eq!(joined.packet, url_only_result(BASE));
    assert_eq!(joined.codes, 0);
    let (last, asked) = joined.calls.split_last().expect("calls");
    assert_eq!(last, "notify RPC_RESULT_UUID");
    let disconnects = asked.iter().filter(|call| *call != "list").count();
    assert!(disconnects >= 2, "disconnected again: {asked:?}");
    let expected: Vec<String> = ["list".to_string(), format!("disconnect {OTHER}")]
        .into_iter()
        .cycle()
        .take(asked.len())
        .collect();
    assert_eq!(asked, expected);
}

/// The last listing before the code is minted does not see a central that
/// connects again before BlueZ sends the result, and BlueZ keeps a bonded
/// central's subscription, so it may get the code too. Once the result is
/// handed to BlueZ the server lists the centrals again and revokes the
/// code when anyone but the writer is connected, or when BlueZ cannot say
/// (device authorization contract, section 8): the result still carried
/// the pair URL, but its code no longer pairs. Expected values: the order
/// `send_to_client` then `check_code_receivers` make the calls in (the
/// listing after the RPC_RESULT notification), and no live code.
#[test]
fn pairing_code_is_revoked_when_another_central_was_there_at_the_send() {
    for (case, after_send) in [
        (
            "another central connected",
            Ok(vec![PHONE.into(), OTHER.into()]),
        ),
        (
            "the BlueZ listing fails",
            Err("org.bluez.Error.Failed".to_string()),
        ),
    ] {
        let joined = joined(
            Some(PHONE),
            FakeCentrals::new(vec![Ok(vec![PHONE.into()]), after_send]),
            Duration::from_secs(2),
        );
        assert!(has_pair_url(&joined.packet), "{case}");
        assert_eq!(
            joined.calls,
            calls(&["list", "notify RPC_RESULT_UUID", "list"]),
            "{case}"
        );
        assert_eq!(joined.codes, 0, "{case}");
    }
}

/// A BlueZ that does not answer in time holds the result back no longer
/// than [`SOLE_CENTRAL_TIMEOUT`], gives the URL-only result and mints no
/// code; and the call is given up then (`on_bluez`), not left running on
/// the runtime holding the server, one more for every Wi-Fi request.
/// Expected values: the deadline `on_bluez` waits for and bounds the call
/// by; the fake records `answered 0` when its held listing answers.
#[test]
fn bluez_not_answering_in_time_gives_no_code_and_leaves_no_call_behind() {
    let hold = Duration::from_millis(600);
    let joined = joined(
        Some(PHONE),
        FakeCentrals {
            held: Some((0, hold)),
            ..FakeCentrals::new(listings(&[&[PHONE]]))
        },
        Duration::from_millis(200),
    );
    assert_eq!(joined.packet, url_only_result(BASE));
    assert_eq!(joined.codes, 0);
    assert_eq!(joined.calls, calls(&["list", "notify RPC_RESULT_UUID"]));
    // Past the time the held listing would have answered: it never did.
    std::thread::sleep(hold);
    assert_eq!(
        bluez_calls(&joined.server),
        calls(&["list", "notify RPC_RESULT_UUID"])
    );
}

/// A join that leaves the machine without an address has no URL to pair
/// against: no code is minted.
#[test]
fn approved_join_without_an_address_mints_no_code() {
    let (_dir, app, server, wifi) = server();
    *wifi.config.lock() = network_config(true, "HomeNet", &[]);
    server
        .write_request(RPC_COMMAND_UUID, &wifi_settings(b"HomeNet", b"x"), None)
        .expect("no exception");
    app.notifications
        .acknowledge(&wifi_question(&app).id, Some("Connect".into()));
    let pushed = pushed_after_write(&server, 4);
    assert_eq!(
        pushed[3],
        (RPC_RESULT_UUID.to_string(), vec![0x01, 0x00, 0x01])
    );
    assert_eq!(app.auth.live_codes(), 0);
}

/// Reject on the Dial: NOT_AUTHORIZED to the app, back to AUTHORIZED, and
/// the machine never touches the Wi-Fi.
#[test]
fn rejected_wifi_request_answers_not_authorized() {
    let (_dir, app, server, wifi) = server();
    server
        .write_request(RPC_COMMAND_UUID, &wifi_settings(b"HomeNet", b"x"), None)
        .expect("no exception");
    let question = wifi_question(&app);
    assert_eq!(
        question.message,
        "A device is requesting connection to HomeNet"
    );

    app.notifications
        .acknowledge(&question.id, Some("Reject".into()));

    assert_eq!(
        pushed_after_write(&server, 2),
        vec![error(0x04), status(0x02)]
    );
    assert!(wifi.credentials.lock().is_empty());
    assert_eq!(server.read_request(ERROR_UUID).expect("read"), vec![0x04]);
    assert_eq!(server.read_request(STATUS_UUID).expect("read"), vec![0x02]);
}

/// Nobody answers within the timeout (WIFI_APPROVAL_TIMEOUT, 60 s): the
/// same as Reject, and the question on the Dial turns into a notice.
#[test]
fn unanswered_wifi_request_times_out_as_rejected() {
    let (_dir, app, server, wifi) = server();
    server.approval_timeout_ms.store(20, Ordering::Relaxed);
    server
        .write_request(RPC_COMMAND_UUID, &wifi_settings(b"HomeNet", b"x"), None)
        .expect("no exception");
    let question = wifi_question(&app);

    assert_eq!(
        pushed_after_write(&server, 2),
        vec![error(0x04), status(0x02)]
    );
    assert!(wifi.credentials.lock().is_empty());
    let notice = wifi_question(&app);
    assert_eq!(notice.id, question.id);
    assert_eq!(notice.responses, ["Ok"]);
    assert_eq!(
        notice.message,
        "A device was not allowed to connect to HomeNet: nobody answered in time"
    );
}

/// An approved join that fails notifies ERROR with UNABLE_TO_CONNECT.
#[test]
fn failed_join_notifies_the_error() {
    let (_dir, app, server, wifi) = server();
    *wifi.connect_result.lock() = Ok(false);
    server
        .write_request(RPC_COMMAND_UUID, &wifi_settings(b"X", b"y"), None)
        .expect("no exception");
    let question = wifi_question(&app);
    app.notifications
        .acknowledge(&question.id, Some("Connect".into()));
    assert_eq!(
        pushed_after_write(&server, 4),
        vec![status(0x02), status(0x03), error(0x03), status(0x02)]
    );
    assert_eq!(server.read_request(ERROR_UUID).expect("read"), vec![0x03]);
}

/// The question names the network, but the log redactor only knows saved
/// networks: the notification's text stays out of the log.
#[test]
fn wifi_question_does_not_log_the_ssid() {
    let (_dir, _app, server, _wifi) = server();
    let ssid = b"SENTINEL-PRIVATE-NETWORK";
    let (_result, logged) =
        logged(|| server.write_request(RPC_COMMAND_UUID, &wifi_settings(ssid, b"pw"), None));
    assert!(!logged.contains("SENTINEL-PRIVATE-NETWORK"));
    assert!(logged.contains("message not logged"));
}

/// GET_WIFI_NETWORKS answers with every scanned SSID, empty ones included.
#[test]
fn network_list_rpc() {
    let (_dir, _app, server, _wifi) = server();
    let notifications = server
        .write_request(RPC_COMMAND_UUID, &[0x04], None)
        .expect("no exception");
    let packet = &notifications[0].1;
    assert_eq!(&packet[..2], &[0x04, 9]);
    assert_eq!(&packet[2..11], b"\x07HomeNet\x00");
}

/// MACHINE_IDENT: the comma-separated identity; PARITY: without an active
/// connection the read fails (`str + None`).
#[test]
fn machine_ident() {
    let (_dir, _app, server, wifi) = server();
    assert_eq!(
        String::from_utf8(server.read_request(MACHINE_IDENT_UUID).expect("read")).expect("utf8"),
        "MeticulousEspresso,meticulous,black,v10.1.0,103,1,MyWiFi,192.168.1.100"
    );
    let mut config = network_config(false, "", &[]);
    config.connection_name = None;
    *wifi.config.lock() = config;
    let error = server.read_request(MACHINE_IDENT_UUID).unwrap_err();
    assert_eq!(error.kind, "TypeError");
}

/// Manufacturer data: 2 in hotspot mode, 1 connected, 0 neither, then the
/// IPv4 addresses.
#[test]
fn manufacturer_data() {
    let (_dir, app, server, wifi) = server();
    app.config.set(keys::WIFI, keys::WIFI_MODE, json!("CLIENT"));
    assert_eq!(
        server.build_manufacturer_data().expect("data"),
        vec![0x01, 192, 168, 1, 100]
    );
    *wifi.config.lock() = network_config(
        true,
        "Net",
        &["192.168.1.100/24", "fe80::1/64", "10.0.0.2/8"],
    );
    app.config.set(keys::WIFI, keys::WIFI_MODE, json!("AP"));
    assert_eq!(
        server.build_manufacturer_data().expect("data"),
        vec![0x02, 192, 168, 1, 100, 10, 0, 0, 2]
    );
    app.config.set(keys::WIFI, keys::WIFI_MODE, json!("CLIENT"));
    *wifi.config.lock() = network_config(false, "", &[]);
    assert_eq!(server.build_manufacturer_data().expect("data"), vec![0x00]);
}

/// Subscriptions count as connected clients; an unknown unsubscribe fails
/// like `list.remove`.
#[test]
fn subscriptions() {
    let (_dir, _app, server, _wifi) = server();
    assert!(!server.check_client_connections());
    server
        .created
        .store(true, std::sync::atomic::Ordering::Relaxed);
    server.start_notify(STATUS_UUID);
    assert!(server.check_client_connections());
    assert!(server.stop_notify(STATUS_UUID).is_ok());
    assert!(server.stop_notify(STATUS_UUID).is_err());
    assert!(!server.check_client_connections());
}

/// IDENTIFY shows the writing phone's name on the Dial, once per machine.
#[test]
fn identify_notifies_with_the_client_name() {
    let (_dir, app, server, _wifi) = server();
    let phone = BleClient {
        name: Some("  Ana's\u{7}iPhone ".into()),
        ..BleClient::default()
    };
    let packets = server
        .write_request(RPC_COMMAND_UUID, &[0x02], Some(&phone))
        .expect("no exception");
    assert_eq!(
        packets,
        vec![(RPC_RESULT_UUID.to_string(), vec![0x02, 0x00, 0x02])]
    );
    let shown = app.notifications.unacknowledged();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].id, IDENTIFY_NOTIFICATION_ID);
    assert_eq!(shown[0].responses, ["Ok"]);
    assert_eq!(
        shown[0].message,
        "\u{201c}Ana'siPhone\u{201d} is connected to this machine over Bluetooth"
    );

    // Again, and from a client without a name: still one notification.
    server
        .write_request(RPC_COMMAND_UUID, &[0x02], Some(&BleClient::default()))
        .expect("no exception");
    server
        .write_request(RPC_COMMAND_UUID, &[0x02], None)
        .expect("no exception");
    let shown = app.notifications.unacknowledged();
    assert_eq!(shown.len(), 1);
    assert_eq!(
        shown[0].message,
        "A device is connected to this machine over Bluetooth"
    );
}

#[test]
fn long_client_names_are_capped() {
    let name = "x".repeat(MAX_CLIENT_NAME_CHARS + 10);
    let shown = BleClient {
        name: Some(name),
        ..BleClient::default()
    }
    .display_name()
    .expect("name");
    // The ellipsis counts: a token carries the name and holds 48 at most.
    assert_eq!(shown.chars().count(), MAX_CLIENT_NAME_CHARS);
    assert!(shown.ends_with('…'));
    assert_eq!(
        BleClient {
            name: Some(" \t".into()),
            ..BleClient::default()
        }
        .display_name(),
        None
    );
}
