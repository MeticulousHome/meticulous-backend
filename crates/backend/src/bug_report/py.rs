//! How Python spells the values that end up in report files, logs and error
//! bodies: `repr()`, `str.strip()`, `datetime.isoformat()`, `OSError`
//! messages, `json.loads` errors and `os.stat().st_mtime`.

use std::path::Path;

use serde_json::{Map, Value};

/// `repr(value)` of a value decoded from JSON (dicts keep their order).
pub fn repr(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => i.to_string(),
            (None, Some(u)) => u.to_string(),
            _ => pyfmt::repr_f64(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => repr_str(s),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", repr_str(k), repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `str.isprintable()` for the characters a report name can plausibly hold:
/// controls, the format characters and the separators other than space.
fn printable(c: char) -> bool {
    !(c.is_control()
        || matches!(
            c,
            '\u{a0}'
                | '\u{ad}'
                | '\u{1680}'
                | '\u{2000}'..='\u{200f}'
                | '\u{2028}'..='\u{202f}'
                | '\u{205f}'..='\u{206f}'
                | '\u{3000}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{e000}'..='\u{f8ff}'
        ))
}

/// `repr(str)`.
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if !printable(c) => {
                let code = c as u32;
                if code <= 0xff {
                    out.push_str(&format!("\\x{code:02x}"));
                } else if code <= 0xffff {
                    out.push('\\');
                    out.push_str(&format!("u{code:04x}"));
                } else {
                    out.push_str(&format!("\\U{code:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `str.isspace()`: Unicode whitespace plus the ASCII file, group, record
/// and unit separators, which Python counts and Rust does not.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()`.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `datetime.now(timezone.utc).isoformat()`.
pub fn isoformat_utc(now: chrono::DateTime<chrono::Utc>) -> String {
    if now.timestamp_subsec_micros() == 0 {
        now.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        now.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

/// `str(OSError)` for an error about `path`:
/// `[Errno 2] No such file or directory: '/path'`.
pub fn os_error(error: &std::io::Error, path: &Path) -> String {
    match error.raw_os_error() {
        Some(code) => format!(
            "[Errno {code}] {}: {}",
            strerror(code),
            repr_str(&path.display().to_string())
        ),
        None => error.to_string(),
    }
}

/// `os.strerror(code)`.
pub fn strerror(code: i32) -> String {
    let text = std::io::Error::from_raw_os_error(code).to_string();
    match text.rfind(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

/// The Python exception class an `OSError` with this kind becomes.
pub fn os_error_name(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        std::io::ErrorKind::AlreadyExists => "FileExistsError",
        std::io::ErrorKind::IsADirectory => "IsADirectoryError",
        std::io::ErrorKind::NotADirectory => "NotADirectoryError",
        _ => "OSError",
    }
}

/// `os.stat(path).st_mtime`: CPython adds the nanoseconds as a double, so
/// the float can round up past the second.
pub fn mtime(meta: &std::fs::Metadata) -> f64 {
    use std::os::unix::fs::MetadataExt;
    meta.mtime() as f64 + meta.mtime_nsec() as f64 * 1e-9
}

/// `int(path.stat().st_mtime)`.
pub fn mtime_int(meta: &std::fs::Metadata) -> i64 {
    mtime(meta) as i64
}

/// The value of `isinstance(value, int) and not isinstance(value, bool)`.
pub fn as_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) if !n.is_f64() => n.as_i64(),
        _ => None,
    }
}

/// `int(value)` of a column value: ints as they are, floats truncated.
pub fn int_of(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// `bytes.decode("utf-8")`'s `UnicodeDecodeError` message.
pub fn utf8_error(bytes: &[u8], error: &std::str::Utf8Error) -> String {
    let pos = error.valid_up_to();
    let byte = bytes.get(pos).copied().unwrap_or(0);
    match error.error_len() {
        None if bytes.len() - pos > 1 => format!(
            "'utf-8' codec can't decode bytes in position {pos}-{}: unexpected end of data",
            bytes.len() - 1
        ),
        None => format!(
            "'utf-8' codec can't decode byte 0x{byte:02x} in position {pos}: unexpected end of data"
        ),
        Some(_) => {
            let reason =
                if (0x80..=0xbf).contains(&byte) || byte >= 0xf5 || byte == 0xc0 || byte == 0xc1 {
                    "invalid start byte"
                } else {
                    "invalid continuation byte"
                };
            format!("'utf-8' codec can't decode byte 0x{byte:02x} in position {pos}: {reason}")
        }
    }
}

/// `json.loads(text)` of a `str`, with CPython's error message.
pub fn json_loads(text: &str) -> Result<Value, String> {
    if text.starts_with('\u{feff}') {
        return Err(
            "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)".into(),
        );
    }
    crate::http::parse_python_json(text.as_bytes()).map_err(|e| e.message)
}

/// `_json_loads_body`: the request body as a JSON object (an empty body is
/// `{}`); the error is the `ValueError` message.
pub fn json_loads_body(body: &[u8]) -> Result<Map<String, Value>, String> {
    let value = if body.is_empty() {
        Value::Object(Map::new())
    } else {
        let text = std::str::from_utf8(body)
            .map_err(|e| format!("Invalid JSON: {}", utf8_error(body, &e)))?;
        json_loads(text).map_err(|e| format!("Invalid JSON: {e}"))?
    };
    match value {
        Value::Object(map) => Ok(map),
        _ => Err("Request body must be a JSON object".into()),
    }
}

/// `", ".join(sorted(keys))`.
pub fn sorted_join<'a>(keys: impl IntoIterator<Item = &'a String>) -> String {
    let mut keys: Vec<&String> = keys.into_iter().collect();
    keys.sort();
    keys.into_iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reprs_like_python() {
        assert_eq!(repr(&json!("a'b")), "\"a'b\"");
        assert_eq!(repr(&json!("x")), "'x'");
        assert_eq!(repr(&json!(1.5)), "1.5");
        assert_eq!(repr(&json!(null)), "None");
        assert_eq!(repr(&json!([1, "a", {"k": true}])), "[1, 'a', {'k': True}]");
        assert_eq!(repr_str("a\u{1}\u{2028}"), "'a\\x01\\u2028'");
    }

    #[test]
    fn strips_like_python() {
        assert_eq!(strip("\u{1c} x \n"), "x");
        assert_eq!(strip("   "), "");
    }

    #[test]
    fn isoformat_drops_zero_microseconds() {
        let t = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        assert_eq!(isoformat_utc(t), "2023-11-14T22:13:20+00:00");
        let t = chrono::DateTime::from_timestamp(1_700_000_000, 1_000).unwrap();
        assert_eq!(isoformat_utc(t), "2023-11-14T22:13:20.000001+00:00");
    }

    #[test]
    fn os_errors_read_like_python() {
        let error = std::io::Error::from_raw_os_error(2);
        assert_eq!(
            os_error(&error, Path::new("/x")),
            "[Errno 2] No such file or directory: '/x'"
        );
    }

    #[test]
    fn body_errors_read_like_python() {
        assert_eq!(json_loads_body(b"").unwrap(), Map::new());
        assert_eq!(
            json_loads_body(b"[]").unwrap_err(),
            "Request body must be a JSON object"
        );
        assert_eq!(
            json_loads_body(b"\xff").unwrap_err(),
            "Invalid JSON: 'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"
        );
        assert!(
            json_loads_body(b"{")
                .unwrap_err()
                .starts_with("Invalid JSON: ")
        );
    }
}
