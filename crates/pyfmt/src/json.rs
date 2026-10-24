use serde_json::{Map, Value};

use crate::float::repr_f64;

/// The `json.dumps` options this backend uses.
#[derive(Debug, Clone, Copy)]
pub struct DumpOpts {
    /// `ensure_ascii`: escape everything outside printable ASCII as `\uXXXX`.
    pub ensure_ascii: bool,
    /// `indent`: `None` writes a single line.
    pub indent: Option<usize>,
    /// `sort_keys`.
    pub sort_keys: bool,
    /// `separators[0]`.
    pub item_separator: &'static str,
    /// `separators[1]`.
    pub key_separator: &'static str,
}

impl Default for DumpOpts {
    /// `json.dumps(obj)`: ASCII only, one line, `", "` and `": "`.
    fn default() -> Self {
        DumpOpts {
            ensure_ascii: true,
            indent: None,
            sort_keys: false,
            item_separator: ", ",
            key_separator: ": ",
        }
    }
}

impl DumpOpts {
    /// `json.dumps(obj, indent=n)`: with an indent the item separator loses
    /// its trailing space.
    pub fn indented(indent: usize) -> Self {
        DumpOpts {
            indent: Some(indent),
            item_separator: ",",
            ..DumpOpts::default()
        }
    }

    /// `json.dumps(obj, ensure_ascii=False)`.
    pub fn unicode() -> Self {
        DumpOpts {
            ensure_ascii: false,
            ..DumpOpts::default()
        }
    }
}

/// `json.dumps(value)` with the default options.
pub fn dumps_default(value: &Value) -> String {
    dumps(value, &DumpOpts::default())
}

/// `json.dumps(value, ...)` byte for byte.
pub fn dumps(value: &Value, opts: &DumpOpts) -> String {
    let mut out = String::new();
    write_value(&mut out, value, opts, 0);
    out
}

/// What tornado's `RequestHandler.write(dict)` sends: `json.dumps` with the
/// defaults and every `</` escaped so the body can sit inside a script tag.
pub fn tornado_json_encode(value: &Value) -> String {
    dumps_default(value).replace("</", "<\\/")
}

fn write_value(out: &mut String, value: &Value, opts: &DumpOpts, level: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            if n.is_f64() {
                out.push_str(&repr_f64(n.as_f64().unwrap_or(0.0)));
            } else {
                out.push_str(&n.to_string());
            }
        }
        Value::String(s) => write_string(out, s, opts.ensure_ascii),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(opts.item_separator);
                }
                newline(out, opts, level + 1);
                write_value(out, item, opts, level + 1);
            }
            newline(out, opts, level);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            let mut write_entry = |i: usize, key: &str, item: &Value| {
                if i > 0 {
                    out.push_str(opts.item_separator);
                }
                newline(out, opts, level + 1);
                write_string(out, key, opts.ensure_ascii);
                out.push_str(opts.key_separator);
                write_value(out, item, opts, level + 1);
            };
            if opts.sort_keys {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for (i, key) in keys.into_iter().enumerate() {
                    write_entry(i, key, &map[key]);
                }
            } else {
                for (i, (key, item)) in map.iter().enumerate() {
                    write_entry(i, key, item);
                }
            }
            newline(out, opts, level);
            out.push('}');
        }
    }
}

fn newline(out: &mut String, opts: &DumpOpts, level: usize) {
    if let Some(indent) = opts.indent {
        out.push('\n');
        for _ in 0..indent * level {
            out.push(' ');
        }
    }
}

/// `json.encoder.py_encode_basestring(_ascii)`.
fn write_string(out: &mut String, s: &str, ensure_ascii: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if ensure_ascii && !(' '..='~').contains(&c) => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `json.loads` for documents Python wrote: Python accepts and writes the
/// non-standard tokens `NaN`, `Infinity` and `-Infinity`, which strict JSON
/// parsers reject. They are read as `null`.
pub fn loads_lenient(text: &str) -> Result<Value, serde_json::Error> {
    match serde_json::from_str(text) {
        Ok(value) => Ok(value),
        Err(error) => {
            let replaced = replace_nonfinite_tokens(text);
            if replaced.len() == text.len() && replaced == text {
                return Err(error);
            }
            serde_json::from_str(&replaced)
        }
    }
}

/// Replace the bare tokens `NaN`, `Infinity` and `-Infinity` outside strings.
fn replace_nonfinite_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            rest = &rest[c.len_utf8()..];
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            rest = &rest[1..];
            continue;
        }
        let token = ["-Infinity", "Infinity", "NaN"]
            .into_iter()
            .find(|token| rest.starts_with(token));
        match token {
            Some(token) => {
                out.push_str("null");
                rest = &rest[token.len()..];
            }
            None => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    out
}

/// Build a JSON object from key/value pairs, keeping their order.
pub fn object<I, K>(entries: I) -> Value
where
    I: IntoIterator<Item = (K, Value)>,
    K: Into<String>,
{
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.into(), value);
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `\uXXXX` built at run time, so the escape survives editors and tools
    /// that would turn it into the character.
    fn esc(hex: &str) -> String {
        format!("{}u{hex}", '\\')
    }

    #[test]
    fn default_separators_and_ascii() {
        let value = json!({"name": "Caf\u{e9} \u{2615}", "n": 1, "f": 1.0, "l": [1, 2.5, null, true], "e": {}, "a": []});
        let expected = format!(
            r#"{{"name": "Caf{} {}", "n": 1, "f": 1.0, "l": [1, 2.5, null, true], "e": {{}}, "a": []}}"#,
            esc("00e9"),
            esc("2615")
        );
        assert_eq!(dumps_default(&value), expected);
    }

    #[test]
    fn indent_four_like_json_dump() {
        let value = json!({"id": "x", "stages": [{"a": 1}], "empty": []});
        assert_eq!(
            dumps(&value, &DumpOpts::indented(4)),
            "{\n    \"id\": \"x\",\n    \"stages\": [\n        {\n            \"a\": 1\n        }\n    ],\n    \"empty\": []\n}"
        );
    }

    #[test]
    fn escapes_like_cpython() {
        let value = json!("a\"b\\c\n\u{7f}\u{1}/\u{1f600}");
        let expected = format!(
            r#""a\"b\\c\n{}{}/{}{}""#,
            esc("007f"),
            esc("0001"),
            esc("d83d"),
            esc("de00")
        );
        assert_eq!(dumps_default(&value), expected);
        let expected_unicode = format!(r#""a\"b\\c\n{}{}/{}""#, '\u{7f}', esc("0001"), '\u{1f600}');
        assert_eq!(dumps(&value, &DumpOpts::unicode()), expected_unicode);
    }

    #[test]
    fn sort_keys() {
        let value = json!({"b": 1, "a": 2});
        let opts = DumpOpts {
            sort_keys: true,
            ..DumpOpts::default()
        };
        assert_eq!(dumps(&value, &opts), r#"{"a": 2, "b": 1}"#);
    }

    #[test]
    fn tornado_escapes_closing_tags() {
        assert_eq!(
            tornado_json_encode(&json!({"x": "</script>"})),
            r#"{"x": "<\/script>"}"#
        );
    }

    #[test]
    fn lenient_loads_reads_python_tokens() {
        let value = loads_lenient(r#"{"a": NaN, "b": [Infinity, -Infinity], "c": "NaN"}"#)
            .expect("lenient parse");
        assert_eq!(value, json!({"a": null, "b": [null, null], "c": "NaN"}));
    }
}
