use serde_json::{Number, Value};

/// CPython `repr(float)` (and `str(float)`, which is the same since 3.2).
///
/// The digits are the shortest that round-trip, with ties between two
/// equally close candidates broken to the even digit. That is what CPython's
/// dtoa (mode 0) and Ryu do; Rust's own `{:e}` breaks those ties upwards, so
/// the digits come from Ryu. Only the layout is CPython's: it switches to exponent notation
/// when the decimal point would sit more than 16 places right of the first
/// digit or 4 or more places left of it, writes the exponent with a sign and
/// at least two digits (`1e-05`, `1e+16`), and appends `.0` to integral values
/// in positional notation.
pub fn repr_f64(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }

    let (negative, digits, decpt) = shortest_digits(v);

    let mut out = String::with_capacity(digits.len() + 8);
    if negative {
        out.push('-');
    }
    if decpt <= -4 || decpt > 16 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let e = decpt - 1;
        out.push('e');
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", e.unsigned_abs()));
    } else if decpt <= 0 {
        out.push_str("0.");
        for _ in 0..(-decpt) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let point = decpt as usize;
        if digits.len() <= point {
            out.push_str(&digits);
            for _ in 0..(point - digits.len()) {
                out.push('0');
            }
            out.push_str(".0");
        } else {
            out.push_str(&digits[..point]);
            out.push('.');
            out.push_str(&digits[point..]);
        }
    }
    out
}

/// The shortest round-trip digits of a finite, non-zero `v` without leading
/// or trailing zeros, and the decimal point position: `v = ±0.<digits> × 10^decpt`.
fn shortest_digits(v: f64) -> (bool, String, i32) {
    let mut buffer = ryu::Buffer::new();
    let text = buffer.format_finite(v);
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (text, 0),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all: String = format!("{int_part}{frac_part}");
    let leading_zeros = all.len() - all.trim_start_matches('0').len();
    let digits = all.trim_matches('0').to_string();
    // Position of the point counted from the start of `all`, then shifted
    // past the leading zeros that were dropped.
    let decpt = int_part.len() as i32 + exponent - leading_zeros as i32;
    (negative, digits, decpt)
}

/// CPython `float(s)`: `None` where CPython raises `ValueError`.
///
/// Accepts surrounding whitespace, a sign, `inf`/`infinity`/`nan` in any case
/// and `_` between digits, like CPython. Non-ASCII digits, which CPython also
/// accepts, are rejected: the ESP32 only sends ASCII.
pub fn py_float(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let (negative, body) = split_sign(t);
    if body.starts_with(['+', '-']) {
        return None;
    }
    let value = match body.to_ascii_lowercase().as_str() {
        "inf" | "infinity" => f64::INFINITY,
        "nan" => f64::NAN,
        _ => {
            let cleaned = strip_grouping_underscores(body)?;
            // Rust accepts "inf"/"nan" spellings CPython handles above, and
            // nothing CPython rejects once those are excluded.
            if cleaned
                .chars()
                .any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E')
            {
                return None;
            }
            cleaned.parse::<f64>().ok()?
        }
    };
    Some(if negative { -value } else { value })
}

/// CPython `int(s)` in base 10, bounded to `i64`.
pub fn py_int(s: &str) -> Option<i64> {
    let t = s.trim();
    let (negative, body) = split_sign(t);
    let cleaned = strip_grouping_underscores(body)?;
    if cleaned.is_empty() || !cleaned.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let v = cleaned.parse::<i64>().ok()?;
    Some(if negative { -v } else { v })
}

fn split_sign(s: &str) -> (bool, &str) {
    if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    }
}

/// CPython allows `_` in numeric strings only between two digits.
fn strip_grouping_underscores(s: &str) -> Option<String> {
    if !s.contains('_') {
        return Some(s.to_string());
    }
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b != b'_' {
            continue;
        }
        let prev_digit = i
            .checked_sub(1)
            .and_then(|p| bytes.get(p))
            .is_some_and(u8::is_ascii_digit);
        let next_digit = bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
        if !prev_digit || !next_digit {
            return None;
        }
    }
    Some(s.replace('_', ""))
}

/// `esp_serial.data.safeFloat`: `float(s)` with non-finite values clamped to
/// `0`. `None` where `float()` raises.
pub fn safe_float(s: &str) -> Option<f64> {
    py_float(s).map(|v| if v.is_finite() { v } else { 0.0 })
}

/// A value of `esp_serial.data.safe_float_with_nan`: a float, or the string
/// `"NaN"` when the input does not parse or is NaN. Infinity stays a float,
/// as in Python.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PyFloat {
    Num(f64),
    NanString,
}

impl PyFloat {
    /// Python `str()` of the value.
    pub fn to_py_string(self) -> String {
        match self {
            PyFloat::Num(v) => repr_f64(v),
            PyFloat::NanString => "NaN".to_string(),
        }
    }

    /// The value as Python would put it in a JSON document. Infinity has no
    /// JSON spelling; Python writes the invalid token `Infinity`, which this
    /// backend replaces with `null` (see `loads_lenient`).
    pub fn to_json(self) -> Value {
        match self {
            PyFloat::Num(v) => Number::from_f64(v).map_or(Value::Null, Value::Number),
            PyFloat::NanString => Value::String("NaN".to_string()),
        }
    }

    /// The float, with the `"NaN"` string read back as NaN.
    pub fn as_f64(self) -> f64 {
        match self {
            PyFloat::Num(v) => v,
            PyFloat::NanString => f64::NAN,
        }
    }
}

/// `esp_serial.data.safe_float_with_nan`.
pub fn safe_float_with_nan(s: &str) -> PyFloat {
    match py_float(s) {
        Some(v) if !v.is_nan() => PyFloat::Num(v),
        _ => PyFloat::NanString,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_matches_cpython_layout() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (-1.5, "-1.5"),
            (45.33, "45.33"),
            (0.1, "0.1"),
            (0.1 + 0.2, "0.30000000000000004"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (0.000012345, "1.2345e-05"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (123456789012345680.0, "1.2345678901234568e+17"),
            (1e300, "1e+300"),
            (5e-324, "5e-324"),
            (1785373233.6271884, "1785373233.6271884"),
            (100.0, "100.0"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
        ];
        for (value, expected) in cases {
            assert_eq!(repr_f64(*value), *expected, "repr({value:?})");
        }
    }

    #[test]
    fn float_parsing_follows_cpython() {
        assert_eq!(py_float(" 1.5 "), Some(1.5));
        assert_eq!(py_float("1_000.5"), Some(1000.5));
        assert_eq!(py_float("1__0"), None);
        assert_eq!(py_float("_1"), None);
        assert_eq!(py_float("-inf"), Some(f64::NEG_INFINITY));
        assert_eq!(py_float("Infinity"), Some(f64::INFINITY));
        assert!(py_float("nan").is_some_and(f64::is_nan));
        assert_eq!(py_float("1e3"), Some(1000.0));
        assert_eq!(py_float(""), None);
        assert_eq!(py_float("abc"), None);
        assert_eq!(py_float("0x10"), None);
        assert_eq!(py_float("9999+++"), None);
        assert_eq!(safe_float("inf"), Some(0.0));
        assert_eq!(safe_float_with_nan("nan"), PyFloat::NanString);
        assert_eq!(safe_float_with_nan("x"), PyFloat::NanString);
        assert_eq!(safe_float_with_nan("2.5"), PyFloat::Num(2.5));
        assert_eq!(py_int(" 42 "), Some(42));
        assert_eq!(py_int("4.2"), None);
        assert_eq!(py_int("-7"), Some(-7));
    }
}
