//! Python's subscript, membership and iteration rules on JSON values.
//!
//! `ProfilePreprocessor.processVariables` indexes into client documents
//! without checking their shape and turns the resulting `KeyError` and
//! `TypeError` into its own `FormatException`, quoting the built-in message.
//! These helpers raise the exception CPython 3.11 raises for the same
//! operation on the object `json.loads` made of the value.

use serde_json::Value;

use crate::pyrepr::{py_repr_str, type_name};

/// A built-in Python exception.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyExc {
    /// `KeyError`; the payload is `str(error)`, the repr of the key.
    Key(String),
    Type(String),
    Index(String),
    Attribute(String),
}

impl PyExc {
    pub fn class_name(&self) -> &'static str {
        match self {
            PyExc::Key(_) => "KeyError",
            PyExc::Type(_) => "TypeError",
            PyExc::Index(_) => "IndexError",
            PyExc::Attribute(_) => "AttributeError",
        }
    }

    /// `str(error)`.
    pub fn message(&self) -> &str {
        match self {
            PyExc::Key(m) | PyExc::Type(m) | PyExc::Index(m) | PyExc::Attribute(m) => m,
        }
    }
}

/// `container[key]` with a `str` key.
pub fn getitem_str<'a>(container: &'a Value, key: &str) -> Result<&'a Value, PyExc> {
    match container {
        Value::Object(map) => map.get(key).ok_or_else(|| PyExc::Key(py_repr_str(key))),
        Value::Array(_) => Err(PyExc::Type(
            "list indices must be integers or slices, not str".into(),
        )),
        Value::String(_) => Err(PyExc::Type(
            "string indices must be integers, not 'str'".into(),
        )),
        other => Err(not_subscriptable(other)),
    }
}

/// `container[index]` with a non-negative `int` index.
pub fn getitem_index(container: &Value, index: usize) -> Result<Value, PyExc> {
    match container {
        Value::Array(items) => items
            .get(index)
            .cloned()
            .ok_or_else(|| PyExc::Index("list index out of range".into())),
        Value::String(s) => s
            .chars()
            .nth(index)
            .map(|c| Value::String(c.to_string()))
            .ok_or_else(|| PyExc::Index("string index out of range".into())),
        // A JSON object only has str keys, so an int key is never found.
        Value::Object(_) => Err(PyExc::Key(index.to_string())),
        other => Err(not_subscriptable(other)),
    }
}

/// `container[index] = value`.
pub fn setitem_index(container: &mut Value, index: usize, value: Value) -> Result<(), PyExc> {
    match container {
        Value::Array(items) => match items.get_mut(index) {
            Some(slot) => {
                *slot = value;
                Ok(())
            }
            None => Err(PyExc::Index("list assignment index out of range".into())),
        },
        other => Err(no_item_assignment(other)),
    }
}

/// `container[key] = value` with a `str` key.
pub fn setitem_str(container: &mut Value, key: &str, value: Value) -> Result<(), PyExc> {
    match container {
        Value::Object(map) => {
            map.insert(key.to_string(), value);
            Ok(())
        }
        Value::Array(_) => Err(PyExc::Type(
            "list indices must be integers or slices, not str".into(),
        )),
        other => Err(no_item_assignment(other)),
    }
}

/// `key in container` with a `str` key.
pub fn contains_str(container: &Value, key: &str) -> Result<bool, PyExc> {
    match container {
        Value::Object(map) => Ok(map.contains_key(key)),
        Value::Array(items) => Ok(items.iter().any(|item| item.as_str() == Some(key))),
        Value::String(s) => Ok(s.contains(key)),
        other => Err(PyExc::Type(format!(
            "argument of type '{}' is not iterable",
            type_name(other)
        ))),
    }
}

/// What `for item in container` yields, when it is not a list: the keys of
/// a dict and the characters of a str, as new str objects.
pub enum PyIter {
    /// A list: its items are the container's own elements.
    List,
    /// Anything else iterable: the items, which are fresh objects.
    Items(Vec<Value>),
}

/// `iter(container)`.
pub fn iterate(container: &Value) -> Result<PyIter, PyExc> {
    match container {
        Value::Array(_) => Ok(PyIter::List),
        Value::Object(map) => Ok(PyIter::Items(
            map.keys().map(|k| Value::String(k.clone())).collect(),
        )),
        Value::String(s) => Ok(PyIter::Items(
            s.chars().map(|c| Value::String(c.to_string())).collect(),
        )),
        other => Err(not_iterable(other)),
    }
}

/// `hash(value)` fails for lists and dicts.
pub fn check_hashable(value: &Value) -> Result<(), PyExc> {
    match value {
        Value::Array(_) | Value::Object(_) => Err(PyExc::Type(format!(
            "unhashable type: '{}'",
            type_name(value)
        ))),
        _ => Ok(()),
    }
}

pub fn not_iterable(value: &Value) -> PyExc {
    PyExc::Type(format!("'{}' object is not iterable", type_name(value)))
}

fn not_subscriptable(value: &Value) -> PyExc {
    PyExc::Type(format!(
        "'{}' object is not subscriptable",
        type_name(value)
    ))
}

fn no_item_assignment(value: &Value) -> PyExc {
    PyExc::Type(format!(
        "'{}' object does not support item assignment",
        type_name(value)
    ))
}

/// `value.get(...)` on something that is not a dict.
pub fn no_attribute_get(value: &Value) -> PyExc {
    PyExc::Attribute(format!(
        "'{}' object has no attribute 'get'",
        type_name(value)
    ))
}
