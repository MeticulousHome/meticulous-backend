//! tests/test_api_wifi.py: the real handlers drive the real Wi-Fi manager;
//! only NetworkManager, the subprocesses, DNS, the internet probe, the clock,
//! background threads, zeroconf and the BLE advertisement are replaced
//! (`crate::wifi::fake`).

use std::sync::Arc;

use http::Method;
use serde_json::{Value, json};

use crate::app::App;
use crate::config::{WIFI_MODE_AP, WIFI_MODE_CLIENT, keys};
use crate::env::BackendMode;
use crate::http::Reply;
use crate::py::PyError;
use crate::testing::{app_with, call, json};
use crate::wifi::fake::{
    AP_NAME, AP_PASSWORD, Call, HOME_ROUTE, HOSTNAME, MAC, STATION, WifiFixture, scan_entry, wpa2,
};
use crate::wifi::types::AP_CONNECTION;

/// The Python test builds an `Application` of the real handlers, which on a
/// machine (not the emulator) are always the ones registered.
fn setup() -> (tempfile::TempDir, Arc<App>, WifiFixture) {
    let (dir, app) = app_with(BackendMode::Fika, |_| {});
    let fixture = WifiFixture::install(&app);
    (dir, app, fixture)
}

async fn get(app: &Arc<App>, path: &str) -> Reply {
    call(app, Method::GET, path, "").await
}

async fn post(app: &Arc<App>, path: &str, body: Value) -> Reply {
    call(app, Method::POST, path, body.to_string()).await
}

async fn post_raw(app: &Arc<App>, path: &str, body: &'static str) -> Reply {
    call(app, Method::POST, path, body).await
}

fn body_text(reply: &Reply) -> String {
    String::from_utf8_lossy(reply.body()).into_owned()
}

fn health_json(overrides: Value) -> Value {
    let mut health = json!({
        "mode": WIFI_MODE_CLIENT,
        "link_connected": true,
        "has_ipv4": true,
        "gateway_reachable": false,
        "dns_resolves": false,
        "internet_reachable": false,
        "ap_active": false,
        "degraded": false,
        "verified": false,
        "last_error": "",
        "message": "",
        "last_recovery_action": "",
        "last_recovery_result": "not_needed",
    });
    if let (Value::Object(base), Value::Object(changes)) = (&mut health, overrides) {
        for (k, v) in changes {
            base.insert(k, v);
        }
    }
    health
}

fn hotspot_health() -> Value {
    health_json(json!({
        "mode": WIFI_MODE_AP,
        "ap_active": true,
        "verified": true,
        "message": "Hotspot active. Connect your phone or computer to the machine's Wi-Fi network.",
    }))
}

fn home_status() -> Value {
    json!({
        "connected": true,
        "connection_name": "HomeNet",
        "gateway": "192.168.1.1",
        "routes": [HOME_ROUTE],
        "ips": ["192.168.1.42"],
        "dns": ["192.168.1.1"],
        "mac": MAC,
        "hostname": HOSTNAME,
    })
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn type_lookup(name: &str) -> Vec<String> {
    strings(&[
        "nmcli",
        "-g",
        "802-11-wireless-security.key-mgmt",
        "connection",
        "show",
        name,
    ])
}

fn hotspot_commands(channel: &str) -> Vec<Vec<String>> {
    vec![
        strings(&[
            "nmcli",
            "connection",
            "add",
            "type",
            "wifi",
            "ifname",
            STATION,
            "con-name",
            AP_CONNECTION,
            "ssid",
            AP_NAME,
        ]),
        strings(&[
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
            channel,
            "802-11-wireless.ap-isolation",
            "0",
            "802-11-wireless.powersave",
            "2",
            "802-11-wireless-security.key-mgmt",
            "wpa-psk",
            "802-11-wireless-security.auth-alg",
            "open",
            "802-11-wireless-security.psk",
            AP_PASSWORD,
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
        ]),
        strings(&["nmcli", "--wait", "35", "connection", "up", AP_CONNECTION]),
    ]
}

// ---- GET /wifi/config ----

/// test_api_wifi.py::TestWifiConfigGet::test_client_mode_reports_config_status_shallow_health_and_saved_networks
#[tokio::test]
async fn client_mode_reports_config_status_shallow_health_and_saved_networks() {
    let (_dir, app, f) = setup();
    f.connect_home();
    f.network.add_connection("CafeOpen", "wifi");
    f.network.add_connection("Wired connection 1", "ethernet");
    f.network.add_connection(AP_CONNECTION, "wifi");
    {
        let mut state = f.network.state.lock();
        state
            .saved_key_mgmt
            .insert("HomeNet".into(), "wpa-psk".into());
        state.saved_key_mgmt.insert("CafeOpen".into(), "".into());
    }

    let reply = get(&app, "/api/v1/wifi/config").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        json(&reply),
        json!({
            "config": {"mode": WIFI_MODE_CLIENT, "apName": AP_NAME, "apPassword": AP_PASSWORD},
            "status": home_status(),
            "health": health_json(json!({})),
            "known_wifis": {
                "HomeNet": {"ssid": "HomeNet", "type": "PSK"},
                "CafeOpen": {"ssid": "CafeOpen", "type": "OPEN"},
            },
        })
    );
    // The shallow answer never probes inline; it schedules a refresh.
    assert_eq!(f.network.threads(), vec!["WifiHealthRefresh"]);
    assert_eq!(f.network.state.lock().internet_calls, 0);
    assert_eq!(
        f.network.commands(),
        vec![type_lookup("HomeNet"), type_lookup("CafeOpen")]
    );
}

