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
    let server = GattServer::new(wifi.clone(), app.hostname.clone(), app.config.clone(), 8080);
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
        vec!["http://192.168.1.100:8080", "http://[fe80::1]:8080"]
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

    let (_result, logged) = logged(|| server.write_request(RPC_COMMAND_UUID, payload));

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

/// A provisioning write notifies RPC_RESULT with the URL packets.
#[test]
fn provisioning_write_notifies_the_rpc_result() {
    let (_dir, _app, server, wifi) = server();

    let notifications = server
        .write_request(RPC_COMMAND_UUID, &wifi_settings(b"HomeNet", b"hunter22"))
        .expect("no exception");

    assert_eq!(wifi.last()["ssid"], json!("HomeNet"));
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].0, RPC_RESULT_UUID);
    let url = b"http://192.168.1.100:8080";
    let mut expected = vec![0x01, (url.len() + 1) as u8, url.len() as u8];
    expected.extend_from_slice(url);
    expected.push(checksum(&expected));
    assert_eq!(notifications[0].1, expected);
    // PARITY: reading STATUS re-authorizes, so it says AUTHORIZED (2) even
    // after provisioning.
    assert_eq!(server.read_request(STATUS_UUID).expect("read"), vec![0x02]);
    // RPC_RESULT reads back the first packet.
    assert_eq!(
        server.read_request(RPC_RESULT_UUID).expect("read"),
        notifications[0].1
    );
}

/// A failed join notifies ERROR with UNABLE_TO_CONNECT.
#[test]
fn failed_join_notifies_the_error() {
    let (_dir, _app, server, wifi) = server();
    *wifi.connect_result.lock() = Ok(false);
    let notifications = server
        .write_request(RPC_COMMAND_UUID, &wifi_settings(b"X", b"y"))
        .expect("no exception");
    assert_eq!(notifications, vec![(ERROR_UUID.to_string(), vec![0x03])]);
    assert_eq!(server.read_request(ERROR_UUID).expect("read"), vec![0x03]);
}

/// GET_WIFI_NETWORKS answers with every scanned SSID, empty ones included.
#[test]
fn network_list_rpc() {
    let (_dir, _app, server, _wifi) = server();
    let notifications = server
        .write_request(RPC_COMMAND_UUID, &[0x04])
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
