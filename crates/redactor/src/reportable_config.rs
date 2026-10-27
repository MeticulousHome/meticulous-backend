//! Privacy-reviewed configuration for diagnostics: a port of
//! `reportable_config.py`.
//!
//! An allowlist, not a blocklist: it protects config keys nobody has added
//! yet, which no regex can. Used for the Sentry `config` context and the
//! `CONF:` debug dump.

use serde_json::{Map, Value};

type Validator = fn(&Value) -> bool;

fn is_bool(value: &Value) -> bool {
    value.is_boolean()
}

/// `isinstance(value, int) and not isinstance(value, bool)`.
fn is_int(value: &Value) -> bool {
    value.is_i64() || value.is_u64()
}

/// `isinstance(value, (int, float)) and not isinstance(value, bool)`.
fn is_number(value: &Value) -> bool {
    value.is_number()
}

fn is_string(value: &Value) -> bool {
    value.is_string()
}

fn is_list(value: &Value) -> bool {
    value.is_array()
}

/// `_APPROVED_PATHS`, in order: the output's key order follows it.
const APPROVED_PATHS: &[(&[&str], Validator)] = &[
    (&["version"], is_int),
    (&["logging", "log_all_sensor_messages"], is_bool),
    (&["system", "notifications_ttl"], is_int),
    (&["system", "serial"], is_string),
    (&["system", "batch_number"], is_string),
    (&["system", "build_date"], is_string),
    (&["system", "color"], is_string),
    (&["system", "last_system_versions"], is_list),
    (&["user", "enable_sounds"], is_bool),
    (&["user", "disallow_firmware_flashing"], is_bool),
    (&["user", "debug_shot_data_retention_days"], is_int),
    (&["user", "auto_start_shot"], is_bool),
    (&["user", "auto_purge_after_shot"], is_bool),
    (&["user", "tare_behavior"], is_string),
    (&["user", "partial_retraction"], is_number),
    (&["user", "heat_on_boot"], is_bool),
    (&["user", "heating_timeout"], is_number),
    (&["user", "update_channel"], is_string),
    (&["user", "idle_screen"], is_string),
    (&["user", "reverse_scrolling", "home"], is_bool),
    (&["user", "reverse_scrolling", "keyboard"], is_bool),
    (&["user", "reverse_scrolling", "menus"], is_bool),
    (&["user", "clock_format_24_hour"], is_bool),
    (&["user", "allow_legacy_json"], is_bool),
    (&["user", "shot_data_sharing"], is_bool),
    (&["user", "allow_stage_skipping"], is_bool),
    (&["user", "usb_mode"], is_string),
    (&["user", "timezone_sync"], is_string),
    (&["user", "ssh_enabled"], is_bool),
    (&["wifi", "mode"], is_string),
    (&["manufacturing", "enabled"], is_bool),
    (&["manufacturing", "last_boot_mode"], is_string),
    (&["manufacturing", "skip_stage"], is_bool),
];

fn read_path<'a>(config: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(config, |current, part| current.as_object()?.get(*part))
}

fn write_path(output: &mut Map<String, Value>, path: &[&str], value: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut current = output;
    for part in parents {
        let entry = current
            .entry((*part).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        // Every parent here was created as a map by this function.
        let Value::Object(map) = entry else {
            return;
        };
        current = map;
    }
    current.insert((*last).to_owned(), value);
}

/// `get_reportable_config(config)`: a new object holding only approved,
/// type-valid values, plus `system.sounds_theme` coarsened to `default` or
/// `custom`. A non-object input gives `{}`.
pub fn get_reportable_config(config: &Value) -> Value {
    let mut output = Map::new();
    if !config.is_object() {
        return Value::Object(output);
    }
    for (path, validator) in APPROVED_PATHS {
        if let Some(value) = read_path(config, path)
            && validator(value)
        {
            write_path(&mut output, path, value.clone());
        }
    }
    if let Some(Value::String(theme)) = read_path(config, &["system", "sounds_theme"]) {
        let coarse = if theme == "default" {
            "default"
        } else {
            "custom"
        };
        write_path(
            &mut output,
            &["system", "sounds_theme"],
            Value::String(coarse.to_owned()),
        );
    }
    Value::Object(output)
}