/// test_api_wifi.py::TestWifiConfigGet::test_hotspot_mode_reports_the_active_hotspot
#[tokio::test]
async fn hotspot_mode_reports_the_active_hotspot() {
    let (_dir, app, f) = setup();
    f.start_in_hotspot_mode(&app);

    let body = json(&get(&app, "/api/v1/wifi/config").await);

    assert_eq!(
        body["config"],
        json!({"mode": WIFI_MODE_AP, "apName": AP_NAME, "apPassword": AP_PASSWORD})
    );
    assert_eq!(
        body["status"],
        json!({
            "connected": true,
            "connection_name": AP_CONNECTION,
            "gateway": "",
            "routes": [],
            "ips": ["10.42.0.1"],
            "dns": [],
            "mac": MAC,
            "hostname": HOSTNAME,
        })
    );
    assert_eq!(body["health"], hotspot_health());
    assert_eq!(body["known_wifis"], json!({}));
}

/// test_api_wifi.py::TestWifiConfigGet::test_disconnected_client_reports_not_connected_health
#[tokio::test]
async fn disconnected_client_reports_not_connected_health() {
    let (_dir, app, _f) = setup();

    let body = json(&get(&app, "/api/v1/wifi/config").await);

    assert_eq!(
        body["status"],
        json!({
            "connected": false,
            "connection_name": null,
            "gateway": "",
            "routes": [],
            "ips": [],
            "dns": [],
            "mac": MAC,
            "hostname": HOSTNAME,
        })
    );
    assert_eq!(
        body["health"],
        health_json(json!({
            "link_connected": false,
            "has_ipv4": false,
            "degraded": true,
            "verified": true,
            "last_error": "wifi_not_connected",
            "message": "The machine is not connected to Wi-Fi.",
        }))
    );
}

/// test_api_wifi.py::TestWifiConfigGet::test_legacy_saved_password_is_migrated_into_network_manager_and_scrubbed
#[tokio::test]
async fn legacy_saved_password_is_migrated_into_network_manager_and_scrubbed() {
    let (_dir, app, f) = setup();
    app.config.set(
        keys::WIFI,
        keys::WIFI_KNOWN_WIFIS,
        json!({"OldNet": "legacy-pass"}),
    );

    let body = json(&get(&app, "/api/v1/wifi/config").await);

    assert_eq!(
        f.network.commands_except_type_lookups(),
        vec![
            strings(&[
                "nmcli",
                "connection",
                "add",
                "type",
                "wifi",
                "ifname",
                "*",
                "con-name",
                "OldNet",
                "ssid",
                "OldNet",
            ]),
            strings(&[
                "nmcli",
                "connection",
                "modify",
                "OldNet",
                "connection.autoconnect",
                "no",
                "802-11-wireless-security.key-mgmt",
                "wpa-psk",
                "802-11-wireless-security.psk",
                "legacy-pass",
            ]),
        ]
    );
    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_KNOWN_WIFIS),
        json!({"OldNet": {"ssid": "OldNet", "type": "PSK"}})
    );
    assert_eq!(f.saves.get(), 1);
    assert_eq!(
        body["known_wifis"],
        json!({"OldNet": {"ssid": "OldNet", "type": "PSK"}})
    );
}

/// GET /wifi/config when NetworkManager fails: tornado's 500.
#[tokio::test]
async fn config_get_with_failing_network_manager_is_a_500() {
    let (_dir, app, f) = setup();
    f.network.state.lock().devices.clear();
    app.wifi.set_host(Arc::new(BrokenNmcli(f.network.clone())));

    let reply = get(&app, "/api/v1/wifi/config").await;

    assert_eq!(reply.status_code(), 500);
}

/// A NetworkManager whose `device status` fails like a stopped daemon.
struct BrokenNmcli(Arc<crate::wifi::fake::FakeNetwork>);

impl crate::wifi::nmcli::Nmcli for BrokenNmcli {
    fn device_status(&self) -> crate::py::PyResult<Vec<crate::wifi::types::Device>> {
        Err(PyError::new(
            "NetworkManagerNotRunningException",
            "NetworkManager is not running",
        ))
    }
    fn device_show(&self, i: &str) -> crate::py::PyResult<crate::wifi::types::DeviceDetails> {
        self.0.device_show(i)
    }
    fn device_show_all(&self) -> crate::py::PyResult<Vec<crate::wifi::types::DeviceDetails>> {
        self.0.device_show_all()
    }
    fn device_wifi(&self, r: Option<bool>) -> crate::py::PyResult<Vec<crate::wifi::ScanEntry>> {
        self.0.device_wifi(r)
    }
    fn device_wifi_connect(&self, s: &str, p: Option<&str>) -> crate::py::PyResult<()> {
        self.0.device_wifi_connect(s, p)
    }
    fn connections(&self) -> crate::py::PyResult<Vec<crate::wifi::types::Connection>> {
        self.0.connections()
    }
    fn connection_up(&self, n: &str, w: Option<u32>) -> crate::py::PyResult<()> {
        self.0.connection_up(n, w)
    }
    fn connection_down(&self, n: &str) -> crate::py::PyResult<()> {
        self.0.connection_down(n)
    }
    fn connection_delete(&self, n: &str) -> crate::py::PyResult<()> {
        self.0.connection_delete(n)
    }
    fn connection_modify(&self, n: &str, o: &[(&str, &str)]) -> crate::py::PyResult<()> {
        self.0.connection_modify(n, o)
    }
}

