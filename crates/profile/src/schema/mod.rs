//! A JSON Schema validator that reports what Python's `jsonschema` 4.23 does.
//!
//! The backend answers an invalid espresso profile with
//! `f"JSON validation error: {err.message}"`, where `err` is the error
//! `jsonschema.validate` raises: `best_match` over every error the
//! validator yields, in the order it yields them. Pour Over profiles list
//! every error with its path. Matching those answers needs the same errors,
//! in the same order, with the same `%r`-formatted messages, so this module
//! follows `jsonschema`'s keyword functions one by one rather than wrapping a
//! Rust validator whose errors and ordering differ.
//!
//! Only the keywords the bundled schemas can use are implemented;
//! [`Validator::unsupported_keywords`] lists any other keyword a schema
//! contains, and the crate's tests assert the bundled schemas have none.

mod format;
mod keywords;
mod pattern;

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};

use regex::Regex;
use serde_json::Value;

pub use format::is_uuid;

/// The JSON Schema dialect, from the schema's `$schema`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Draft {
    Draft7,
    Draft202012,
}

/// One step of an instance or schema path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathElem {
    Key(String),
    Index(usize),
}

impl PathElem {
    /// The step as Python has it in `error.path`: a str or an int.
    pub fn to_json(&self) -> Value {
        match self {
            PathElem::Key(k) => Value::String(k.clone()),
            PathElem::Index(i) => Value::from(*i),
        }
    }

    /// `str(part)`.
    pub fn to_py_str(&self) -> String {
        match self {
            PathElem::Key(k) => k.clone(),
            PathElem::Index(i) => i.to_string(),
        }
    }
}

impl PartialOrd for PathElem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PathElem {
    /// Python compares path deques element by element. Two paths into the
    /// same instance first differ inside one container, so the two steps
    /// are both keys or both indexes; the mixed case is only ordered to keep
    /// `Ord` total.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (PathElem::Key(a), PathElem::Key(b)) => a.cmp(b),
            (PathElem::Index(a), PathElem::Index(b)) => a.cmp(b),
            (PathElem::Index(_), PathElem::Key(_)) => Ordering::Less,
            (PathElem::Key(_), PathElem::Index(_)) => Ordering::Greater,
        }
    }
}

/// `error.validator`: unset until the keyword loop that yielded the error
/// fills it in, or Python's `None` for a `False` schema.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Keyword {
    Unset,
    PyNone,
    Name(String),
}

/// A `jsonschema.exceptions.ValidationError`.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    /// `error.message`.
    pub message: String,
    /// `error.path`: relative to the instance of the error that holds this
    /// one in its `context`, absolute for the errors `iter_errors` yields.
    pub path: VecDeque<PathElem>,
    /// `error.schema_path`.
    pub schema_path: VecDeque<PathElem>,
    /// `error.context`: the errors of the subschemas of `anyOf`/`oneOf`.
    pub context: Vec<ValidationError>,
    validator: Keyword,
    /// `error._matches_type()`, fixed when the validator is set.
    matches_type: bool,
}

impl ValidationError {
    fn new(message: impl Into<String>) -> Self {
        ValidationError {
            message: message.into(),
            path: VecDeque::new(),
            schema_path: VecDeque::new(),
            context: Vec::new(),
            validator: Keyword::Unset,
            matches_type: false,
        }
    }

    /// `error.validator`: the keyword that failed (`None` for a `False`
    /// schema).
    pub fn validator(&self) -> Option<&str> {
        match &self.validator {
            Keyword::Name(name) => Some(name),
            _ => None,
        }
    }

    /// `list(error.path)` as JSON (`list(error.absolute_path)` for the
    /// errors `iter_errors` yields).
    pub fn path_json(&self) -> Value {
        Value::Array(self.path.iter().map(PathElem::to_json).collect())
    }

    /// `jsonschema.exceptions.relevance(error)`.
    fn relevance(&self) -> Relevance<'_> {
        let weak = matches!(&self.validator, Keyword::Name(k) if k == "anyOf" || k == "oneOf");
        Relevance {
            neg_depth: -(self.path.len() as i64),
            path: &self.path,
            not_weak: !weak,
            strong: false,
            not_matches_type: !self.matches_type,
        }
    }
}

/// The tuple `relevance` returns, compared the way Python compares tuples.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Relevance<'a> {
    neg_depth: i64,
    path: &'a VecDeque<PathElem>,
    not_weak: bool,
    strong: bool,
    not_matches_type: bool,
}

/// `jsonschema.exceptions.best_match(errors)`.
pub fn best_match(errors: Vec<ValidationError>) -> Option<ValidationError> {
    // `max` keeps the first of equally relevant errors.
    let mut best: Option<ValidationError> = None;
    for error in errors {
        let replace = match &best {
            None => true,
            Some(current) => error.relevance() > current.relevance(),
        };
        if replace {
            best = Some(error);
        }
    }
    let mut best = best?;
    while !best.context.is_empty() {
        // `heapq.nsmallest(2, best.context, key=relevance)`: stable, so the
        // first of equally relevant errors comes first.
        let mut order: Vec<usize> = (0..best.context.len()).collect();
        order.sort_by(|&a, &b| {
            best.context[a]
                .relevance()
                .cmp(&best.context[b].relevance())
        });
        if order.len() >= 2
            && best.context[order[0]].relevance() == best.context[order[1]].relevance()
        {
            return Some(best);
        }
        let smallest = order[0];
        best = best.context.swap_remove(smallest);
    }
    Some(best)
}

