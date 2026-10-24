//! `pour_over_history.py`: the Pour Over brew history (`brew_history` rows
//! pointing at zstd JSON records under `HISTORY_PATH/pour-over/`) and the
//! `PourOverSession` contract the Dial posts.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, ErrorCode, params_from_iter};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};

use crate::db::HistoryDb;
use crate::error::{Error, Result};
use crate::files::{self, DirectoryEntry};
use crate::localtime::parse_stored;
use crate::pydantic::{
    AwareTime, Bounds, ErrorDetail, Outcome, ValidationError, loc_with, too_long,
    validate_aware_datetime, validate_float, validate_int, validate_list, validate_literal,
    validate_str,
};
use crate::sqlvalue::to_json;

/// `MAX_POUR_OVER_DURATION_MS`.
pub const MAX_POUR_OVER_DURATION_MS: f64 = 10.0 * 60.0 * 1000.0;
/// `MAX_POUR_OVER_SAMPLES`.
pub const MAX_POUR_OVER_SAMPLES: usize = 10 * 60 * 1000 / 200 + 1;
/// `MAX_POUR_OVER_RECORD_BYTES` (also the handler's body limit).
pub const MAX_POUR_OVER_RECORD_BYTES: usize = 2 * 1024 * 1024;

/// The quotas Python keeps as module constants (its tests patch them).
#[derive(Debug, Clone)]
pub struct PourOverLimits {
    /// `MAX_POUR_OVER_HISTORY_RECORDS`.
    pub max_history_records: usize,
    /// `MAX_POUR_OVER_HISTORY_BYTES`.
    pub max_history_bytes: u64,
    /// `MIN_POUR_OVER_FREE_BYTES`.
    pub min_free_bytes: u64,
}

impl Default for PourOverLimits {
    fn default() -> Self {
        Self {
            max_history_records: 1_000,
            max_history_bytes: 128 * 1024 * 1024,
            min_free_bytes: 128 * 1024 * 1024,
        }
    }
}

// ---------------------------------------------------------------------------
// The contract (pydantic models with extra="allow", allow_inf_nan=False)