impl crate::wifi::WifiHost for BrokenNmcli {
    fn nmcli(&self) -> &dyn crate::wifi::nmcli::Nmcli {
        self
    }
    fn run(
        &self,
        argv: &[String],
        timeout: Option<f64>,
    ) -> Result<crate::py::Completed, crate::py::RunError> {
        self.0.run(argv, timeout)
    }
    fn which(&self, name: &str) -> bool {
        self.0.which(name)
    }
    fn path_exists(&self, path: &str) -> bool {
        self.0.path_exists(path)
    }
    fn getaddrinfo(&self, host: &str, port: u16) -> Result<(), String> {
        self.0.getaddrinfo(host, port)
    }
    fn http_status(
        &self,
        url: &str,
        timeout: f64,
        agent: &str,
    ) -> Result<u16, crate::http_client::HttpError> {
        self.0.http_status(url, timeout, agent)
    }
    fn gethostname(&self) -> String {
        self.0.gethostname()
    }
    fn time(&self) -> f64 {
        self.0.time()
    }
    fn monotonic(&self) -> f64 {
        self.0.monotonic()
    }
    fn sleep(&self, seconds: f64) {
        self.0.sleep(seconds)
    }
    fn spawn(&self, name: &str, task: crate::wifi::host::Task) {
        self.0.spawn(name, task)
    }
}

// ---- POST /wifi/config ----

/// test_api_wifi.py::TestWifiConfigPost::test_new_ap_password_in_client_mode_is_saved_without_starting_the_hotspot
#[tokio::test]
async fn new_ap_password_in_client_mode_is_saved_without_starting_the_hotspot() {
    let (_dir, app, f) = setup();
    f.connect_home();
    f.network.state.lock().scan =
        vec![scan_entry("HomeNet", 70, "WPA2", true, "00:11:22:33:44:55")];

    let reply = post(
        &app,
        "/api/v1/wifi/config",
        json!({"apPassword": "n3w-Pa55word"}),
    )
    .await;

    assert_eq!(reply.status_code(), 200);
    let body = json(&reply);
    assert_eq!(
        body["config"],
        json!({"mode": WIFI_MODE_CLIENT, "apName": AP_NAME, "apPassword": "n3w-Pa55word"})
    );
    assert_eq!(body["status"], home_status());
    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_AP_PASSWORD),
        json!("n3w-Pa55word")
    );
    assert_eq!(f.saves.get(), 1);
    // Client mode: stop any hotspot, rescan once, re-announce.
    assert_eq!(f.network.calls(), vec![Call::DeviceWifi(Some(true))]);
    assert!(f.network.commands_except_type_lookups().is_empty());
    assert_eq!(f.restarts(), 1);
    assert_eq!(f.gatt.get(), 1);
}

/// test_api_wifi.py::TestWifiConfigPost::test_switching_to_hotspot_mode_creates_and_starts_the_access_point
#[tokio::test]
async fn switching_to_hotspot_mode_creates_and_starts_the_access_point() {
    let (_dir, app, f) = setup();
    f.connect_home();

    let reply = post(&app, "/api/v1/wifi/config", json!({"mode": WIFI_MODE_AP})).await;

    assert_eq!(reply.status_code(), 200);
    let body = json(&reply);
    assert_eq!(
        body["config"],
        json!({"mode": WIFI_MODE_AP, "apName": AP_NAME, "apPassword": AP_PASSWORD})
    );
    assert_eq!(body["status"]["connection_name"], json!(AP_CONNECTION));
    assert_eq!(body["health"], hotspot_health());
    assert_eq!(
        f.network.commands_except_type_lookups(),
        hotspot_commands("6")
    );
    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_MODE),
        json!(WIFI_MODE_AP)
    );
    assert_eq!(f.saves.get(), 1);
}

/// test_api_wifi.py::TestWifiConfigPost::test_failed_hotspot_start_rolls_back_to_client_mode_and_reports_why
///
/// An xfail in Python. PARITY: after the hotspot fails on every channel the
/// AP recovery raises `AttributeError` (`initial_health.connected`), so the
/// answer is `400 Failed to write config` and the mode stays AP in memory.
#[tokio::test]
async fn failed_hotspot_start_answers_failed_to_write_config() {
    let (_dir, app, f) = setup();
    f.connect_home();
    f.network.state.lock().failing_command_prefix = Some((
        strings(&[
            "nmcli",
            "connection",
            "add",
            "type",
            "wifi",
            "ifname",
            STATION,
        ]),
        "Error: device busy".into(),
    ));

    let reply = post(&app, "/api/v1/wifi/config", json!({"mode": WIFI_MODE_AP})).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(body_text(&reply), "Failed to write config");
    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_MODE),
        json!(WIFI_MODE_AP)
    );
    assert_eq!(
        app.wifi.last_health_error(),
        "hotspot_start_failed: channel=11: Error: device busy"
    );
    assert_eq!(f.saves.get(), 0);
}

/// The same request once `repairWifiConnection` reads `link_connected`:
/// what the xfail test expects. The AP recovery cannot start the hotspot
/// either, so the settings roll back to client mode.
#[tokio::test]
async fn failed_hotspot_start_rolls_back_to_client_mode_once_repair_works() {
    let (_dir, app, f) = setup();
    app.wifi.set_health_has_no_connected(false);
    f.connect_home();
    f.network.state.lock().failing_command_prefix = Some((
        strings(&[
            "nmcli",
            "connection",
            "add",
            "type",
            "wifi",
            "ifname",
            STATION,
        ]),
        "Error: device busy".into(),
    ));

    let reply = post(&app, "/api/v1/wifi/config", json!({"mode": WIFI_MODE_AP})).await;

    assert_eq!(reply.status_code(), 400);
    let body = json(&reply);
    assert_eq!(body["status"], json!("error"));
    assert_eq!(
        body["error"],
        json!(
            "The machine could not start its Wi-Fi hotspot. Please try again. If this keeps happening, restart the machine and try again."
        )
    );
    assert_eq!(
        body["code"],
        json!("hotspot_start_failed: channel=11: Error: device busy")
    );
    assert_eq!(body["config"]["config"]["mode"], json!(WIFI_MODE_CLIENT));
    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_MODE),
        json!(WIFI_MODE_CLIENT)
    );
}

