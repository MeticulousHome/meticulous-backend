//! `profile_preprocessor.py`: replace `$variable` references with their values.
//!
//! A stage point, exit trigger value or limit value may be a string `"$key"`
//! naming an entry of the profile's `variables`; the ESP32 only understands
//! numbers, so the references are resolved before the profile is sent. The
//! backend reports a failure to clients as
//! `f"variable error: {err.__class__.__name__}:{err}"`, so the exception class
//! names and messages here are part of the HTTP contract.
//!
//! The Python indexes into the document without checking its shape first:
//! wrong shapes surface as the `KeyError` or `TypeError` CPython raises, which
//! it reports inside a `FormatException`, or, for the errors it does not
//! catch, as the built-in exception itself. [`process_variables`] follows the
//! same steps on the same values and fails with the same exception.

use std::collections::HashMap;

use serde_json::Value;

use crate::pynum::py_eq;
use crate::pyops::{
    self, PyExc, PyIter, check_hashable, contains_str, getitem_index, getitem_str, iterate,
    setitem_index, setitem_str,
};
use crate::pyrepr::py_str;

/// An exception `ProfilePreprocessor.processVariables` raises.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PreprocessError {
    /// `UndefinedVariableException`: a `$key` names no variable.
    #[error("{0}")]
    UndefinedVariable(String),
    /// `VariableTypeException`: the variable's type is not the one its use
    /// expects.
    #[error("{0}")]
    VariableType(String),
    /// `FormatException`: the document's shape is wrong.
    #[error("{0}")]
    Format(String),
    /// A built-in exception the Python lets through (raised outside its
    /// `try`, or not a `KeyError`/`TypeError`): `IndexError` for a short
    /// point, `KeyError`/`TypeError` from a malformed `variables` list,
    /// `AttributeError` for a document that is not an object.
    #[error("{}", .0.message())]
    Builtin(PyExc),
}

impl PreprocessError {
    /// `err.__class__.__name__`.
    pub fn class_name(&self) -> &'static str {
        match self {
            PreprocessError::UndefinedVariable(_) => "UndefinedVariableException",
            PreprocessError::VariableType(_) => "VariableTypeException",
            PreprocessError::Format(_) => "FormatException",
            PreprocessError::Builtin(exc) => exc.class_name(),
        }
    }

    /// One of the three classes the load handler reports as a
    /// `variable error`; the built-in ones fall through to its generic
    /// `failed to load profile` answer.
    pub fn is_variable_error(&self) -> bool {
        !matches!(self, PreprocessError::Builtin(_))
    }
}

/// What can go wrong inside the Python's `try`: its own exceptions pass
/// through, the built-in ones are sorted by the `except` clauses.
#[derive(Clone)]
enum Raised {
    Own(PreprocessError),
    Builtin(PyExc),
}

impl From<PyExc> for Raised {
    fn from(exc: PyExc) -> Self {
        Raised::Builtin(exc)
    }
}

impl From<PreprocessError> for Raised {
    fn from(err: PreprocessError) -> Self {
        Raised::Own(err)
    }
}

/// `variables_map`: key -> (value, type). Only str keys can ever be looked up.
type Variables = HashMap<String, (Value, Value)>;

/// `ProfilePreprocessor.processVariables(profile)`: a copy of `profile` with
/// every `$key` reference in stage points, exit trigger values and limit
/// values replaced by the variable's value.
pub fn process_variables(in_profile: &Value) -> Result<Value, PreprocessError> {
    let mut profile = in_profile.clone();

    let Value::Object(map) = &profile else {
        return Err(PreprocessError::Builtin(pyops::no_attribute_get(&profile)));
    };
    // Built before the `try`, so its exceptions are not wrapped.
    let variables = variables_map(map.get("variables")).map_err(PreprocessError::Builtin)?;

    match process_stages(&mut profile, &variables) {
        Ok(()) => Ok(profile),
        Err(Raised::Own(err)) => Err(err),
        Err(Raised::Builtin(PyExc::Key(key))) => Err(PreprocessError::Format(format!(
            "Missing field detected: {key}"
        ))),
        Err(Raised::Builtin(PyExc::Type(message))) => Err(PreprocessError::Format(format!(
            "Invalid format detected: {message}"
        ))),
        Err(Raised::Builtin(other)) => Err(PreprocessError::Builtin(other)),
    }
}

/// `{var["key"]: (var["value"], var["type"]) for var in profile.get("variables", [])}`
fn variables_map(variables: Option<&Value>) -> Result<Variables, PyExc> {
    let mut map = Variables::new();
    let Some(variables) = variables else {
        return Ok(map);
    };
    let items: Vec<Value> = match iterate(variables)? {
        PyIter::List => variables.as_array().cloned().unwrap_or_default(),
        PyIter::Items(items) => items,
    };
    for var in &items {
        // The key, then the value, then the insertion that hashes the key.
        let key = getitem_str(var, "key")?;
        let value = getitem_str(var, "value")?;
        let var_type = getitem_str(var, "type")?;
        check_hashable(key)?;
        if let Value::String(key) = key {
            map.insert(key.clone(), (value.clone(), var_type.clone()));
        }
    }
    Ok(map)
}

