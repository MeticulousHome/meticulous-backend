//! The pure parts of `profiles.py` (`ProfileManager`): validation, what is
//! sent to the ESP32, the Manual mode profile and the normalisation applied
//! to profiles read from disk.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde_json::{Map, Value, json};

use crate::converter::ReferenceIds;
use crate::manual_program::{ProgramError, ProgramSettings, build_manual_program};
use crate::preprocessor::{PreprocessError, process_variables};
use crate::pynum::{float_value, py_eq};
use crate::pyops::{self, PyExc, PyIter, contains_str, getitem_str, setitem_str};
use crate::schema::{ValidationError, Validator};

/// `profile_schema/schema.json` (the espresso-profile-schema submodule),
/// vendored so the crate builds without the submodule checked out. A test
/// compares it with the submodule when that is present.
pub const ESPRESSO_PROFILE_SCHEMA: &str = include_str!("../schemas/espresso_profile_schema.json");

/// Only recipe and runtime data goes over the UART: the node engine reads
/// `name`/`stages`, the simplified engine also `temperature`,
/// `final_weight` and `variables`.
pub const ESP32_PROFILE_FIELDS: [&str; 5] =
    ["name", "temperature", "final_weight", "variables", "stages"];

/// Manual mode profile (cross-repo contract "Manual mode" v5, section 2).
/// The id is stable across boots and machines; clients still detect a
/// manual profile by `manual is True`, never by id or name.
pub const MANUAL_MODE_PROFILE_ID: &str = "4d616e75-616c-4d6f-8465-000000000001";
pub const MANUAL_MODE_PROFILE_NAME: &str = "Manual mode";
pub const MANUAL_MODE_PRESSURE_STAGE_NAME: &str = "Manual pressure";
pub const MANUAL_MODE_PRESSURE_STAGE_KEY: &str = "manual_pressure";
pub const MANUAL_MODE_FLOW_STAGE_NAME: &str = "Manual flow";
pub const MANUAL_MODE_FLOW_STAGE_KEY: &str = "manual_flow";
pub const MANUAL_MODE_TEMPERATURE: f64 = 90.0;
/// "No weight stop": the schema's maximum `final_weight`, a target the brew
/// cannot reach. v1 shipped 1000.0, which a large enough carafe could hit.
pub const MANUAL_MODE_FINAL_WEIGHT_SENTINEL: f64 = 2000.0;
pub const MANUAL_MODE_LEGACY_FINAL_WEIGHT_SENTINEL: f64 = 1000.0;
pub const MANUAL_MODE_AUTHOR: &str = "Meticulous";
pub const MANUAL_MODE_AUTHOR_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The author id `refresh_profile_list` gives a profile without one.
pub const ZERO_AUTHOR_ID: &str = "00000000-0000-0000-0000-000000000000";

static ESPRESSO_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    Validator::from_json(ESPRESSO_PROFILE_SCHEMA, false)
        .expect("the vendored espresso profile schema loads (checked by the crate's tests)")
});

/// The espresso profile schema as `jsonschema.validate` applies it (no
/// format checker, so `format: uuid` is not asserted).
pub fn espresso_validator() -> &'static Validator {
    &ESPRESSO_VALIDATOR
}

/// Why a profile cannot be validated or sent.
///
/// How the handlers answer (`api/profiles.py`):
/// - `Schema`: 400 `{"status": "error", "error": "JSON validation error: <message>"}`
///   on save and load alike.
/// - on `POST /profile/load`, a `Preprocess` error whose
///   [`PreprocessError::is_variable_error`] holds: 400
///   `{"status": "error", "error": "variable error: <class_name>:<message>"}`;
/// - anything else on load: 400 `{"status": "error", "error": "failed to load profile <message>"}`;
/// - anything else on save: 400
///   `{"status": "error", "error": "failed to save profile", "cause": "<message>"}`.
///
/// `<message>` is this error's `Display`, Python's `str(err)`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProfileError {
    /// `processVariables` failed.
    #[error("{0}")]
    Preprocess(PreprocessError),
    /// `validate_profile` logs `data['id']` before validating, so a profile
    /// without an id raises `KeyError('id')`. Every caller sets an id first.
    #[error("'id'")]
    MissingId,
    /// `jsonschema.validate` raised this `best_match` error.
    #[error("{}", .0.message)]
    Schema(Box<ValidationError>),
    /// `build_manual_program` failed.
    #[error("{0}")]
    Program(ProgramError),
}