/// test_api_wifi.py::TestWifiConfigPost::test_failed_hotspot_stop_rolls_back_to_hotspot_mode
#[tokio::test]
async fn failed_hotspot_stop_rolls_back_to_hotspot_mode() {
    let (_dir, app, f) = setup();
    f.start_in_hotspot_mode(&app);
    {
        let mut state = f.network.state.lock();
        state.down_error = Some(PyError::new(
            "RuntimeError",
            "Error: Connection deactivation failed",
        ));
        state.scan = vec![wpa2("HomeNet", 70)];
    }

    let reply = post(
        &app,
        "/api/v1/wifi/config",
        json!({"mode": WIFI_MODE_CLIENT}),
    )
    .await;

    assert_eq!(reply.status_code(), 400);
    let body = json(&reply);
    // The rollback restarts the hotspot, which clears hotspot_stop_failed,
    // so the client only sees the generic fallback.
    assert_eq!(body["status"], json!("error"));
    assert_eq!(body["error"], json!("Failed to apply Wi-Fi settings."));
    assert_eq!(body["code"], json!(""));
    assert_eq!(body["config"]["config"]["mode"], json!(WIFI_MODE_AP));
    assert_eq!(
        body["config"]["status"]["connection_name"],
        json!(AP_CONNECTION)
    );
    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_MODE),
        json!(WIFI_MODE_AP)
    );
    assert!(
        f.network
            .calls()
            .contains(&Call::Down(AP_CONNECTION.into()))
    );
    assert_eq!(
        f.network.commands_except_type_lookups(),
        hotspot_commands("6")
    );
    assert_eq!(f.saves.get(), 1);
}

/// test_api_wifi.py::TestWifiConfigPost::test_unknown_mode_changes_nothing_and_returns_the_current_config
#[tokio::test]
async fn unknown_mode_changes_nothing_and_returns_the_current_config() {
    let (_dir, app, f) = setup();
    f.connect_home();

    let reply = post(&app, "/api/v1/wifi/config", json!({"mode": "mesh"})).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(json(&reply)["config"]["mode"], json!(WIFI_MODE_CLIENT));
    assert!(f.network.calls().is_empty());
    assert_eq!(f.saves.get(), 0);
}

/// test_api_wifi.py::TestWifiConfigPost::test_invalid_json_is_rejected
#[tokio::test]
async fn config_post_invalid_json_is_rejected() {
    let (_dir, app, f) = setup();

    let reply = post_raw(&app, "/api/v1/wifi/config", "{mode").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(reply.body(), b"Invalid JSON");
    assert_eq!(f.saves.get(), 0);
}

/// Python's `"mode" in data` on bodies that are not objects.
#[tokio::test]
async fn config_post_non_object_bodies() {
    let (_dir, app, _f) = setup();
    for (body, status) in [("[1]", 200), ("null", 400), ("5", 400), ("\"mode\"", 400)] {
        let reply = call(&app, Method::POST, "/api/v1/wifi/config", body).await;
        assert_eq!(reply.status_code(), status, "{body}");
        if status == 400 {
            assert_eq!(body_text(&reply), "Failed to write config");
        }
    }
}

// ---- /wifi/config/qr.png ----

async fn fetch_qr(app: &Arc<App>) -> Reply {
    let reply = get(app, "/api/v1/wifi/config/qr.png").await;
    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.header("Content-Type"), Some("image/png"));
    assert!(reply.body().starts_with(b"\x89PNG\r\n\x1a\n"));
    reply
}

/// test_api_wifi.py::TestWifiQrCode::test_hotspot_qr_encodes_join_credentials_with_escaped_values
#[tokio::test]
async fn hotspot_qr_encodes_join_credentials_with_escaped_values() {
    let (_dir, app, f) = setup();
    f.start_in_hotspot_mode(&app);
    app.config
        .set(keys::WIFI, keys::WIFI_AP_NAME, json!("Meti;cu,lous\""));
    app.config
        .set(keys::WIFI, keys::WIFI_AP_PASSWORD, json!("p:a\\ss"));

    fetch_qr(&app).await;

    assert_eq!(
        super::qr_contents(&app).expect("contents"),
        "WIFI:S:Meti\\;cu\\,lous\\\";T:WPA2;P:p\\:a\\\\ss;H:false;;"
    );
}

/// test_api_wifi.py::TestWifiQrCode::test_client_qr_encodes_the_machine_url_on_its_ipv4_address
#[tokio::test]
async fn client_qr_encodes_the_machine_url_on_its_ipv4_address() {
    let (_dir, app, f) = setup();
    f.connect_home();

    fetch_qr(&app).await;

    assert_eq!(
        super::qr_contents(&app).expect("contents"),
        format!("http://192.168.1.42:{}", app.env.port)
    );
}

/// test_api_wifi.py::TestWifiQrCode::test_client_qr_brackets_an_ipv6_address
#[tokio::test]
async fn client_qr_brackets_an_ipv6_address() {
    let (_dir, app, f) = setup();
    f.network
        .connect_station("HomeNet", None, Some("192.168.1.1"), Some("fd00::42/64"));

    fetch_qr(&app).await;

    assert_eq!(
        super::qr_contents(&app).expect("contents"),
        format!("http://[fd00::42]:{}", app.env.port)
    );
}

/// test_api_wifi.py::TestWifiQrCode::test_qr_without_any_address_uses_the_mdns_hostname
#[tokio::test]
async fn qr_without_any_address_uses_the_mdns_hostname() {
    let (_dir, app, _f) = setup();

    fetch_qr(&app).await;

    assert_eq!(
        super::qr_contents(&app).expect("contents"),
        format!("http://{HOSTNAME}.local:{}", app.env.port)
    );
}

