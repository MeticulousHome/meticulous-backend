//! The values the log filter sweeps, read from the config: a port of
//! `_redaction_values()` in `config.py`, as a pure function over the config
//! document so the backend can use it as its value provider.

use serde_json::{Map, Value};

use crate::filter::{RedactionFailure, SweepValues};

const CONFIG_WIFI: &str = "wifi";
const CONFIG_SYSTEM: &str = "system";
const WIFI_KNOWN_WIFIS: &str = "KnownWifis";
const WIFI_AP_PASSWORD: &str = "APPassword";
const ROOT_PASSWORD: &str = "root_password";
const HTTP_AUTH_KEY: &str = "auth_key";

/// Python truthiness of a JSON value.
pub(crate) fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `x.get(key) or {}`, where `x.get` raises `AttributeError` unless `x` is a
/// mapping.
fn get_mapping<'a>(
    parent: &'a Value,
    key: &str,
) -> Result<Option<&'a Map<String, Value>>, RedactionFailure> {
    let Value::Object(parent) = parent else {
        return Err(RedactionFailure::new("AttributeError"));
    };
    get_mapping_in(parent, key)
}

fn get_mapping_in<'a>(
    parent: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a Map<String, Value>>, RedactionFailure> {
    match parent.get(key) {
        Some(value) if py_truthy(value) => match value {
            Value::Object(map) => Ok(Some(map)),
            _ => Err(RedactionFailure::new("AttributeError")),
        },
        _ => Ok(None),
    }
}

fn string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// `_redaction_values()`: KnownWifis names as SSIDs; APPassword,
/// root_password, auth_key and every known network's password as credentials.
///
/// APName is deliberately absent: it is machine-derived, like the hostname.
///
/// PARITY: the filter only sweeps `str` values, so a password YAML loaded as
/// a number (`password: 12345678`) is not swept. A truthy non-mapping `wifi`,
/// `system` or `KnownWifis` raises `AttributeError` in Python, which fails
/// every record until the config is fixed; that failure is returned here.
pub fn sweep_values_from_config(config: &Value) -> Result<SweepValues, RedactionFailure> {
    let wifi = get_mapping(config, CONFIG_WIFI)?;
    let system = get_mapping(config, CONFIG_SYSTEM)?;
    let known_wifis = match wifi {
        Some(wifi) => get_mapping_in(wifi, WIFI_KNOWN_WIFIS)?,
        None => None,
    };
    let mut values = SweepValues::default();
    if let Some(known) = known_wifis {
        values.ssids = known.keys().cloned().collect();
    }
    values
        .credentials
        .extend(string(wifi.and_then(|w| w.get(WIFI_AP_PASSWORD))));
    values
        .credentials
        .extend(string(system.and_then(|s| s.get(ROOT_PASSWORD))));
    values
        .credentials
        .extend(string(system.and_then(|s| s.get(HTTP_AUTH_KEY))));
    if let Some(known) = known_wifis {
        for entry in known.values() {
            // Legacy entries are the bare password; current ones are a map.
            match entry {
                Value::String(password) => values.credentials.push(password.clone()),
                Value::Object(map) => values.credentials.extend(string(map.get("password"))),
                _ => {}
            }
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn collects_known_networks_and_credentials() {
        let config = json!({
            "wifi": {
                "APName": "MeticulousMachine",
                "APPassword": "ap-secret",
                "KnownWifis": {
                    "HomeNet": {"password": "home-secret", "last_used": "x"},
                    "Legacy": "legacy-secret",
                    "Numeric": {"password": 12345678},
                },
            },
            "system": {"root_password": "root-secret", "auth_key": "auth-secret"},
        });
        let values = sweep_values_from_config(&config).unwrap();
        assert_eq!(values.ssids, ["HomeNet", "Legacy", "Numeric"]);
        assert_eq!(
            values.credentials,
            [
                "ap-secret",
                "root-secret",
                "auth-secret",
                "home-secret",
                "legacy-secret"
            ]
        );
    }

    #[test]
    fn missing_or_empty_sections_are_empty() {
        let values = sweep_values_from_config(&json!({"wifi": null, "system": {}})).unwrap();
        assert_eq!(values, SweepValues::default());
    }

    #[test]
    fn a_non_mapping_section_fails_like_python() {
        let failure = sweep_values_from_config(&json!({"wifi": ["x"]})).unwrap_err();
        assert_eq!(failure.exception_type, "AttributeError");
        let failure =
            sweep_values_from_config(&json!({"wifi": {"KnownWifis": "HomeNet"}})).unwrap_err();
        assert_eq!(failure.exception_type, "AttributeError");
    }
}