#[derive(Clone, Copy)]
enum Lit {
    S(&'static str),
    I(i64),
}

#[derive(Clone, Copy)]
enum Ty {
    Str(Option<usize>, Option<usize>),
    Float(Bounds),
    Int(Bounds),
    Literal(&'static [Lit]),
    Datetime,
    Model(&'static Model),
    List(&'static Ty, Option<usize>, Option<usize>),
    FloatPair,
    Nullable(&'static Ty),
}

struct Field {
    name: &'static str,
    ty: Ty,
    /// `= None` / `Field(default=None, ...)`: may be omitted.
    optional: bool,
}

type After = fn(&Validated) -> Option<&'static str>;

struct Model {
    class: &'static str,
    fields: &'static [Field],
    after: Option<After>,
}

/// A model's validated fields: the dumped values and the datetimes.
struct Validated {
    values: Map<String, Value>,
    times: HashMap<&'static str, AwareTime>,
}

impl Validated {
    fn float(&self, name: &str) -> Option<f64> {
        self.values.get(name).and_then(Value::as_f64)
    }
}

const fn field(name: &'static str, ty: Ty) -> Field {
    Field {
        name,
        ty,
        optional: false,
    }
}

const fn default_none(name: &'static str, ty: Ty) -> Field {
    Field {
        name,
        ty,
        optional: true,
    }
}

const TIME_MS: Ty = Ty::Float(Bounds {
    ge: Some(0.0),
    gt: None,
    le: Some(MAX_POUR_OVER_DURATION_MS),
});
const GE0: Ty = Ty::Float(Bounds::GE0);
const GT0: Ty = Ty::Float(Bounds::GT0);
const ANY_FLOAT: Ty = Ty::Float(Bounds::NONE);
const DURATION_GT0: Ty = Ty::Float(Bounds {
    ge: None,
    gt: Some(0.0),
    le: Some(MAX_POUR_OVER_DURATION_MS),
});
const NULLABLE_GE0: Ty = Ty::Nullable(&GE0);
const NULLABLE_GT0: Ty = Ty::Nullable(&GT0);
const NULLABLE_STR: Ty = Ty::Nullable(&Ty::Str(None, None));

static SAMPLE: Model = Model {
    class: "FreePourSample",
    fields: &[
        field("t", TIME_MS),
        field("w", GE0),
        field("f", GE0),
        field("p", Ty::Int(Bounds::GE0)),
    ],
    after: None,
};

static POUR: Model = Model {
    class: "FreePourPour",
    fields: &[
        field("number", Ty::Int(Bounds::GT0)),
        field("startTimeMs", TIME_MS),
        field("endTimeMs", TIME_MS),
        field("startWeightG", GE0),
        field("endWeightG", GE0),
        field("waterG", GE0),
        field("averageFlowGps", GE0),
        field("peakFlowGps", GE0),
    ],
    after: Some(|v| {
        (v.float("endTimeMs") < v.float("startTimeMs"))
            .then_some("Pour end time cannot precede its start time")
    }),
};

static POUR_TARGET: Model = Model {
    class: "PourOverPourTarget",
    fields: &[
        field("number", Ty::Int(Bounds::GT0)),
        field("startTimeMs", TIME_MS),
        field("stopWeightG", GE0),
        default_none("flowGps", NULLABLE_GE0),
        default_none("flowRangeGps", Ty::Nullable(&Ty::FloatPair)),
    ],
    after: Some(|v| {
        let range = v.values.get("flowRangeGps")?.as_array()?;
        let low = range.first()?.as_f64()?;
        let high = range.get(1)?.as_f64()?;
        (low < 0.0 || high < 0.0 || low > high)
            .then_some("Flow range must be non-negative and ordered")
    }),
};

static RECIPE: Model = Model {
    class: "PourOverRecipe",
    fields: &[
        field("profileId", NULLABLE_STR),
        field("profileName", Ty::Str(Some(1), Some(512))),
        field("doseG", GT0),
        default_none("temperatureC", NULLABLE_GT0),
        field("targetWaterG", NULLABLE_GT0),
        default_none("targetDurationMs", Ty::Nullable(&DURATION_GT0)),
        field(
            "pourTargets",
            Ty::List(&Ty::Model(&POUR_TARGET), None, Some(128)),
        ),
    ],
    after: None,
};

static MEASUREMENTS: Model = Model {
    class: "PourOverMeasurements",
    fields: &[
        default_none("emptyServerG", NULLABLE_GE0),
        default_none("serverBaselineG", Ty::Nullable(&ANY_FLOAT)),
        default_none("brewerG", NULLABLE_GE0),
        field("setupG", GE0),
        default_none("doseG", NULLABLE_GT0),
        default_none("waterTemperatureC", NULLABLE_GT0),
        field("waterPouredG", GE0),
        field("beverageG", NULLABLE_GE0),
        field("retainedG", NULLABLE_GE0),
        field("durationMs", TIME_MS),
        field(
            "status",
            Ty::Literal(&[Lit::S("measured"), Lit::S("skipped")]),
        ),
    ],
    after: Some(|v| {
        let missing = |name: &str| v.values.get(name).is_none_or(Value::is_null);
        (missing("emptyServerG") && missing("serverBaselineG"))
            .then_some("A server baseline measurement is required")
    }),
};

static SYNC: Model = Model {
    class: "PourOverSync",
    fields: &[
        field(
            "status",
            Ty::Literal(&[Lit::S("pending"), Lit::S("uploaded"), Lit::S("failed")]),
        ),
        field("attempts", Ty::Int(Bounds::GE0)),
        default_none("uploadedAt", Ty::Nullable(&Ty::Datetime)),
    ],
    after: None,
};

static SESSION: Model = Model {
    class: "PourOverSession",
    fields: &[
        field(
            "schemaVersion",
            Ty::Literal(&[Lit::I(1), Lit::I(2), Lit::I(3), Lit::I(4)]),
        ),
        field("id", Ty::Str(Some(1), Some(256))),
        field("brewType", Ty::Literal(&[Lit::S("pour_over")])),
        field(
            "mode",
            Ty::Literal(&[Lit::S("free_pour"), Lit::S("profile")]),
        ),
        field("name", Ty::Str(Some(1), Some(512))),
        field("source", Ty::Literal(&[Lit::S("dial")])),
        field("startedAt", Ty::Datetime),
        field("completedAt", Ty::Datetime),
        field("recipe", Ty::Model(&RECIPE)),
        field("measurements", Ty::Model(&MEASUREMENTS)),
        field("pours", Ty::List(&Ty::Model(&POUR), None, Some(128))),
        field(
            "samples",
            Ty::List(&Ty::Model(&SAMPLE), Some(1), Some(MAX_POUR_OVER_SAMPLES)),
        ),
        field(
            "completion",
            Ty::Literal(&[Lit::S("brewer_removed"), Lit::S("dial_fallback")]),
        ),
        field("sync", Ty::Model(&SYNC)),
    ],
    after: Some(|v| {
        let started = v.times.get("startedAt")?;
        let completed = v.times.get("completedAt")?;
        (completed.utc_micros < started.utc_micros)
            .then_some("Completion time cannot precede brew start time")
    }),
};

fn float_value(f: f64) -> Value {
    Number::from_f64(f).map_or(Value::Null, Value::Number)
}

/// Validate one value; datetimes are reported to `times` under `name`.
fn validate_ty(
    ty: &Ty,
    value: &Value,
    loc: &[Value],
    name: &'static str,
    times: &mut HashMap<&'static str, AwareTime>,
) -> Outcome<Value> {
    match ty {
        Ty::Str(min, max) => validate_str(value, loc, *min, *max).map(Value::String),
        Ty::Float(bounds) => validate_float(value, loc, false, *bounds).map(float_value),
        Ty::Int(bounds) => validate_int(value, loc, *bounds).map(Value::from),
        Ty::Literal(options) => {
            let options: Vec<Value> = options
                .iter()
                .map(|o| match o {
                    Lit::S(s) => Value::from(*s),
                    Lit::I(i) => Value::from(*i),
                })
                .collect();
            validate_literal(value, loc, &options)
        }
        Ty::Datetime => {
            let time = validate_aware_datetime(value, loc)?;
            times.insert(name, time);
            Ok(Value::String(time.to_pydantic_json()))
        }
        Ty::Model(model) => validate_model(model, value, loc).map(|v| Value::Object(v.values)),
        Ty::List(item, min, max) => validate_list(value, loc, *min, *max, |element, item_loc| {
            validate_ty(item, element, item_loc, name, &mut HashMap::new())
        })
        .map(Value::Array),
        Ty::FloatPair => {
            let Value::Array(items) = value else {
                return Err(vec![ErrorDetail::new(
                    "tuple_type",
                    loc,
                    "Input should be a valid tuple",
                    value,
                )]);
            };
            let mut output = Vec::new();
            let mut errors = Vec::new();
            for index in 0..2 {
                match items.get(index) {
                    Some(element) => {
                        match validate_float(element, &loc_with(loc, index), false, Bounds::NONE) {
                            Ok(f) => output.push(float_value(f)),
                            Err(e) => errors.extend(e),
                        }
                    }
                    None => errors.push(ErrorDetail::new(
                        "missing",
                        &loc_with(loc, index),
                        "Field required",
                        value,
                    )),
                }
            }
            if items.len() > 2 {
                return Err(vec![too_long(loc, value, "Tuple", 2, items.len())]);
            }
            if errors.is_empty() {
                Ok(Value::Array(output))
            } else {
                Err(errors)
            }
        }
        Ty::Nullable(inner) => {
            if value.is_null() {
                Ok(Value::Null)
            } else {
                validate_ty(inner, value, loc, name, times)
            }
        }
    }
}

fn validate_model(model: &'static Model, value: &Value, loc: &[Value]) -> Outcome<Validated> {
    let Value::Object(input) = value else {
        return Err(vec![
            ErrorDetail::new(
                "model_type",
                loc,
                format!(
                    "Input should be a valid dictionary or instance of {}",
                    model.class
                ),
                value,
            )
            .with_ctx(pyfmt::object([("class_name", Value::from(model.class))])),
        ]);
    };
    let mut validated = Validated {
        values: Map::new(),
        times: HashMap::new(),
    };
    let mut errors = Vec::new();
    for field in model.fields {
        let field_loc = loc_with(loc, field.name);
        match input.get(field.name) {
            None if field.optional => {
                validated.values.insert(field.name.into(), Value::Null);
            }
            None => errors.push(ErrorDetail::new(
                "missing",
                &field_loc,
                "Field required",
                value,
            )),
            Some(item) => {
                match validate_ty(
                    &field.ty,
                    item,
                    &field_loc,
                    field.name,
                    &mut validated.times,
                ) {
                    Ok(v) => {
                        validated.values.insert(field.name.into(), v);
                    }
                    Err(e) => errors.extend(e),
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    // extra="allow": unknown keys are kept after the declared fields.
    for (key, item) in input {
        if !model.fields.iter().any(|f| f.name == key) {
            validated.values.insert(key.clone(), item.clone());
        }
    }
    if let Some(after) = model.after
        && let Some(message) = after(&validated)
    {
        return Err(vec![
            ErrorDetail::new("value_error", loc, format!("Value error, {message}"), value)
                .with_ctx(pyfmt::object([("error", Value::from(message))])),
        ]);
    }
    Ok(validated)
}

/// A validated `PourOverSession`.
#[derive(Debug, Clone, PartialEq)]
pub struct PourOverSession {
    /// `session.model_dump(mode="json")`: what the record file stores.
    pub payload: Value,
    pub id: String,
    pub brew_type: String,
    pub mode: String,
    pub name: String,
    pub schema_version: i64,
    pub started_at: AwareTime,
    pub completed_at: AwareTime,
}

impl PourOverSession {
    /// `PourOverSession.model_validate(raw)`; the error is what the handler
    /// answers with 422 (`error.to_json(false)` is
    /// `json.loads(error.json(include_url=False))`).
    ///
    /// PARITY: Python's json.loads accepts bare `NaN`/`Infinity` in the body
    /// and pydantic then reports `finite_number`; a body parsed with
    /// `pyfmt::loads_lenient` holds `null` there and is reported as the
    /// field's type error instead.
    pub fn validate(raw: &Value) -> Result<Self, ValidationError> {
        let validated =
            validate_model(&SESSION, raw, &[]).map_err(|errors| ValidationError { errors })?;
        let text = |name: &str| -> String {
            validated
                .values
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let (Some(started_at), Some(completed_at)) = (
            validated.times.get("startedAt").copied(),
            validated.times.get("completedAt").copied(),
        ) else {
            return Err(ValidationError { errors: Vec::new() });
        };
        Ok(Self {
            id: text("id"),
            brew_type: text("brewType"),
            mode: text("mode"),
            name: text("name"),
            schema_version: validated
                .values
                .get("schemaVersion")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            started_at,
            completed_at,
            payload: Value::Object(validated.values),
        })
    }

    /// `_timestamp_to_file_path`: `<UTC day>/<HH-MM-SS-mmm>-<sha256(id)[:12]>.pour-over.json.zst`.
    pub fn file_path(&self) -> String {
        use chrono::Timelike;
        let started = self.started_at.utc_naive().unwrap_or_default();
        let digest: String = Sha256::digest(self.id.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let timestamp = format!(
            "{}-{:06}",
            started.format("%H-%M-%S"),
            started.nanosecond() / 1000
        );
        format!(
            "{}/{}-{}.pour-over.json.zst",
            started.format("%Y-%m-%d"),
            &timestamp[..12],
            &digest[..12]
        )
    }
}

// ---------------------------------------------------------------------------
// Files

/// `read_compressed_record`: the decompressed record, refusing more than
/// [`MAX_POUR_OVER_RECORD_BYTES`].
pub fn read_compressed_record(path: &Path) -> Result<Vec<u8>> {
    let raw = files::decompress_file(path, Some(MAX_POUR_OVER_RECORD_BYTES as u64 + 1))?;
    if raw.len() > MAX_POUR_OVER_RECORD_BYTES {
        return Err(Error::PourOverTooLarge(
            "Pour Over history entry exceeds the decompressed size limit".into(),
        ));
    }
    Ok(raw)
}

/// What `PourOverFileHandler.get` finds for a request path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PourOverFile {
    /// 404 with the path: no such record.
    NotFound,
    /// 404 without the path: the request leaves the root.
    Outside,
    /// The directory listing: newest first by mtime, at most 200 entries,
    /// `name` without the `.zst` suffix.
    Directory(Vec<DirectoryEntry>),
    /// Serve [`read_compressed_record`] of this file.
    Record(PathBuf),
}

/// `Path.resolve()`: lexical `..` handling, then symlinks for what exists.
fn resolve(path: &Path) -> PathBuf {
    let mut lexical = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                lexical.pop();
            }
            Component::CurDir => {}
            other => lexical.push(other.as_os_str()),
        }
    }
    lexical.canonicalize().unwrap_or(lexical)
}

/// Resolve a `/history/pour-over/files/<path>` request.
pub fn resolve_pour_over_file(root: &Path, relative: &str) -> Result<PourOverFile> {
    let root = resolve(root);
    let requested = resolve(&root.join(relative));
    if !requested.starts_with(&root) {
        return Ok(PourOverFile::Outside);
    }
    if requested.is_dir() {
        let mut entries: Vec<(f64, String)> = Vec::new();
        for entry in fs::read_dir(&requested)? {
            let entry = entry?;
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0.0, |d| d.as_secs_f64());
            entries.push((modified, entry.file_name().to_string_lossy().into_owned()));
        }
        entries.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        return Ok(PourOverFile::Directory(
            entries
                .into_iter()
                .take(200)
                .map(|(_, name)| DirectoryEntry {
                    name: name.strip_suffix(".zst").unwrap_or(&name).to_string(),
                    url: name,
                })
                .collect(),
        ));
    }
    let mut compressed = requested.clone();
    if !compressed.is_file() && !compressed.to_string_lossy().ends_with(".zst") {
        let mut with_suffix = compressed.into_os_string();
        with_suffix.push(".zst");
        compressed = PathBuf::from(with_suffix);
    }
    if !compressed.is_file() {
        return Ok(PourOverFile::NotFound);
    }
    Ok(PourOverFile::Record(compressed))
}

/// Python's `==` on JSON-decoded values: numbers by value, `True == 1`,
/// dicts regardless of key order.
fn py_eq(a: &Value, b: &Value) -> bool {
    fn number(v: &Value) -> Option<f64> {
        match v {
            Value::Number(n) => n.as_f64(),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    }
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(x), Some(y)) => x == y,
            _ => x.as_f64() == y.as_f64(),
        },
        (Value::Bool(_) | Value::Number(_), Value::Bool(_) | Value::Number(_)) => {
            number(a) == number(b)
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| py_eq(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, value)| y.get(key).is_some_and(|other| py_eq(value, other)))
        }
        _ => a == b,
    }
}

/// `_comparable_payload`: the record without its mutable `sync` block.
fn comparable(payload: &Value) -> Value {
    match payload {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "sync")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A `brew_history` row.
#[derive(Debug, Clone)]
struct BrewRow {
    id: Value,
    uuid: Value,
    brew_type: Value,
    mode: Value,
    file: String,
    time: Option<String>,
    completed_time: Option<String>,
    name: Value,
    schema_version: Value,
}

const BREW_COLUMNS: &str = "brew_history.id, brew_history.uuid, brew_history.brew_type, brew_history.mode, brew_history.file, brew_history.time, brew_history.completed_time, brew_history.name, brew_history.schema_version";

fn read_rows(conn: &Connection, sql: &str, args: Vec<SqlValue>) -> Result<Vec<BrewRow>> {
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query(params_from_iter(args))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let text = |index: usize| -> Result<Option<String>> {
            Ok(match row.get_ref(index)? {
                rusqlite::types::ValueRef::Null => None,
                other => Some(match to_json(other)? {
                    Value::String(s) => s,
                    v => v.to_string(),
                }),
            })
        };
        result.push(BrewRow {
            id: to_json(row.get_ref(0)?)?,
            uuid: to_json(row.get_ref(1)?)?,
            brew_type: to_json(row.get_ref(2)?)?,
            mode: to_json(row.get_ref(3)?)?,
            file: text(4)?.unwrap_or_default(),
            time: text(5)?,
            completed_time: text(6)?,
            name: to_json(row.get_ref(7)?)?,
            schema_version: to_json(row.get_ref(8)?)?,
        });
    }
    Ok(result)
}

/// A `DATETIME` column read as an aware datetime (naive values are UTC).
fn stored_utc(text: Option<&str>) -> Result<AwareTime> {
    let text = text
        .ok_or_else(|| Error::InvalidValue("'NoneType' object has no attribute 'tzinfo'".into()))?;
    let stored = parse_stored(text)?;
    let offset = stored.offset.unwrap_or(0);
    Ok(AwareTime {
        utc_micros: (stored.naive.seconds - i64::from(offset)) * 1_000_000
            + i64::from(stored.naive.microsecond),
        offset,
    })
}

/// `_row_to_metadata`.
fn row_to_metadata(row: &BrewRow) -> Result<Value> {
    let started = stored_utc(row.time.as_deref())?;
    let completed = stored_utc(row.completed_time.as_deref())?;
    Ok(pyfmt::object([
        ("db_key", row.id.clone()),
        ("id", row.uuid.clone()),
        ("brewType", row.brew_type.clone()),
        ("mode", row.mode.clone()),
        ("file", Value::String(row.file.clone())),
        ("time", float_value(started.timestamp())),
        (
            "startedAt",
            Value::String(started.isoformat().replace("+00:00", "Z")),
        ),
        (
            "completedAt",
            Value::String(completed.isoformat().replace("+00:00", "Z")),
        ),
        ("name", row.name.clone()),
        ("schemaVersion", row.schema_version.clone()),
    ]))
}

/// The UTC wall clock in SQLAlchemy's `DATETIME` storage format.
fn utc_storage(time: &AwareTime) -> String {
    let utc = time.utc_naive().unwrap_or_default();
    crate::localtime::NaiveTime::from_naive(utc)
        .to_sqlite()
        .unwrap_or_default()
}

/// `shutil.disk_usage(path).free`.
fn free_bytes(path: &Path) -> Result<u64> {
    #[cfg(unix)]
    {
        let stat = rustix::fs::statvfs(path).map_err(std::io::Error::from)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(u64::MAX)
    }
}

/// What `PourOverHistoryManager.save` answers: the history metadata and
/// whether the record was created.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedPourOver {
    pub history: Value,
    pub created: bool,
}

impl HistoryDb {
    fn find_brew(conn: &Connection, session_id: &str) -> Result<Option<BrewRow>> {
        Ok(read_rows(
            conn,
            &format!("SELECT {BREW_COLUMNS} \nFROM brew_history \nWHERE brew_history.uuid = ?"),
            vec![SqlValue::Text(session_id.to_string())],
        )?
        .into_iter()
        .next())
    }