// ---- /wifi/list ----

/// test_api_wifi.py::TestWifiList::test_cached_scan_is_deduplicated_filtered_and_sorted_by_signal
#[tokio::test]
async fn cached_scan_is_deduplicated_filtered_and_sorted_by_signal() {
    let (_dir, app, f) = setup();
    app.wifi.set_scan_cache(vec![
        scan_entry("HomeNet", 80, "WPA2", false, "00:00:00:00:00:01"),
        scan_entry("HomeNet", 60, "WPA2", true, "00:00:00:00:00:02"),
        scan_entry("HomeNet", 90, "WPA2", false, "00:00:00:00:00:03"),
        scan_entry("CafeOpen", 70, "", false, "00:11:22:33:44:55"),
        scan_entry("Office", 40, "WPA2 802.1X", false, "00:11:22:33:44:55"),
        scan_entry("Office", 55, "WPA2 802.1X", false, "00:11:22:33:44:55"),
        scan_entry("Neighbor", 30, "WPA2 WPA3", false, "00:11:22:33:44:55"),
        scan_entry("OldRouter", 20, "WEP", false, "00:11:22:33:44:55"),
        scan_entry("", 99, "WPA2", false, "00:11:22:33:44:55"),
        scan_entry("Mystery", 50, "XYZ", false, "00:11:22:33:44:55"),
    ]);

    let reply = get(&app, "/api/v1/wifi/list").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        json(&reply),
        json!([
            {"type": "OPEN", "security": "", "ssid": "CafeOpen", "signal": 70, "rate": 130, "in_use": false},
            {"type": "PSK", "security": "WPA2", "ssid": "HomeNet", "signal": 60, "rate": 130, "in_use": true},
            {"type": "802.1X", "security": "WPA2 802.1X", "ssid": "Office", "signal": 55, "rate": 130, "in_use": false},
            {"type": "SAE", "security": "WPA2 WPA3", "ssid": "Neighbor", "signal": 30, "rate": 130, "in_use": false},
            {"type": "WEP", "security": "WEP", "ssid": "OldRouter", "signal": 20, "rate": 130, "in_use": false},
        ])
    );
    // `json.dumps` spacing, not tornado's dict encoding.
    assert!(body_text(&reply).starts_with("[{\"type\": \"OPEN\", \"security\": \"\""));
    // A cached answer never blocks; it schedules one background rescan.
    assert!(f.network.calls().is_empty());
    assert_eq!(f.network.threads(), vec!["WifiScanRefresh"]);
}

/// test_api_wifi.py::TestWifiList::test_cold_start_blocks_on_one_scan_instead_of_returning_nothing
#[tokio::test]
async fn cold_start_blocks_on_one_scan_instead_of_returning_nothing() {
    let (_dir, app, f) = setup();
    f.network.state.lock().scan = vec![wpa2("HomeNet", 65)];

    let reply = get(&app, "/api/v1/wifi/list").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        json(&reply),
        json!([{"type": "PSK", "security": "WPA2", "ssid": "HomeNet", "signal": 65, "rate": 130, "in_use": false}])
    );
    assert_eq!(f.network.calls(), vec![Call::DeviceWifi(Some(true))]);
    assert_eq!(app.wifi.scan_cache(), vec![wpa2("HomeNet", 65)]);
    assert!(f.network.threads().is_empty());
}

/// test_api_wifi.py::TestWifiList::test_cold_start_with_radio_not_ready_returns_an_empty_list
#[tokio::test]
async fn cold_start_with_radio_not_ready_returns_an_empty_list() {
    let (_dir, app, f) = setup();
    f.network.disconnect_station("unavailable");

    let reply = get(&app, "/api/v1/wifi/list").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(json(&reply), json!([]));
    assert!(f.network.calls().is_empty());
    assert_eq!(app.wifi.last_health_error(), "wifi_device_unavailable");
}

/// test_api_wifi.py::TestWifiList::test_scan_failure_answers_a_single_json_error
///
/// An xfail in Python, reachable only by mocking `getAvailableNetworks` to
/// raise: the scan path catches every NetworkManager error. A failing scan
/// answers an empty list.
#[tokio::test]
async fn scan_failure_answers_an_empty_list() {
    let (_dir, app, f) = setup();
    f.network.state.lock().scan_error = Some(PyError::new(
        "UnspecifiedException",
        "NetworkManager is not running",
    ));

    let reply = get(&app, "/api/v1/wifi/list").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(body_text(&reply), "[]");
}

// ---- /wifi/connect ----

fn seed_connect_scan(app: &App) {
    app.wifi.set_scan_cache(vec![
        wpa2("HomeNet", 70),
        scan_entry("CafeOpen", 60, "", false, "00:11:22:33:44:55"),
        scan_entry("Office", 50, "WPA2 802.1X", false, "00:11:22:33:44:55"),
    ]);
}

/// test_api_wifi.py::TestWifiConnect::test_new_network_is_joined_through_network_manager
#[tokio::test]
async fn new_network_is_joined_through_network_manager() {
    let (_dir, app, f) = setup();
    seed_connect_scan(&app);

    let reply = post(
        &app,
        "/api/v1/wifi/connect",
        json!({"ssid": "HomeNet", "password": "hunter22", "type": "PSK"}),
    )
    .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        json(&reply),
        json!({"status": "ok", "health": health_json(json!({}))})
    );
    assert_eq!(
        f.network.calls(),
        vec![Call::WifiConnect("HomeNet".into(), Some("hunter22".into()))]
    );
    assert_eq!(app.wifi.auto_connect_suppression_remaining(), 45);
    assert_eq!(f.restarts(), 1);
    assert_eq!(
        f.network.threads(),
        vec!["WifiScanRefresh", "WifiHealthRefresh", "WifiHealthRefresh"]
    );
    // Already in client mode and no legacy entry: nothing to persist.
    assert_eq!(f.saves.get(), 0);
}