fn process_stages(profile: &mut Value, variables: &Variables) -> Result<(), Raised> {
    let Some(stages) = profile.get_mut("stages") else {
        return Ok(());
    };
    for_each_item(stages, |index, stage| {
        process_stage(index, stage, variables)
    })
}

/// `for index, item in enumerate(container): f(index, item)`, where a list's
/// items are its own (mutable) elements and other iterables yield fresh
/// objects.
fn for_each_item(
    container: &mut Value,
    mut f: impl FnMut(usize, &mut Value) -> Result<(), Raised>,
) -> Result<(), Raised> {
    match iterate(container)? {
        PyIter::List => {
            if let Value::Array(items) = container {
                for (index, item) in items.iter_mut().enumerate() {
                    f(index, item)?;
                }
            }
        }
        PyIter::Items(mut items) => {
            for (index, item) in items.iter_mut().enumerate() {
                f(index, item)?;
            }
        }
    }
    Ok(())
}

fn process_stage(index: usize, stage: &mut Value, variables: &Variables) -> Result<(), Raised> {
    if !contains_str(stage, "type")? {
        return Err(PreprocessError::Format(format!("stage {index} missing 'type' field")).into());
    }
    if !contains_str(stage, "dynamics")? {
        return Err(
            PreprocessError::Format(format!("stage {index} missing 'dynamics' field")).into(),
        );
    }
    if !contains_str(getitem_str(stage, "dynamics")?, "points")? {
        return Err(PreprocessError::Format(format!(
            "stage {index} dynamics are missing the points array"
        ))
        .into());
    }
    // `stage["dynamics"]` succeeded, so the stage is a dict from here on.
    let stage_type = getitem_str(stage, "type")?.clone();
    getitem_str(getitem_str(stage, "dynamics")?, "points")?;

    // `stage.get(stage["dynamics"]["over"], "time")`, evaluated for every
    // point. PARITY: this looks the `over` value up as a key of the *stage*,
    // not of its dynamics, so the x type is "time" unless the stage happens
    // to have a key named like the `over` value.
    let x_type = point_x_type(stage);

    let points = stage
        .get_mut("dynamics")
        .and_then(|dynamics| dynamics.get_mut("points"))
        .expect("checked above: stage[\"dynamics\"][\"points\"] exists");
    for_each_item(points, |_, point| {
        let x_type = x_type.clone()?;
        let x = replace_variable(getitem_index(point, 0)?, &x_type, variables)?;
        setitem_index(point, 0, x)?;
        let y = replace_variable(getitem_index(point, 1)?, &stage_type, variables)?;
        setitem_index(point, 1, y)?;
        Ok(())
    })?;

    for (field, label) in [("exit_triggers", "exitTrigger"), ("limits", "limit")] {
        let Some(entries) = stage.get_mut(field) else {
            continue;
        };
        for_each_item(entries, |index, entry| {
            if !contains_str(entry, "type")? {
                return Err(PreprocessError::Format(format!(
                    "{label} {index} missing 'type' field"
                ))
                .into());
            }
            if !contains_str(entry, "value")? {
                return Err(PreprocessError::Format(format!(
                    "{label} {index} missing 'value' field"
                ))
                .into());
            }
            let value = getitem_str(entry, "value")?.clone();
            let expected = getitem_str(entry, "type")?.clone();
            let replaced = replace_variable(value, &expected, variables)?;
            setitem_str(entry, "value", replaced)?;
            Ok(())
        })?;
    }
    Ok(())
}

fn point_x_type(stage: &Value) -> Result<Value, Raised> {
    let over = getitem_str(getitem_str(stage, "dynamics")?, "over")?;
    check_hashable(over)?;
    Ok(match over {
        Value::String(key) => stage
            .get(key)
            .cloned()
            .unwrap_or_else(|| Value::String("time".into())),
        _ => Value::String("time".into()),
    })
}

/// `ProfilePreprocessor._replace_variable`.
fn replace_variable(
    value_or_variable: Value,
    expected_type: &Value,
    variables: &Variables,
) -> Result<Value, Raised> {
    let Value::String(text) = &value_or_variable else {
        return Ok(value_or_variable);
    };
    let Some(var_key) = text.strip_prefix('$') else {
        return Err(PreprocessError::Format(format!(
            "Entry {text} is not referencing a variable but is a string"
        ))
        .into());
    };
    let Some((var_value, var_type)) = variables.get(var_key) else {
        return Err(PreprocessError::UndefinedVariable(format!(
            "Variable {var_key} is not defined"
        ))
        .into());
    };
    if !py_eq(var_type, expected_type) {
        return Err(PreprocessError::VariableType(format!(
            "Variable {var_key} of type {} used as {}",
            py_str(var_type),
            py_str(expected_type)
        ))
        .into());
    }
    Ok(var_value.clone())
}
