//! The pure parts of `pour_over_profiles.py` and the error answers of
//! `api/pour_over_profiles.py`.
//!
//! A Pour Over profile must match `pour_over_profile_schema/schema.json`
//! (with `format: uuid` asserted) and then a set of cross-field rules; every
//! problem is reported with its path, message and code. Storage, listing and
//! the change events stay in the backend.

use std::sync::LazyLock;

use regex::bytes::Regex as BytesRegex;
use serde_json::{Value, json};

use crate::pynum::PyNum;
use crate::pyrepr::py_strip;
use crate::pyround::py_format_fixed;
use crate::schema::{PathElem, Validator};

pub const POUR_OVER_PROFILE_SCHEMA: &str =
    include_str!("../../../pour_over_profile_schema/schema.json");

/// The largest stored profile and request body.
pub const MAX_POUR_OVER_PROFILE_BYTES: usize = 512 * 1024;
pub const MAX_EMBEDDED_PROFILE_IMAGE_BYTES: usize = 300 * 1024;
pub const MAX_POUR_OVER_PROFILES: usize = 100;
pub const MIN_FREE_PROFILE_BYTES: u64 = 32 * 1024 * 1024;

const JPEG_DATA_URI_PREFIX: &str = "data:image/jpeg;base64,";

static POUR_OVER_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    Validator::from_json(POUR_OVER_PROFILE_SCHEMA, true)
        .expect("the bundled Pour Over schema loads (checked by the crate's tests)")
});

/// `Draft202012Validator(schema, format_checker=FormatChecker())`.
pub fn pour_over_validator() -> &'static Validator {
    &POUR_OVER_VALIDATOR
}

/// One entry of `PourOverProfileValidationError.issues`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PourOverIssue {
    pub path: Vec<PathElem>,
    pub message: String,
    pub code: String,
}

impl PourOverIssue {
    fn new(path: Vec<PathElem>, message: impl Into<String>, code: &str) -> Self {
        PourOverIssue {
            path,
            message: message.into(),
            code: code.into(),
        }
    }

    /// `{"path": [...], "message": ..., "code": ...}`.
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path.iter().map(PathElem::to_json).collect::<Vec<_>>(),
            "message": self.message,
            "code": self.code,
        })
    }
}

/// The `PourOverProfileError` family.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PourOverProfileError {
    /// `PourOverProfileValidationError(issues)`.
    #[error("Invalid Pour Over profile")]
    Validation(Vec<PourOverIssue>),
    /// `PourOverProfileTooLargeError()`.
    #[error("")]
    TooLarge,
    /// `PourOverProfileLimitError()`: `MAX_POUR_OVER_PROFILES` are stored.
    #[error("")]
    Limit,
    /// `PourOverProfileStorageError(message)`.
    #[error("{0}")]
    Storage(String),
    /// `PourOverProfileUnavailableError(message)`.
    #[error("{0}")]
    Unavailable(String),
    /// `PourOverProfileError` itself (no code raises it today).
    #[error("{0}")]
    Other(String),
}

impl PourOverProfileError {
    pub fn class_name(&self) -> &'static str {
        match self {
            PourOverProfileError::Validation(_) => "PourOverProfileValidationError",
            PourOverProfileError::TooLarge => "PourOverProfileTooLargeError",
            PourOverProfileError::Limit => "PourOverProfileLimitError",
            PourOverProfileError::Storage(_) => "PourOverProfileStorageError",
            PourOverProfileError::Unavailable(_) => "PourOverProfileUnavailableError",
            PourOverProfileError::Other(_) => "PourOverProfileError",
        }
    }

    /// `write_pour_over_profile_error(handler, error)`: status and body.
    pub fn http_response(&self) -> (u16, Value) {
        match self {
            PourOverProfileError::Validation(issues) => (
                422,
                json!({
                    "status": "error",
                    "error": "Invalid Pour Over profile",
                    "details": issues.iter().map(PourOverIssue::to_json).collect::<Vec<_>>(),
                }),
            ),
            PourOverProfileError::TooLarge => (
                413,
                json!({"status": "error", "error": "Pour Over profile is too large"}),
            ),
            PourOverProfileError::Limit => (
                409,
                json!({"status": "error", "error": "Pour Over profile limit reached"}),
            ),
            PourOverProfileError::Storage(_) => (
                507,
                json!({"status": "error", "error": "Could not access Pour Over profile storage"}),
            ),
            PourOverProfileError::Unavailable(_) => (
                503,
                json!({"status": "error", "error": "Pour Over profiles are unavailable"}),
            ),
            PourOverProfileError::Other(_) => (
                500,
                json!({"status": "error", "error": "Could not process Pour Over profile"}),
            ),
        }
    }
}