/// test_api_wifi.py::TestWifiConnect::test_already_connected_network_is_not_rejoined
#[tokio::test]
async fn already_connected_network_is_not_rejoined() {
    let (_dir, app, f) = setup();
    seed_connect_scan(&app);
    f.connect_home();

    let reply = post(
        &app,
        "/api/v1/wifi/connect",
        json!({"ssid": "HomeNet", "password": "hunter22"}),
    )
    .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        json(&reply),
        json!({"status": "ok", "health": health_json(json!({}))})
    );
    assert!(f.network.calls().is_empty());
    assert_eq!(f.restarts(), 1);
    assert_eq!(f.gatt.get(), 1);
}

/// test_api_wifi.py::TestWifiConnect::test_joining_from_hotspot_mode_persists_client_mode
#[tokio::test]
async fn joining_from_hotspot_mode_persists_client_mode() {
    let (_dir, app, f) = setup();
    seed_connect_scan(&app);
    f.start_in_hotspot_mode(&app);

    let reply = post(
        &app,
        "/api/v1/wifi/connect",
        json!({"ssid": "HomeNet", "password": "hunter22"}),
    )
    .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        f.network.calls(),
        vec![Call::WifiConnect("HomeNet".into(), Some("hunter22".into()))]
    );
    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_MODE),
        json!(WIFI_MODE_CLIENT)
    );
    assert_eq!(f.saves.get(), 1);
}

/// test_api_wifi.py::TestWifiConnect::test_open_network_is_joined_without_password
#[tokio::test]
async fn open_network_is_joined_without_password() {
    let (_dir, app, f) = setup();
    seed_connect_scan(&app);

    let reply = post(
        &app,
        "/api/v1/wifi/connect",
        json!({"ssid": "CafeOpen", "type": "OPEN"}),
    )
    .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        f.network.calls(),
        vec![Call::WifiConnect("CafeOpen".into(), None)]
    );
}

/// test_api_wifi.py::TestWifiConnect::test_saved_network_without_password_is_brought_up_from_its_profile
#[tokio::test]
async fn saved_network_without_password_is_brought_up_from_its_profile() {
    let (_dir, app, f) = setup();
    seed_connect_scan(&app);
    f.network.add_connection("HomeNet", "wifi");

    let reply = post(&app, "/api/v1/wifi/connect", json!({"ssid": "HomeNet"})).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        f.network.calls(),
        vec![Call::Up("HomeNet".into(), Some(12))]
    );
}

/// test_api_wifi.py::TestWifiConnect::test_wrong_password_is_reported_as_invalid_credentials
#[tokio::test]
async fn wrong_password_is_reported_as_invalid_credentials() {
    let (_dir, app, f) = setup();
    seed_connect_scan(&app);
    f.network.state.lock().join_error = Some(PyError::exception(
        "Error: Connection activation failed: Secrets were required, but not provided.",
    ));

    let reply = post(
        &app,
        "/api/v1/wifi/connect",
        json!({"ssid": "HomeNet", "password": "wrong"}),
    )
    .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": "Incorrect Wi-Fi password. Please check it and try again.",
            "code": "invalid_credentials",
        })
    );
    assert_eq!(f.gatt.get(), 1);
}

/// test_api_wifi.py::TestWifiConnect::test_network_that_disappeared_is_reported_as_not_found
#[tokio::test]
async fn network_that_disappeared_is_reported_as_not_found() {
    let (_dir, app, f) = setup();
    seed_connect_scan(&app);
    f.network.state.lock().join_error = Some(PyError::exception(
        "Error: No network with SSID 'HomeNet' found.",
    ));

    let reply = post(
        &app,
        "/api/v1/wifi/connect",
        json!({"ssid": "HomeNet", "password": "hunter22"}),
    )
    .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": "Wi-Fi network was not found. Move closer and try again.",
            "code": "network_not_found",
        })
    );
}

/// test_api_wifi.py::TestWifiConnect::test_join_that_lands_on_another_network_is_reported_as_preempted
#[tokio::test]
async fn join_that_lands_on_another_network_is_reported_as_preempted() {
    let (_dir, app, f) = setup();
    seed_connect_scan(&app);
    f.network.state.lock().join_redirect = Some("OtherNet".into());

    let reply = post(
        &app,
        "/api/v1/wifi/connect",
        json!({"ssid": "HomeNet", "password": "hunter22"}),
    )
    .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": "Connected to OtherNet instead of HomeNet.",
            "code": "connection_preempted",
        })
    );
}

/// test_api_wifi.py::TestWifiConnect::test_psk_network_without_password_or_saved_profile_is_rejected
#[tokio::test]
async fn psk_network_without_password_or_saved_profile_is_rejected() {
    let (_dir, app, f) = setup();
    seed_connect_scan(&app);

    let reply = post(
        &app,
        "/api/v1/wifi/connect",
        json!({"ssid": "HomeNet", "type": "PSK"}),
    )
    .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": "Wi-Fi password was not provided.",
            "code": "missing_credentials",
        })
    );
    assert!(f.network.calls().is_empty());
}

/// test_api_wifi.py::TestWifiConnect::test_enterprise_network_is_rejected_as_unsupported
#[tokio::test]
async fn enterprise_network_is_rejected_as_unsupported() {
    let (_dir, app, f) = setup();
    seed_connect_scan(&app);

    let reply = post(
        &app,
        "/api/v1/wifi/connect",
        json!({"ssid": "Office", "password": "x", "type": "802.1X"}),
    )
    .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": "Enterprise Wi-Fi networks are not supported yet.",
            "code": "unsupported_security",
        })
    );
    assert!(f.network.calls().is_empty());
}

