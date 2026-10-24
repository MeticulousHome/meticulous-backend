//! `api/settings.py`: the user settings, the timezone picker's data and the
//! factory settings.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::app::App;
use crate::config::{self, keys};
use crate::http::{Kind, Reply, Request, Route};
use crate::metconfig::py_repr;

pub fn routes() -> Vec<Route> {
    let m = "api.settings";
    vec![
        Route::new(
            "/manufacturing[/]*",
            Kind::Base,
            m,
            "ManufacturingSettingsHandler",
        )
        .get(manufacturing_get)
        .post(manufacturing_post),
        Route::new("/settings[/]*(.*)", Kind::Base, m, "SettingsHandler")
            .get(get)
            .post(post),
        Route::new("/timezones/(.*)", Kind::Base, m, "TimezoneUIHandler").get(timezones),
    ]
}

/// Python's `type(value)` for a JSON value.
pub fn python_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "<class 'NoneType'>",
        Value::Bool(_) => "<class 'bool'>",
        Value::Number(n) if n.is_f64() => "<class 'float'>",
        Value::Number(_) => "<class 'int'>",
        Value::String(_) => "<class 'str'>",
        Value::Array(_) => "<class 'list'>",
        Value::Object(_) => "<class 'dict'>",
    }
}

/// `str(KeyError(message))`, which is `repr(message)`.
fn key_error(message: &str) -> String {
    py_repr(&Value::String(message.to_string()))
}

fn settings_reply(app: &App) -> Reply {
    Reply::base().dumps(app.config.section(keys::USER))
}

async fn get(app: Arc<App>, req: Request) -> Reply {
    get_setting(&app, req.arg(0).unwrap_or(""))
}

fn get_setting(app: &App, name: &str) -> Reply {
    if name.is_empty() {
        return settings_reply(app);
    }
    let user = app.config.section(keys::USER);
    match user.get(name) {
        Some(value) => {
            let mut body = Map::new();
            body.insert(name.to_string(), value.clone());
            Reply::base().dumps(Value::Object(body))
        }
        None => Reply::base()
            .status(404)
            .json(json!({"status": "error", "error": "setting not found", "setting": name})),
    }
}

/// Why a setting was refused: a `KeyError` (404) or another exception (400).
enum Refusal {
    Key(String),
    Other(String),
}

/// `SettingsHandler.validate_setting`.
fn validate(user: &Map<String, Value>, target: &str, value: &Value) -> Result<(), Refusal> {
    let Some(current) = user.get(target) else {
        return Err(Refusal::Key(key_error(&format!(
            "setting {target} not found"
        ))));
    };
    if !profile_settings_type_ok(target, value, current) {
        return Err(Refusal::Key(key_error(&format!(
            "setting value invalid, received {} and expected {}",
            python_type(value),
            python_type(current)
        ))));
    }
    if target == keys::PROFILE_TARE_BEHAVIOR {
        let ok = value
            .as_str()
            .is_some_and(|v| config::TARE_BEHAVIORS.contains(&v));
        if !ok {
            return Err(Refusal::Other(format!(
                "unsupported tare behavior: {}",
                python_str(value)
            )));
        }
    }
    if target == keys::PROFILE_PARTIAL_RETRACTION {
        let v = value.as_f64().unwrap_or(f64::NAN);
        if !v.is_finite()
            || !(config::PARTIAL_RETRACTION_MIN..=config::PARTIAL_RETRACTION_MAX).contains(&v)
        {
            return Err(Refusal::Other(format!(
                "partial_retraction must be between {:.2} and {:.2} mm",
                config::PARTIAL_RETRACTION_MIN,
                config::PARTIAL_RETRACTION_MAX
            )));
        }
    }
    Ok(())
}

/// `settings_validation.is_valid_setting_type`.
pub fn profile_settings_type_ok(name: &str, value: &Value, current: &Value) -> bool {
    match name {
        // Tri-state until the user answers, then a bool.
        keys::SHOT_DATA_SHARING => value.is_boolean(),
        // A string the user may clear again.
        keys::REPORT_CONTACT_MAIL => value.is_null() || value.is_string(),
        _ => python_type(value) == python_type(current),
    }
}