    fn pour_over_rows(conn: &Connection) -> Result<Vec<BrewRow>> {
        read_rows(
            conn,
            &format!(
                "SELECT {BREW_COLUMNS} \nFROM brew_history \nWHERE brew_history.brew_type = ? ORDER BY brew_history.time ASC, brew_history.id ASC"
            ),
            vec![SqlValue::Text("pour_over".into())],
        )
    }

    fn file_size(&self, relative: &str) -> u64 {
        fs::metadata(self.pour_over_path(Path::new(relative))).map_or(0, |m| m.len())
    }

    /// `_remove_history_row`: delete the row, then its file; returns the
    /// file's size.
    fn remove_brew_row(&self, conn: &Connection, row: &BrewRow) -> Result<u64> {
        let path = self.pour_over_path(Path::new(&row.file));
        let size = fs::metadata(&path).map_or(0, |m| m.len());
        let id = crate::sqlvalue::bind(&row.id)?;
        conn.execute(
            "DELETE FROM brew_history WHERE brew_history.id = ?",
            params_from_iter([id]),
        )?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                "Could not remove pruned Pour Over history file {}: {:?}",
                path.display(),
                error.kind()
            ),
        }
        tracing::info!("Pruned Pour Over history entry {}", row.uuid);
        Ok(size)
    }