/// test_api_wifi.py::TestWifiConnect::test_missing_ssid_is_rejected
#[tokio::test]
async fn missing_ssid_is_rejected() {
    let (_dir, app, _f) = setup();
    seed_connect_scan(&app);

    let reply = post(
        &app,
        "/api/v1/wifi/connect",
        json!({"password": "hunter22"}),
    )
    .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": "Wi-Fi network name was not provided.",
            "code": "missing_ssid",
        })
    );
}

/// test_api_wifi.py::TestWifiConnect::test_null_body_is_rejected_as_missing_credentials
#[tokio::test]
async fn null_body_is_rejected_as_missing_credentials() {
    let (_dir, app, _f) = setup();
    seed_connect_scan(&app);

    let reply = post_raw(&app, "/api/v1/wifi/connect", "null").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": "Wi-Fi credentials were not provided.",
            "code": "missing_credentials",
        })
    );
}

/// test_api_wifi.py::TestWifiConnect::test_invalid_json_is_rejected
#[tokio::test]
async fn connect_invalid_json_is_rejected() {
    let (_dir, app, _f) = setup();

    let reply = post_raw(&app, "/api/v1/wifi/connect", "{ssid").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({"status": "error", "error": "failed to connect to wifi: JSONDecodeError"})
    );
}

/// A list body: `credentials.to_dict()` raises.
#[tokio::test]
async fn connect_list_body_raises_attribute_error() {
    let (_dir, app, _f) = setup();

    let reply = post_raw(&app, "/api/v1/wifi/connect", "[\"HomeNet\"]").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({"status": "error", "error": "failed to connect to wifi: AttributeError"})
    );
}

/// test_api_wifi.py::TestWifiConnect::test_connect_without_networking_hardware_is_rejected
#[tokio::test]
async fn connect_without_networking_hardware_is_rejected() {
    let (_dir, app, f) = setup();
    app.wifi.set_networking_available(false);

    let reply = post(
        &app,
        "/api/v1/wifi/connect",
        json!({"ssid": "HomeNet", "password": "hunter22"}),
    )
    .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": "Wi-Fi hardware is not available.",
            "code": "networking_unavailable",
        })
    );
    assert!(f.network.calls().is_empty());
}

// ---- /wifi/delete ----

/// test_api_wifi.py::TestWifiDelete::test_saved_network_manager_profile_is_deleted
#[tokio::test]
async fn saved_network_manager_profile_is_deleted() {
    let (_dir, app, f) = setup();
    f.network.add_connection("HomeNet", "wifi");

    let reply = post(&app, "/api/v1/wifi/delete", json!({"ssid": "HomeNet"})).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(json(&reply), json!({"status": "ok"}));
    assert_eq!(f.network.calls(), vec![Call::Delete("HomeNet".into())]);
    assert_eq!(f.saves.get(), 0);
}

/// test_api_wifi.py::TestWifiDelete::test_legacy_config_entry_is_removed_and_saved
#[tokio::test]
async fn legacy_config_entry_is_removed_and_saved() {
    let (_dir, app, f) = setup();
    app.config.set(
        keys::WIFI,
        keys::WIFI_KNOWN_WIFIS,
        json!({
            "OldNet": {"ssid": "OldNet", "type": "PSK"},
            "HomeNet": {"ssid": "HomeNet", "type": "PSK"},
        }),
    );

    let reply = post(&app, "/api/v1/wifi/delete", json!({"ssid": "OldNet"})).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(json(&reply), json!({"status": "ok"}));
    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_KNOWN_WIFIS),
        json!({"HomeNet": {"ssid": "HomeNet", "type": "PSK"}})
    );
    assert_eq!(f.saves.get(), 1);
    assert!(f.network.calls().is_empty());
}

/// test_api_wifi.py::TestWifiDelete::test_unknown_network_is_rejected
#[tokio::test]
async fn unknown_network_is_rejected() {
    let (_dir, app, f) = setup();
    f.network.add_connection("HomeNet", "wifi");

    let reply = post(&app, "/api/v1/wifi/delete", json!({"ssid": "Ghost"})).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({"status": "error", "error": "failed to delete unknown wifi"})
    );
    assert!(f.network.calls().is_empty());
}

/// test_api_wifi.py::TestWifiDelete::test_the_hotspot_profile_cannot_be_deleted
#[tokio::test]
async fn the_hotspot_profile_cannot_be_deleted() {
    let (_dir, app, f) = setup();
    f.network.add_connection(AP_CONNECTION, "wifi");

    let reply = post(&app, "/api/v1/wifi/delete", json!({"ssid": AP_CONNECTION})).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({"status": "error", "error": "failed to delete unknown wifi"})
    );
    assert!(f.network.calls().is_empty());
}

/// test_api_wifi.py::TestWifiDelete::test_body_without_ssid_is_rejected
#[tokio::test]
async fn body_without_ssid_is_rejected() {
    let (_dir, app, _f) = setup();

    let reply = post(&app, "/api/v1/wifi/delete", json!({"name": "HomeNet"})).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({"status": "error", "error": "failed to delete wifi: KeyError"})
    );
}

// ---- /wifi/repair ----

async fn repair(app: &Arc<App>) -> Reply {
    call(app, Method::POST, "/api/v1/wifi/repair", "").await
}

