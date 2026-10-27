//! api/emulation.py: the emulator's canned Wi-Fi routes replace the real
//! ones only when NetworkManager is unavailable.

use std::sync::Arc;

use http::Method;
use serde_json::json;

use crate::app::App;
use crate::config::keys;
use crate::testing::{app, call, json};
use crate::wifi::fake::Counter;

fn emulated() -> (tempfile::TempDir, Arc<App>, Arc<Counter>) {
    let (dir, app) = app();
    app.wifi.set_networking_available(false);
    let saves = Arc::new(Counter::default());
    let counter = saves.clone();
    app.config.on_save(move || counter.bump());
    (dir, app, saves)
}

fn text(reply: &crate::http::Reply) -> String {
    String::from_utf8_lossy(reply.body()).into_owned()
}

/// `register_emulation_handlers()` replaces `/wifi/config`, `/wifi/list`,
/// `/wifi/connect` and the QR code; repair and delete stay real.
#[test]
fn emulation_routes_replace_the_real_ones_without_networking() {
    let (_dir, app, _saves) = emulated();
    let router = crate::api::router(&app);
    let handler = |pattern: &str| {
        router
            .routes()
            .iter()
            .find(|r| r.pattern == format!("/api/v1{pattern}"))
            .map(|r| (r.module, r.handler))
    };
    assert_eq!(
        handler("/wifi/config"),
        Some(("api.emulation", "WiFiConfigHandler"))
    );
    assert_eq!(
        handler("/wifi/list"),
        Some(("api.emulation", "WiFiListHandler"))
    );
    assert_eq!(
        handler("/wifi/connect"),
        Some(("api.emulation", "WiFiConnectHandler"))
    );
    assert_eq!(
        handler("/wifi/config/qr.png"),
        Some(("api.emulation", "WiFiQRHandler"))
    );
    assert_eq!(
        handler("/wifi/repair"),
        Some(("api.wifi", "WiFiRepairHandler"))
    );
    assert_eq!(
        handler("/wifi/delete"),
        Some(("api.wifi", "WiFiDeleteHandler"))
    );
    let count = router
        .routes()
        .iter()
        .filter(|r| r.pattern.starts_with("/api/v1/wifi/"))
        .count();
    assert_eq!(count, 6);

    app.wifi.set_networking_available(true);
    let router = crate::api::router(&app);
    let real = router
        .routes()
        .iter()
        .find(|r| r.pattern == "/api/v1/wifi/config")
        .map(|r| r.module);
    assert_eq!(real, Some("api.wifi"));
}

/// GET /wifi/config: the placeholders join the live (unsaved) settings.
#[tokio::test]
async fn emulated_config_lists_placeholder_networks() {
    let (_dir, app, saves) = emulated();
    app.config.set(
        keys::WIFI,
        keys::WIFI_KNOWN_WIFIS,
        json!({"Mine": {"ssid": "Mine", "type": "PSK"}}),
    );

    let reply = call(&app, Method::GET, "/api/v1/wifi/config", "").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.header("Content-Type"), Some("application/json"));
    let body = json(&reply);
    assert_eq!(body["status"]["connection_name"], json!("MeticulousWifi"));
    assert_eq!(body["status"]["hostname"], json!("imx8mn-var-som"));
    assert_eq!(
        body["known_wifis"]["Mine"],
        json!({"ssid": "Mine", "type": "PSK"})
    );
    assert_eq!(
        body["known_wifis"]["MeticulousWifi"],
        json!("MeticulousWifiPassword")
    );
    assert_eq!(body["known_wifis"].as_object().map(|m| m.len()), Some(9));
    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_KNOWN_WIFIS)["Placeholder6"],
        json!("Placeholder6")
    );
    assert_eq!(saves.get(), 0);
    assert!(text(&reply).starts_with("{\"config\": {\"mode\": "));
}

/// POST /wifi/config saves the mode and the password and answers the GET.
#[tokio::test]
async fn emulated_config_post_saves_and_answers_the_config() {
    let (_dir, app, saves) = emulated();

    let reply = call(
        &app,
        Method::POST,
        "/api/v1/wifi/config",
        r#"{"mode": "CLIENT", "apPassword": "12345678", "extra": 1}"#,
    )
    .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(json(&reply)["config"]["mode"], json!("CLIENT"));
    assert_eq!(json(&reply)["config"]["apPassword"], json!("12345678"));
    assert_eq!(saves.get(), 2);

    let invalid = call(&app, Method::POST, "/api/v1/wifi/config", "{mode").await;
    assert_eq!(invalid.status_code(), 400);
    assert_eq!(text(&invalid), "Invalid JSON");
}

/// GET /wifi/list: four canned networks.
#[tokio::test]
async fn emulated_list() {
    let (_dir, app, _saves) = emulated();
    let reply = call(&app, Method::GET, "/api/v1/wifi/list", "").await;
    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        json(&reply),
        json!([
            {"ssid": "MeticulousWifi", "signal": 75, "rate": 195, "in_use": true},
            {"ssid": "StarGate Legacy", "signal": 59, "rate": 195, "in_use": true},
            {"ssid": "StarGate Enterprise", "signal": 57, "rate": 405, "in_use": true},
            {"ssid": "StarGate Picard", "signal": 57, "rate": 405, "in_use": true},
        ])
    );
}

/// POST /wifi/connect echoes the credentials; a missing key is a text 400.
#[tokio::test]
async fn emulated_connect() {
    let (_dir, app, _saves) = emulated();
    let reply = call(
        &app,
        Method::POST,
        "/api/v1/wifi/connect",
        r#"{"ssid": "Home", "password": "pw"}"#,
    )
    .await;
    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        json(&reply),
        json!({"status": "ok", "ssid": "Home", "password": "pw"})
    );

    let missing = call(
        &app,
        Method::POST,
        "/api/v1/wifi/connect",
        r#"{"ssid": "Home"}"#,
    )
    .await;
    assert_eq!(missing.status_code(), 400);
    assert_eq!(text(&missing), "Error: 'password'");

    let invalid = call(&app, Method::POST, "/api/v1/wifi/connect", "").await;
    assert_eq!(invalid.status_code(), 400);
    assert_eq!(
        text(&invalid),
        "Error: Expecting value: line 1 column 1 (char 0)"
    );
}

/// The real repair route answers that there is no Wi-Fi hardware.
#[tokio::test]
async fn real_repair_route_without_networking() {
    let (_dir, app, _saves) = emulated();
    let reply = call(&app, Method::POST, "/api/v1/wifi/repair", "").await;
    assert_eq!(reply.status_code(), 400);
    assert_eq!(json(&reply)["code"], json!("networking_unavailable"));
    let qr = call(&app, Method::GET, "/api/v1/wifi/config/qr.png", "").await;
    assert_eq!(qr.status_code(), 200);
    assert_eq!(qr.header("Content-Type"), Some("image/png"));
}

/// `LEGACY_DUMMY_PROFILE` keeps its float and key order.
#[test]
fn legacy_dummy_profile() {
    let profile = super::legacy_dummy_profile();
    let text = pyfmt::dumps_default(&profile);
    assert!(text.starts_with(
        "{\"id\": \"0eddc1d9-aa2e-404b-a209-e7e76c9552bc\", \"name\": \"LEGACY JSON PLACEHOLDER\""
    ));
    assert!(text.contains("\"last_changed\": 1723127046.5605135"));
    assert_eq!(profile["stages"][0]["dynamics"]["points"], json!([[0, 0]]));
}
