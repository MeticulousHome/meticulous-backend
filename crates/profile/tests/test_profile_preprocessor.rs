//! `tests/test_profile_preprocessor.py`.

use profile::{PreprocessError, process_variables};
use serde_json::{Value, json};

fn profile() -> Value {
    json!({
        "variables": [
            {"name": "Pressure", "key": "pressure_1", "type": "pressure", "value": 9},
            {"name": "Time", "key": "time_1", "type": "time", "value": 30},
        ],
        "stages": [
            {
                "name": "Test Stage",
                "type": "pressure",
                "dynamics": {
                    "points": [["$time_1", "$pressure_1"]],
                    "over": "time",
                    "interpolation": "linear",
                },
                "exit_triggers": [{"type": "time", "value": "$time_1"}],
                "limits": [
                    {"type": "flow", "value": 2.1},
                    {"type": "pressure", "value": "$pressure_1"},
                ],
            }
        ],
    })
}

/// `TestProfilePreprocessor.test_successful_processing`.
#[test]
fn successful_processing() {
    let processed = process_variables(&profile()).expect("processes");
    assert_eq!(
        processed["stages"][0]["dynamics"]["points"][0],
        json!([30, 9])
    );
    assert_eq!(
        processed["stages"][0]["exit_triggers"][0]["value"],
        json!(30)
    );
    assert_eq!(processed["stages"][0]["limits"][1]["value"], json!(9));
    // The input is left as it was (Python works on a deep copy).
    assert_eq!(
        profile()["stages"][0]["dynamics"]["points"][0],
        json!(["$time_1", "$pressure_1"])
    );
}

/// `TestProfilePreprocessor.test_missing_variables`.
#[test]
fn missing_variables() {
    let mut profile = profile();
    profile["variables"] = json!([]);
    let error = process_variables(&profile).expect_err("undefined variable");
    assert!(matches!(error, PreprocessError::UndefinedVariable(_)));
    assert_eq!(error.class_name(), "UndefinedVariableException");
    assert_eq!(error.to_string(), "Variable time_1 is not defined");
}

/// `TestProfilePreprocessor.test_missing_variables_field`.
#[test]
fn missing_variables_field() {
    let mut profile = profile();
    profile.as_object_mut().expect("object").remove("variables");
    let error = process_variables(&profile).expect_err("undefined variable");
    assert!(matches!(error, PreprocessError::UndefinedVariable(_)));
}

/// `TestProfilePreprocessor.test_incorrect_variable_key`.
#[test]
fn incorrect_variable_key() {
    let mut profile = profile();
    profile["stages"][0]["dynamics"]["points"][0][0] = json!("$time_2");
    let error = process_variables(&profile).expect_err("undefined variable");
    assert_eq!(
        error,
        PreprocessError::UndefinedVariable("Variable time_2 is not defined".into())
    );
}

/// `TestProfilePreprocessor.test_wrong_variable_type`.
#[test]
fn wrong_variable_type() {
    let mut profile = profile();
    profile["variables"][0]["type"] = json!("time");
    let error = process_variables(&profile).expect_err("type mismatch");
    assert_eq!(error.class_name(), "VariableTypeException");
    assert_eq!(
        error.to_string(),
        "Variable pressure_1 of type time used as pressure"
    );
}

/// `TestProfilePreprocessor.test_missing_stage_key`.
#[test]
fn missing_stage_key() {
    let mut profile = profile();
    profile["stages"][0]
        .as_object_mut()
        .expect("object")
        .remove("type");
    let error = process_variables(&profile).expect_err("format");
    assert_eq!(
        error,
        PreprocessError::Format("stage 0 missing 'type' field".into())
    );
}

/// `TestProfilePreprocessor.test_invalid_stage_structure`.
#[test]
fn invalid_stage_structure() {
    let mut profile = profile();
    profile["stages"][0]["dynamics"]["points"] = json!(1234);
    let error = process_variables(&profile).expect_err("format");
    assert_eq!(
        error,
        PreprocessError::Format("Invalid format detected: 'int' object is not iterable".into())
    );
}