/// test_api_wifi.py::TestWifiRepair::test_repair_without_networking_hardware_is_rejected
#[tokio::test]
async fn repair_without_networking_hardware_is_rejected() {
    let (_dir, app, _f) = setup();
    app.wifi.set_networking_available(false);

    let reply = repair(&app).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": "Wi-Fi hardware is not available.",
            "code": "networking_unavailable",
            "health": health_json(json!({
                "link_connected": false,
                "has_ipv4": false,
                "degraded": true,
                "verified": true,
                "last_error": "networking_unavailable",
                "message": "Wi-Fi hardware is not available.",
                "last_recovery_result": "failed",
            })),
        })
    );
}

/// test_api_wifi.py::TestWifiRepair::test_repair_without_any_saved_network_never_touches_the_radio
#[tokio::test]
async fn repair_without_any_saved_network_never_touches_the_radio() {
    let (_dir, app, f) = setup();

    let reply = repair(&app).await;

    let no_saved = "There is no saved Wi-Fi connection to repair. Choose a Wi-Fi network on the \
                    machine or connect it from the Meticulous app.";
    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": no_saved,
            "code": "no_saved_wifi_connection",
            "health": health_json(json!({
                "link_connected": false,
                "has_ipv4": false,
                "degraded": true,
                "verified": true,
                "last_error": "no_saved_wifi_connection",
                "message": no_saved,
                "last_recovery_action": "health_check",
                "last_recovery_result": "not_recoverable",
            })),
        })
    );
    assert!(f.network.calls().is_empty());
    assert!(f.network.commands().is_empty());
    assert!(!app.wifi.is_auto_connect_suppressed());
}

/// test_api_wifi.py::TestWifiRepair::test_repair_while_another_repair_runs_reports_generic_failure
#[tokio::test]
async fn repair_while_another_repair_runs_reports_generic_failure() {
    let (_dir, app, f) = setup();
    f.connect_home();
    app.wifi.set_repair_in_progress(true);

    let reply = repair(&app).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": "Wi-Fi repair could not complete.",
            "code": "wifi_repair_failed",
            "health": health_json(json!({
                "gateway_reachable": true,
                "dns_resolves": true,
                "internet_reachable": true,
                "verified": true,
                "last_recovery_result": "in_progress",
            })),
        })
    );
    // The deep health check pings the gateway and resolves DNS.
    assert_eq!(
        f.network.commands(),
        vec![strings(&["ping", "-c", "1", "-W", "2", "192.168.1.1"])]
    );
    assert_eq!(
        f.network.state.lock().dns_lookups,
        vec![("meticuloushome.com".to_string(), 443)]
    );
    assert!(f.network.calls().is_empty());
}

/// test_api_wifi.py::TestWifiRepair::test_repair_of_a_healthy_connection_reports_ok
///
/// An xfail in Python. PARITY: the repair reads `.connected` on the health
/// report and raises, so the answer is the handler's exception path.
#[tokio::test]
async fn repair_of_a_healthy_connection_raises_attribute_error() {
    let (_dir, app, f) = setup();
    f.connect_home();

    let reply = repair(&app).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply),
        json!({
            "status": "error",
            "error": "failed to repair wifi: 'WifiHealthStatus' object has no attribute 'connected'",
        })
    );
    assert!(f.network.calls().is_empty());
}

/// What the xfail test expects once the repair reads `link_connected`.
#[tokio::test]
async fn repair_of_a_healthy_connection_reports_ok_once_fixed() {
    let (_dir, app, f) = setup();
    app.wifi.set_health_has_no_connected(false);
    f.connect_home();

    let reply = repair(&app).await;

    assert_eq!(reply.status_code(), 200);
    let body = json(&reply);
    assert_eq!(body["status"], json!("ok"));
    assert_eq!(body["health"]["degraded"], json!(false));
    assert_eq!(body["health"]["verified"], json!(true));
    assert!(f.network.calls().is_empty());
}

/// test_api_wifi.py::TestWifiRepair::test_repair_restarts_a_connection_whose_gateway_stopped_answering
///
/// An xfail in Python: PARITY, the same `AttributeError` answer.
#[tokio::test]
async fn repair_of_a_degraded_connection_raises_attribute_error() {
    let (_dir, app, f) = setup();
    f.connect_home();
    f.network.state.lock().ping_returncodes = [1, 0].into();

    let reply = repair(&app).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        json(&reply)["error"],
        json!("failed to repair wifi: 'WifiHealthStatus' object has no attribute 'connected'")
    );
    assert!(f.network.calls().is_empty());
    // The `finally` still ran.
    assert_eq!(app.wifi.auto_connect_suppression_remaining(), 15);
}

/// What the xfail test expects once the repair reads `link_connected`:
/// restarting the connection brings the gateway back.
#[tokio::test]
async fn repair_restarts_a_connection_whose_gateway_stopped_answering_once_fixed() {
    let (_dir, app, f) = setup();
    app.wifi.set_health_has_no_connected(false);
    f.connect_home();
    f.network.state.lock().ping_returncodes = [1, 0].into();

    let reply = repair(&app).await;

    assert_eq!(reply.status_code(), 200);
    let body = json(&reply);
    assert_eq!(body["status"], json!("ok"));
    assert_eq!(body["health"]["degraded"], json!(false));
    assert_eq!(
        f.network.calls(),
        vec![
            Call::Down("HomeNet".into()),
            Call::Up("HomeNet".into(), Some(15))
        ]
    );
    assert_eq!(f.restarts(), 1);
}

/// The routes this module registers, as `api/wifi.py` does.
#[tokio::test]
async fn routes_are_registered() {
    let patterns: Vec<String> = super::routes().iter().map(|r| r.pattern.clone()).collect();
    assert_eq!(
        patterns,
        [
            "/wifi/config",
            "/wifi/config/qr.png",
            "/wifi/list",
            "/wifi/connect",
            "/wifi/repair",
            "/wifi/delete"
        ]
    );
}
