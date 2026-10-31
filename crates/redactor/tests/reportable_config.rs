//! Port of `tests/test_reportable_config.py`.

use std::collections::BTreeSet;

use redactor::get_reportable_config;
use serde_json::{Value, json};

/// `_leaf_paths`.
fn leaf_paths(value: &Value, prefix: Vec<String>, out: &mut BTreeSet<Vec<String>>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                let mut path = prefix.clone();
                path.push(key.clone());
                leaf_paths(item, path, out);
            }
        }
        _ => {
            out.insert(prefix);
        }
    }
}

/// `test_reportable_config_is_allowlist_only_and_does_not_mutate_input`.
#[test]
fn reportable_config_is_allowlist_only_and_does_not_mutate_input() {
    let config = json!({
        "version": 1,
        "logging": {"log_all_sensor_messages": true, "unknown": "drop"},
        "system": {
            "notifications_ttl": 3600,
            "sounds_theme": "my-custom-theme",
            "serial": "M123",
            "batch_number": "B1",
            "build_date": "2026-01-02",
            "color": "silver",
            "last_system_versions": ["1.0", "0.9"],
            "machine_name": ["Private", "Name"],
            "root_password": "secret",
            "auth_key": "secret",
        },
        "user": {
            "enable_sounds": true,
            "disallow_firmware_flashing": false,
            "debug_shot_data_retention_days": 31,
            "auto_start_shot": false,
            "auto_purge_after_shot": true,
            "tare_behavior": "before_retraction",
            "disturbance_resilience": true,
            "partial_retraction": 45.0,
            "heat_on_boot": true,
            "heating_timeout": 10,
            "update_channel": "stable",
            "idle_screen": "default",
            "reverse_scrolling": {
                "home": true,
                "keyboard": false,
                "menus": true,
                "future": true,
            },
            "clock_format_24_hour": true,
            "allow_legacy_json": false,
            "allow_stage_skipping": false,
            "usb_mode": "host",
            "timezone_sync": "automatic",
            "ssh_enabled": false,
            "disable_ui_features": true,
            "time_zone": "America/Mexico_City",
        },
        "wifi": {
            "mode": "CLIENT",
            "APName": "Private SSID",
            "APPassword": "secret",
            "KnownWifis": {"Private SSID": {"password": "secret"}},
        },
        "manufacturing": {
            "enabled": false,
            "last_boot_mode": "normal",
            "skip_stage": false,
        },
        "future": {"secret": "drop"},
    });
    let original = config.clone();

    let mut reportable = get_reportable_config(&config);

    assert_eq!(reportable["system"]["sounds_theme"], "custom");
    assert_eq!(reportable["wifi"], json!({"mode": "CLIENT"}));
    assert_eq!(
        reportable["manufacturing"],
        json!({"enabled": false, "last_boot_mode": "normal", "skip_stage": false})
    );
    assert!(reportable["system"].get("machine_name").is_none());
    assert!(reportable["system"].get("root_password").is_none());
    assert!(reportable["user"].get("disable_ui_features").is_none());
    assert!(reportable["user"].get("time_zone").is_none());
    assert!(reportable.get("future").is_none());

    let mut paths = BTreeSet::new();
    leaf_paths(&reportable, Vec::new(), &mut paths);
    let expected: BTreeSet<Vec<String>> = [
        &["version"][..],
        &["logging", "log_all_sensor_messages"],
        &["system", "notifications_ttl"],
        &["system", "sounds_theme"],
        &["system", "serial"],
        &["system", "batch_number"],
        &["system", "build_date"],
        &["system", "color"],
        &["system", "last_system_versions"],
        &["user", "enable_sounds"],
        &["user", "disallow_firmware_flashing"],
        &["user", "debug_shot_data_retention_days"],
        &["user", "auto_start_shot"],
        &["user", "auto_purge_after_shot"],
        &["user", "tare_behavior"],
        &["user", "disturbance_resilience"],
        &["user", "partial_retraction"],
        &["user", "heat_on_boot"],
        &["user", "heating_timeout"],
        &["user", "update_channel"],
        &["user", "idle_screen"],
        &["user", "reverse_scrolling", "home"],
        &["user", "reverse_scrolling", "keyboard"],
        &["user", "reverse_scrolling", "menus"],
        &["user", "clock_format_24_hour"],
        &["user", "allow_legacy_json"],
        &["user", "allow_stage_skipping"],
        &["user", "usb_mode"],
        &["user", "timezone_sync"],
        &["user", "ssh_enabled"],
        &["wifi", "mode"],
        &["manufacturing", "enabled"],
        &["manufacturing", "last_boot_mode"],
        &["manufacturing", "skip_stage"],
    ]
    .iter()
    .map(|path| path.iter().map(|p| (*p).to_owned()).collect())
    .collect();
    assert_eq!(paths, expected);

    reportable["system"]["last_system_versions"]
        .as_array_mut()
        .unwrap()
        .push(json!("changed"));
    assert_eq!(config, original);
    assert_eq!(
        config["system"]["last_system_versions"],
        json!(["1.0", "0.9"])
    );
}

/// `test_reportable_config_normalizes_default_theme`.
#[test]
fn reportable_config_normalizes_default_theme() {
    assert_eq!(
        get_reportable_config(&json!({"system": {"sounds_theme": "default"}})),
        json!({"system": {"sounds_theme": "default"}})
    );
}

/// `test_reportable_config_omits_invalid_types_and_non_mapping_input`.
#[test]
fn reportable_config_omits_invalid_types_and_non_mapping_input() {
    assert_eq!(
        get_reportable_config(&json!({
            "version": true,
            "system": {"notifications_ttl": "3600", "sounds_theme": null},
            "user": {"enable_sounds": 1, "partial_retraction": true},
        })),
        json!({})
    );
    assert_eq!(get_reportable_config(&Value::Null), json!({}));
}

/// `test_reportable_config_never_includes_the_contact_mail`.
#[test]
fn reportable_config_never_includes_the_contact_mail() {
    let reportable = get_reportable_config(&json!({
        "user": {"report_contact_mail": "user@example.com", "enable_sounds": true},
        "system": {"report_contact_mail": "user@example.com"},
    }));

    assert_eq!(reportable, json!({"user": {"enable_sounds": true}}));
    let text = reportable.to_string();
    assert!(!text.contains("report_contact_mail"));
    assert!(!text.contains("user@example.com"));
}

/// Not in the Python suite: the output's key order follows the allowlist,
/// with `sounds_theme` written last, as the Python dict does. Matters for the
/// `CONF:` dump, which serialises this object.
#[test]
fn reportable_config_key_order_follows_the_allowlist() {
    let reportable = get_reportable_config(&json!({
        "user": {"enable_sounds": true},
        "system": {"sounds_theme": "x", "serial": "M1"},
        "version": 1,
        "wifi": {"mode": "AP"},
    }));
    assert_eq!(
        serde_json::to_string(&reportable).unwrap(),
        r#"{"version":1,"system":{"serial":"M1","sounds_theme":"custom"},"user":{"enable_sounds":true},"wifi":{"mode":"AP"}}"#
    );
    let float_int = get_reportable_config(&json!({"version": 1.0}));
    assert_eq!(float_int, json!({}));
}
