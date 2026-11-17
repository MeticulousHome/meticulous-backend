//! CPython's `repr()`, `str()` and whitespace rules for JSON values.
//!
//! The messages the Python backend sends to clients are built with f-strings
//! such as `f"{instance!r} is not of type 'number'"`, so a JSON value has to be
//! spelled the way CPython spells the object `json.loads` made of it: `None`,
//! `True`, `'text'`, `{'a': [1, 2.5]}`.

mod unicode_tables;

use serde_json::{Number, Value};

/// `repr(value)` of the object `json.loads` returns for `value`.
pub fn py_repr(value: &Value) -> String {
    let mut out = String::new();
    write_repr(&mut out, value);
    out
}

/// `str(value)`: a string is itself, everything else is its `repr`.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `repr()` of a Python int or float held in a JSON number.
pub fn py_repr_number(n: &Number) -> String {
    if n.is_f64() {
        pyfmt::repr_f64(n.as_f64().unwrap_or(0.0))
    } else {
        n.to_string()
    }
}

fn write_repr(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("None"),
        Value::Bool(true) => out.push_str("True"),
        Value::Bool(false) => out.push_str("False"),
        Value::Number(n) => out.push_str(&py_repr_number(n)),
        Value::String(s) => write_repr_str(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_repr(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_repr_str(out, key);
                out.push_str(": ");
                write_repr(out, item);
            }
            out.push('}');
        }
    }
}

/// `repr(s)` for a `str` (CPython `unicode_repr`).
pub fn py_repr_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    write_repr_str(&mut out, s);
    out
}

fn write_repr_str(out: &mut String, s: &str) {
    // Single quotes unless the text has a single quote and no double quote;
    // with both, single quotes and the single quotes escaped.
    let has_single = s.contains('\'');
    let has_double = s.contains('"');
    let quote = if has_single && !has_double { '"' } else { '\'' };
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if (c as u32) < 0x7f => out.push(c),
            c if is_printable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
}

/// `str.isprintable()` for one character (CPython 3.11, Unicode 14.0.0).
pub fn is_printable(c: char) -> bool {
    let cp = c as u32;
    unicode_tables::NON_PRINTABLE
        .binary_search_by(|&(start, end)| {
            if end < cp {
                std::cmp::Ordering::Less
            } else if start > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_err()
}

/// `str.isspace()` for one character: Rust's `White_Space` set plus the four
/// information separators U+001C..U+001F, which CPython counts as whitespace
/// through their bidirectional class.
pub fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()` with no argument.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// `type(value).__name__` of the object `json.loads` returns for `value`.
pub fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Python truthiness of a JSON value.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(u) = n.as_u64() {
                u != 0
            } else {
                n.as_f64().is_some_and(|f| f != 0.0)
            }
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repr_of_json_values() {
        assert_eq!(py_repr(&json!(null)), "None");
        assert_eq!(py_repr(&json!(true)), "True");
        assert_eq!(py_repr(&json!(1)), "1");
        assert_eq!(py_repr(&json!(1.0)), "1.0");
        assert_eq!(py_repr(&json!("a'b")), "\"a'b\"");
        assert_eq!(py_repr(&json!("a'b\"")), "'a\\'b\"'");
        assert_eq!(
            py_repr(&json!({"a": [1, 2.5, null, "x"], "b": {}})),
            "{'a': [1, 2.5, None, 'x'], 'b': {}}"
        );
        assert_eq!(
            py_repr(&json!("tab\there\u{7f}\u{a0}\u{e9}")),
            "'tab\\there\\x7f\\xa0\u{e9}'"
        );
        assert_eq!(py_repr(&json!("\u{200d}\u{1f600}")), "'\\u200d\u{1f600}'");
    }

    #[test]
    fn whitespace_includes_the_separators() {
        assert_eq!(py_strip("\u{1c} x \u{3000}"), "x");
        assert!(!py_isspace('\u{200b}'));
    }
}
