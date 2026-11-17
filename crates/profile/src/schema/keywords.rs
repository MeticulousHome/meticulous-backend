//! `jsonschema/_keywords.py` (and the draft 7 `items` from
//! `_legacy_keywords.py`): one function per keyword, yielding errors in the
//! same order with the same messages.

use std::cmp::Ordering;

use serde_json::Value;

use super::{Draft, PathElem, ValidationError, Validator, format};
use crate::pynum::PyNum;
use crate::pyrepr::{py_repr, py_repr_number, py_str};

/// The keywords `Draft7Validator.VALIDATORS` maps to a function.
const DRAFT7_KEYWORDS: &[&str] = &[
    "$ref",
    "additionalItems",
    "additionalProperties",
    "allOf",
    "anyOf",
    "const",
    "contains",
    "dependencies",
    "enum",
    "exclusiveMaximum",
    "exclusiveMinimum",
    "format",
    "if",
    "items",
    "maxItems",
    "maxLength",
    "maxProperties",
    "maximum",
    "minItems",
    "minLength",
    "minProperties",
    "minimum",
    "multipleOf",
    "not",
    "oneOf",
    "pattern",
    "patternProperties",
    "properties",
    "propertyNames",
    "required",
    "type",
    "uniqueItems",
];

/// The keywords `Draft202012Validator.VALIDATORS` maps to a function.
const DRAFT202012_KEYWORDS: &[&str] = &[
    "$dynamicRef",
    "$ref",
    "additionalProperties",
    "allOf",
    "anyOf",
    "const",
    "contains",
    "dependentRequired",
    "dependentSchemas",
    "enum",
    "exclusiveMaximum",
    "exclusiveMinimum",
    "format",
    "if",
    "items",
    "maxItems",
    "maxLength",
    "maxProperties",
    "maximum",
    "minItems",
    "minLength",
    "minProperties",
    "minimum",
    "multipleOf",
    "not",
    "oneOf",
    "pattern",
    "patternProperties",
    "prefixItems",
    "properties",
    "propertyNames",
    "required",
    "type",
    "unevaluatedItems",
    "unevaluatedProperties",
    "uniqueItems",
];

/// The keywords implemented here.
const IMPLEMENTED: &[&str] = &[
    "additionalProperties",
    "allOf",
    "anyOf",
    "const",
    "enum",
    "exclusiveMaximum",
    "exclusiveMinimum",
    "format",
    "items",
    "maxItems",
    "maxLength",
    "maxProperties",
    "maximum",
    "minItems",
    "minLength",
    "minProperties",
    "minimum",
    "multipleOf",
    "not",
    "oneOf",
    "pattern",
    "prefixItems",
    "properties",
    "required",
    "type",
];

const TYPE_NAMES: &[&str] = &[
    "array", "boolean", "integer", "null", "number", "object", "string",
];

/// The formats checked when the validator has a format checker.
const CHECKED_FORMATS: &[&str] = &["uuid"];

fn known_keywords(draft: Draft) -> &'static [&'static str] {
    match draft {
        Draft::Draft7 => DRAFT7_KEYWORDS,
        Draft::Draft202012 => DRAFT202012_KEYWORDS,
    }
}

