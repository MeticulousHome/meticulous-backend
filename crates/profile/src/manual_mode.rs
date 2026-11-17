//! `manual_mode.py`: turn a recorded manual brew into a saveable profile.
//!
//! During a manual brew the encoder drives the pressure or flow target and
//! the backend records it like any other setpoint. Replaying the recorded
//! targets as the points of a curve reproduces the brew. The user can switch
//! between the pressure and flow stage at will, so the samples are cut into
//! runs of consecutive samples under the same control and each run becomes
//! one stage.
//!
//! Section 5 ("Profile construction") of the Manual mode cross-repo
//! contract. Reading the shot from the history (`find_manual_shot`) is the
//! backend's: [`manual_shot_search_ids`] and [`pick_manual_shot`] are the
//! decisions around its query.

use serde_json::{Map, Value, json};

use crate::profiles::{
    MANUAL_MODE_AUTHOR_ID, MANUAL_MODE_FINAL_WEIGHT_SENTINEL, MANUAL_MODE_PROFILE_ID,
    MANUAL_MODE_TEMPERATURE,
};
use crate::pynum::{PyNum, float_value};
use crate::pyops::{self, PyExc, getitem_str};
use crate::pyrepr::{py_strip, truthy};
use crate::pyround::py_round;

pub const MAX_PROFILE_NAME_LENGTH: usize = 64;
pub const MIN_STAGE_DURATION: f64 = 0.1;
pub const MIN_MEANINGFUL_WEIGHT: f64 = 1.0;

/// What `find_manual_shot` and `build_profile_from_manual_shot` raise.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManualShotError {
    /// `ManualShotNotFound`: no history entry was brewed with the Manual
    /// mode profile (or the requested one was not).
    #[error("{0}")]
    NotFound(String),
    /// `ManualShotHasNoTargets`: the brew recorded no pressure or flow
    /// targets to build a stage from.
    #[error("the manual brew recorded no pressure or flow targets")]
    NoTargets,
    /// Any other exception, with its class name and `str()`: a shot without
    /// a `time`, a malformed history entry, a timestamp the local clock
    /// cannot convert.
    #[error("{message}")]
    Other { class: String, message: String },
}

impl ManualShotError {
    pub fn class_name(&self) -> &str {
        match self {
            ManualShotError::NotFound(_) => "ManualShotNotFound",
            ManualShotError::NoTargets => "ManualShotHasNoTargets",
            ManualShotError::Other { class, .. } => class,
        }
    }

    /// The answer `CreateProfileFromManualHandler` sends for this error:
    /// status and body.
    pub fn http_response(&self) -> (u16, Value) {
        match self {
            ManualShotError::NotFound(_) => (
                404,
                json!({"status": "error", "error": "no manual brew found"}),
            ),
            ManualShotError::NoTargets => (
                409,
                json!({"status": "error", "error": "shot has no target samples"}),
            ),
            ManualShotError::Other { message, .. } => (
                400,
                json!({
                    "status": "error",
                    "error": "failed to create profile from manual brew",
                    "cause": message,
                }),
            ),
        }
    }
}

impl From<PyExc> for ManualShotError {
    fn from(exc: PyExc) -> Self {
        ManualShotError::Other {
            class: exc.class_name().into(),
            message: exc.message().into(),
        }
    }
}

/// The `ids` of the history query `find_manual_shot` runs: the requested
/// shot, or the Manual mode profile id (the query matches either a shot's
/// db id, its uuid, or its profile's id). The query sorts by time,
/// descending, takes one result and loads the shot data.
pub fn manual_shot_search_ids(shot_id: Option<&str>) -> Vec<String> {
    match shot_id {
        Some(id) if !id.is_empty() => vec![id.to_string()],
        _ => vec![MANUAL_MODE_PROFILE_ID.to_string()],
    }
}

/// The rest of `find_manual_shot`: the first search result, which must have
/// been brewed with the Manual mode profile when a shot was named.
pub fn pick_manual_shot(
    shot_id: Option<&str>,
    results: Vec<Value>,
) -> Result<Value, ManualShotError> {
    let Some(shot) = results.into_iter().next() else {
        return Err(ManualShotError::NotFound(
            "no manual brew in history".into(),
        ));
    };
    let named = shot_id.is_some_and(|id| !id.is_empty());
    if named {
        let profile_id = shot
            .get("profile")
            .filter(|p| truthy(p))
            .and_then(|p| p.get("id"));
        if profile_id.and_then(Value::as_str) != Some(MANUAL_MODE_PROFILE_ID) {
            return Err(ManualShotError::NotFound(
                "the requested shot was not brewed in manual mode".into(),
            ));
        }
    }
    Ok(shot)
}