/// `PourOverProfileManager.validate_profile(value)`: the schema errors, or
/// when there are none the semantic issues; the profile back (a deep copy
/// in Python) when there are neither.
pub fn validate_pour_over_profile(value: &Value) -> Result<Value, PourOverProfileError> {
    if !value.is_object() {
        return Err(PourOverProfileError::Validation(vec![PourOverIssue::new(
            Vec::new(),
            "Profile must be an object",
            "invalid",
        )]));
    }
    let mut schema_errors: Vec<(Vec<String>, PourOverIssue)> = pour_over_validator()
        .iter_errors(value)
        .into_iter()
        .map(|error| {
            let path: Vec<PathElem> = error.path.iter().cloned().collect();
            let sort_key = path.iter().map(PathElem::to_py_str).collect();
            (sort_key, PourOverIssue::new(path, error.message, "schema"))
        })
        .collect();
    // `sorted(..., key=lambda e: (tuple(str(p) for p in e.absolute_path), e.message))`.
    schema_errors.sort_by(|(a, ia), (b, ib)| a.cmp(b).then_with(|| ia.message.cmp(&ib.message)));
    let mut issues: Vec<PourOverIssue> = schema_errors.into_iter().map(|(_, i)| i).collect();
    if issues.is_empty() {
        issues = validate_semantics(value);
    }
    if issues.is_empty() {
        Ok(value.clone())
    } else {
        Err(PourOverProfileError::Validation(issues))
    }
}

fn key(name: &str) -> PathElem {
    PathElem::Key(name.into())
}

fn num(value: Option<&Value>) -> f64 {
    value
        .and_then(PyNum::from_value)
        .map_or(f64::NAN, PyNum::as_f64)
}

