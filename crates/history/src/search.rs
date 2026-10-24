//! `shot_database.SearchParams` and its pydantic validation.

use rusqlite::types::Value as SqlValue;
use serde_json::{Map, Value};

use crate::pydantic::{
    Bounds, ErrorDetail, Outcome, validate_bool, validate_enum, validate_float, validate_int,
    validate_list, validate_str,
};
pub use crate::pydantic::{ErrorDetail as SearchError, ValidationError};

/// `SearchOrder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchOrder {
    Ascending,
    #[default]
    Descending,
}

impl SearchOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ascending => "asc",
            Self::Descending => "desc",
        }
    }
}

/// `SearchOrderBy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOrderBy {
    Date,
    Profile,
}

impl SearchOrderBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Date => "date",
            Self::Profile => "profile",
        }
    }
}

/// An entry of `ids: List[str | int]`: a history id, a history uuid or a
/// profile id, bound to SQL with its Python type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchId {
    Str(String),
    Int(i64),
}

impl SearchId {
    pub(crate) fn to_sql(&self) -> SqlValue {
        match self {
            Self::Str(s) => SqlValue::Text(s.clone()),
            Self::Int(i) => SqlValue::Integer(*i),
        }
    }
}

impl From<&str> for SearchId {
    fn from(value: &str) -> Self {
        Self::Str(value.to_string())
    }
}

impl From<i64> for SearchId {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

/// `SearchParams`, with the same defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchParams {
    pub query: Option<String>,
    pub ids: Vec<SearchId>,
    pub start_date: Option<f64>,
    pub end_date: Option<f64>,
    pub order_by: Vec<SearchOrderBy>,
    pub sort: SearchOrder,
    pub max_results: i64,
    pub dump_data: bool,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            query: None,
            ids: Vec::new(),
            start_date: None,
            end_date: None,
            order_by: vec![SearchOrderBy::Date],
            sort: SearchOrder::Descending,
            max_results: 20,
            dump_data: true,
        }
    }
}

fn optional<T>(
    value: Option<&Value>,
    validate: impl FnOnce(&Value) -> Outcome<T>,
) -> Outcome<Option<T>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => validate(value).map(Some),
    }
}

/// `str | int` in smart mode: an exact str or int wins, then lax int.
fn validate_id(value: &Value, loc: &[Value]) -> Outcome<SearchId> {
    match value {
        Value::String(s) => Ok(SearchId::Str(s.clone())),
        Value::Number(n) if n.as_i64().is_some() => {
            Ok(SearchId::Int(n.as_i64().unwrap_or_default()))
        }
        _ => {
            let int_loc = crate::pydantic::loc_with(loc, "int");
            match validate_int(value, &int_loc, Bounds::NONE) {
                Ok(i) => Ok(SearchId::Int(i)),
                Err(int_errors) => {
                    let str_loc = crate::pydantic::loc_with(loc, "str");
                    let mut errors = validate_str(value, &str_loc, None, None)
                        .err()
                        .unwrap_or_default();
                    errors.extend(int_errors);
                    Err(errors)
                }
            }
        }
    }
}

impl SearchParams {
    /// `SearchParams(**data)`: validate the keyword arguments the history
    /// handlers pass (a JSON body, or the query arguments as strings and
    /// lists of strings). Unknown keys are ignored.
    pub fn from_json(data: &Map<String, Value>) -> Result<Self, ValidationError> {
        let mut errors: Vec<ErrorDetail> = Vec::new();
        let mut params = SearchParams::default();
        let loc = |name: &str| vec![Value::from(name)];

        match optional(data.get("query"), |v| {
            validate_str(v, &loc("query"), None, None)
        }) {
            Ok(v) => params.query = v,
            Err(e) => errors.extend(e),
        }
        if let Some(value) = data.get("ids") {
            match validate_list(value, &loc("ids"), None, None, validate_id) {
                Ok(v) => params.ids = v,
                Err(e) => errors.extend(e),
            }
        }
        for (key, slot) in [
            ("start_date", &mut params.start_date),
            ("end_date", &mut params.end_date),
        ] {
            match optional(data.get(key), |v| {
                validate_float(v, &loc(key), true, Bounds::NONE)
            }) {
                Ok(v) => *slot = v,
                Err(e) => errors.extend(e),
            }
        }
        if let Some(value) = data.get("order_by") {
            let order = validate_list(value, &loc("order_by"), None, None, |item, item_loc| {
                validate_enum(item, item_loc, &["date", "profile"]).map(|v| {
                    if v == "date" {
                        SearchOrderBy::Date
                    } else {
                        SearchOrderBy::Profile
                    }
                })
            });
            match order {
                Ok(v) => params.order_by = v,
                Err(e) => errors.extend(e),
            }
        }
        if let Some(value) = data.get("sort") {
            match validate_enum(value, &loc("sort"), &["asc", "desc"]) {
                Ok(v) => {
                    params.sort = if v == "asc" {
                        SearchOrder::Ascending
                    } else {
                        SearchOrder::Descending
                    }
                }
                Err(e) => errors.extend(e),
            }
        }
        if let Some(value) = data.get("max_results") {
            match validate_int(value, &loc("max_results"), Bounds::NONE) {
                Ok(v) => params.max_results = v,
                Err(e) => errors.extend(e),
            }
        }
        if let Some(value) = data.get("dump_data") {
            match validate_bool(value, &loc("dump_data")) {
                Ok(v) => params.dump_data = v,
                Err(e) => errors.extend(e),
            }
        }
        if errors.is_empty() {
            Ok(params)
        } else {
            Err(ValidationError { errors })
        }
    }
}