    /// `_ensure_storage_capacity`: prune the oldest records to fit the store's
    /// own quotas (never the newest one), then require free disk space.
    fn ensure_storage_capacity(
        &self,
        conn: &Connection,
        incoming: u64,
        additional_records: usize,
    ) -> Result<()> {
        let limits = &self.pour_over_limits;
        if incoming > limits.max_history_bytes {
            return Err(Error::Storage(
                "Pour Over record exceeds the history storage quota".into(),
            ));
        }
        let mut rows = Self::pour_over_rows(conn)?;
        let mut sizes: Vec<u64> = rows.iter().map(|r| self.file_size(&r.file)).collect();
        let mut total: u64 = sizes.iter().sum();
        let over = |count: usize, total: u64| {
            count + additional_records > limits.max_history_records
                || total + incoming > limits.max_history_bytes
        };
        while rows.len() > 1 && over(rows.len(), total) {
            let row = rows.remove(0);
            let expected = sizes.remove(0);
            let removed = self.remove_brew_row(conn, &row)?;
            total = total.saturating_sub(expected.max(removed));
        }
        if over(rows.len(), total) {
            return Err(Error::Storage(
                "Pour Over history quota cannot accommodate this record".into(),
            ));
        }
        let dir = self.paths.pour_over_dir();
        let path = if dir.exists() {
            dir
        } else {
            self.paths.history_dir.clone()
        };
        fs::create_dir_all(&path)?;
        if free_bytes(&path)? < limits.min_free_bytes.saturating_add(incoming) {
            return Err(Error::Storage(
                "Insufficient free disk space for Pour Over history; existing records were preserved"
                    .into(),
            ));
        }
        Ok(())
    }