/// `_validate_semantics(profile)`, on a profile that passed the schema.
///
/// `_reject_non_finite` is not needed: a `serde_json::Value` cannot hold
/// NaN or infinity, and the Python's own JSON parsing rejects them first.
fn validate_semantics(profile: &Value) -> Vec<PourOverIssue> {
    let mut issues = Vec::new();

    for field in ["name", "author"] {
        let blank =
            matches!(profile.get(field), Some(Value::String(text)) if py_strip(text).is_empty());
        if blank {
            issues.push(PourOverIssue::new(
                vec![key(field)],
                "Text cannot be blank",
                "blank",
            ));
        }
    }

    let (Some(recipe), Some(Value::Array(stages))) = (
        profile.get("recipe").filter(|r| r.is_object()),
        profile.get("stages"),
    ) else {
        return issues;
    };
    if stages.is_empty() {
        return issues;
    }

    let target_time = num(recipe.get("target_total_time_s"));
    let upper_too_early = recipe
        .get("target_total_time_max_s")
        .filter(|v| !v.is_null())
        .is_some_and(|max| num(Some(max)) < target_time);
    if upper_too_early {
        issues.push(PourOverIssue::new(
            vec![key("recipe"), key("target_total_time_max_s")],
            "Upper target time cannot be earlier than the target time",
            "time_range",
        ));
    }

    let mut keys: Vec<&str> = Vec::new();
    let mut cumulative_water = 0.0_f64;
    let mut previous_end = 0.0_f64;
    for (index, stage) in stages.iter().enumerate() {
        let at = |field: &str| vec![key("stages"), PathElem::Index(index), key(field)];
        let at_pour = |field: &str| {
            vec![
                key("stages"),
                PathElem::Index(index),
                key("pour"),
                key(field),
            ]
        };
        let stage_key = stage.get("key").and_then(Value::as_str).unwrap_or("");
        if py_strip(stage_key).is_empty() {
            issues.push(PourOverIssue::new(
                at("key"),
                "Key cannot be blank",
                "blank",
            ));
        }
        let name = stage.get("name").and_then(Value::as_str).unwrap_or("");
        if py_strip(name).is_empty() {
            issues.push(PourOverIssue::new(
                at("name"),
                "Name cannot be blank",
                "blank",
            ));
        }
        if keys.contains(&stage_key) {
            issues.push(PourOverIssue::new(
                at("key"),
                "Stage keys must be unique",
                "duplicate",
            ));
        }
        keys.push(stage_key);

        let starts_at = num(stage.get("starts_at_s"));
        if index == 0 && starts_at != 0.0 {
            issues.push(PourOverIssue::new(
                at("starts_at_s"),
                "The first pour must start at 0 seconds",
                "first_start",
            ));
        }
        if index > 0 && starts_at + 0.001 < previous_end {
            issues.push(PourOverIssue::new(
                at("starts_at_s"),
                "A pour cannot start before the previous pour ends",
                "overlap",
            ));
        }

        let pour = stage.get("pour").unwrap_or(&Value::Null);
        let water = num(pour.get("water_g"));
        let duration = num(pour.get("duration_s"));
        cumulative_water += water;
        let expected_flow = water / duration;
        let target_flow = num(pour.get("flow_rate_g_s"));
        let scaled = expected_flow * 0.02;
        let flow_tolerance = if scaled > 0.1 { scaled } else { 0.1 };
        if (target_flow - expected_flow).abs() > flow_tolerance {
            issues.push(PourOverIssue::new(
                at_pour("flow_rate_g_s"),
                format!(
                    "Flow must equal water divided by duration ({} g/s)",
                    py_format_fixed(expected_flow, 2)
                ),
                "flow_consistency",
            ));
        }
        if (num(pour.get("target_cumulative_water_g")) - cumulative_water).abs() > 0.1 {
            issues.push(PourOverIssue::new(
                at_pour("target_cumulative_water_g"),
                format!(
                    "Cumulative target must be {} g",
                    py_format_fixed(cumulative_water, 1)
                ),
                "water_consistency",
            ));
        }
        if let Some(Value::Array(range)) = pour.get("flow_range_g_s") {
            let low = num(range.first());
            let high = num(range.get(1));
            if low > high {
                issues.push(PourOverIssue::new(
                    at_pour("flow_range_g_s"),
                    "Flow range must be ordered",
                    "flow_range",
                ));
            } else if target_flow < low || target_flow > high {
                issues.push(PourOverIssue::new(
                    at_pour("flow_range_g_s"),
                    "Flow target must sit inside the flow range",
                    "flow_range",
                ));
            }
        }
        previous_end = starts_at + duration;
    }

    if (cumulative_water - num(recipe.get("total_water_g"))).abs() > 0.1 {
        issues.push(PourOverIssue::new(
            vec![key("recipe"), key("total_water_g")],
            format!(
                "Total water must equal the {} g across all pours",
                py_format_fixed(cumulative_water, 1)
            ),
            "water_consistency",
        ));
    }
    if previous_end > target_time + 0.001 {
        issues.push(PourOverIssue::new(
            vec![key("recipe"), key("target_total_time_s")],
            "Target brew time cannot end before the final pour",
            "time_consistency",
        ));
    }

    validate_image(profile, &mut issues);
    issues
}

/// `_validate_image(profile, issues)`.
fn validate_image(profile: &Value, issues: &mut Vec<PourOverIssue>) {
    let Some(Value::String(image)) = profile.get("display").and_then(|d| d.get("image")) else {
        return;
    };
    let Some(data) = image.strip_prefix(JPEG_DATA_URI_PREFIX) else {
        return;
    };
    let path = || vec![key("display"), key("image")];
    let Some(decoded) = b64decode_strict(data) else {
        issues.push(PourOverIssue::new(
            path(),
            "Image must contain valid base64",
            "image_base64",
        ));
        return;
    };
    if decoded.len() > MAX_EMBEDDED_PROFILE_IMAGE_BYTES {
        issues.push(PourOverIssue::new(
            path(),
            "Embedded image is too large",
            "image_too_large",
        ));
    }
    if !is_jpeg(&decoded) {
        issues.push(PourOverIssue::new(
            path(),
            "Embedded image must be a JPEG",
            "image_format",
        ));
    }
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes.starts_with(&[0xff, 0xd8]) && bytes.ends_with(&[0xff, 0xd9])
}

