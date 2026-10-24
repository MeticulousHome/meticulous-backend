//! `api/machine.py`'s `get_machine_info()`: what `GET /machine` answers, what
//! a debug shot and a bug report record about the machine.

use serde_json::{Map, Value, json};

use crate::app::App;
use crate::config::keys;

/// The identity part both `get_machine_info()` and `ShotDebugManager` build:
/// name, hostname and the OS image the machine runs.
pub fn identity(app: &App) -> Map<String, Value> {
    let mut machine = Map::new();
    machine.insert("name".into(), json!(crate::pending::network::device_name(app)));
    machine.insert("hostname".into(), json!(crate::pending::network::hostname(app)));
    insert_image(app, &mut machine);
    machine
}

fn insert_image(app: &App, machine: &mut Map<String, Value>) {
    machine.insert("software_version".into(), json!(app.ota.software_version()));
    machine.insert("image_build_channel".into(), json!(app.ota.image_channel()));
    machine.insert("image_version".into(), json!(app.ota.image_version()));
    let mut repositories = Map::new();
    if let Some(Value::Object(info)) = app.ota.repository_info() {
        for (repo, details) in info {
            repositories.insert(
                repo,
                json!({
                    "branch": details.get("branch").cloned().unwrap_or(Value::Null),
                    "commit": details.get("last_commit").cloned().unwrap_or(Value::Null),
                }),
            );
        }
    }
    machine.insert("repository_info".into(), Value::Object(repositories));
}

/// `MeticulousConfig[CONFIG_SYSTEM][key]`, `""` when unset.
fn system_or_empty(app: &App, key: &str) -> Value {
    match app.config.get(keys::SYSTEM, key) {
        Value::Null => json!(""),
        other => other,
    }
}

/// `get_machine_info()`.
pub fn machine_info(app: &App) -> Value {
    let mut response = Map::new();
    response.insert("name".into(), json!(crate::pending::network::device_name(app)));
    response.insert("hostname".into(), json!(crate::pending::network::hostname(app)));
    let esp_info = app.machine.esp_info();
    if let Some(info) = &esp_info {
        response.insert("firmware".into(), json!(info.firmware_v));
        response.insert("mainVoltage".into(), json!(info.main_voltage));
    }
    response.insert(
        "tare_behavior_supported".into(),
        json!(esp_info.as_ref().is_some_and(|i| i.tare_behavior.is_some())),
    );
    let pour_over = app.pour_over.is_available();
    response.insert("pour_over_profiles_supported".into(), json!(pour_over));
    response.insert(
        "pour_over_profile_schema_version".into(),
        if pour_over { json!(1) } else { Value::Null },
    );
    response.insert(
        "serial".into(),
        app.config.get(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER),
    );
    response.insert("color".into(), system_or_empty(app, keys::MACHINE_COLOR));
    response.insert(
        "batch_number".into(),
        system_or_empty(app, keys::MACHINE_BATCH_NUMBER),
    );
    response.insert(
        "build_date".into(),
        system_or_empty(app, keys::MACHINE_BUILD_DATE),
    );
    insert_image(app, &mut response);
    response.insert(
        "manufacturing".into(),
        json!(app.machine.enable_manufacturing()),
    );
    response.insert("upgrade_first_boot".into(), json!(app.ota.is_changed()));
    let history = match app.config.get(keys::SYSTEM, keys::LAST_SYSTEM_VERSIONS) {
        Value::Null => json!([]),
        other => other,
    };
    response.insert("version_history".into(), history);
    Value::Object(response)
}
