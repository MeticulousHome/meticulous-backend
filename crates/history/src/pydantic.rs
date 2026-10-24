//! The pydantic (2.12, lax mode, Python input) behaviour the history models
//! rely on: how `SearchParams` and `PourOverSession` coerce values parsed
//! from JSON, and the errors they report (`ValidationError.json()`).

use serde_json::{Map, Number, Value};

/// The pydantic minor version in `uv.lock`; it appears in error URLs.
pub const PYDANTIC_VERSION: &str = "2.12";

/// One entry of `ValidationError.errors()`.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorDetail {
    pub kind: &'static str,
    pub loc: Vec<Value>,
    pub msg: String,
    pub input: Value,
    pub ctx: Option<Value>,
}

impl ErrorDetail {
    pub fn new(kind: &'static str, loc: &[Value], msg: impl Into<String>, input: &Value) -> Self {
        Self {
            kind,
            loc: loc.to_vec(),
            msg: msg.into(),
            input: input.clone(),
            ctx: None,
        }
    }

    pub fn with_ctx(mut self, ctx: Value) -> Self {
        self.ctx = Some(ctx);
        self
    }

    /// The error as `ValidationError.json()` writes it.
    pub fn to_json(&self, include_url: bool) -> Value {
        let mut map = Map::new();
        map.insert("type".into(), Value::from(self.kind));
        map.insert("loc".into(), Value::Array(self.loc.clone()));
        map.insert("msg".into(), Value::from(self.msg.clone()));
        map.insert("input".into(), self.input.clone());
        if let Some(ctx) = &self.ctx {
            map.insert("ctx".into(), ctx.clone());
        }
        if include_url {
            map.insert(
                "url".into(),
                Value::from(format!(
                    "https://errors.pydantic.dev/{PYDANTIC_VERSION}/v/{}",
                    self.kind
                )),
            );
        }
        Value::Object(map)
    }
}

/// A failed validation: every error, in field order.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub errors: Vec<ErrorDetail>,
}

impl ValidationError {
    /// `json.loads(error.json(include_url=...))`.
    pub fn to_json(&self, include_url: bool) -> Value {
        Value::Array(self.errors.iter().map(|e| e.to_json(include_url)).collect())
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} validation error(s)", self.errors.len())?;
        for error in &self.errors {
            write!(f, "; {:?}: {}", error.loc, error.msg)?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

pub type Outcome<T> = Result<T, Vec<ErrorDetail>>;

pub fn loc_with(loc: &[Value], item: impl Into<Value>) -> Vec<Value> {
    let mut loc = loc.to_vec();
    loc.push(item.into());
    loc
}

fn float_value(f: f64) -> Value {
    Number::from_f64(f).map_or(Value::Null, Value::Number)
}

/// Python's rules for `_` in numeric literals: single, between digits.
fn strip_underscores(text: &str) -> Option<String> {
    if !text.contains('_') {
        return Some(text.to_string());
    }
    let bytes = text.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'_' {
            let before = i.checked_sub(1).and_then(|j| bytes.get(j));
            let after = bytes.get(i + 1);
            if !before.is_some_and(u8::is_ascii_digit) || !after.is_some_and(u8::is_ascii_digit) {
                return None;
            }
        }
    }
    Some(text.replace('_', ""))
}

/// `str`: strings only, then the length constraints (in code points).
pub fn validate_str(
    value: &Value,
    loc: &[Value],
    min_length: Option<usize>,
    max_length: Option<usize>,
) -> Outcome<String> {
    let Value::String(text) = value else {
        return Err(vec![ErrorDetail::new(
            "string_type",
            loc,
            "Input should be a valid string",
            value,
        )]);
    };
    let length = text.chars().count();
    if let Some(min) = min_length
        && length < min
    {
        return Err(vec![
            ErrorDetail::new(
                "string_too_short",
                loc,
                format!(
                    "String should have at least {min} character{}",
                    if min == 1 { "" } else { "s" }
                ),
                value,
            )
            .with_ctx(pyfmt::object([("min_length", Value::from(min))])),
        ]);
    }
    if let Some(max) = max_length
        && length > max
    {
        return Err(vec![
            ErrorDetail::new(
                "string_too_long",
                loc,
                format!(
                    "String should have at most {max} character{}",
                    if max == 1 { "" } else { "s" }
                ),
                value,
            )
            .with_ctx(pyfmt::object([("max_length", Value::from(max))])),
        ]);
    }
    Ok(text.clone())
}

/// Numeric bounds of a `Field(...)`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bounds {
    pub ge: Option<f64>,
    pub gt: Option<f64>,
    pub le: Option<f64>,
}

