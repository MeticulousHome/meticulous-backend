//! The YAML the Python writes with `yaml.dump(config, default_flow_style=False,
//! allow_unicode=True)`, readable by PyYAML.
//!
//! A machine that rolls back to the Python image reads this file with
//! PyYAML's YAML 1.1 resolver, which turns unquoted `yes`, `on`, `no`, `1:30`
//! or `2024-01-01` into booleans, numbers and dates. Strings are therefore
//! written plain only when nothing could mistake them, and quoted otherwise.
//! The layout follows PyYAML: block style, sorted keys, sequences inside
//! mappings not indented, `{}`/`[]` for empty collections.

use serde_json::Value;

pub fn to_yaml(value: &Value) -> String {
    let mut out = String::new();
    match value {
        Value::Object(map) if !map.is_empty() => write_mapping(&mut out, value, 0),
        Value::Array(items) if !items.is_empty() => write_sequence(&mut out, value, 0),
        scalar => {
            out.push_str(&scalar_text(scalar));
            out.push('\n');
        }
    }
    out
}

fn write_mapping(out: &mut String, value: &Value, indent: usize) {
    let Value::Object(map) = value else { return };
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    for key in keys {
        let item = &map[key];
        push_indent(out, indent);
        out.push_str(&string_text(key));
        out.push(':');
        match item {
            Value::Object(child) if !child.is_empty() => {
                out.push('\n');
                write_mapping(out, item, indent + 2);
            }
            Value::Array(child) if !child.is_empty() => {
                out.push('\n');
                // PyYAML does not indent a sequence under its key.
                write_sequence(out, item, indent);
            }
            scalar => {
                out.push(' ');
                out.push_str(&scalar_text(scalar));
                out.push('\n');
            }
        }
    }
}

fn write_sequence(out: &mut String, value: &Value, indent: usize) {
    let Value::Array(items) = value else { return };
    for item in items {
        push_indent(out, indent);
        out.push_str("- ");
        match item {
            Value::Object(child) if !child.is_empty() => {
                // The first key shares the dash's line; the rest align with it.
                let mut nested = String::new();
                write_mapping(&mut nested, item, indent + 2);
                out.push_str(nested.trim_start());
            }
            Value::Array(child) if !child.is_empty() => {
                let mut nested = String::new();
                write_sequence(&mut nested, item, indent + 2);
                out.push_str(nested.trim_start());
            }
            scalar => {
                out.push_str(&scalar_text(scalar));
                out.push('\n');
            }
        }
    }
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(n) if n.is_f64() => float_text(n.as_f64().unwrap_or(0.0)),
        Value::Number(n) => n.to_string(),
        Value::String(s) => string_text(s),
        Value::Object(_) => "{}".to_string(),
        Value::Array(_) => "[]".to_string(),
    }
}

/// PyYAML `represent_float`: `repr()` lowercased, `.0` added before an
/// exponent that has no point, and the YAML spellings of the special values.
fn float_text(v: f64) -> String {
    if v.is_nan() {
        return ".nan".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { ".inf" } else { "-.inf" }.to_string();
    }
    let repr = pyfmt::repr_f64(v).to_lowercase();
    if !repr.contains('.') && repr.contains('e') {
        repr.replacen('e', ".0e", 1)
    } else {
        repr
    }
}

/// Words PyYAML's resolver reads as something other than a string, plus the
/// single letters YAML 1.1 lists as booleans.
const RESERVED: &[&str] = &[
    "y", "Y", "n", "N", "yes", "Yes", "YES", "no", "No", "NO", "true", "True", "TRUE", "false",
    "False", "FALSE", "on", "On", "ON", "off", "Off", "OFF", "null", "Null", "NULL", "~",
];

fn plain_safe(s: &str) -> bool {
    let Some(first) = s.chars().next() else {
        return false;
    };
    if !first.is_alphabetic() || s.ends_with(' ') || RESERVED.contains(&s) {
        return false;
    }
    if s.contains(": ") || s.contains(" #") || s.ends_with(':') {
        return false;
    }
    s.chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ' ' | '@' | '+'))
}

fn string_text(s: &str) -> String {
    if plain_safe(s) {
        return s.to_string();
    }
    if s.chars().any(|c| c.is_control()) {
        // Double quotes carry escapes; single quotes cannot hold a newline
        // without YAML folding it.
        let mut out = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        return out;
    }
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pyyaml_layout() {
        let config = json!({
            "version": 1,
            "wifi": {"mode": "AP", "KnownWifis": {}, "APName": "MeticulousEspresso"},
            "system": {"machine_name": ["Roasted", "Robusta"], "serial": null, "always_allowed_networks": []},
            "user": {"partial_retraction": 45.33, "heating_timeout": 10, "enable_sounds": true,
                     "reverse_scrolling": {"home": false}, "tiny": 0.00001},
        });
        assert_eq!(
            to_yaml(&config),
            "system:\n  always_allowed_networks: []\n  machine_name:\n  - Roasted\n  - Robusta\n  serial: null\n\
             user:\n  enable_sounds: true\n  heating_timeout: 10\n  partial_retraction: 45.33\n  reverse_scrolling:\n    home: false\n  tiny: 1.0e-05\n\
             version: 1\n\
             wifi:\n  APName: MeticulousEspresso\n  KnownWifis: {}\n  mode: AP\n"
        );
    }

    #[test]
    fn quotes_what_pyyaml_would_misread() {
        for s in [
            "yes",
            "on",
            "No",
            "1:30",
            "2024-01-01",
            "123456789012",
            "",
            " x",
            "a: b",
            "#x",
            "~",
            "'q'",
        ] {
            let text = string_text(s);
            assert!(
                text.starts_with('\'') || text.starts_with('"'),
                "{s:?} -> {text}"
            );
        }
        assert_eq!(string_text("it's"), "'it''s'");
        assert_eq!(string_text("line\nbreak"), "\"line\\nbreak\"");
        assert_eq!(string_text("Home Wifi 5G"), "Home Wifi 5G");
    }

    #[test]
    fn sequences_of_mappings() {
        let value = json!({"versions": [{"a": 1, "b": "x"}, {"a": 2}]});
        assert_eq!(to_yaml(&value), "versions:\n- a: 1\n  b: x\n- a: 2\n");
    }

    #[test]
    fn round_trips_through_a_yaml_reader() {
        let value =
            json!({"user": {"x": "yes", "y": "1:30", "z": [1, 2.5, "on"], "w": {"k": null}}});
        let parsed: Value = serde_norway::from_str(&to_yaml(&value)).expect("parse");
        assert_eq!(parsed, value);
    }
}
