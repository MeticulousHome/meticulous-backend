//! Port of `tests/test_sentry_privacy.py`.

use redactor::{drop_breadcrumb, sanitize_sentry_event};
use serde_json::json;

/// `test_sanitize_event_preserves_complete_esp_diagnostics`.
#[test]
fn sanitize_event_preserves_complete_esp_diagnostics() {
    let esp_data = json!({
        "stage_name": format!("Infusion {}", "x".repeat(200)),
        "future_sensor": "raw=value/with,punctuation",
        "diagnostic_code": "HW-42",
    });

    let sanitized = sanitize_sentry_event(&json!({"contexts": {"esp-data": esp_data}})).unwrap();

    assert_eq!(sanitized["contexts"]["esp-data"], esp_data);
}

/// `test_sanitize_event_removes_automatic_personal_context`.
#[test]
fn sanitize_event_removes_automatic_personal_context() {
    let event = json!({
        "server_name": "private-host",
        "user": {"id": "person"},
        "request": {
            "headers": {"Authorization": "Bearer secret"},
            "cookies": {"session": "secret"},
        },
        "breadcrumbs": {"values": [{"message": "Private SSID"}]},
        "tags": {
            "machine": "private-machine",
            "hostname": "private-host",
            "serial": "M123",
            "build-version": "2026M1330-stable",
            "firmware-version": "1.2.3-4-gabc123",
        },
        "release": "espresso-firmware@1.2.3-4-gabc123",
        "contexts": {
            "config": {
                "version": 1,
                "system": {
                    "serial": "M123",
                    "sounds_theme": "private-theme",
                    "root_password": "secret",
                },
                "wifi": {"mode": "CLIENT", "APName": "Private SSID"},
            },
            "esp-data": {
                "stage_name": "Infusion",
                "requested": "10.0",
                "future_sensor": "raw=value/with,punctuation",
            },
            "runtime": {
                "hostname": "private-host",
                "ip": "192.0.2.1",
                "version": "3.11",
            },
        },
        "extra": {
            "authorization": "secret",
            "password_hash": "secret",
            "diagnostic": {"value": 2},
        },
        "exception": {
            "values": [
                {
                    "stacktrace": {
                        "frames": [
                            {
                                "function": "connect",
                                "vars": {"password": "secret"},
                            }
                        ]
                    }
                }
            ]
        },
    });

    let sanitized = sanitize_sentry_event(&event).unwrap();

    for dropped in ["server_name", "user", "request", "breadcrumbs"] {
        assert!(sanitized.get(dropped).is_none(), "{dropped} kept");
    }
    assert_eq!(
        sanitized["tags"],
        json!({
            "serial": "M123",
            "build-version": "2026M1330-stable",
            "firmware-version": "1.2.3-4-gabc123",
        })
    );
    assert_eq!(sanitized["release"], "espresso-firmware@1.2.3-4-gabc123");
    assert_eq!(
        sanitized["contexts"]["config"],
        json!({
            "version": 1,
            "system": {"serial": "M123", "sounds_theme": "custom"},
            "wifi": {"mode": "CLIENT"},
        })
    );
    assert_eq!(
        sanitized["contexts"]["esp-data"],
        json!({
            "stage_name": "Infusion",
            "requested": "10.0",
            "future_sensor": "raw=value/with,punctuation",
        })
    );
    assert_eq!(sanitized["contexts"]["runtime"], json!({"version": "3.11"}));
    assert_eq!(sanitized["extra"], json!({"diagnostic": {"value": 2}}));
    assert!(
        sanitized["exception"]["values"][0]["stacktrace"]["frames"][0]
            .get("vars")
            .is_none()
    );

    // The input is not modified.
    assert_eq!(event["server_name"], "private-host");
    assert!(
        event["exception"]["values"][0]["stacktrace"]["frames"][0]
            .get("vars")
            .is_some()
    );
}

/// Not in the Python suite: key order survives (events keep Python's dict
/// order), and the event-level keys that stay keep their place.
#[test]
fn sanitize_event_keeps_key_order() {
    let event = json!({
        "a": 1,
        "server_name": "x",
        "contexts": {"z": {"k": 1}, "config": {"version": 2}, "esp-data": [1], "hostname": "h"},
        "b": 2,
    });
    let sanitized = sanitize_sentry_event(&event).unwrap();
    assert_eq!(
        serde_json::to_string(&sanitized).unwrap(),
        r#"{"a":1,"contexts":{"z":{"k":1},"config":{"version":2},"esp-data":[1]},"b":2}"#
    );
}

/// Not in the Python suite: an event that is not an object makes the Python
/// hook raise, which sentry-sdk turns into a dropped event.
#[test]
fn a_non_object_event_is_dropped() {
    assert_eq!(sanitize_sentry_event(&json!([1, 2])), None);
}

/// `drop_breadcrumb` disables every automatic breadcrumb.
#[test]
fn every_breadcrumb_is_dropped() {
    assert_eq!(
        drop_breadcrumb(&json!({"message": "Private SSID"}), &json!({})),
        None
    );
}