impl Bounds {
    pub const NONE: Bounds = Bounds {
        ge: None,
        gt: None,
        le: None,
    };
    pub const GE0: Bounds = Bounds {
        ge: Some(0.0),
        gt: None,
        le: None,
    };
    pub const GT0: Bounds = Bounds {
        ge: None,
        gt: Some(0.0),
        le: None,
    };
}

fn check_bounds(
    number: f64,
    value: &Value,
    loc: &[Value],
    bounds: Bounds,
    as_int: bool,
) -> Outcome<()> {
    let constraint = |bound: f64| -> Value {
        if as_int {
            Value::from(bound as i64)
        } else {
            float_value(bound)
        }
    };
    let shown = |bound: f64| -> String {
        if as_int {
            (bound as i64).to_string()
        } else {
            bound.to_string()
        }
    };
    if let Some(gt) = bounds.gt
        && number <= gt
    {
        return Err(vec![
            ErrorDetail::new(
                "greater_than",
                loc,
                format!("Input should be greater than {}", shown(gt)),
                value,
            )
            .with_ctx(pyfmt::object([("gt", constraint(gt))])),
        ]);
    }
    if let Some(ge) = bounds.ge
        && number < ge
    {
        return Err(vec![
            ErrorDetail::new(
                "greater_than_equal",
                loc,
                format!("Input should be greater than or equal to {}", shown(ge)),
                value,
            )
            .with_ctx(pyfmt::object([("ge", constraint(ge))])),
        ]);
    }
    if let Some(le) = bounds.le
        && number > le
    {
        return Err(vec![
            ErrorDetail::new(
                "less_than_equal",
                loc,
                format!("Input should be less than or equal to {}", shown(le)),
                value,
            )
            .with_ctx(pyfmt::object([("le", constraint(le))])),
        ]);
    }
    Ok(())
}

/// `float` in lax mode: bools, ints, floats and numeric strings.
pub fn validate_float(
    value: &Value,
    loc: &[Value],
    allow_inf_nan: bool,
    bounds: Bounds,
) -> Outcome<f64> {
    let number = match value {
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => {
            let parsed = strip_underscores(text.trim()).and_then(|t| t.parse::<f64>().ok());
            match parsed {
                Some(number) => number,
                None => {
                    return Err(vec![ErrorDetail::new(
                        "float_parsing",
                        loc,
                        "Input should be a valid number, unable to parse string as a number",
                        value,
                    )]);
                }
            }
        }
        _ => {
            return Err(vec![ErrorDetail::new(
                "float_type",
                loc,
                "Input should be a valid number",
                value,
            )]);
        }
    };
    if !allow_inf_nan && !number.is_finite() {
        return Err(vec![ErrorDetail::new(
            "finite_number",
            loc,
            "Input should be a finite number",
            value,
        )]);
    }
    check_bounds(number, value, loc, bounds, false)?;
    Ok(number)
}

