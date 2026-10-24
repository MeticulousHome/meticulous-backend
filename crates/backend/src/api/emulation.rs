//! `api/emulation.py`: canned Wi-Fi answers for the emulator when
//! NetworkManager is not reachable. They replace the real handlers of the
//! same paths; `/wifi/repair` and `/wifi/delete` stay real.
//!
//! Also `LEGACY_DUMMY_PROFILE`, which the profiles API serves in place of a
//! legacy JSON profile.

use std::sync::Arc;

use serde_json::{Value, json};

use super::wifi::json_body;
use crate::app::App;
use crate::config::{WIFI_MODE_AP, WIFI_MODE_CLIENT, keys};
use crate::http::{Kind, Reply, Request, Route};
use crate::py::{PyError, PyResult, contains, py_repr, subscript};

pub fn routes() -> Vec<Route> {
    let m = "api.emulation";
    vec![
        Route::new("/wifi/config", Kind::Base, m, "WiFiConfigHandler")
            .get(get_config)
            .post(post_config),
        Route::new("/wifi/list", Kind::Base, m, "WiFiListHandler").get(get_list),
        Route::new("/wifi/connect", Kind::Base, m, "WiFiConnectHandler").post(post_connect),
        Route::new(
            "/wifi/config/qr.png",
            Kind::Base,
            "api.wifi",
            "WiFiQRHandler",
        )
        .get(super::wifi::get_qr),
    ]
}

const PLACEHOLDER_WIFIS: [(&str, &str); 8] = [
    ("MeticulousWifi", "MeticulousWifiPassword"),
    ("Test123", "Password123"),
    ("Placeholder1", "Placeholder1"),
    ("Placeholder2", "Placeholder2"),
    ("Placeholder3", "Placeholder3"),
    ("Placeholder4", "Placeholder4"),
    ("Placeholder5", "Placeholder5"),
    ("Placeholder6", "Placeholder6"),
];

/// `WiFiConfigHandler.get()` of the emulation.
fn emulated_config(app: &App) -> PyResult<Value> {
    tracing::info!("serving emulated wifi config");
    // PARITY: the placeholders are written into the live settings (not
    // saved), so the next save persists them.
    let known = app.config.update(|tree| {
        let entry = tree
            .get_mut(keys::WIFI)
            .and_then(|w| w.get_mut(keys::WIFI_KNOWN_WIFIS));
        match entry {
            Some(Value::Object(known)) => {
                for (ssid, password) in PLACEHOLDER_WIFIS {
                    known.insert(ssid.into(), json!(password));
                }
                Ok(Value::Object(known.clone()))
            }
            Some(other) => Err(PyError::type_error(format!(
                "'{}' object does not support item assignment",
                crate::py::type_name(other)
            ))),
            None => Err(PyError::key_error(keys::WIFI_KNOWN_WIFIS)),
        }
    })?;
    Ok(json!({
        "config": {
            "mode": app.config.get(keys::WIFI, keys::WIFI_MODE),
            "apName": app.config.get(keys::WIFI, keys::WIFI_AP_NAME),
            "apPassword": app.config.get(keys::WIFI, keys::WIFI_AP_PASSWORD),
        },
        "status": {
            "connected": true,
            "connection_name": "MeticulousWifi",
            "gateway": "192.168.2.1",
            "routes": [
                "dst = 192.168.2.0/24, nh = 0.0.0.0, mt = 600",
                "dst = 0.0.0.0/0, nh = 192.168.2.1, mt = 600",
                "dst = fe80::/64, nh = ::, mt = 1024",
                "dst = fd00:0:0:d00d::/64, nh = ::, mt = 600",
                "dst = 2003:fb:ef0c:e100::/64, nh = ::, mt = 600",
                "dst = fd00:0:0:d00d::/64, nh = fe80::3e37:12ff:fe90:92e5, mt = 605",
                "dst = 2003:fb:ef0c:e100::/56, nh = fe80::3e37:12ff:fe90:92e5, mt = 600",
                "dst = ::/0, nh = fe80::3e37:12ff:fe90:92e5, mt = 600",
            ],
            "ips": [
                "192.168.2.223",
                "2003:fb:ef0c:e100:54a5:91c1:7bd4:d23e",
                "fd00::d00d:269c:62bd:3a17:df38",
                "fe80::4e8:f348:8479:1afe",
            ],
            "dns": [
                "192.168.2.1",
                "fd00::d00d:3e37:12ff:fe90:92e5",
                "2003:fb:ef0c:e100:3e37:12ff:fe90:92e5",
            ],
            "mac": "C0:EE:40:A4:7D:A9",
            "hostname": "imx8mn-var-som",
        },
        "known_wifis": known,
    }))
}

async fn get_config(app: Arc<App>, req: Request) -> Reply {
    match emulated_config(&app) {
        Ok(config) => Reply::base().dumps(config),
        Err(error) => super::wifi::uncaught(&req, &error),
    }
}