/// A local date and time, as `datetime.fromtimestamp` gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalDateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// What the builder needs from outside: fresh uuid4 strings and the local
/// clock.
pub trait ManualShotEnv {
    /// `str(uuid.uuid4())`.
    fn new_uuid(&mut self) -> String;
    /// `datetime.fromtimestamp(timestamp)` in the machine's time zone. Python
    /// rounds the timestamp to microseconds, half to even, before splitting
    /// it. An error is reported as the exception `(class, message)` Python
    /// raises (`OverflowError`, `ValueError`, `OSError`).
    fn local_time(&self, timestamp: f64) -> Result<LocalDateTime, (String, String)>;
}

/// `_is_number(value)`: a finite int or float, not a bool.
fn is_number(value: Option<&Value>) -> Option<PyNum> {
    let n = PyNum::from_value(value?)?;
    match n {
        PyNum::Float(f) if !f.is_finite() => None,
        n => Some(n),
    }
}

struct Run {
    control: &'static str,
    points: Vec<Value>,
    start_time: PyNum,
    last_x: Option<f64>,
    last_sample: Value,
}

/// `round(value, ndigits)`: an int stays the same int.
fn round_number(n: PyNum, ndigits: usize) -> Value {
    match n {
        PyNum::Int(_) => n.to_value(),
        PyNum::Float(f) => float_value(py_round(f, ndigits)),
    }
}

/// `_target_runs(shot)`.
fn target_runs(shot: &Value) -> Result<Vec<Run>, ManualShotError> {
    let data = match shot.get("data") {
        Some(data) if truthy(data) => data,
        _ => return Ok(Vec::new()),
    };
    let samples: Vec<Value> = match pyops::iterate(data)? {
        pyops::PyIter::List => data.as_array().cloned().unwrap_or_default(),
        pyops::PyIter::Items(items) => items,
    };

    let mut runs: Vec<Run> = Vec::new();
    for sample in &samples {
        let Some(sample_map) = sample.as_object() else {
            continue;
        };
        let Some(recorded) = sample_map.get("shot").filter(|r| r.is_object()) else {
            continue;
        };
        let Some(setpoints) = recorded.get("setpoints").filter(|s| s.is_object()) else {
            continue;
        };
        let control = match setpoints.get("active").and_then(Value::as_str) {
            Some("pressure") => "pressure",
            Some("flow") => "flow",
            _ => continue,
        };
        let Some(target) = is_number(setpoints.get(control)) else {
            continue;
        };
        // `sample.get("profile_time", sample.get("time"))`. PARITY: a present
        // but null `profile_time` does not fall back to `time`; the sample is skipped.
        let timestamp = match sample_map.get("profile_time") {
            Some(value) => is_number(Some(value)),
            None => is_number(sample_map.get("time")),
        };
        let Some(timestamp) = timestamp else {
            continue;
        };

        if runs.last().is_none_or(|run| run.control != control) {
            runs.push(Run {
                control,
                points: Vec::new(),
                start_time: timestamp,
                last_x: None,
                last_sample: Value::Null,
            });
        }
        let Some(run) = runs.last_mut() else {
            continue;
        };
        // The newest usable target, even if its timestamp adds no point.
        run.last_sample = recorded.clone();

        let elapsed = (timestamp - run.start_time).as_f64() / 1000.0;
        let x = py_round(elapsed, 2);
        if run.last_x.is_some_and(|last| x <= last) {
            continue;
        }
        run.points
            .push(Value::Array(vec![float_value(x), round_number(target, 2)]));
        run.last_x = Some(x);
    }
    Ok(runs)
}

/// `str.capitalize()` of the two control names.
fn capitalized(control: &str) -> &'static str {
    if control == "pressure" {
        "Pressure"
    } else {
        "Flow"
    }
}