fn int_from_str(text: &str) -> Option<i64> {
    let text = strip_underscores(text.trim())?;
    let (sign, digits) = match text.as_bytes().first() {
        Some(b'+') => (1, &text[1..]),
        Some(b'-') => (-1, &text[1..]),
        _ => (1, text.as_str()),
    };
    let digits = match digits.split_once('.') {
        Some((whole, fraction)) if fraction.bytes().all(|b| b == b'0') => whole,
        Some(_) => return None,
        None => digits,
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i64>().ok().map(|n| sign * n)
}

/// `int` in lax mode: bools, ints, whole floats and integer strings.
pub fn validate_int(value: &Value, loc: &[Value], bounds: Bounds) -> Outcome<i64> {
    let number = match value {
        Value::Bool(b) => i64::from(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i,
            None => {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if !f.is_finite() {
                    return Err(vec![ErrorDetail::new(
                        "finite_number",
                        loc,
                        "Input should be a finite number",
                        value,
                    )]);
                }
                if f.fract() != 0.0 {
                    return Err(vec![ErrorDetail::new(
                        "int_from_float",
                        loc,
                        "Input should be a valid integer, got a number with a fractional part",
                        value,
                    )]);
                }
                if f.abs() >= 9.223_372_036_854_776e18 {
                    return Err(vec![ErrorDetail::new(
                        "int_parsing_size",
                        loc,
                        "Unable to parse input string as an integer, exceeded maximum size",
                        value,
                    )]);
                }
                f as i64
            }
        },
        Value::String(text) => match int_from_str(text) {
            Some(number) => number,
            None => {
                return Err(vec![ErrorDetail::new(
                    "int_parsing",
                    loc,
                    "Input should be a valid integer, unable to parse string as an integer",
                    value,
                )]);
            }
        },
        _ => {
            return Err(vec![ErrorDetail::new(
                "int_type",
                loc,
                "Input should be a valid integer",
                value,
            )]);
        }
    };
    check_bounds(number as f64, value, loc, bounds, true)?;
    Ok(number)
}

/// `bool` in lax mode.
pub fn validate_bool(value: &Value, loc: &[Value]) -> Outcome<bool> {
    let parsing = || {
        vec![ErrorDetail::new(
            "bool_parsing",
            loc,
            "Input should be a valid boolean, unable to interpret input",
            value,
        )]
    };
    match value {
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(0), _) => Ok(false),
            (Some(1), _) => Ok(true),
            (Some(_), _) => Err(parsing()),
            (None, Some(0.0)) => Ok(false),
            (None, Some(1.0)) => Ok(true),
            _ => Err(vec![ErrorDetail::new(
                "bool_type",
                loc,
                "Input should be a valid boolean",
                value,
            )]),
        },
        Value::String(text) => match text.to_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
            _ => Err(parsing()),
        },
        _ => Err(vec![ErrorDetail::new(
            "bool_type",
            loc,
            "Input should be a valid boolean",
            value,
        )]),
    }
}

/// How pydantic lists the allowed values: `'a'`, `'a' or 'b'`, `'a', 'b' or 'c'`.
pub fn expected_list(options: &[Value]) -> String {
    let shown: Vec<String> = options
        .iter()
        .map(|o| match o {
            Value::String(s) => format!("'{s}'"),
            other => other.to_string(),
        })
        .collect();
    match shown.len() {
        0 => String::new(),
        1 => shown[0].clone(),
        n => format!("{} or {}", shown[..n - 1].join(", "), shown[n - 1]),
    }
}

/// `Literal[...]`: strings must match exactly; ints also accept equal bools
/// and whole floats.
pub fn validate_literal(value: &Value, loc: &[Value], options: &[Value]) -> Outcome<Value> {
    for option in options {
        let matches = match (option, value) {
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
            (Value::Number(a), Value::Bool(b)) => a.as_i64() == Some(i64::from(*b)),
            _ => false,
        };
        if matches {
            return Ok(option.clone());
        }
    }
    let expected = expected_list(options);
    Err(vec![
        ErrorDetail::new(
            "literal_error",
            loc,
            format!("Input should be {expected}"),
            value,
        )
        .with_ctx(pyfmt::object([("expected", Value::from(expected))])),
    ])
}

/// A `str` `Enum`: the value must be one of the members' values.
pub fn validate_enum(value: &Value, loc: &[Value], options: &[&str]) -> Outcome<String> {
    if let Value::String(text) = value
        && options.contains(&text.as_str())
    {
        return Ok(text.clone());
    }
    let values: Vec<Value> = options.iter().map(|o| Value::from(*o)).collect();
    let expected = expected_list(&values);
    Err(vec![
        ErrorDetail::new("enum", loc, format!("Input should be {expected}"), value)
            .with_ctx(pyfmt::object([("expected", Value::from(expected))])),
    ])
}

