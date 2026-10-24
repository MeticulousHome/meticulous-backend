//! `radio_diagnostics.py`: one structured, credential-free log line per step
//! of a shared Wi-Fi/Bluetooth radio recovery.

use serde_json::{Map, Value, json};

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

/// `safe_code(value)`: a known code before any `:` detail, else `other`.
pub fn safe_code(value: &str) -> String {
    let value = if value.is_empty() { "unknown" } else { value };
    let candidate = value
        .split_once(':')
        .map(|(head, _)| head)
        .unwrap_or(value)
        .trim()
        .to_lowercase();
    if SAFE_CODES.contains(&candidate.as_str()) {
        candidate
    } else {
        "other".into()
    }
}

/// The optional fields of a recovery event.
#[derive(Debug, Default, Clone)]
pub struct RecoveryEvent<'a> {
    pub reason: Option<&'a str>,
    pub mode: Option<String>,
    pub connected: Option<bool>,
    pub has_ipv4: Option<bool>,
    pub action: Option<&'a str>,
    pub result: Option<&'a str>,
    /// `type(error).__name__`.
    pub error_type: Option<&'a str>,
}

/// `emit_radio_recovery_event(...)`: the JSON payload, keys sorted.
pub fn recovery_payload(
    event: &str,
    operation_id: &str,
    elapsed_ms: i64,
    fields: &RecoveryEvent<'_>,
) -> String {
    let mut entries: Vec<(&str, Value)> = vec![
        ("schema_version", json!(1)),
        ("event", json!(safe_code(event))),
        ("operation_id", json!(operation_id)),
        ("elapsed_ms", json!(elapsed_ms.max(0))),
    ];
    if let Some(reason) = fields.reason {
        entries.push(("reason", json!(safe_code(reason))));
    }
    if let Some(mode) = &fields.mode {
        entries.push(("mode", json!(safe_code(mode))));
    }
    if let Some(connected) = fields.connected {
        entries.push(("connected", json!(connected)));
    }
    if let Some(has_ipv4) = fields.has_ipv4 {
        entries.push(("has_ipv4", json!(has_ipv4)));
    }
    if let Some(action) = fields.action {
        entries.push(("action", json!(safe_code(action))));
    }
    if let Some(result) = fields.result {
        entries.push(("result", json!(safe_code(result))));
    }
    if let Some(error_type) = fields.error_type {
        entries.push(("error_type", json!(error_type)));
    }
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value);
    }
    pyfmt::dumps_default(&Value::Object(map))
}

pub fn emit_radio_recovery_event(
    event: &str,
    operation_id: &str,
    elapsed_ms: i64,
    fields: &RecoveryEvent<'_>,
) {
    tracing::info!(
        "RADIO_RECOVERY {}",
        recovery_payload(event, operation_id, elapsed_ms, fields)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tests/test_radio_diagnostics.py::test_safe_code_rejects_network_names_and_credentials
    #[test]
    fn safe_code_rejects_network_names_and_credentials() {
        assert_eq!(safe_code("gateway_unreachable"), "gateway_unreachable");
        assert_eq!(
            safe_code("hotspot_start_failed: details"),
            "hotspot_start_failed"
        );
        assert_eq!(safe_code("Private SSID / secret-password"), "other");
    }

    /// tests/test_radio_diagnostics.py::test_radio_event_contains_only_bounded_structured_metadata
    #[test]
    fn radio_event_contains_only_bounded_structured_metadata() {
        let secret = "SENTINEL-PRIVATE-NETWORK-PASSWORD";
        let payload = recovery_payload(
            "step_exception",
            "abc123",
            1250,
            &RecoveryEvent {
                reason: Some(secret),
                mode: Some("client".into()),
                connected: Some(true),
                has_ipv4: Some(false),
                action: Some("restart_wifi_radio"),
                error_type: Some("RuntimeError"),
                ..RecoveryEvent::default()
            },
        );
        assert!(!payload.contains(secret));
        let parsed: Value = serde_json::from_str(&payload).expect("json");
        assert_eq!(
            parsed,
            json!({
                "action": "restart_wifi_radio",
                "connected": true,
                "elapsed_ms": 1250,
                "error_type": "RuntimeError",
                "event": "step_exception",
                "has_ipv4": false,
                "mode": "client",
                "operation_id": "abc123",
                "reason": "other",
                "schema_version": 1,
            })
        );
    }

    /// Keys come out sorted, with `json.dumps` spacing.
    #[test]
    fn payload_is_sorted_and_allowlisted() {
        assert_eq!(
            safe_code("hotspot_start_failed: channel=6: busy"),
            "hotspot_start_failed"
        );
        assert_eq!(safe_code("HomeNet"), "other");
        assert_eq!(safe_code(""), "other");
        let payload = recovery_payload(
            "step_exception",
            "abc123",
            -5,
            &RecoveryEvent {
                reason: Some("manual"),
                mode: Some("CLIENT".into()),
                action: Some("restart_connection"),
                error_type: Some("RuntimeError"),
                ..RecoveryEvent::default()
            },
        );
        assert_eq!(
            payload,
            "{\"action\": \"restart_connection\", \"elapsed_ms\": 0, \"error_type\": \"RuntimeError\", \
             \"event\": \"step_exception\", \"mode\": \"client\", \"operation_id\": \"abc123\", \
             \"reason\": \"manual\", \"schema_version\": 1}"
        );
    }
}
