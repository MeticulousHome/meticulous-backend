//! Port of `tests/test_radio_diagnostics.py`.

use redactor::{RadioRecoveryEvent, radio_recovery_message, safe_code};
use serde_json::json;

/// `test_safe_code_rejects_network_names_and_credentials`.
#[test]
fn safe_code_rejects_network_names_and_credentials() {
    assert_eq!(
        safe_code(Some("gateway_unreachable")),
        "gateway_unreachable"
    );
    assert_eq!(
        safe_code(Some("hotspot_start_failed: details")),
        "hotspot_start_failed"
    );
    assert_eq!(safe_code(Some("Private SSID / secret-password")), "other");
    // `str(value or "unknown")`: unknown is not a code either.
    assert_eq!(safe_code(None), "other");
    assert_eq!(safe_code(Some("")), "other");
    assert_eq!(safe_code(Some("  CLIENT ")), "client");
}

/// `test_radio_event_contains_only_bounded_structured_metadata`.
#[test]
fn radio_event_contains_only_bounded_structured_metadata() {
    let secret = "SENTINEL-PRIVATE-NETWORK-PASSWORD";

    let message = radio_recovery_message(&RadioRecoveryEvent {
        event: "step_exception",
        operation_id: "abc123",
        elapsed_ms: 1250,
        reason: Some(secret),
        mode: Some("client"),
        connected: Some(true),
        has_ipv4: Some(false),
        action: Some("restart_wifi_radio"),
        result: None,
        error_type: Some("RuntimeError"),
    });

    assert!(!message.contains(secret));
    let (prefix, encoded) = message.split_once(' ').unwrap();
    let payload: serde_json::Value = serde_json::from_str(encoded).unwrap();
    assert_eq!(prefix, "RADIO_RECOVERY");
    assert_eq!(
        payload,
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
    // json.dumps(payload, sort_keys=True), byte for byte.
    assert_eq!(
        message,
        "RADIO_RECOVERY {\"action\": \"restart_wifi_radio\", \"connected\": true, \
         \"elapsed_ms\": 1250, \"error_type\": \"RuntimeError\", \"event\": \"step_exception\", \
         \"has_ipv4\": false, \"mode\": \"client\", \"operation_id\": \"abc123\", \
         \"reason\": \"other\", \"schema_version\": 1}"
    );
}

/// Not in the Python suite: `max(0, int(elapsed_ms))` and omitted fields.
#[test]
fn radio_event_minimal_payload() {
    let message = radio_recovery_message(&RadioRecoveryEvent {
        event: "started",
        operation_id: "op",
        elapsed_ms: -5,
        ..RadioRecoveryEvent::default()
    });
    assert_eq!(
        message,
        "RADIO_RECOVERY {\"elapsed_ms\": 0, \"event\": \"started\", \"operation_id\": \"op\", \"schema_version\": 1}"
    );
}