fn text(status: u16, body: &str) -> Reply {
    let mut reply = Reply::base().status(status);
    reply.write_str(body);
    reply
}

async fn post_config(app: Arc<App>, req: Request) -> Reply {
    tracing::info!("serving emulated wifi config");
    let data = match json_body(&req) {
        Ok(data) => data,
        Err(error) if error.kind == "JSONDecodeError" => {
            tracing::warn!("Failed to parse passed JSON: {}", error.message);
            return text(400, "Invalid JSON");
        }
        Err(error) => return failed(&error),
    };
    let app_for_task = app.clone();
    let result = tokio::task::spawn_blocking(move || apply_emulated(&app_for_task, data))
        .await
        .unwrap_or_else(|e| Err(PyError::new("RuntimeError", e.to_string())));
    match result {
        Ok(config) => Reply::base().dumps(config),
        Err(error) => failed(&error),
    }
}

fn failed(error: &PyError) -> Reply {
    tracing::warn!(
        "Failed to accept passed config: {}: {}",
        error.kind,
        error.message
    );
    text(400, "Failed to write config")
}

fn apply_emulated(app: &App, mut data: Value) -> PyResult<Value> {
    if contains(&data, "mode")? {
        let mode = subscript(&data, "mode")?.clone();
        if matches!(mode.as_str(), Some(WIFI_MODE_AP | WIFI_MODE_CLIENT)) {
            tracing::warn!("Changing wifi mode");
            app.config.set(keys::WIFI, keys::WIFI_MODE, mode);
            app.config.save();
            app.wifi.reset_wifi_mode()?;
            remove_key(&mut data, "mode")?;
        }
    }
    if contains(&data, "apPassword")? {
        tracing::warn!("Changing wifi ap password");
        let password = subscript(&data, "apPassword")?.clone();
        app.config.set(keys::WIFI, keys::WIFI_AP_PASSWORD, password);
        app.config.save();
        app.wifi.reset_wifi_mode()?;
        remove_key(&mut data, "apPassword")?;
    }
    tracing::info!("Unused request entries: {}", py_repr(&data));
    emulated_config(app)
}

/// `del data[key]`.
fn remove_key(data: &mut Value, key: &str) -> PyResult<()> {
    match data {
        Value::Object(map) => {
            map.shift_remove(key);
            Ok(())
        }
        other => Err(PyError::type_error(format!(
            "'{}' object doesn't support item deletion",
            crate::py::type_name(other)
        ))),
    }
}

async fn get_list(_app: Arc<App>, _req: Request) -> Reply {
    tracing::info!("serving emulated wifi config");
    Reply::base().dumps(json!([
        {"ssid": "MeticulousWifi", "signal": 75, "rate": 195, "in_use": true},
        {"ssid": "StarGate Legacy", "signal": 59, "rate": 195, "in_use": true},
        {"ssid": "StarGate Enterprise", "signal": 57, "rate": 405, "in_use": true},
        {"ssid": "StarGate Picard", "signal": 57, "rate": 405, "in_use": true},
    ]))
}

async fn post_connect(_app: Arc<App>, req: Request) -> Reply {
    tracing::info!("serving emulated wifi config");
    let answer = json_body(&req).and_then(|data| {
        let ssid = subscript(&data, "ssid")?.clone();
        let password = subscript(&data, "password")?.clone();
        Ok(json!({"status": "ok", "ssid": ssid, "password": password}))
    });
    match answer {
        Ok(body) => Reply::base().json(body),
        Err(error) => text(400, &format!("Error: {}", error.message)),
    }
}

/// `LEGACY_DUMMY_PROFILE`.
#[allow(dead_code, reason = "served by the profiles API, ported separately")]
pub fn legacy_dummy_profile() -> Value {
    json!({
        "id": "0eddc1d9-aa2e-404b-a209-e7e76c9552bc",
        "name": "LEGACY JSON PLACEHOLDER",
        "author": "",
        "author_id": "00000000-0000-0000-0000-000000000000",
        "previous_authors": [{"name": "", "author_id": "", "profile_id": ""}],
        "temperature": 88,
        "final_weight": 36,
        "display": {
            "image": "/api/v1/profile/image/f98ff6e029e70ee9a99565796615d55c.png",
            "accentColor": "#42a59d",
        },
        "variables": [],
        "last_changed": 1723127046.5605135,
        "stages": [
            {
                "key": "206c654b-5766-4ebb-a4bc-dcafa5aacd8b",
                "name": "EMPTY FLOW STAGE",
                "type": "flow",
                "dynamics": {"points": [[0, 0]], "over": "time", "interpolation": "linear"},
                "exit_triggers": [],
                "limits": [],
            }
        ],
    })
}

#[cfg(test)]
mod tests;