/// `base64.b64decode(s, validate=True)` of CPython 3.11 (`binascii.a2b_base64`
/// in strict mode); `None` where it raises.
pub fn b64decode_strict(s: &str) -> Option<Vec<u8>> {
    // A str argument is encoded as ASCII first (UnicodeEncodeError).
    if !s.is_ascii() {
        return None;
    }
    let bytes = s.as_bytes();
    if bytes.first() == Some(&b'=') {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut quad_pos = 0;
    let mut pads = 0;
    let mut padding_started = false;
    let mut leftchar: u8 = 0;
    for (i, &ch) in bytes.iter().enumerate() {
        if ch == b'=' {
            padding_started = true;
            if quad_pos >= 2 {
                pads += 1;
                if quad_pos + pads >= 4 {
                    // Strict mode rejects anything after the padding.
                    return (i + 1 == bytes.len()).then_some(out);
                }
            }
            continue;
        }
        let value = base64_value(ch)?;
        if padding_started {
            return None;
        }
        pads = 0;
        match quad_pos {
            0 => {
                quad_pos = 1;
                leftchar = value;
            }
            1 => {
                quad_pos = 2;
                out.push((leftchar << 2) | (value >> 4));
                leftchar = value & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push((leftchar << 4) | (value >> 2));
                leftchar = value & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push((leftchar << 6) | value);
                leftchar = 0;
            }
        }
    }
    (quad_pos == 0).then_some(out)
}

fn base64_value(ch: u8) -> Option<u8> {
    match ch {
        b'A'..=b'Z' => Some(ch - b'A'),
        b'a'..=b'z' => Some(ch - b'a' + 26),
        b'0'..=b'9' => Some(ch - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// The bytes `save_profile` writes: `json.dumps(profile, ensure_ascii=False,
/// allow_nan=False, indent=2)` in UTF-8, refused above
/// `MAX_POUR_OVER_PROFILE_BYTES`.
pub fn serialize_pour_over_profile(profile: &Value) -> Result<String, PourOverProfileError> {
    let opts = pyfmt::DumpOpts {
        ensure_ascii: false,
        ..pyfmt::DumpOpts::indented(2)
    };
    let text = pyfmt::dumps(profile, &opts);
    if text.len() > MAX_POUR_OVER_PROFILE_BYTES {
        return Err(PourOverProfileError::TooLarge);
    }
    Ok(text)
}

/// `_without_image(profile)`: the copy kept in the list, without the
/// embedded image.
pub fn pour_over_summary(profile: &Value) -> Value {
    let mut summary = profile.clone();
    if let Some(Value::Object(display)) = summary.get_mut("display") {
        display.shift_remove("image");
    }
    summary
}

static POUR_OVER_BREW_TYPE: LazyLock<BytesRegex> = LazyLock::new(|| {
    BytesRegex::new(r#"(?-u)"brew_type"[ \t\n\r\x0B\x0C]*:[ \t\n\r\x0B\x0C]*"pour_over""#)
        .expect("a constant pattern")
});

/// `SaveProfileHandler`'s check before parsing: a body over
/// `MAX_POUR_OVER_PROFILE_BYTES` that looks like a Pour Over profile
/// (`rb'"brew_type"\s*:\s*"pour_over"'`) is refused as too large.
pub fn is_oversized_pour_over_body(body: &[u8]) -> bool {
    body.len() > MAX_POUR_OVER_PROFILE_BYTES && POUR_OVER_BREW_TYPE.is_match(body)
}

/// The decoded JPEG of a stored profile, as `get_profile_image` serves it:
/// `Ok(None)` without a JPEG data URI, `Storage` for one that does not
/// decode to a JPEG within the size limit.
pub fn pour_over_profile_image(profile: &Value) -> Result<Option<Vec<u8>>, PourOverProfileError> {
    let Some(Value::String(image)) = profile.get("display").and_then(|d| d.get("image")) else {
        return Ok(None);
    };
    let Some(data) = image.strip_prefix(JPEG_DATA_URI_PREFIX) else {
        return Ok(None);
    };
    let decoded = b64decode_strict(data)
        .ok_or_else(|| PourOverProfileError::Storage("Could not read profile image".into()))?;
    if decoded.len() > MAX_EMBEDDED_PROFILE_IMAGE_BYTES || !is_jpeg(&decoded) {
        return Err(PourOverProfileError::Storage(
            "Could not read profile image".into(),
        ));
    }
    Ok(Some(decoded))
}
