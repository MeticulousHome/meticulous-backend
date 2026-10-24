//! The backend's external contracts, as the snapshots in `tests/contracts/`
//! record them (`tests/contracts/extract.py` read them from the Python
//! source): the HTTP routes and the methods each serves, the Socket.IO events,
//! the keys of the payloads clients read by name, and the default settings.
//!
//! `meticulous-backend contract` prints them; the test below compares them
//! with the snapshots, so a contract change shows up as a snapshot diff.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::api;
use crate::app::App;
use crate::config::{self, keys};

/// The events the server handles (`backend.py`).
pub const LISTENS: [&str; 6] = ["action", "calibrate", "connect", "disconnect", "notification", "profileHover"];

/// Every event the server emits.
pub const EMITS: [&str; 10] = [
    "OSUpdate",
    "button",
    "heater_status",
    "notification",
    "profile",
    "profileHover",
    "sensors",
    "settings",
    "status",
    "upload_report",
];

/// `http_routes()`: every route with its handler, module and methods.
pub fn http_routes(app: &App) -> Value {
    let mut routes: Vec<(String, String, String, Vec<String>)> = api::contract_routes(app)
        .iter()
        .map(|route| {
            let mut methods = route.contract_methods();
            methods.sort_unstable();
            methods.dedup();
            (
                route.pattern.clone(),
                route.handler.to_string(),
                route.module.to_string(),
                methods,
            )
        })
        .filter(|(pattern, ..)| pattern.starts_with("/api/"))
        .collect();
    routes.sort_by(|a, b| (&a.0, &a.2).cmp(&(&b.0, &b.2)));
    Value::Array(
        routes
            .into_iter()
            .map(|(path, handler, module, methods)| {
                json!({"path": path, "handler": handler, "module": module, "methods": methods})
            })
            .collect(),
    )
}

pub fn socketio_events() -> Value {
    json!({"listens": LISTENS, "emits": EMITS})
}

fn sorted_keys(value: &Value) -> Vec<String> {
    let set: BTreeSet<String> = value.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    set.into_iter().collect()
}

/// `payload_keys()`.
pub fn payload_keys() -> Value {
    let status = esp::ShotData::default().to_sio();
    let mut status_keys: BTreeSet<String> = sorted_keys(&status).into_iter().collect();
    // live() adds the loaded profile's name and id to every status.
    status_keys.insert("loaded_profile".into());
    status_keys.insert("id".into());
    let button = esp::ButtonEventData::from_args(&["CW"]).map(|b| b.to_sio()).unwrap_or(Value::Null);
    json!({
        "status": status_keys.into_iter().collect::<Vec<_>>(),
        "status.sensors": sorted_keys(&status["sensors"]),
        "sensors": sorted_keys(&esp::SensorData::default().to_sio_sensors()),
        "button": sorted_keys(&button),
    })
}

/// `settings_defaults()`: every key, and the user section's default values.
pub fn settings_defaults() -> Value {
    let mut contract = Map::new();
    let defaults = config::defaults();
    for (section, values) in defaults.as_object().into_iter().flatten() {
        match values {
            Value::Object(map) if section == keys::USER => {
                let mut sorted: Vec<(&String, &Value)> = map.iter().collect();
                sorted.sort_by(|a, b| a.0.cmp(b.0));
                contract.insert(section.clone(), Value::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v.clone())).collect()));
            }
            Value::Object(_) => {
                contract.insert(section.clone(), json!(sorted_keys(values)));
            }
            other => {
                contract.insert(section.clone(), other.clone());
            }
        }
    }
    Value::Object(contract)
}

/// All four snapshots by file name.
pub fn snapshots(app: &App) -> Vec<(&'static str, Value)> {
    vec![
        ("http_routes.json", http_routes(app)),
        ("socketio_events.json", socketio_events()),
        ("socketio_payloads.json", payload_keys()),
        ("settings_defaults.json", settings_defaults()),
    ]
}

/// `json.dumps(value, indent=2) + "\n"`, how the snapshots are written.
pub fn render(value: &Value) -> String {
    format!("{}\n", pyfmt::dumps(value, &pyfmt::DumpOpts::indented(2)))
}

/// `meticulous-backend contract`: the snapshots as one JSON object.
pub fn print(app: &App) {
    let mut all = Map::new();
    for (name, value) in snapshots(app) {
        all.insert(name.to_string(), value);
    }
    println!("{}", pyfmt::dumps(&Value::Object(all), &pyfmt::DumpOpts::indented(2)));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tests/test_api_contracts.py: the contracts against the snapshots.
    /// `UPDATE_CONTRACTS=1 cargo test -p meticulous-backend contract` rewrites
    /// them; commit the diff with the change that caused it.
    #[test]
    fn contracts_match_their_snapshots() {
        let (_dir, app) = crate::testing::app();
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/contracts");
        let update = std::env::var("UPDATE_CONTRACTS").as_deref() == Ok("1");
        let mut differences = Vec::new();
        for (name, value) in snapshots(&app) {
            let current = render(&value);
            let path = dir.join(name);
            if update {
                std::fs::write(&path, &current).expect("write snapshot");
                continue;
            }
            let expected = std::fs::read_to_string(&path).unwrap_or_default();
            if current != expected {
                differences.push(format!("{name} changed:\n--- snapshot\n{expected}\n+++ backend\n{current}"));
            }
        }
        assert!(
            differences.is_empty(),
            "contracts changed; if intended run UPDATE_CONTRACTS=1 cargo test -p meticulous-backend contract and commit the snapshots\n{}",
            differences.join("\n")
        );
    }

    /// tests/test_api_contracts.py: no route without a method.
    #[test]
    fn no_route_is_registered_without_a_method() {
        let (_dir, app) = crate::testing::app();
        let silent: Vec<Value> = http_routes(&app)
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["methods"].as_array().is_none_or(Vec::is_empty))
            .map(|r| r["path"].clone())
            .collect();
        assert_eq!(silent, Vec::<Value>::new());
    }

    /// Every event name the code emits is in EMITS (extract.py read them from
    /// the source the same way).
    #[test]
    fn every_emitted_event_is_listed() {
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = BTreeSet::new();
        let pattern = regex::Regex::new(r#"\.emit(?:_to|_skip)?\((?:[^,()]+,\s*)?"([A-Za-z_]+)""#).expect("regex");
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("src").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).expect("read");
                    for caps in pattern.captures_iter(&text) {
                        found.insert(caps[1].to_string());
                    }
                }
            }
        }
        let listed: BTreeSet<String> = EMITS.iter().map(|e| e.to_string()).collect();
        let missing: Vec<&String> = found.difference(&listed).collect();
        assert!(missing.is_empty(), "emitted but not in EMITS: {missing:?}");
    }
}