    /// `_write_record`: compact JSON (`ensure_ascii=False`, `(",", ":")`),
    /// zstd level 10, quota check, then an atomic replace.
    fn write_record(
        &self,
        conn: &Connection,
        relative: &str,
        payload: &Value,
        additional_records: usize,
    ) -> Result<()> {
        let destination = self.pour_over_path(Path::new(relative));
        let parent = destination
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.paths.pour_over_dir());
        fs::create_dir_all(&parent)?;
        let raw = pyfmt::dumps(
            payload,
            &pyfmt::DumpOpts {
                ensure_ascii: false,
                indent: None,
                sort_keys: false,
                item_separator: ",",
                key_separator: ":",
            },
        );
        if raw.len() > MAX_POUR_OVER_RECORD_BYTES {
            return Err(Error::PourOverTooLarge(
                "Pour Over history entry exceeds the size limit".into(),
            ));
        }
        let compressed = files::compress(raw.as_bytes(), files::SHOT_COMPRESSION_LEVEL)?;
        self.ensure_storage_capacity(conn, compressed.len() as u64, additional_records)?;
        let name = destination
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let temporary = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            output.write_all(&compressed)?;
            output.flush()?;
            output.sync_all()?;
            fs::rename(&temporary, &destination)?;
            Ok(())
        })();
        if temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn read_record(&self, relative: &str) -> Result<Value> {
        let raw = read_compressed_record(&self.pour_over_path(Path::new(relative)))?;
        let text = String::from_utf8(raw)
            .map_err(|e| Error::InvalidValue(format!("record is not UTF-8: {e}")))?;
        Ok(pyfmt::loads_lenient(&text)?)
    }

    /// `_validate_existing_record`: rewrite a missing file, or refuse a
    /// different record under the same session id.
    fn validate_existing(&self, conn: &Connection, row: &BrewRow, payload: &Value) -> Result<()> {
        if !self.pour_over_path(Path::new(&row.file)).is_file() {
            self.write_record(conn, &row.file, payload, 0)?;
            tracing::warn!(
                "Repaired missing Pour Over history file for session {}",
                row.uuid
            );
            return Ok(());
        }
        let existing = self.read_record(&row.file)?;
        if !py_eq(&comparable(&existing), &comparable(payload)) {
            return Err(Error::PourOverConflict);
        }
        Ok(())
    }

    fn unlink_record(&self, relative: &str) {
        let _ = fs::remove_file(self.pour_over_path(Path::new(relative)));
    }

    /// `PourOverHistoryManager.save`.
    ///
    /// PARITY: a new record answers `startedAt`/`completedAt` with
    /// `datetime.isoformat()` of the posted values (`+00:00`), an existing one
    /// with the stored UTC values written with `Z`.
    pub fn save_pour_over(&self, session: &PourOverSession) -> Result<SavedPourOver> {
        let conn = self.connection();
        if let Some(existing) = Self::find_brew(&conn, &session.id)? {
            self.validate_existing(&conn, &existing, &session.payload)?;
            return Ok(SavedPourOver {
                history: row_to_metadata(&existing)?,
                created: false,
            });
        }
        let relative = session.file_path();
        self.write_record(&conn, &relative, &session.payload, 1)?;
        let inserted = conn.execute(
            "INSERT INTO brew_history (uuid, brew_type, mode, file, time, completed_time, name, schema_version) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                session.id,
                session.brew_type,
                session.mode,
                relative,
                utc_storage(&session.started_at),
                utc_storage(&session.completed_at),
                session.name,
                session.schema_version,
            ],
        );
        let db_key = match inserted {
            Ok(_) => conn.last_insert_rowid(),
            Err(rusqlite::Error::SqliteFailure(failure, message))
                if failure.code == ErrorCode::ConstraintViolation =>
            {
                let Some(existing) = Self::find_brew(&conn, &session.id)? else {
                    self.unlink_record(&relative);
                    return Err(Error::Sqlite(rusqlite::Error::SqliteFailure(
                        failure, message,
                    )));
                };
                if relative != existing.file {
                    self.unlink_record(&relative);
                }
                self.validate_existing(&conn, &existing, &session.payload)?;
                return Ok(SavedPourOver {
                    history: row_to_metadata(&existing)?,
                    created: false,
                });
            }
            Err(error) => {
                self.unlink_record(&relative);
                return Err(error.into());
            }
        };
        tracing::info!(
            "Saved Pour Over session {} in brew history with id {db_key}",
            session.id
        );
        Ok(SavedPourOver {
            history: pyfmt::object([
                ("db_key", Value::from(db_key)),
                ("id", Value::String(session.id.clone())),
                ("brewType", Value::String(session.brew_type.clone())),
                ("mode", Value::String(session.mode.clone())),
                ("file", Value::String(relative)),
                ("time", float_value(session.started_at.timestamp())),
                ("startedAt", Value::String(session.started_at.isoformat())),
                (
                    "completedAt",
                    Value::String(session.completed_at.isoformat()),
                ),
                ("name", Value::String(session.name.clone())),
                ("schemaVersion", Value::from(session.schema_version)),
            ]),
            created: true,
        })
    }

    /// `PourOverHistoryManager.search`: metadata of the Pour Over records
    /// ordered by file (then id), after the `after` cursor, for one mode.
    pub fn search_pour_over(
        &self,
        max_results: i64,
        descending: bool,
        after: Option<&str>,
        mode: Option<&str>,
    ) -> Result<Vec<Value>> {
        let conn = self.connection();
        let mut sql =
            format!("SELECT {BREW_COLUMNS} \nFROM brew_history \nWHERE brew_history.brew_type = ?");
        let mut args = vec![SqlValue::Text("pour_over".into())];
        if let Some(after) = after.filter(|a| !a.is_empty()) {
            sql.push_str(" AND brew_history.file > ?");
            args.push(SqlValue::Text(after.to_string()));
        }
        if let Some(mode) = mode.filter(|m| !m.is_empty()) {
            sql.push_str(" AND brew_history.mode = ?");
            args.push(SqlValue::Text(mode.to_string()));
        }
        let direction = if descending { "DESC" } else { "ASC" };
        sql.push_str(&format!(
            " ORDER BY brew_history.file {direction}, brew_history.id {direction}\n LIMIT ? OFFSET ?"
        ));
        args.push(SqlValue::Integer(max_results));
        args.push(SqlValue::Integer(0));
        read_rows(&conn, &sql, args)?
            .iter()
            .map(row_to_metadata)
            .collect()
    }

    /// `PourOverHistoryManager.latest`.
    pub fn latest_pour_over(&self) -> Result<Option<Value>> {
        Ok(self
            .search_pour_over(1, true, None, None)?
            .into_iter()
            .next())
    }

    /// The `brew_history` row of a session id, as metadata.
    pub fn find_pour_over(&self, session_id: &str) -> Result<Option<Value>> {
        let conn = self.connection();
        Self::find_brew(&conn, session_id)?
            .as_ref()
            .map(row_to_metadata)
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn python_equality_of_records() {
        assert!(py_eq(
            &json!({"a": 1, "b": [1.0, true]}),
            &json!({"b": [1, 1], "a": 1.0})
        ));
        assert!(!py_eq(&json!({"a": 1}), &json!({"a": 2})));
        assert!(!py_eq(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
    }
}