/// A validated `AwareDatetime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwareTime {
    /// Microseconds since the Unix epoch (UTC).
    pub utc_micros: i64,
    /// The UTC offset in seconds the value carried.
    pub offset: i32,
}

impl AwareTime {
    /// The wall clock fields in the value's own offset.
    fn local_fields(self) -> Option<chrono::NaiveDateTime> {
        let local = self.utc_micros + i64::from(self.offset) * 1_000_000;
        chrono::DateTime::from_timestamp_micros(local).map(|d| d.naive_utc())
    }

    /// pydantic's JSON form: ISO 8601 with `Z` for UTC.
    pub fn to_pydantic_json(self) -> String {
        self.format(true)
    }

    /// `datetime.isoformat()` (`+00:00` for UTC).
    pub fn isoformat(self) -> String {
        self.format(false)
    }

    fn format(self, z: bool) -> String {
        use chrono::Timelike;
        let Some(fields) = self.local_fields() else {
            return String::new();
        };
        let micro = fields.nanosecond() / 1000;
        let mut text = fields.format("%Y-%m-%dT%H:%M:%S").to_string();
        if micro != 0 {
            text.push_str(&format!(".{micro:06}"));
        }
        if self.offset == 0 && z {
            text.push('Z');
        } else {
            let sign = if self.offset < 0 { '-' } else { '+' };
            let offset = self.offset.unsigned_abs();
            text.push_str(&format!(
                "{sign}{:02}:{:02}",
                offset / 3600,
                offset % 3600 / 60
            ));
            if offset % 60 != 0 {
                text.push_str(&format!(":{:02}", offset % 60));
            }
        }
        text
    }

    /// `datetime.timestamp()` of an aware datetime.
    pub fn timestamp(self) -> f64 {
        self.utc_micros as f64 / 1e6
    }

    /// The UTC wall clock, as `astimezone(timezone.utc)` gives it.
    pub fn utc_naive(self) -> Option<chrono::NaiveDateTime> {
        chrono::DateTime::from_timestamp_micros(self.utc_micros).map(|d| d.naive_utc())
    }
}

fn digits(bytes: &[u8], range: std::ops::Range<usize>) -> Option<u32> {
    let slice = bytes.get(range)?;
    if slice.iter().all(u8::is_ascii_digit) {
        std::str::from_utf8(slice).ok()?.parse().ok()
    } else {
        None
    }
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

/// speedate's date part: `YYYY-MM-DD`, with its error messages.
fn parse_date(bytes: &[u8]) -> Result<(u32, u32, u32), &'static str> {
    if bytes.len() < 10 {
        return Err("input is too short");
    }
    let year = digits(bytes, 0..4).ok_or("invalid character in year")?;
    if bytes[4] != b'-' {
        return Err("invalid date separator, expected `-`");
    }
    let month = digits(bytes, 5..7).ok_or("invalid character in month")?;
    if bytes[7] != b'-' {
        return Err("invalid date separator, expected `-`");
    }
    let day = digits(bytes, 8..10).ok_or("invalid character in day")?;
    if !(1..=12).contains(&month) {
        return Err("month value is outside expected range of 1-12");
    }
    if day == 0 || day > days_in_month(year, month) {
        return Err("day value is outside expected range");
    }
    Ok((year, month, day))
}

/// speedate's datetime: date, `T`/`t`/space, `HH:MM[:SS[(.|,)fraction]]`,
/// then `Z`/`z` or `±HH:MM`/`±HHMM`.
/// The fields of a parsed datetime string.
struct ParsedDatetime {
    date: (u32, u32, u32),
    hour: u32,
    minute: u32,
    second: u32,
    micro: u32,
    offset: Option<i32>,
}

