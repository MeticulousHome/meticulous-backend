//! The `format` checks of `jsonschema.FormatChecker()` the schemas use.

use crate::pyrepr::{py_isspace, py_strip};

/// `jsonschema._format.is_uuid`: `uuid.UUID(instance)` parses and the
/// hyphens sit at positions 8, 13, 18 and 23.
///
/// `uuid.UUID` is lenient: it drops every `urn:` and `uuid:`, strips braces
/// from both ends, drops every hyphen and hands the rest to `int(hex, 16)`,
/// which accepts surrounding whitespace, a sign, a `0x` prefix and `_`
/// between digits. All of that is followed here, except that `int()` also
/// accepts non-ASCII decimal digits, which this rejects.
pub fn is_uuid(instance: &str) -> bool {
    let hex = instance.replace("urn:", "").replace("uuid:", "");
    let hex = hex.trim_matches(|c| c == '{' || c == '}').replace('-', "");
    if hex.chars().count() != 32 || !parses_as_128_bit_hex(&hex) {
        return false;
    }
    let chars: Vec<char> = instance.chars().collect();
    [8, 13, 18, 23]
        .iter()
        .all(|&position| chars.get(position) == Some(&'-'))
}

/// `0 <= int(text, 16) < 1 << 128`.
fn parses_as_128_bit_hex(text: &str) -> bool {
    let text = py_strip(text);
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (body, after_prefix) = match body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        Some(rest) => (rest, true),
        None => (body, false),
    };
    // After a base prefix one leading underscore is allowed ("0x_ff").
    let body = if after_prefix {
        body.strip_prefix('_').unwrap_or(body)
    } else {
        body
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return false;
    }
    let mut value: u128 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        if py_isspace(c) {
            return false;
        }
        let Some(digit) = c.to_digit(16) else {
            return false;
        };
        let Some(next) = value
            .checked_mul(16)
            .and_then(|v| v.checked_add(u128::from(digit)))
        else {
            return false;
        };
        value = next;
    }
    !negative || value == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_uuids() {
        assert!(is_uuid("11111111-2222-4333-8444-555555555555"));
        assert!(is_uuid("F47AC10B-58CC-4372-A567-0E02B2C3D479"));
        assert!(!is_uuid("11111111222243338444555555555555"));
        assert!(!is_uuid("not-a-uuid"));
        assert!(!is_uuid("11111111-2222-4333-8444-55555555555g"));
        assert!(!is_uuid(""));
    }

    #[test]
    fn lenient_like_python() {
        // `uuid.UUID` accepts the braces, but the brace shifts the hyphens
        // off positions 8/13/18/23.
        assert!(!is_uuid("{11111111-2222-4333-8444-555555555555}"));
        // Underscores between digits are accepted by int().
        assert!(is_uuid("1111_111-2222-4333-8444-555555555555"));
    }
}
