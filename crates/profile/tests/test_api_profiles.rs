//! The pure parts of `tests/test_api_profiles.py`: the validation outcomes
//! and error texts the save and load handlers answer with, and the bytes a
//! load streams to the ESP32. Files, events, config and HTTP routing are the
//! backend's.

mod common;

use common::{TestEnv, dumps, md5_hex, repo_json};
use profile::manual_mode::build_profile_from_manual_shot;
use profile::{
    ESP32_PROFILE_FIELDS, ProfileError, ProgramSettings, ReferenceIds, build_manual_mode_profile,
    prepare_for_esp32, validate_profile,
};
use serde_json::{Map, Value, json};

fn example_profile() -> Value {
    repo_json("profile_schema/example_profile.json")
}

fn simple_profile() -> Value {
    repo_json("simple_profile.json")
}

/// The test's `substitute_variables`.
fn substitute_variables(node: &Value, values: &Map<String, Value>) -> Value {
    match node {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), substitute_variables(v, values)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| substitute_variables(v, values))
                .collect(),
        ),
        Value::String(s) if s.starts_with('$') => values[&s[1..]].clone(),
        other => other.clone(),
    }
}

/// The test's `expected_esp32_payload`: only the runtime fields, with the
/// `$variables` resolved.
fn expected_esp32_payload(profile: &Value) -> Value {
    let values: Map<String, Value> = profile
        .get("variables")
        .and_then(Value::as_array)
        .map(|vars| {
            vars.iter()
                .map(|v| {
                    (
                        v["key"].as_str().expect("key").to_string(),
                        v["value"].clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut payload = Map::new();
    for field in ESP32_PROFILE_FIELDS {
        if let Some(value) = profile.get(field) {
            payload.insert(field.to_string(), value.clone());
        }
    }
    payload.insert(
        "stages".into(),
        substitute_variables(&profile["stages"], &values),
    );
    Value::Object(payload)
}

/// The test's `expected_serial_writes`.
fn expected_serial_writes(payload: &Value) -> Vec<Vec<u8>> {
    let body = dumps(payload);
    let digest = md5_hex(&body);
    vec![
        b"hash,".to_vec(),
        digest.into_bytes(),
        b"\x03".to_vec(),
        format!("json\n{body}\x03").into_bytes(),
    ]
}

fn sent(profile: &Value) -> Result<Value, ProfileError> {
    prepare_for_esp32(
        profile,
        &ProgramSettings::default(),
        &mut ReferenceIds::default(),
    )
}

/// `test_save_rejects_a_profile_missing_a_required_field`.
#[test]
fn save_rejects_a_profile_missing_a_required_field() {
    let mut profile = example_profile();
    profile.as_object_mut().expect("object").remove("stages");
    let error = validate_profile(&profile).expect_err("invalid");
    assert_eq!(
        error.validation_message().as_deref(),
        Some("JSON validation error: 'stages' is a required property")
    );
}

/// `test_save_rejects_a_temperature_above_the_schema_maximum`.
#[test]
fn save_rejects_a_temperature_above_the_schema_maximum() {
    let mut profile = example_profile();
    profile["temperature"] = json!(150);
    let error = validate_profile(&profile).expect_err("invalid");
    assert_eq!(
        error.validation_message().as_deref(),
        Some("JSON validation error: 150 is greater than the maximum of 100")
    );
}

/// `test_save_rejects_a_profile_with_an_undefined_variable`: the save
/// handler answers `failed to save profile` with `str(err)` as the cause.
#[test]
fn save_rejects_a_profile_with_an_undefined_variable() {
    let mut profile = example_profile();
    profile["stages"][0]["dynamics"]["points"][0][1] = json!("$missing_var");
    let error = validate_profile(&profile).expect_err("invalid");
    assert_eq!(error.validation_message(), None);
    assert_eq!(error.to_string(), "Variable missing_var is not defined");
}

/// `test_loading_a_saved_profile_streams_only_its_runtime_fields_to_the_esp32`.
#[test]
fn loading_a_saved_profile_streams_only_its_runtime_fields() {
    let profile = example_profile();
    let payload = expected_esp32_payload(&profile);
    assert_eq!(payload["stages"][1]["dynamics"]["points"], json!([[0, 8]]));

    let actual = sent(&profile).expect("loads");
    assert_eq!(
        expected_serial_writes(&actual),
        expected_serial_writes(&payload)
    );
}

/// `test_loading_a_profile_body_streams_it_without_saving_it`.
#[test]
fn loading_a_profile_body_streams_it() {
    let profile = simple_profile();
    let payload = expected_esp32_payload(&profile);
    assert_eq!(
        payload["stages"][0]["dynamics"]["points"],
        json!([[0, 10], [3, 10]])
    );
    assert_eq!(
        expected_serial_writes(&sent(&profile).expect("loads")),
        expected_serial_writes(&payload)
    );
}

/// `test_loading_a_profile_body_without_an_id_assigns_one`: the backend
/// assigns the id; the bytes do not carry it.
#[test]
fn loading_a_profile_body_without_an_id_streams_the_same_bytes() {
    let mut profile = example_profile();
    profile["id"] = json!("6f1c5a52-6f4c-4d38-9a51-0a8f2a8f7c11");
    assert_eq!(
        expected_serial_writes(&sent(&profile).expect("loads")),
        expected_serial_writes(&expected_esp32_payload(&example_profile()))
    );
}

/// `test_loading_a_schema_invalid_profile_body_is_rejected_before_the_esp32`.
#[test]
fn loading_a_schema_invalid_profile_body_is_rejected() {
    let mut profile = example_profile();
    profile["temperature"] = json!(150);
    let error = sent(&profile).expect_err("rejected");
    assert_eq!(
        error.validation_message().as_deref(),
        Some("JSON validation error: 150 is greater than the maximum of 100")
    );
}

/// `test_loading_a_profile_body_with_an_undefined_variable_is_rejected`.
#[test]
fn loading_a_profile_body_with_an_undefined_variable_is_rejected() {
    let mut profile = example_profile();
    profile["stages"][0]["dynamics"]["points"][0][1] = json!("$missing_var");
    let error = sent(&profile).expect_err("rejected");
    assert!(error.is_variable_error());
    assert_eq!(
        format!("variable error: {}:{}", error.class_name(), error),
        "variable error: UndefinedVariableException:Variable missing_var is not defined"
    );
}

/// `test_limited_access_loads_the_bundled_simple_profile_by_id`: the bytes
/// streamed for the bundled profile.
#[test]
fn limited_access_streams_the_bundled_simple_profile() {
    let profile = simple_profile();
    assert_eq!(
        expected_serial_writes(&sent(&profile).expect("loads")),
        expected_serial_writes(&expected_esp32_payload(&profile))
    );
}

/// `test_profile_built_from_a_manual_brew_is_saved_like_any_other_profile`:
/// the profile the builder makes from that brew passes the save validation.
#[test]
fn profile_built_from_a_manual_brew_is_saved_like_any_other_profile() {
    let mut source = build_manual_mode_profile();
    source["temperature"] = json!(93.5);
    let data: Vec<Value> = [(1000, 6.0, 0.0), (5000, 9.0, 36.0)]
        .iter()
        .map(|(time, pressure, weight)| {
            json!({
                "shot": {
                    "pressure": pressure,
                    "flow": 1.2,
                    "weight": weight,
                    "setpoints": {"active": "pressure", "pressure": pressure},
                },
                "time": time,
                "profile_time": time,
                "status": "Manual",
            })
        })
        .collect();
    let shot = json!({
        "id": "8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6",
        "time": 1757193600.0,
        "file": "2026-09-07/10:00:00.shot.json.zst",
        "name": "Manual mode",
        "data": data,
        "profile": source,
    });

    let saved =
        build_profile_from_manual_shot(&shot, Some("Morning manual"), &mut TestEnv::default())
            .expect("builds");

    assert_eq!(validate_profile(&saved), Ok(()));
    assert_eq!(saved["name"], json!("Morning manual"));
    assert_eq!(saved["temperature"], json!(93.5));
    assert!(saved.get("manual").is_none());
}