/// The errors of `keyword` (with value `value`, in `schema`) on `instance`.
pub(super) fn apply(
    v: &Validator,
    keyword: &str,
    value: &Value,
    instance: &Value,
    schema: &Value,
) -> Vec<ValidationError> {
    if !known_keywords(v.draft).contains(&keyword) {
        return Vec::new();
    }
    match keyword {
        "type" => type_(value, instance),
        "properties" => properties(v, value, instance),
        "required" => required(value, instance),
        "additionalProperties" => additional_properties(v, value, instance, schema),
        "items" => match v.draft {
            Draft::Draft7 => items_draft7(v, value, instance),
            Draft::Draft202012 => items(v, value, instance, schema),
        },
        "prefixItems" => prefix_items(v, value, instance),
        "const" => const_(value, instance),
        "enum" => enum_(value, instance),
        "minimum" => bound(
            value,
            instance,
            |o| o == Ordering::Less,
            "is less than the minimum of",
        ),
        "maximum" => bound(
            value,
            instance,
            |o| o == Ordering::Greater,
            "is greater than the maximum of",
        ),
        "exclusiveMinimum" => bound(
            value,
            instance,
            |o| o != Ordering::Greater,
            "is less than or equal to the minimum of",
        ),
        "exclusiveMaximum" => bound(
            value,
            instance,
            |o| o != Ordering::Less,
            "is greater than or equal to the maximum of",
        ),
        "multipleOf" => multiple_of(value, instance),
        "minItems" => length_bound(value, instance.as_array().map(Vec::len), instance, true),
        "maxItems" => length_bound(value, instance.as_array().map(Vec::len), instance, false),
        "minLength" => length_bound(
            value,
            instance.as_str().map(|s| s.chars().count()),
            instance,
            true,
        ),
        "maxLength" => length_bound(
            value,
            instance.as_str().map(|s| s.chars().count()),
            instance,
            false,
        ),
        "minProperties" => properties_bound(value, instance, true),
        "maxProperties" => properties_bound(value, instance, false),
        "pattern" => pattern(v, value, instance),
        "format" => format_(v, value, instance),
        "anyOf" => any_of(v, value, instance),
        "allOf" => all_of(v, value, instance),
        "oneOf" => one_of(v, value, instance),
        "not" => not_(v, value, instance),
        // A keyword `jsonschema` knows but this module does not implement;
        // `find_unsupported` reports it.
        _ => Vec::new(),
    }
}

/// `validator.is_type(instance, type)` with the draft 4+ type checker.
pub(super) fn is_type(instance: &Value, type_name: &str) -> bool {
    match type_name {
        "array" => instance.is_array(),
        "boolean" => instance.is_boolean(),
        "integer" => match instance {
            Value::Number(n) => !n.is_f64() || n.as_f64().is_some_and(|f| f.fract() == 0.0),
            _ => false,
        },
        "null" => instance.is_null(),
        "number" => instance.is_number(),
        "object" => instance.is_object(),
        "string" => instance.is_string(),
        _ => false,
    }
}

/// `jsonschema._utils.equal`: Python equality, except that a bool only
/// equals a bool.
pub(super) fn equal(one: &Value, two: &Value) -> bool {
    match (one, two) {
        (Value::String(a), Value::String(b)) => a == b,
        (Value::String(_), _) | (_, Value::String(_)) => false,
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| equal(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(k, x)| b.get(k).is_some_and(|y| equal(x, y)))
        }
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Bool(_), _) | (_, Value::Bool(_)) => false,
        (Value::Number(a), Value::Number(b)) => PyNum::from_number(a).py_eq(PyNum::from_number(b)),
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