/// A schema ready to validate instances: `Draft7Validator(schema)` or
/// `Draft202012Validator(schema, format_checker=...)`.
#[derive(Debug, Clone)]
pub struct Validator {
    schema: Value,
    draft: Draft,
    check_formats: bool,
    patterns: HashMap<String, Regex>,
}

/// A schema this module cannot load.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SchemaLoadError {
    #[error("schema is not valid JSON: {0}")]
    Json(String),
    #[error("unsupported $schema {0:?}")]
    UnknownDraft(String),
    #[error("pattern {pattern:?} does not compile: {error}")]
    Pattern { pattern: String, error: String },
}

impl Validator {
    /// Load `schema`. `check_formats` asserts `format` the way a validator
    /// built with `format_checker=jsonschema.FormatChecker()` does; without
    /// it `format` is an annotation, as for `jsonschema.validate`.
    pub fn new(schema: Value, check_formats: bool) -> Result<Validator, SchemaLoadError> {
        let draft = match schema.get("$schema").and_then(Value::as_str) {
            Some("http://json-schema.org/draft-07/schema#")
            | Some("http://json-schema.org/draft-07/schema") => Draft::Draft7,
            Some("https://json-schema.org/draft/2020-12/schema") | None => Draft::Draft202012,
            Some(other) => return Err(SchemaLoadError::UnknownDraft(other.to_string())),
        };
        let mut patterns = HashMap::new();
        collect_patterns(&schema, &mut patterns)?;
        Ok(Validator {
            schema,
            draft,
            check_formats,
            patterns,
        })
    }

    /// Load a schema from its JSON text.
    pub fn from_json(text: &str, check_formats: bool) -> Result<Validator, SchemaLoadError> {
        let schema: Value =
            serde_json::from_str(text).map_err(|e| SchemaLoadError::Json(e.to_string()))?;
        Validator::new(schema, check_formats)
    }

    pub fn schema(&self) -> &Value {
        &self.schema
    }

    pub fn draft(&self) -> Draft {
        self.draft
    }

    /// `validator.iter_errors(instance)`, in the order Python yields them.
    pub fn iter_errors(&self, instance: &Value) -> Vec<ValidationError> {
        self.descend(instance, &self.schema)
    }

    /// `best_match(validator.iter_errors(instance))`: the error
    /// `jsonschema.validate` raises.
    pub fn validate(&self, instance: &Value) -> Result<(), Box<ValidationError>> {
        match best_match(self.iter_errors(instance)) {
            Some(error) => Err(Box::new(error)),
            None => Ok(()),
        }
    }

    pub fn is_valid(&self, instance: &Value) -> bool {
        self.iter_errors(instance).is_empty()
    }

    /// The keywords in the schema that `jsonschema` would apply but this
    /// module does not implement, as JSON-pointer-like paths.
    pub fn unsupported_keywords(&self) -> Vec<String> {
        let mut out = Vec::new();
        keywords::find_unsupported(self, &self.schema, "#", &mut out);
        out
    }

    /// `validator.descend(instance, schema)` without a path: the errors of
    /// `instance` against the subschema `schema`.
    fn descend(&self, instance: &Value, schema: &Value) -> Vec<ValidationError> {
        match schema {
            Value::Bool(true) => Vec::new(),
            Value::Bool(false) => {
                let mut error = ValidationError::new(format!(
                    "False schema does not allow {}",
                    crate::pyrepr::py_repr(instance)
                ));
                error.validator = Keyword::PyNone;
                vec![error]
            }
            Value::Object(map) => {
                let mut errors = Vec::new();
                for (keyword, value) in map {
                    for mut error in keywords::apply(self, keyword, value, instance, schema) {
                        if error.validator == Keyword::Unset {
                            error.validator = Keyword::Name(keyword.clone());
                            error.matches_type = matches_type(schema, instance);
                        }
                        error.schema_path.push_front(PathElem::Key(keyword.clone()));
                        errors.push(error);
                    }
                }
                errors
            }
            // Not a schema; `jsonschema` would fail on `schema.items()`.
            _ => Vec::new(),
        }
    }

    /// `descend(instance, schema, path=path, schema_path=schema_path)`.
    fn descend_at(
        &self,
        instance: &Value,
        schema: &Value,
        path: Option<PathElem>,
        schema_path: Option<PathElem>,
    ) -> Vec<ValidationError> {
        let mut errors = self.descend(instance, schema);
        for error in &mut errors {
            if let Some(step) = &path {
                error.path.push_front(step.clone());
            }
            if let Some(step) = &schema_path {
                error.schema_path.push_front(step.clone());
            }
        }
        errors
    }
}

/// `ValidationError._matches_type()` for an error on `instance` raised by a
/// keyword of `schema`.
fn matches_type(schema: &Value, instance: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(t)) => keywords::is_type(instance, t),
        Some(Value::Array(types)) => types
            .iter()
            .any(|t| t.as_str().is_some_and(|t| keywords::is_type(instance, t))),
        _ => false,
    }
}

fn collect_patterns(
    schema: &Value,
    patterns: &mut HashMap<String, Regex>,
) -> Result<(), SchemaLoadError> {
    match schema {
        Value::Object(map) => {
            for (key, value) in map {
                match value {
                    Value::String(p) if key == "pattern" && !patterns.contains_key(p) => {
                        let regex =
                            pattern::compile(p).map_err(|error| SchemaLoadError::Pattern {
                                pattern: p.clone(),
                                error,
                            })?;
                        patterns.insert(p.clone(), regex);
                    }
                    _ => collect_patterns(value, patterns)?,
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_patterns(item, patterns)?;
            }
        }
        _ => {}
    }
    Ok(())
}
