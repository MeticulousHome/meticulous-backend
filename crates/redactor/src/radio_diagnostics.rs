//! Structured, credential-free diagnostics for Wi-Fi radio recovery: a port
//! of `radio_diagnostics.py`. Every free-form value is reduced to a code from
//! a closed list, so a network name or password passed by mistake becomes
//! `other`.

use serde_json::{Map, Value};

use crate::pychar::py_strip;

/// `_SAFE_CODES`.
const SAFE_CODES: [&str; 32] = [
    "already_in_progress",
    "ap",
    "apply_ap_mode",
    "client",
    "completed",
    "dns_unreachable",
    "driver_in_band_reset",
    "failed",
    "gateway_unreachable",
    "health_assessed",
    "healthy",
    "hotspot_not_active",
    "hotspot_start_failed",
    "hotspot_stop_failed",
    "internet_unreachable",
    "manual",
    "missing_ipv4",
    "networking_unavailable",
    "not_needed",
    "not_recoverable",
    "recovered",
    "rejected",
    "restart_connection",
    "restart_wifi_radio",
    "restart_wifi_service",
    "started",
    "start_hotspot",
    "step_assessed",
    "step_exception",
    "step_started",
    "wifi_device_unavailable",
    "wifi_not_connected",
];

/// `safe_code(value)`: the text before the first `:`, stripped and
/// lowercased, if it is a known code; `other` otherwise. A missing or empty
/// value reads as `unknown`, which is not a code either.
pub fn safe_code(value: Option<&str>) -> &'static str {
    let value = value.filter(|v| !v.is_empty()).unwrap_or("unknown");
    let head = value.split(':').next().unwrap_or(value);
    let candidate = py_strip(head).to_lowercase();
    SAFE_CODES
        .iter()
        .find(|code| **code == candidate)
        .copied()
        .unwrap_or("other")
}

/// The arguments of `emit_radio_recovery_event`. `None` leaves a field out,
/// as Python's `None` does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RadioRecoveryEvent<'a> {
    /// The event code (reduced with [`safe_code`]).
    pub event: &'a str,
    /// `operation_id`, emitted as given.
    pub operation_id: &'a str,
    /// Milliseconds since the operation started; negative values become 0.
    pub elapsed_ms: i64,
    pub reason: Option<&'a str>,
    pub mode: Option<&'a str>,
    pub connected: Option<bool>,
    pub has_ipv4: Option<bool>,
    pub action: Option<&'a str>,
    pub result: Option<&'a str>,
    /// The error's type name (`type(error).__name__`), never its message.
    pub error_type: Option<&'a str>,
}

/// The line `emit_radio_recovery_event` logs at INFO:
/// `RADIO_RECOVERY <json.dumps(payload, sort_keys=True)>`.
pub fn radio_recovery_message(event: &RadioRecoveryEvent<'_>) -> String {
    let mut payload = Map::new();
    payload.insert("schema_version".into(), Value::from(1));
    payload.insert("event".into(), Value::from(safe_code(Some(event.event))));
    payload.insert("operation_id".into(), Value::from(event.operation_id));
    payload.insert("elapsed_ms".into(), Value::from(event.elapsed_ms.max(0)));
    let codes = [
        ("reason", event.reason),
        ("mode", event.mode),
        ("action", event.action),
        ("result", event.result),
    ];
    for (name, value) in codes {
        if let Some(value) = value {
            payload.insert(name.into(), Value::from(safe_code(Some(value))));
        }
    }
    for (name, value) in [("connected", event.connected), ("has_ipv4", event.has_ipv4)] {
        if let Some(value) = value {
            payload.insert(name.into(), Value::from(value));
        }
    }
    if let Some(error_type) = event.error_type {
        payload.insert("error_type".into(), Value::from(error_type));
    }
    let opts = pyfmt::DumpOpts {
        sort_keys: true,
        ..pyfmt::DumpOpts::default()
    };
    format!(
        "RADIO_RECOVERY {}",
        pyfmt::dumps(&Value::Object(payload), &opts)
    )
}
