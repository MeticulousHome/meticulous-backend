//! Values crossing between JSON and SQLite the way Python's sqlite3 adapts them.

use rusqlite::types::{Value as SqlValue, ValueRef};
use serde_json::{Number, Value};

use crate::error::{Error, Result};

/// The SQLite value pysqlite binds for a Python value parsed from JSON:
/// `None` NULL, `bool` and `int` INTEGER, `float` REAL, `str` TEXT. Lists and
/// dicts are rejected (`Error binding parameter ... type 'dict' is not supported`).
pub fn bind(value: &Value) -> Result<SqlValue> {
    Ok(match value {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
        Value::Number(n) => number(n)?,
        Value::String(s) => SqlValue::Text(s.clone()),
        Value::Array(_) => {
            return Err(Error::InvalidValue(
                "Error binding parameter: type 'list' is not supported".into(),
            ));
        }
        Value::Object(_) => {
            return Err(Error::InvalidValue(
                "Error binding parameter: type 'dict' is not supported".into(),
            ));
        }
    })
}

fn number(n: &Number) -> Result<SqlValue> {
    if let Some(i) = n.as_i64() {
        Ok(SqlValue::Integer(i))
    } else if n.is_u64() {
        Err(Error::InvalidValue(
            "Python int too large to convert to SQLite INTEGER".into(),
        ))
    } else {
        Ok(SqlValue::Real(n.as_f64().unwrap_or(f64::NAN)))
    }
}

/// The Python value sqlite3 returns for a column, as JSON.
pub fn to_json(value: ValueRef<'_>) -> Result<Value> {
    Ok(match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        // PARITY: a stored infinity is `float('inf')` in Python, which
        // json.dumps writes as `Infinity`; JSON values here cannot hold it.
        ValueRef::Real(f) => Number::from_f64(f).map_or(Value::Null, Value::Number),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(_) => {
            return Err(Error::InvalidValue(
                "Object of type bytes is not JSON serializable".into(),
            ));
        }
    })
}

/// A SQLAlchemy `JSON` column: `json.loads(value)` of the stored text.
pub fn json_column(value: ValueRef<'_>) -> Result<Value> {
    match value {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Text(t) | ValueRef::Blob(t) => {
            let text = std::str::from_utf8(t)
                .map_err(|e| Error::InvalidValue(format!("JSON column is not UTF-8: {e}")))?;
            Ok(pyfmt::loads_lenient(text)?)
        }
        // JSON has NUMERIC affinity: a bare JSON number is stored as a number,
        // and json.loads() of an int raises TypeError in Python.
        ValueRef::Integer(_) | ValueRef::Real(_) => Err(Error::InvalidValue(
            "the JSON object must be str, bytes or bytearray".into(),
        )),
    }
}