fn parse_datetime(bytes: &[u8]) -> Option<ParsedDatetime> {
    let date = parse_date(bytes).ok()?;
    if !matches!(bytes.get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let hour = digits(bytes, 11..13)?;
    if bytes.get(13) != Some(&b':') {
        return None;
    }
    let minute = digits(bytes, 14..16)?;
    let mut position = 16;
    let mut second = 0;
    let mut micro = 0;
    if bytes.get(position) == Some(&b':') {
        second = digits(bytes, position + 1..position + 3)?;
        position += 3;
        if matches!(bytes.get(position), Some(b'.' | b',')) {
            position += 1;
            let start = position;
            while bytes.get(position).is_some_and(u8::is_ascii_digit) {
                position += 1;
            }
            if position == start {
                return None;
            }
            let mut fraction: String = std::str::from_utf8(&bytes[start..position.min(start + 6)])
                .ok()?
                .to_string();
            while fraction.len() < 6 {
                fraction.push('0');
            }
            micro = fraction.parse().ok()?;
        }
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let offset = match bytes.get(position) {
        None => None,
        Some(b'Z' | b'z') => {
            position += 1;
            Some(0)
        }
        Some(sign @ (b'+' | b'-')) => {
            let sign = if *sign == b'-' { -1 } else { 1 };
            let hours = digits(bytes, position + 1..position + 3)?;
            let minutes = if bytes.get(position + 3) == Some(&b':') {
                position += 6;
                digits(bytes, position - 2..position)?
            } else {
                position += 5;
                digits(bytes, position - 2..position)?
            };
            if hours > 23 || minutes > 59 {
                return None;
            }
            Some(sign * (hours as i32 * 3600 + minutes as i32 * 60))
        }
        Some(_) => return None,
    };
    if position != bytes.len() {
        return None;
    }
    Some(ParsedDatetime {
        date,
        hour,
        minute,
        second,
        micro,
        offset,
    })
}

/// `AwareDatetime` in lax mode, Python input.
pub fn validate_aware_datetime(value: &Value, loc: &[Value]) -> Outcome<AwareTime> {
    let naive_error = || {
        vec![ErrorDetail::new(
            "timezone_aware",
            loc,
            "Input should have timezone info",
            value,
        )]
    };
    let from_timestamp = |seconds: f64| -> Outcome<AwareTime> {
        // speedate reads values beyond 2e10 as milliseconds.
        let micros = if seconds.abs() > 2e10 {
            (seconds * 1000.0).round()
        } else {
            (seconds * 1e6).round()
        };
        Ok(AwareTime {
            utc_micros: micros as i64,
            offset: 0,
        })
    };
    match value {
        Value::Number(n) => from_timestamp(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(text) => {
            let bytes = text.as_bytes();
            if !bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit) {
                if let Ok(seconds) = text.parse::<f64>() {
                    return from_timestamp(seconds);
                }
            }
            if let Some(ParsedDatetime {
                date: (year, month, day),
                hour,
                minute,
                second,
                micro,
                offset,
            }) = parse_datetime(bytes)
            {
                if year == 0 {
                    return Err(vec![
                        ErrorDetail::new(
                            "datetime_parsing",
                            loc,
                            "Input should be a valid datetime, year 0 is out of range",
                            value,
                        )
                        .with_ctx(pyfmt::object([(
                            "error",
                            Value::from("year 0 is out of range"),
                        )])),
                    ]);
                }
                let Some(offset) = offset else {
                    return Err(naive_error());
                };
                let wall = chrono::NaiveDate::from_ymd_opt(year as i32, month, day)
                    .and_then(|d| d.and_hms_micro_opt(hour, minute, second, micro))
                    .map(|d| d.and_utc().timestamp_micros());
                let Some(wall) = wall else {
                    return Err(naive_error());
                };
                return Ok(AwareTime {
                    utc_micros: wall - i64::from(offset) * 1_000_000,
                    offset,
                });
            }
            let message = match parse_date(bytes) {
                Err(message) => message,
                Ok(_) if bytes.len() > 10 => "unexpected extra characters at the end of the input",
                Ok(_) => return Err(naive_error()),
            };
            Err(vec![
                ErrorDetail::new(
                    "datetime_from_date_parsing",
                    loc,
                    format!("Input should be a valid datetime or date, {message}"),
                    value,
                )
                .with_ctx(pyfmt::object([("error", Value::from(message))])),
            ])
        }
        _ => Err(vec![ErrorDetail::new(
            "datetime_type",
            loc,
            "Input should be a valid datetime",
            value,
        )]),
    }
}

/// The length errors of `list[...]` and `tuple[...]`.
pub fn too_long(
    loc: &[Value],
    value: &Value,
    field_type: &str,
    max: usize,
    actual: usize,
) -> ErrorDetail {
    ErrorDetail::new(
        "too_long",
        loc,
        format!(
            "{field_type} should have at most {max} item{} after validation, not {actual}",
            if max == 1 { "" } else { "s" }
        ),
        value,
    )
    .with_ctx(pyfmt::object([
        ("field_type", Value::from(field_type)),
        ("max_length", Value::from(max)),
        ("actual_length", Value::from(actual)),
    ]))
}

pub fn too_short(
    loc: &[Value],
    value: &Value,
    field_type: &str,
    min: usize,
    actual: usize,
) -> ErrorDetail {
    ErrorDetail::new(
        "too_short",
        loc,
        format!(
            "{field_type} should have at least {min} item{} after validation, not {actual}",
            if min == 1 { "" } else { "s" }
        ),
        value,
    )
    .with_ctx(pyfmt::object([
        ("field_type", Value::from(field_type)),
        ("min_length", Value::from(min)),
        ("actual_length", Value::from(actual)),
    ]))
}

/// `list[T]`: each item, stopping at the first item past `max`.
pub fn validate_list<T>(
    value: &Value,
    loc: &[Value],
    min: Option<usize>,
    max: Option<usize>,
    mut item: impl FnMut(&Value, &[Value]) -> Outcome<T>,
) -> Outcome<Vec<T>> {
    let Value::Array(items) = value else {
        return Err(vec![ErrorDetail::new(
            "list_type",
            loc,
            "Input should be a valid list",
            value,
        )]);
    };
    let mut output = Vec::with_capacity(items.len());
    let mut errors = Vec::new();
    for (index, element) in items.iter().enumerate() {
        match item(element, &loc_with(loc, index)) {
            Ok(v) => output.push(v),
            Err(e) => errors.extend(e),
        }
        if let Some(max) = max
            && index + 1 > max
        {
            return Err(vec![too_long(loc, value, "List", max, items.len())]);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    if let Some(min) = min
        && output.len() < min
    {
        return Err(vec![too_short(loc, value, "List", min, output.len())]);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expected_lists_read_like_pydantic() {
        assert_eq!(expected_list(&[json!("a")]), "'a'");
        assert_eq!(expected_list(&[json!("a"), json!("b")]), "'a' or 'b'");
        assert_eq!(
            expected_list(&[json!(1), json!(2), json!(3), json!(4)]),
            "1, 2, 3 or 4"
        );
    }

    #[test]
    fn integer_strings() {
        assert_eq!(int_from_str(" -3 "), Some(-3));
        assert_eq!(int_from_str("+3"), Some(3));
        assert_eq!(int_from_str("1_000"), Some(1000));
        assert_eq!(int_from_str("2.00"), Some(2));
        assert_eq!(int_from_str("2.5"), None);
        assert_eq!(int_from_str("1__0"), None);
        assert_eq!(int_from_str("_1"), None);
        assert_eq!(int_from_str("1e2"), None);
    }

    #[test]
    fn datetimes_format_like_pydantic() {
        let parse =
            |text: &str| validate_aware_datetime(&json!(text), &[]).map(|t| t.to_pydantic_json());
        assert_eq!(
            parse("2026-08-16T08:00:00.000Z").as_deref(),
            Ok("2026-08-16T08:00:00Z")
        );
        assert_eq!(
            parse("2026-08-16T08:00:00.1234567Z").as_deref(),
            Ok("2026-08-16T08:00:00.123456Z")
        );
        assert_eq!(
            parse("2026-08-16T08:00:00+0130").as_deref(),
            Ok("2026-08-16T08:00:00+01:30")
        );
        assert_eq!(
            parse("2026-08-16T08:00:00-00:00").as_deref(),
            Ok("2026-08-16T08:00:00Z")
        );
        assert_eq!(parse("1786867200").as_deref(), Ok("2026-08-16T08:00:00Z"));
    }
}