fn type_(types: &Value, instance: &Value) -> Vec<ValidationError> {
    // `ensure_list`: a single str becomes a list of one.
    let names: Vec<&Value> = match types {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    let matched = names
        .iter()
        .any(|t| t.as_str().is_some_and(|t| is_type(instance, t)));
    if matched {
        return Vec::new();
    }
    let reprs: Vec<String> = names.iter().map(|t| py_repr(t)).collect();
    vec![ValidationError::new(format!(
        "{} is not of type {}",
        py_repr(instance),
        reprs.join(", ")
    ))]
}

fn properties(v: &Validator, properties: &Value, instance: &Value) -> Vec<ValidationError> {
    let (Value::Object(instance_map), Value::Object(properties)) = (instance, properties) else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    for (property, subschema) in properties {
        if let Some(child) = instance_map.get(property) {
            errors.extend(v.descend_at(
                child,
                subschema,
                Some(PathElem::Key(property.clone())),
                Some(PathElem::Key(property.clone())),
            ));
        }
    }
    errors
}

fn required(required: &Value, instance: &Value) -> Vec<ValidationError> {
    let (Value::Object(map), Value::Array(required)) = (instance, required) else {
        return Vec::new();
    };
    required
        .iter()
        .filter(|property| property.as_str().is_none_or(|p| !map.contains_key(p)))
        .map(|property| {
            ValidationError::new(format!("{} is a required property", py_repr(property)))
        })
        .collect()
}

fn additional_properties(
    v: &Validator,
    additional: &Value,
    instance: &Value,
    schema: &Value,
) -> Vec<ValidationError> {
    let Value::Object(map) = instance else {
        return Vec::new();
    };
    let declared = schema.get("properties").and_then(Value::as_object);
    let extras: Vec<&String> = map
        .keys()
        .filter(|key| declared.is_none_or(|d| !d.contains_key(*key)))
        .collect();
    match additional {
        Value::Object(_) => {
            // Python walks a set here, whose order depends on string hashing;
            // the bundled schemas never use a schema for additionalProperties.
            let mut errors = Vec::new();
            for extra in extras {
                errors.extend(v.descend_at(
                    &map[extra],
                    additional,
                    Some(PathElem::Key(extra.clone())),
                    None,
                ));
            }
            errors
        }
        Value::Bool(false) if !extras.is_empty() => {
            let mut sorted: Vec<&String> = extras;
            sorted.sort();
            let verb = if sorted.len() == 1 { "was" } else { "were" };
            let names: Vec<String> = sorted
                .iter()
                .map(|k| crate::pyrepr::py_repr_str(k))
                .collect();
            vec![ValidationError::new(format!(
                "Additional properties are not allowed ({} {verb} unexpected)",
                names.join(", ")
            ))]
        }
        _ => Vec::new(),
    }
}

/// Draft 7 `items`: one schema for every item, or one schema per position.
fn items_draft7(v: &Validator, items: &Value, instance: &Value) -> Vec<ValidationError> {
    let Value::Array(elements) = instance else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    match items {
        Value::Array(schemas) => {
            for (index, (item, subschema)) in elements.iter().zip(schemas).enumerate() {
                errors.extend(v.descend_at(
                    item,
                    subschema,
                    Some(PathElem::Index(index)),
                    Some(PathElem::Index(index)),
                ));
            }
        }
        _ => {
            for (index, item) in elements.iter().enumerate() {
                errors.extend(v.descend_at(item, items, Some(PathElem::Index(index)), None));
            }
        }
    }
    errors
}

/// Draft 2020-12 `items`: the items after `prefixItems`.
fn items(v: &Validator, items: &Value, instance: &Value, schema: &Value) -> Vec<ValidationError> {
    let Value::Array(elements) = instance else {
        return Vec::new();
    };
    let prefix = schema
        .get("prefixItems")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let total = elements.len();
    if total <= prefix {
        return Vec::new();
    }
    let extra = total - prefix;
    if items == &Value::Bool(false) {
        let rest = if extra != 1 {
            Value::Array(elements[prefix..].to_vec())
        } else {
            elements[prefix].clone()
        };
        let noun = if prefix != 1 { "items" } else { "item" };
        return vec![ValidationError::new(format!(
            "Expected at most {prefix} {noun} but found {extra} extra: {}",
            py_repr(&rest)
        ))];
    }
    let mut errors = Vec::new();
    for (index, item) in elements.iter().enumerate().skip(prefix) {
        errors.extend(v.descend_at(item, items, Some(PathElem::Index(index)), None));
    }
    errors
}

fn prefix_items(v: &Validator, prefix: &Value, instance: &Value) -> Vec<ValidationError> {
    let (Value::Array(elements), Value::Array(schemas)) = (instance, prefix) else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    for (index, (item, subschema)) in elements.iter().zip(schemas).enumerate() {
        errors.extend(v.descend_at(
            item,
            subschema,
            Some(PathElem::Index(index)),
            Some(PathElem::Index(index)),
        ));
    }
    errors
}

fn const_(constant: &Value, instance: &Value) -> Vec<ValidationError> {
    if equal(instance, constant) {
        return Vec::new();
    }
    vec![ValidationError::new(format!(
        "{} was expected",
        py_repr(constant)
    ))]
}

fn enum_(enums: &Value, instance: &Value) -> Vec<ValidationError> {
    let found = match enums {
        Value::Array(items) => items.iter().any(|each| equal(each, instance)),
        // Iterating a dict yields its keys.
        Value::Object(map) => map
            .keys()
            .any(|key| equal(&Value::String(key.clone()), instance)),
        _ => false,
    };
    if found {
        return Vec::new();
    }
    vec![ValidationError::new(format!(
        "{} is not one of {}",
        py_repr(instance),
        py_repr(enums)
    ))]
}

/// `minimum`, `maximum`, `exclusiveMinimum` and `exclusiveMaximum`: fail
/// when the comparison of the instance with the bound satisfies `fails`.
fn bound(
    limit: &Value,
    instance: &Value,
    fails: impl Fn(Ordering) -> bool,
    phrase: &str,
) -> Vec<ValidationError> {
    let (Value::Number(n), Value::Number(l)) = (instance, limit) else {
        return Vec::new();
    };
    let Some(order) = PyNum::from_number(n).py_cmp(PyNum::from_number(l)) else {
        return Vec::new();
    };
    if !fails(order) {
        return Vec::new();
    }
    vec![ValidationError::new(format!(
        "{} {phrase} {}",
        py_repr_number(n),
        py_repr_number(l)
    ))]
}

fn multiple_of(divisor: &Value, instance: &Value) -> Vec<ValidationError> {
    let (Value::Number(n), Value::Number(d)) = (instance, divisor) else {
        return Vec::new();
    };
    let x = PyNum::from_number(n);
    let failed = match PyNum::from_number(d) {
        PyNum::Float(db) => {
            let quotient = x.as_f64() / db;
            if quotient.is_finite() {
                quotient.trunc() != quotient
            } else if quotient.is_nan() {
                false
            } else {
                // `int(inf)` overflows; Python falls back to exact fractions.
                !dyadic_quotient_is_integer(x, db)
            }
        }
        PyNum::Int(0) => false,
        PyNum::Int(db) => match x {
            PyNum::Int(xi) => xi % db != 0,
            PyNum::Float(xf) => (xf % db as f64) != 0.0,
        },
    };
    if !failed {
        return Vec::new();
    }
    // `{dB}` is `str(dB)`.
    vec![ValidationError::new(format!(
        "{} is not a multiple of {}",
        py_repr_number(n),
        py_str(divisor)
    ))]
}

/// Whether `x / d` is an integer, computed exactly on the binary values.
fn dyadic_quotient_is_integer(x: PyNum, d: f64) -> bool {
    fn split(f: f64) -> (u128, i32) {
        // f = mantissa * 2^exponent with an odd mantissa (or zero).
        let bits = f.abs().to_bits();
        let exp_bits = ((bits >> 52) & 0x7ff) as i32;
        let frac = bits & ((1u64 << 52) - 1);
        let (mut m, mut e) = if exp_bits == 0 {
            (u128::from(frac), -1074)
        } else {
            (u128::from(frac | (1u64 << 52)), exp_bits - 1075)
        };
        if m == 0 {
            return (0, 0);
        }
        let tz = m.trailing_zeros();
        m >>= tz;
        e += tz as i32;
        (m, e)
    }
    let (xm, xe) = match x {
        PyNum::Int(i) => {
            let mut m = i.unsigned_abs();
            if m == 0 {
                return true;
            }
            let tz = m.trailing_zeros();
            m >>= tz;
            (m, tz as i32)
        }
        PyNum::Float(f) => split(f),
    };
    if xm == 0 {
        return true;
    }
    let (dm, de) = split(d);
    if dm == 0 {
        return false;
    }
    xm % dm == 0 && xe >= de
}

/// `minItems`/`maxItems`/`minLength`/`maxLength`.
fn length_bound(
    limit: &Value,
    length: Option<usize>,
    instance: &Value,
    minimum: bool,
) -> Vec<ValidationError> {
    let (Some(length), Value::Number(l)) = (length, limit) else {
        return Vec::new();
    };
    let limit = PyNum::from_number(l);
    let Some(order) = PyNum::Int(length as i128).py_cmp(limit) else {
        return Vec::new();
    };
    let message = if minimum {
        if order != Ordering::Less {
            return Vec::new();
        }
        if limit.py_eq(PyNum::Int(1)) {
            "should be non-empty"
        } else {
            "is too short"
        }
    } else {
        if order != Ordering::Greater {
            return Vec::new();
        }
        if limit.py_eq(PyNum::Int(0)) {
            "is expected to be empty"
        } else {
            "is too long"
        }
    };
    vec![ValidationError::new(format!(
        "{} {message}",
        py_repr(instance)
    ))]
}

fn properties_bound(limit: &Value, instance: &Value, minimum: bool) -> Vec<ValidationError> {
    let (Value::Object(map), Value::Number(l)) = (instance, limit) else {
        return Vec::new();
    };
    let limit = PyNum::from_number(l);
    let Some(order) = PyNum::Int(map.len() as i128).py_cmp(limit) else {
        return Vec::new();
    };
    let message = if minimum {
        if order != Ordering::Less {
            return Vec::new();
        }
        if limit.py_eq(PyNum::Int(1)) {
            "should be non-empty"
        } else {
            "does not have enough properties"
        }
    } else {
        if order != Ordering::Greater {
            return Vec::new();
        }
        if limit.py_eq(PyNum::Int(0)) {
            "is expected to be empty"
        } else {
            "has too many properties"
        }
    };
    vec![ValidationError::new(format!(
        "{} {message}",
        py_repr(instance)
    ))]
}

fn pattern(v: &Validator, pattern: &Value, instance: &Value) -> Vec<ValidationError> {
    let (Value::String(text), Value::String(p)) = (instance, pattern) else {
        return Vec::new();
    };
    let Some(regex) = v.patterns.get(p) else {
        return Vec::new();
    };
    if regex.is_match(text) {
        return Vec::new();
    }
    vec![ValidationError::new(format!(
        "{} does not match {}",
        py_repr(instance),
        py_repr(pattern)
    ))]
}

fn format_(v: &Validator, format_name: &Value, instance: &Value) -> Vec<ValidationError> {
    if !v.check_formats {
        return Vec::new();
    }
    let ok = match (format_name.as_str(), instance) {
        (Some("uuid"), Value::String(s)) => format::is_uuid(s),
        _ => true,
    };
    if ok {
        return Vec::new();
    }
    vec![ValidationError::new(format!(
        "{} is not a {}",
        py_repr(instance),
        py_repr(format_name)
    ))]
}

fn any_of(v: &Validator, schemas: &Value, instance: &Value) -> Vec<ValidationError> {
    let Value::Array(schemas) = schemas else {
        return Vec::new();
    };
    let mut all_errors = Vec::new();
    for (index, subschema) in schemas.iter().enumerate() {
        let errors = v.descend_at(instance, subschema, None, Some(PathElem::Index(index)));
        if errors.is_empty() {
            return Vec::new();
        }
        all_errors.extend(errors);
    }
    let mut error = ValidationError::new(format!(
        "{} is not valid under any of the given schemas",
        py_repr(instance)
    ));
    error.context = all_errors;
    vec![error]
}

fn all_of(v: &Validator, schemas: &Value, instance: &Value) -> Vec<ValidationError> {
    let Value::Array(schemas) = schemas else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    for (index, subschema) in schemas.iter().enumerate() {
        errors.extend(v.descend_at(instance, subschema, None, Some(PathElem::Index(index))));
    }
    errors
}

fn one_of(v: &Validator, schemas: &Value, instance: &Value) -> Vec<ValidationError> {
    let Value::Array(schemas) = schemas else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut all_errors = Vec::new();
    let mut first_valid = None;
    for (index, subschema) in schemas.iter().enumerate() {
        let errors = v.descend_at(instance, subschema, None, Some(PathElem::Index(index)));
        if errors.is_empty() {
            first_valid = Some(index);
            break;
        }
        all_errors.extend(errors);
    }
    let Some(first) = first_valid else {
        let mut error = ValidationError::new(format!(
            "{} is not valid under any of the given schemas",
            py_repr(instance)
        ));
        error.context = all_errors;
        out.push(error);
        return out;
    };
    // The rest of the same iterator: only the subschemas after the first
    // valid one are tried.
    let mut more_valid: Vec<&Value> = schemas[first + 1..]
        .iter()
        .filter(|each| v.descend(instance, each).is_empty())
        .collect();
    if !more_valid.is_empty() {
        more_valid.push(&schemas[first]);
        let reprs: Vec<String> = more_valid.iter().map(|s| py_repr(s)).collect();
        out.push(ValidationError::new(format!(
            "{} is valid under each of {}",
            py_repr(instance),
            reprs.join(", ")
        )));
    }
    out
}

fn not_(v: &Validator, not_schema: &Value, instance: &Value) -> Vec<ValidationError> {
    if !v.descend(instance, not_schema).is_empty() {
        return Vec::new();
    }
    vec![ValidationError::new(format!(
        "{} should not be valid under {}",
        py_repr(instance),
        py_repr(not_schema)
    ))]
}

/// Record every keyword under `schema` that `jsonschema` would apply but
/// `apply` does not implement, and every format or type name it would
/// handle differently.
pub(super) fn find_unsupported(v: &Validator, schema: &Value, at: &str, out: &mut Vec<String>) {
    let Value::Object(map) = schema else {
        return;
    };
    let known = known_keywords(v.draft);
    for (keyword, value) in map {
        let here = format!("{at}/{keyword}");
        if known.contains(&keyword.as_str()) && !IMPLEMENTED.contains(&keyword.as_str()) {
            out.push(here.clone());
            continue;
        }
        if v.draft == Draft::Draft7 && keyword == "prefixItems" {
            continue;
        }
        match keyword.as_str() {
            "properties" => {
                if let Value::Object(props) = value {
                    for (name, sub) in props {
                        find_unsupported(v, sub, &format!("{here}/{name}"), out);
                    }
                }
            }
            "items" | "prefixItems" | "anyOf" | "allOf" | "oneOf" => match value {
                Value::Array(subs) => {
                    for (i, sub) in subs.iter().enumerate() {
                        find_unsupported(v, sub, &format!("{here}/{i}"), out);
                    }
                }
                other => find_unsupported(v, other, &here, out),
            },
            "additionalProperties" | "not" => find_unsupported(v, value, &here, out),
            "format" if v.check_formats => {
                if !value.as_str().is_some_and(|f| CHECKED_FORMATS.contains(&f)) {
                    out.push(here);
                }
            }
            "type" => {
                let names: Vec<&Value> = match value {
                    Value::Array(items) => items.iter().collect(),
                    other => vec![other],
                };
                if !names
                    .iter()
                    .all(|t| t.as_str().is_some_and(|t| TYPE_NAMES.contains(&t)))
                {
                    out.push(here);
                }
            }
            _ => {}
        }
    }
}