impl ProfileError {
    /// `err.__class__.__name__`.
    pub fn class_name(&self) -> &'static str {
        match self {
            ProfileError::Preprocess(err) => err.class_name(),
            ProfileError::MissingId => "KeyError",
            ProfileError::Schema(_) => "ValidationError",
            ProfileError::Program(err) => err.class_name(),
        }
    }

    /// `JSON validation error: <message>` for a schema error.
    pub fn validation_message(&self) -> Option<String> {
        match self {
            ProfileError::Schema(err) => Some(format!("JSON validation error: {}", err.message)),
            _ => None,
        }
    }

    /// One of the three preprocessor exceptions the load handler reports
    /// as `variable error: <class>:<message>`.
    pub fn is_variable_error(&self) -> bool {
        matches!(self, ProfileError::Preprocess(err) if err.is_variable_error())
    }
}

impl From<PreprocessError> for ProfileError {
    fn from(err: PreprocessError) -> Self {
        ProfileError::Preprocess(err)
    }
}

impl From<ProgramError> for ProfileError {
    fn from(err: ProgramError) -> Self {
        ProfileError::Program(err)
    }
}

/// `ProfileManager.validate_profile(data)`: the variables must resolve, then
/// the document (with its `$references` unresolved) must match the schema.
pub fn validate_profile(data: &Value) -> Result<(), ProfileError> {
    process_variables(data)?;
    if data.get("id").is_none() {
        return Err(ProfileError::MissingId);
    }
    espresso_validator()
        .validate(data)
        .map_err(ProfileError::Schema)
}

/// `ProfileManager._profile_for_esp32(profile)`: the runtime fields, in
/// `ESP32_PROFILE_FIELDS` order.
pub fn profile_for_esp32(profile: &Value) -> Value {
    let mut out = Map::new();
    for field in ESP32_PROFILE_FIELDS {
        if let Some(value) = profile.get(field) {
            out.insert(field.to_string(), value.clone());
        }
    }
    Value::Object(out)
}

/// What `ProfileManager.send_profile_to_esp32(data)` passes to
/// `Machine.send_json_with_hash`: validate, resolve the variables, render a
/// manual profile (`"manual": true`) as its node program, keep the runtime
/// fields. Serialise the result with `pyfmt::dumps_default` for the UART.
///
/// The caller does what the Python does around this: refuse while the motor
/// stress alarm is set, give `data` an id if it has none, run `handle_image`
/// (which needs `data["display"]`), and afterwards record the last profile
/// and emit the load and hover events. `settings` and `reference_ids` only
/// matter for a manual profile.
pub fn prepare_for_esp32(
    data: &Value,
    settings: &ProgramSettings,
    reference_ids: &mut ReferenceIds,
) -> Result<Value, ProfileError> {
    validate_profile(data)?;
    let preprocessed = process_variables(data)?;
    let program = if data.get("manual") == Some(&Value::Bool(true)) {
        build_manual_program(&preprocessed, settings, reference_ids)?
    } else {
        preprocessed
    };
    Ok(profile_for_esp32(&program))
}

/// `ProfileManager.manual_mode_stages()`: the seeded stage pair, pressure
/// first. Only `stages[0].dynamics.points[0][1]` is read at load time, as the
/// initial target of the stage the shot starts in.
pub fn manual_mode_stages() -> Value {
    json!([
        {
            "name": MANUAL_MODE_PRESSURE_STAGE_NAME,
            "key": MANUAL_MODE_PRESSURE_STAGE_KEY,
            "type": "pressure",
            "dynamics": {
                "points": [[0, 0]],
                "over": "time",
                "interpolation": "none",
            },
            "exit_triggers": [{"type": "user_interaction", "value": 1}],
            "limits": [],
        },
        {
            "name": MANUAL_MODE_FLOW_STAGE_NAME,
            "key": MANUAL_MODE_FLOW_STAGE_KEY,
            "type": "flow",
            "dynamics": {
                "points": [[0, 0]],
                "over": "time",
                "interpolation": "none",
            },
            "exit_triggers": [{"type": "user_interaction", "value": 1}],
            "limits": [],
        },
    ])
}

/// `ProfileManager.build_manual_mode_profile()`: a fresh seeded Manual mode
/// document.
pub fn build_manual_mode_profile() -> Value {
    json!({
        "id": MANUAL_MODE_PROFILE_ID,
        "name": MANUAL_MODE_PROFILE_NAME,
        "author": MANUAL_MODE_AUTHOR,
        "author_id": MANUAL_MODE_AUTHOR_ID,
        "previous_authors": [],
        "temperature": float_value(MANUAL_MODE_TEMPERATURE),
        "final_weight": float_value(MANUAL_MODE_FINAL_WEIGHT_SENTINEL),
        "variables": [],
        "display": {},
        "manual": true,
        "stages": manual_mode_stages(),
    })
}

