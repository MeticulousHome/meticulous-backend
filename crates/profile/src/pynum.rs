//! Python numbers and equality on JSON values.
//!
//! A JSON number is a Python `int` when it has no fraction or exponent and a
//! `float` otherwise, and Python keeps that distinction through arithmetic
//! (`90 - 2` is `88`, `90.0 - 2` is `88.0`) and compares the two kinds exactly.

use std::cmp::Ordering;

use serde_json::{Number, Value};

/// A Python `int` or `float`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PyNum {
    Int(i128),
    Float(f64),
}

impl PyNum {
    /// The Python number `json.loads` makes of `n`.
    pub fn from_number(n: &Number) -> PyNum {
        if let Some(i) = n.as_i64() {
            PyNum::Int(i128::from(i))
        } else if let Some(u) = n.as_u64() {
            PyNum::Int(i128::from(u))
        } else {
            PyNum::Float(n.as_f64().unwrap_or(f64::NAN))
        }
    }

    /// A number that is not a `bool` (Python's `bool` is an `int`, but none of
    /// the code ported here does arithmetic on one).
    pub fn from_value(value: &Value) -> Option<PyNum> {
        match value {
            Value::Number(n) => Some(PyNum::from_number(n)),
            _ => None,
        }
    }

    /// `float(x)`.
    pub fn as_f64(self) -> f64 {
        match self {
            PyNum::Int(i) => i as f64,
            PyNum::Float(f) => f,
        }
    }

    /// The JSON number `json.dumps` writes for this Python number.
    pub fn to_value(self) -> Value {
        match self {
            PyNum::Int(i) => {
                if let Ok(v) = i64::try_from(i) {
                    Value::from(v)
                } else if let Ok(v) = u64::try_from(i) {
                    Value::from(v)
                } else {
                    // Beyond what serde_json holds as an integer; unreachable
                    // for the bounded values profiles carry.
                    float_value(i as f64)
                }
            }
            PyNum::Float(f) => float_value(f),
        }
    }

    /// Python's exact comparison of two numbers (`None` when one is NaN).
    pub fn py_cmp(self, other: PyNum) -> Option<Ordering> {
        match (self, other) {
            (PyNum::Int(a), PyNum::Int(b)) => Some(a.cmp(&b)),
            (PyNum::Float(a), PyNum::Float(b)) => a.partial_cmp(&b),
            (PyNum::Int(a), PyNum::Float(b)) => cmp_int_float(a, b),
            (PyNum::Float(a), PyNum::Int(b)) => cmp_int_float(b, a).map(Ordering::reverse),
        }
    }

    /// `x == y`.
    pub fn py_eq(self, other: PyNum) -> bool {
        self.py_cmp(other) == Some(Ordering::Equal)
    }
}

impl std::ops::Add for PyNum {
    type Output = PyNum;

    /// `x + y`: int + int stays an int, anything with a float is a float.
    fn add(self, other: PyNum) -> PyNum {
        match (self, other) {
            (PyNum::Int(a), PyNum::Int(b)) => a
                .checked_add(b)
                .map_or(PyNum::Float(a as f64 + b as f64), PyNum::Int),
            (a, b) => PyNum::Float(a.as_f64() + b.as_f64()),
        }
    }
}

impl std::ops::Sub for PyNum {
    type Output = PyNum;

    /// `x - y`.
    fn sub(self, other: PyNum) -> PyNum {
        match (self, other) {
            (PyNum::Int(a), PyNum::Int(b)) => a
                .checked_sub(b)
                .map_or(PyNum::Float(a as f64 - b as f64), PyNum::Int),
            (a, b) => PyNum::Float(a.as_f64() - b.as_f64()),
        }
    }
}

/// A float as a JSON number; non-finite floats have no JSON spelling and
/// become `null`, as `pyfmt::loads_lenient` reads Python's `NaN` tokens.
pub fn float_value(f: f64) -> Value {
    Number::from_f64(f).map_or(Value::Null, Value::Number)
}

fn cmp_int_float(a: i128, b: f64) -> Option<Ordering> {
    if b.is_nan() {
        return None;
    }
    if b.is_infinite() {
        return Some(if b > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let floor = b.floor();
    // 2^127 is exactly representable; anything at or beyond it exceeds i128.
    const LIMIT: f64 = 170_141_183_460_469_231_731_687_303_715_884_105_728.0;
    if floor >= LIMIT {
        return Some(Ordering::Less);
    }
    if floor < -LIMIT {
        return Some(Ordering::Greater);
    }
    let floor_int = floor as i128;
    Some(match a.cmp(&floor_int) {
        Ordering::Equal if b > floor => Ordering::Less,
        other => other,
    })
}

/// Python `a == b` on the objects `json.loads` makes. `True == 1` and
/// `False == 0.0` hold, as `bool` is an `int` subclass.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    match (numeric(a), numeric(b)) {
        (Some(x), Some(y)) => return x.py_eq(y),
        (Some(_), None) | (None, Some(_)) => return false,
        (None, None) => {}
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| py_eq(v, w)))
        }
        _ => false,
    }
}

fn numeric(value: &Value) -> Option<PyNum> {
    match value {
        Value::Bool(b) => Some(PyNum::Int(i128::from(*b))),
        Value::Number(n) => Some(PyNum::from_number(n)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn arithmetic_keeps_the_int_float_distinction() {
        let ninety = PyNum::from_value(&json!(90)).expect("number");
        assert_eq!((ninety - PyNum::Int(2)).to_value(), json!(88));
        let ninety_f = PyNum::from_value(&json!(90.0)).expect("number");
        assert_eq!((ninety_f - PyNum::Int(2)).to_value(), json!(88.0));
        assert_eq!((ninety + PyNum::Float(0.5)).to_value(), json!(90.5));
    }

    #[test]
    fn mixed_comparisons_are_exact() {
        assert_eq!(
            PyNum::Int(1).py_cmp(PyNum::Float(1.0)),
            Some(Ordering::Equal)
        );
        assert_eq!(
            PyNum::Int(1).py_cmp(PyNum::Float(1.5)),
            Some(Ordering::Less)
        );
        assert_eq!(
            PyNum::Int(2).py_cmp(PyNum::Float(1.5)),
            Some(Ordering::Greater)
        );
        assert_eq!(
            PyNum::Int(9_007_199_254_740_993).py_cmp(PyNum::Float(9_007_199_254_740_992.0)),
            Some(Ordering::Greater)
        );
        assert_eq!(
            PyNum::Int(-1).py_cmp(PyNum::Float(-1.5)),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn equality_follows_python() {
        assert!(py_eq(&json!(true), &json!(1)));
        assert!(py_eq(&json!(1000), &json!(1000.0)));
        assert!(!py_eq(&json!("1"), &json!(1)));
        assert!(py_eq(&json!({"a": [1, 2.0]}), &json!({"a": [1.0, 2]})));
    }
}