/// `str(value)` of a JSON value as Python prints it.
fn python_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `normalize_report_contact_mail`.
pub fn normalize_report_contact_mail(value: &Value) -> Result<Value, String> {
    const MAX: usize = 254;
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(s) => {
            let address = s.trim();
            if address.is_empty() {
                return Ok(Value::Null);
            }
            if address.chars().count() > MAX {
                return Err(format!(
                    "report_contact_mail must be at most {MAX} characters"
                ));
            }
            let (local, domain) = address.split_once('@').unwrap_or(("", ""));
            if local.is_empty()
                || domain.is_empty()
                || domain.contains('@')
                || address.chars().any(char::is_whitespace)
            {
                return Err("report_contact_mail must look like name@domain".into());
            }
            Ok(Value::String(address.to_string()))
        }
        _ => Err("report_contact_mail must be a string or null".into()),
    }
}

async fn post(app: Arc<App>, req: Request) -> Reply {
    let settings = match req.json() {
        Ok(settings) => settings,
        Err(e) => {
            return Reply::base().status(403).json(
                json!({"status": "error", "error": "invalid json", "json_error": e.message}),
            );
        }
    };
    let mut work = match app.config.section(keys::USER) {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    // Iterating a JSON array visits its items, which have no .get(): the
    // Python fails with an AttributeError (400) on the first one.
    let entries: Vec<(String, Value)> = match &settings {
        Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Value::Array(items) if items.is_empty() => Vec::new(),
        Value::Array(_) => {
            return refuse(Refusal::Other(
                "'list' object has no attribute 'get'".into(),
            ));
        }
        other => {
            let kind = python_type(other)
                .trim_start_matches("<class '")
                .trim_end_matches("'>")
                .to_string();
            return refuse(Refusal::Other(format!("'{kind}' object is not iterable")));
        }
    };
    let mut reply = Reply::base();
    for (target, mut value) in entries {
        if target == keys::PROFILE_PARTIAL_RETRACTION {
            // isinstance(True, int) holds in Python.
            if let Some(i) = value.as_i64() {
                value = json!(i as f64);
            } else if let Value::Bool(b) = value {
                value = json!(if b { 1.0 } else { 0.0 });
            }
        }
        if target == keys::REPORT_CONTACT_MAIL {
            match normalize_report_contact_mail(&value) {
                Ok(v) => value = v,
                Err(e) => return refuse(Refusal::Other(e)),
            }
        }
        let user_now = work.clone();
        if let Err(refusal) = validate(&user_now, &target, &value) {
            return refuse(refusal);
        }
        match target.as_str() {
            keys::SSH_ENABLED => {
                let enabled = value.as_bool().unwrap_or(false);
                if !app.ssh.set_ssh_state(enabled).await {
                    // PARITY: the error and, later, the settings are both
                    // written into one 500 response.
                    reply.set_status(500);
                    reply.write_json(&json!({
                        "status": "error",
                        "setting": keys::SSH_ENABLED,
                        "details": "Failed to update SSH service state",
                    }));
                }
            }
            keys::TIMEZONE_SYNC => {
                if value.as_str() == Some(config::AUTOMATIC_TIMEZONE_SYNC) {
                    let worker = app.clone();
                    let synced = tokio::task::spawn_blocking(move || {
                        worker
                            .timezone
                            .request_and_sync_tz()
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                    match synced {
                        Ok(Some(new_tz)) => {
                            let tz = Value::String(new_tz);
                            // PARITY: checked against the stored time_zone's
                            // type, so a machine that never had one gets a 404.
                            if let Err(refusal) = validate(&work, keys::TIME_ZONE, &tz) {
                                return refuse(refusal);
                            }
                            work.insert(keys::TIME_ZONE.into(), tz);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            return refuse(Refusal::Other(format!(
                                "failed to sync timezone: {error}"
                            )));
                        }
                    }
                }
            }
            keys::TIME_ZONE => {
                let tz = python_str(&value);
                let app2 = app.clone();
                if let Ok(Err(error)) =
                    tokio::task::spawn_blocking(move || app2.timezone.update_timezone(&tz)).await
                {
                    return refuse(Refusal::Other(format!(
                        "failed to set new timezone: {error}"
                    )));
                }
            }
            keys::MACHINE_HEATING_TIMEOUT => {
                let minutes = value.as_i64().filter(|_| !value.is_f64());
                let result = match minutes {
                    None => Err("Timeout must be an integer".to_string()),
                    Some(m) => app.machine.set_heater_timeout(m),
                };
                if let Err(error) = result {
                    return refuse(Refusal::Other(format!(
                        "Invalid heater timeout value: {error}"
                    )));
                }
            }
            keys::USB_MODE => {
                let mode = match crate::usb::parse_usb_mode(&value) {
                    Ok(mode) => mode,
                    Err(error) => {
                        return refuse(Refusal::Other(format!(
                            "Failed to set the USB mode: {error}"
                        )));
                    }
                };
                let app2 = app.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    app2.usb.set_usb_mode(mode, app2.emulated())
                })
                .await;
            }
            keys::UPDATE_CHANNEL => {
                let channel = python_str(&value);
                let app2 = app.clone();
                let _ = tokio::task::spawn_blocking(move || app2.ota.set_channel(&channel)).await;
            }
            keys::PROFILE_ORDER => app.profiles.on_profile_order_changed(),
            keys::PROFILE_PARTIAL_RETRACTION => app.machine.set_partial_retraction(
                value.as_f64().unwrap_or(config::PARTIAL_RETRACTION_DEFAULT),
            ),
            keys::PROFILE_AUTO_PURGE => app
                .machine
                .set_auto_purge_after_shot(value.as_bool().unwrap_or(false)),
            keys::PROFILE_TARE_BEHAVIOR => app
                .machine
                .set_tare_behavior(value.as_str().unwrap_or_default()),
            keys::SHOT_DATA_SHARING => app.shot_sharing.on_setting_changed(&app, &value),
            _ => {}
        }
        work.insert(target, value);
    }
    app.config.set_section(keys::USER, Value::Object(work));
    app.config.save();
    reply.write_dumps(&app.config.section(keys::USER));
    reply
}

fn refuse(refusal: Refusal) -> Reply {
    match refusal {
        Refusal::Key(message) => {
            tracing::error!("KeyError on Settings Handler: {message}");
            Reply::base()
                .status(404)
                .json(json!({"status": "error", "error": message}))
        }
        Refusal::Other(message) => {
            tracing::error!("Exception on Settings Handler: {message}");
            Reply::base()
                .status(400)
                .json(json!({"status": "error", "error": message}))
        }
    }
}

async fn timezones(app: Arc<App>, req: Request) -> Reply {
    let region = match req.arg(0).unwrap_or("") {
        "" => "countries".to_string(),
        other => other.to_string(),
    };
    let filter = req.get_argument_or("filter", "");
    let map = app.timezone.get_ui_timezones();
    let (status, body) = match region.as_str() {
        "countries" => {
            let countries: Vec<Value> = map
                .as_object()
                .into_iter()
                .flat_map(|m| m.keys())
                .filter(|c| c.to_lowercase().starts_with(&filter))
                .map(|c| Value::String(c.clone()))
                .collect();
            (200, json!({"countries": countries}))
        }
        "cities" => match map.get(&filter).and_then(Value::as_object) {
            Some(cities) => {
                let list: Vec<Value> = cities
                    .iter()
                    .map(|(city, v)| json!({city.clone(): v.clone()}))
                    .collect();
                (200, json!({"cities": list}))
            }
            None => (
                403,
                json!({"status": "error", "error": "invalid country requested"}),
            ),
        },
        _ => (
            403,
            json!({"status": "error", "error": "invalid region type requested"}),
        ),
    };
    Reply::base().status(status).json(body)
}

/// `dial_schema` (`manufacturing.py`): the options the Dial shows in
/// manufacturing mode.
pub fn dial_schema() -> Value {
    let option = |name: &str, value: bool| json!({"name": name, "type": "boolean", "value": value});
    json!({"Elements": [
        {"key": keys::SKIP_STAGE, "label": "Skip Stage", "options": [option("enabled", true), option("disabled", false)]},
        {"key": keys::FORCE_MANUFACTURING_ENABLED, "label": "Enable Manufacturing", "options": [option("enabled", true), option("disabled", false)]},
    ]})
}

async fn manufacturing_get(app: Arc<App>, _req: Request) -> Reply {
    if !app.machine.enable_manufacturing() {
        return Reply::base().status(204);
    }
    Reply::base().json(dial_schema())
}

async fn manufacturing_post(app: Arc<App>, req: Request) -> Reply {
    if !app.machine.enable_manufacturing() {
        return Reply::base()
            .status(410)
            .json(json!({"status": "error", "error": "no configuration available"}));
    }
    let body = match req.json() {
        Ok(body) => body,
        Err(e) => {
            return Reply::base().status(403).json(
                json!({"status": "error", "error": "invalid json", "json_error": e.message}),
            );
        }
    };
    let mut work = match app.config.section(keys::MANUFACTURING) {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if let Value::Object(entries) = body {
        for (key, value) in entries {
            let Some(current) = work.get(&key) else {
                return Reply::base()
                    .status(404)
                    .json(json!({"status": "error", "error": key_error(&format!("setting {key} not found"))}));
            };
            if python_type(&value) != python_type(current) {
                // PARITY: the message names the key's type, not the stored value's.
                return Reply::base().status(404).json(json!({
                    "status": "error",
                    "error": key_error(&format!("setting value invalid, received {} and expected <class 'str'>", python_type(&value))),
                }));
            }
            work.insert(key, value);
        }
    }
    app.config
        .set_section(keys::MANUFACTURING, Value::Object(work));
    let reply = Reply::base().dumps(app.config.section(keys::MANUFACTURING));
    app.config.save();
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tests/test_settings_validation.py.
    #[test]
    fn setting_types_like_python() {
        assert!(profile_settings_type_ok(
            "heating_timeout",
            &json!(5),
            &json!(10)
        ));
        assert!(!profile_settings_type_ok(
            "heating_timeout",
            &json!(5.0),
            &json!(10)
        ));
        assert!(!profile_settings_type_ok(
            "enable_sounds",
            &json!(1),
            &json!(true)
        ));
        assert!(profile_settings_type_ok(
            keys::SHOT_DATA_SHARING,
            &json!(true),
            &Value::Null
        ));
        assert!(!profile_settings_type_ok(
            keys::SHOT_DATA_SHARING,
            &Value::Null,
            &Value::Null
        ));
        assert!(profile_settings_type_ok(
            keys::REPORT_CONTACT_MAIL,
            &Value::Null,
            &json!("a@b")
        ));
        assert_eq!(
            normalize_report_contact_mail(&json!("  a@b.c ")),
            Ok(json!("a@b.c"))
        );
        assert_eq!(
            normalize_report_contact_mail(&json!("   ")),
            Ok(Value::Null)
        );
        assert!(normalize_report_contact_mail(&json!("a b@c")).is_err());
        assert!(normalize_report_contact_mail(&json!("a@b@c")).is_err());
        assert!(normalize_report_contact_mail(&json!(3)).is_err());
    }

    #[test]
    fn key_errors_quote_like_python() {
        assert_eq!(key_error("setting x not found"), "'setting x not found'");
        assert_eq!(
            key_error("setting value invalid, received <class 'int'> and expected <class 'bool'>"),
            "\"setting value invalid, received <class 'int'> and expected <class 'bool'>\""
        );
    }
}