/// `_stage_from_run(run, index)`.
fn stage_from_run(run: &Run, index: usize, env: &mut dyn ManualShotEnv) -> Value {
    let duration = run
        .points
        .last()
        .and_then(|point| point.get(0))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    // `max(duration, MIN_STAGE_DURATION)` keeps the first of equal values.
    let exit_after = if MIN_STAGE_DURATION > duration {
        MIN_STAGE_DURATION
    } else {
        duration
    };
    let mut stage = Map::new();
    stage.insert(
        "name".into(),
        Value::from(format!("{} {index}", capitalized(run.control))),
    );
    stage.insert("key".into(), Value::from(env.new_uuid()));
    stage.insert("type".into(), Value::from(run.control));
    stage.insert(
        "dynamics".into(),
        json!({
            "points": run.points,
            "over": "time",
            "interpolation": "none",
        }),
    );
    stage.insert(
        "exit_triggers".into(),
        json!([{
            "type": "time",
            "value": float_value(exit_after),
            "relative": true,
            "comparison": ">=",
        }]),
    );
    stage.insert("limits".into(), json!([]));
    Value::Object(stage)
}

/// `build_profile_from_manual_shot(shot, name)`: a history entry (the
/// `search_history` shape with `dump_data=True`) to a profile document.
pub fn build_profile_from_manual_shot(
    shot: &Value,
    name: Option<&str>,
    env: &mut dyn ManualShotEnv,
) -> Result<Value, ManualShotError> {
    if !shot.is_object() {
        return Err(pyops::no_attribute_get(shot).into());
    }
    let runs = target_runs(shot)?;
    if runs.is_empty() {
        return Err(ManualShotError::NoTargets);
    }

    let stages: Vec<Value> = runs
        .iter()
        .enumerate()
        .map(|(index, run)| stage_from_run(run, index + 1, env))
        .collect();

    let final_weight = match runs
        .last()
        .and_then(|run| is_number(run.last_sample.get("weight")))
    {
        Some(weight)
            if weight.py_cmp(PyNum::Float(MIN_MEANINGFUL_WEIGHT))
                != Some(std::cmp::Ordering::Less) =>
        {
            round_number(weight, 1)
        }
        _ => float_value(MANUAL_MODE_FINAL_WEIGHT_SENTINEL),
    };

    let empty = Value::Object(Map::new());
    let source_profile = match shot.get("profile") {
        Some(profile) if truthy(profile) => profile,
        _ => &empty,
    };
    if !source_profile.is_object() {
        return Err(pyops::no_attribute_get(source_profile).into());
    }
    let temperature = match source_profile.get("temperature") {
        Some(t) if is_number(Some(t)).is_some() => t.clone(),
        _ => float_value(MANUAL_MODE_TEMPERATURE),
    };

    let profile_name = match name.map(py_strip) {
        Some(stripped) if !stripped.is_empty() => {
            stripped.chars().take(MAX_PROFILE_NAME_LENGTH).collect()
        }
        _ => {
            let time = getitem_str(shot, "time")?;
            let timestamp = match time {
                Value::Number(n) => PyNum::from_number(n).as_f64(),
                Value::Bool(b) => f64::from(u8::from(*b)),
                other => {
                    return Err(ManualShotError::Other {
                        class: "TypeError".into(),
                        message: format!(
                            "'{}' object cannot be interpreted as an integer",
                            crate::pyrepr::type_name(other)
                        ),
                    });
                }
            };
            let local = env
                .local_time(timestamp)
                .map_err(|(class, message)| ManualShotError::Other { class, message })?;
            format!(
                "Manual {}-{:02}-{:02} {:02}:{:02}",
                local.year, local.month, local.day, local.hour, local.minute
            )
        }
    };

    let author = match source_profile.get("author") {
        Some(author) if truthy(author) => author.clone(),
        _ => Value::from(""),
    };
    let author_id = match source_profile.get("author_id") {
        Some(id) if truthy(id) => id.clone(),
        _ => Value::from(MANUAL_MODE_AUTHOR_ID),
    };

    let mut profile = Map::new();
    profile.insert("id".into(), Value::from(env.new_uuid()));
    profile.insert("name".into(), Value::String(profile_name));
    profile.insert("author".into(), author);
    profile.insert("author_id".into(), author_id);
    profile.insert("previous_authors".into(), json!([]));
    profile.insert("temperature".into(), temperature);
    profile.insert("final_weight".into(), final_weight);
    profile.insert("variables".into(), json!([]));
    profile.insert("display".into(), json!({}));
    profile.insert("stages".into(), Value::Array(stages));
    Ok(Value::Object(profile))
}
