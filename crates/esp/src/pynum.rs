use pyfmt::{PyFloat, py_float, repr_f64};
use serde_json::{Number, Value};

/// A value `esp_serial/data.py` keeps in a numeric field.
///
/// The Python fields are typed `float` but hold whatever was assigned:
/// `safeFloat` returns the *int* `0` for non-finite input, `safe_float_with_nan`
/// the *string* `"NaN"`, the class defaults include the int `-1`, and the
/// emulator builds objects from JSON numbers (ints and floats) and may pass
/// `None`. `str()` and `json.dumps()` of each differ (`0` vs `0.0`, `NaN` vs
/// `"NaN"`), and both reach the UART and Socket.IO, so the variant is kept.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PyNum {
    /// A Python `int`.
    Int(i64),
    /// A Python `float`.
    Float(f64),
    /// The string `"NaN"` that `safe_float_with_nan` returns.
    NanString,
    /// `None`.
    None,
}

impl PyNum {
    /// `esp_serial.data.safeFloat`: `float(s)`, with non-finite results
    /// replaced by the int `0`. `None` where `float()` raises.
    pub fn safe_float(s: &str) -> Option<PyNum> {
        // PARITY: the clamp returns the int 0, not 0.0, so str() gives "0"
        // and json.dumps gives 0.
        py_float(s).map(|v| {
            if v.is_finite() {
                PyNum::Float(v)
            } else {
                PyNum::Int(0)
            }
        })
    }

    /// `esp_serial.data.safe_float_with_nan`: a float, or the string `"NaN"`
    /// when `float()` fails or gives NaN. Infinity stays a float.
    pub fn safe_float_with_nan(s: &str) -> PyNum {
        PyNum::from(pyfmt::safe_float_with_nan(s))
    }

    /// Python `str(value)`.
    pub fn to_py_string(&self) -> String {
        match self {
            PyNum::Int(i) => i.to_string(),
            PyNum::Float(f) => repr_f64(*f),
            PyNum::NanString => "NaN".to_string(),
            PyNum::None => "None".to_string(),
        }
    }

    /// The value as it appears in a JSON document Python emits. Python
    /// writes non-finite floats as the invalid tokens `NaN`/`Infinity`; this
    /// backend writes `null` there (the convention of `pyfmt::PyFloat::to_json`).
    pub fn to_json(&self) -> Value {
        match self {
            PyNum::Int(i) => Value::Number(Number::from(*i)),
            PyNum::Float(f) => float_json(*f),
            PyNum::NanString => Value::String("NaN".to_string()),
            PyNum::None => Value::Null,
        }
    }

    /// The numeric value; `None` for the `"NaN"` string and for `None`,
    /// on which Python arithmetic raises `TypeError`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            PyNum::Int(i) => Some(*i as f64),
            PyNum::Float(f) => Some(*f),
            PyNum::NanString | PyNum::None => None,
        }
    }
}

impl From<PyFloat> for PyNum {
    fn from(value: PyFloat) -> Self {
        match value {
            PyFloat::Num(v) => PyNum::Float(v),
            PyFloat::NanString => PyNum::NanString,
        }
    }
}

impl From<f64> for PyNum {
    fn from(value: f64) -> Self {
        PyNum::Float(value)
    }
}

impl From<i64> for PyNum {
    fn from(value: i64) -> Self {
        PyNum::Int(value)
    }
}

/// A Python float in a JSON value; non-finite values become `null`.
pub(crate) fn float_json(v: f64) -> Value {
    Number::from_f64(v).map_or(Value::Null, Value::Number)
}

/// The characters Python's `str.isspace()` accepts: Unicode `White_Space`
/// plus the ASCII separators U+001C..U+001F.
pub(crate) fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python `str.strip()` without arguments.
pub(crate) fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// Python `round(x, ndigits)` for a float: the decimal value correctly
/// rounded, ties to even. Rust's fixed-precision formatting rounds the exact
/// binary value the same way.
pub(crate) fn py_round(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.ndigits$}").parse().unwrap_or(x)
}

/// Python `max(a, b)`: `b` only when `b > a`, so a NaN `a` wins.
pub(crate) fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_and_json_keep_the_python_type() {
        assert_eq!(PyNum::safe_float("inf"), Some(PyNum::Int(0)));
        assert_eq!(PyNum::Int(0).to_py_string(), "0");
        assert_eq!(PyNum::Float(0.0).to_py_string(), "0.0");
        assert_eq!(PyNum::NanString.to_py_string(), "NaN");
        assert_eq!(PyNum::None.to_py_string(), "None");
        assert_eq!(pyfmt::dumps_default(&PyNum::Int(0).to_json()), "0");
        assert_eq!(pyfmt::dumps_default(&PyNum::Float(0.0).to_json()), "0.0");
        assert_eq!(PyNum::Float(f64::INFINITY).to_json(), Value::Null);
    }

    #[test]
    fn python_helpers() {
        assert_eq!(py_strip("\u{1c} a \t"), "a");
        assert_eq!(py_round(0.0625, 3), 0.062);
        assert_eq!(py_round(0.0005, 3), 0.001);
        assert!(py_max(f64::NAN, 0.0).is_nan());
        assert_eq!(py_max(-1.0, 0.005), 0.005);
    }
}