/// `ProfileManager.manual_profile_has_seeded_stages(profile)`: the profile
/// carries the section 2 stage pair, in either order (the order records
/// which stage the shot starts in).
///
/// Python builds a set of `(key, type)` tuples, so a stage whose key or type
/// is a list or dict raises `TypeError`; this answers `false` instead. Known
/// profiles reach this only after schema validation, where both are strings.
pub fn manual_profile_has_seeded_stages(profile: &Value) -> bool {
    let Some(Value::Array(stages)) = profile.get("stages") else {
        return false;
    };
    if stages.len() != 2 {
        return false;
    }
    let mut found = BTreeSet::new();
    for stage in stages {
        let Value::Object(stage) = stage else {
            return false;
        };
        // A pair that is not two strings can never be one of the expected.
        let (Some(Value::String(key)), Some(Value::String(kind))) =
            (stage.get("key"), stage.get("type"))
        else {
            return false;
        };
        found.insert((key.as_str(), kind.as_str()));
    }
    let expected = BTreeSet::from([
        (MANUAL_MODE_PRESSURE_STAGE_KEY, "pressure"),
        (MANUAL_MODE_FLOW_STAGE_KEY, "flow"),
    ]);
    found == expected
}

/// What `ensure_manual_mode_profile` writes on boot.
#[derive(Debug, Clone, PartialEq)]
pub enum ManualModeSeed {
    /// The profile has the seeded pair already: nothing to write.
    UpToDate,
    /// No Manual mode profile: write this fresh one ("Seeded").
    Seed(Value),
    /// The stages are damaged or from v1: write this copy, which keeps the
    /// user's fields and gets the seeded stages ("Migrated").
    Migrate(Value),
}

/// The decision `ProfileManager.ensure_manual_mode_profile()` takes about
/// the known profile with `MANUAL_MODE_PROFILE_ID`. The backend saves the
/// returned profile with `set_last_changed=True` and swallows a failure.
pub fn plan_manual_mode_profile(existing: Option<&Value>) -> ManualModeSeed {
    let Some(existing) = existing else {
        return ManualModeSeed::Seed(build_manual_mode_profile());
    };
    if manual_profile_has_seeded_stages(existing) {
        return ManualModeSeed::UpToDate;
    }
    let mut profile = existing.clone();
    if let Value::Object(map) = &mut profile {
        map.insert("manual".into(), Value::Bool(true));
        map.insert("stages".into(), manual_mode_stages());
        let legacy = map
            .get("final_weight")
            .is_some_and(|w| py_eq(w, &float_value(MANUAL_MODE_LEGACY_FINAL_WEIGHT_SENTINEL)));
        if legacy {
            map.insert(
                "final_weight".into(),
                float_value(MANUAL_MODE_FINAL_WEIGHT_SENTINEL),
            );
        }
    }
    ManualModeSeed::Migrate(profile)
}

/// The defaults `refresh_profile_list` fills into a profile read from disk.
/// Returns whether the profile changed (then the Python re-saves it with
/// `set_last_changed=True` and `skip_validation=True`, after validating it;
/// a missing `last_changed` alone counts as a change). `new_uuid` gives
/// `str(uuid.uuid4())`, called for the id first, then for each stage
/// without a key, in order.
///
/// An error is the exception Python raises for a document of the wrong shape
/// (a list or a string, or stages that are not objects). PARITY: Python does
/// not catch it, so one such file makes `refresh_profile_list`, and with it
/// `ProfileManager.init`, fail.
pub fn normalize_loaded_profile(
    profile: &mut Value,
    new_uuid: &mut dyn FnMut() -> String,
) -> Result<bool, PyExc> {
    let mut changed = false;
    let empty = Value::from("");

    if !contains_str(profile, "id")? || py_eq(getitem_str(profile, "id")?, &empty) {
        setitem_str(profile, "id", Value::from(new_uuid()))?;
        changed = true;
    }
    if !contains_str(profile, "author")? {
        setitem_str(profile, "author", Value::from(""))?;
        changed = true;
    }
    if !contains_str(profile, "author_id")? || py_eq(getitem_str(profile, "author_id")?, &empty) {
        setitem_str(profile, "author_id", Value::from(ZERO_AUTHOR_ID))?;
        changed = true;
    }
    if !contains_str(profile, "last_changed")? {
        changed = true;
    }
    if !contains_str(profile, "stages")? {
        setitem_str(profile, "stages", json!([]))?;
        changed = true;
    } else {
        // Only a dict gets past the `profile["id"]` steps above.
        let stages = profile
            .get_mut("stages")
            .expect("checked above: the profile is a dict with \"stages\"");
        match pyops::iterate(stages)? {
            PyIter::List => {
                if let Value::Array(items) = stages {
                    for stage in items {
                        if !contains_str(stage, "key")? {
                            setitem_str(stage, "key", Value::from(new_uuid()))?;
                            changed = true;
                        }
                    }
                }
            }
            PyIter::Items(mut items) => {
                for stage in &mut items {
                    if !contains_str(stage, "key")? {
                        setitem_str(stage, "key", Value::from(new_uuid()))?;
                        changed = true;
                    }
                }
            }
        }
    }
    Ok(changed)
}
