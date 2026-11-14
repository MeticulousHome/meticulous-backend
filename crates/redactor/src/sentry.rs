//! Privacy controls for automatic Sentry events: a port of
//! `sentry_privacy.py`, shared by the backend client and the ESP client.
//!
//! Events are handled as JSON (`serde_json::Value`), the shape the Python
//! `before_send` hook receives.

use serde_json::{Map, Value};

use crate::pychar::py_strip;
use crate::reportable_config::get_reportable_config;

/// `_SENSITIVE_KEY_PARTS`.
const SENSITIVE_KEY_PARTS: [&str; 22] = [
    "password",
    "passwd",
    "secret",
    "token",
    "credential",
    "authorization",
    "cookie",
    "api_key",
    "auth_key",
    "root_password",
    "ssid",
    "apname",
    "knownwifis",
    "ip",
    "ip_address",
    "ipaddress",
    "client_ip",
    "remote_addr",
    "hostname",
    "machine_name",
    "email",
    "username",
];

/// Tags dropped on top of the sensitive ones.
const DROPPED_TAGS: [&str; 3] = ["machine", "machine_name", "hostname"];

/// `drop_breadcrumb(crumb, hint)`: every automatic breadcrumb is dropped.
pub fn drop_breadcrumb(_crumb: &Value, _hint: &Value) -> Option<Value> {
    None
}

/// `_normalized_key`: `str(key).strip().lower()`, `-` and space to `_`.
fn normalized_key(key: &str) -> String {
    py_strip(key).to_lowercase().replace(['-', ' '], "_")
}

/// `_is_sensitive_key`: a sensitive word as the whole key or as one of its
/// `_`-separated parts.
fn is_sensitive_key(key: &str) -> bool {
    let normalized = normalized_key(key);
    SENSITIVE_KEY_PARTS.iter().any(|part| {
        normalized == *part
            || normalized.starts_with(&format!("{part}_"))
            || normalized.ends_with(&format!("_{part}"))
            || normalized.contains(&format!("_{part}_"))
    })
}

/// `_scrub_structured`: drop sensitive keys at every depth.
fn scrub_structured(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !is_sensitive_key(key))
                .map(|(key, item)| (key.clone(), scrub_structured(item)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(scrub_structured).collect()),
        other => other.clone(),
    }
}

/// `_remove_frame_variables`: drop `vars` from every exception frame.
///
/// PARITY: only `exception.values[].stacktrace.frames[]`; frames under
/// `threads` keep their `vars` (the SDK is configured not to send them).
fn remove_frame_variables(event: &mut Map<String, Value>) {
    let Some(Value::Object(exception)) = event.get_mut("exception") else {
        return;
    };
    let Some(Value::Array(values)) = exception.get_mut("values") else {
        return;
    };
    for exception_value in values {
        let Some(Value::Object(stacktrace)) = exception_value.get_mut("stacktrace") else {
            continue;
        };
        let Some(Value::Array(frames)) = stacktrace.get_mut("frames") else {
            continue;
        };
        for frame in frames {
            if let Value::Object(frame) = frame {
                frame.shift_remove("vars");
            }
        }
    }
}

/// `sanitize_sentry_event(event, hint)`: a sanitized copy of an automatic
/// event. Drops breadcrumbs, request, user and server_name; filters tags;
/// scrubs contexts and extra (the `config` context is replaced by
/// [`get_reportable_config`], `esp-data` is kept whole); drops frame vars.
///
/// Returns `None` for an event that is not an object: the Python hook raises
/// on one, and sentry-sdk drops an event whose `before_send` raises.
pub fn sanitize_sentry_event(event: &Value) -> Option<Value> {
    let mut sanitized = event.as_object()?.clone();
    for key in ["breadcrumbs", "request", "user", "server_name"] {
        sanitized.shift_remove(key);
    }

    if let Some(Value::Object(tags)) = sanitized.get("tags") {
        let kept: Map<String, Value> = tags
            .iter()
            .filter(|(key, _)| {
                !DROPPED_TAGS.contains(&normalized_key(key).as_str()) && !is_sensitive_key(key)
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        sanitized.insert("tags".to_owned(), Value::Object(kept));
    }

    if let Some(Value::Object(contexts)) = sanitized.get("contexts") {
        let mut clean = match scrub_structured(&Value::Object(contexts.clone())) {
            Value::Object(clean) => clean,
            _ => Map::new(),
        };
        if let Some(raw_config) = contexts.get("config").filter(|c| c.is_object()) {
            clean.insert("config".to_owned(), get_reportable_config(raw_config));
        }
        // ESP diagnostics are firmware-controlled hardware data: kept whole so
        // new diagnostics need no backend update.
        if let Some(raw_esp_data) = contexts.get("esp-data").filter(|v| !v.is_null()) {
            clean.insert("esp-data".to_owned(), raw_esp_data.clone());
        }
        sanitized.insert("contexts".to_owned(), Value::Object(clean));
    }

    if let Some(extra) = sanitized.get("extra") {
        let scrubbed = scrub_structured(extra);
        sanitized.insert("extra".to_owned(), scrubbed);
    }

    remove_frame_variables(&mut sanitized);
    Some(Value::Object(sanitized))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_key_matching() {
        assert!(is_sensitive_key("password_hash"));
        assert!(is_sensitive_key("Client-IP"));
        assert!(is_sensitive_key(" Remote Addr "));
        assert!(is_sensitive_key("x_token_y"));
        assert!(!is_sensitive_key("tokenizer"));
        assert!(!is_sensitive_key("version"));
        assert!(!is_sensitive_key("esp-data"));
    }
}
