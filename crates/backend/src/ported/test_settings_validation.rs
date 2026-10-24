//! Port of `tests/test_settings_validation.py`: the checks of
//! `settings_validation.py`, which the Python tests call directly and which
//! live in `api/settings.rs` here.

use serde_json::{Value, json};

use crate::api::settings::{
    normalize_report_contact_mail, profile_settings_type_ok, validate_partial_retraction,
    validate_tare_behavior,
};
use crate::config::{TARE_BEHAVIOR_AFTER_RETRACTION, TARE_BEHAVIOR_BEFORE_RETRACTION};

/// tests/test_settings_validation.py::test_tare_behavior_accepts_supported_values
#[test]
fn tare_behavior_accepts_supported_values() {
    for behavior in [
        TARE_BEHAVIOR_AFTER_RETRACTION,
        TARE_BEHAVIOR_BEFORE_RETRACTION,
    ] {
        assert_eq!(
            validate_tare_behavior(&json!(behavior)),
            Ok(()),
            "{behavior}"
        );
    }
}

/// tests/test_settings_validation.py::test_tare_behavior_rejects_unknown_value
#[test]
fn tare_behavior_rejects_unknown_value() {
    let error = validate_tare_behavior(&json!("during_retraction")).unwrap_err();
    assert!(error.contains("unsupported tare behavior"), "{error}");
}

/// tests/test_settings_validation.py::test_partial_retraction_accepts_supported_range
#[test]
fn partial_retraction_accepts_supported_range() {
    for distance in [36.26, 45.33, 67.99] {
        assert_eq!(validate_partial_retraction(distance), Ok(()), "{distance}");
    }
}

/// tests/test_settings_validation.py::test_partial_retraction_rejects_values_outside_supported_range
#[test]
fn partial_retraction_rejects_values_outside_supported_range() {
    for distance in [36.25, 68.0, f64::NAN, f64::INFINITY] {
        let error = validate_partial_retraction(distance).unwrap_err();
        assert!(
            error.contains("partial_retraction must be between"),
            "{distance}: {error}"
        );
    }
}

/// tests/test_settings_validation.py::test_shot_data_sharing_accepts_booleans_from_any_state
#[test]
fn shot_data_sharing_accepts_booleans_from_any_state() {
    for current in [Value::Null, json!(true), json!(false)] {
        for value in [json!(true), json!(false)] {
            assert!(
                profile_settings_type_ok("shot_data_sharing", &value, &current),
                "{value} over {current}"
            );
        }
    }
}

/// tests/test_settings_validation.py::test_shot_data_sharing_rejects_non_booleans
#[test]
fn shot_data_sharing_rejects_non_booleans() {
    for value in [Value::Null, json!("true"), json!(1), json!(0)] {
        assert!(
            !profile_settings_type_ok("shot_data_sharing", &value, &Value::Null),
            "{value}"
        );
    }
}

/// tests/test_settings_validation.py::test_report_contact_mail_accepts_null_or_string_from_any_state
#[test]
fn report_contact_mail_accepts_null_or_string_from_any_state() {
    for current in [Value::Null, json!("a@b.c")] {
        for value in [Value::Null, json!("x@y.z"), json!("")] {
            assert!(
                profile_settings_type_ok("report_contact_mail", &value, &current),
                "{value} over {current}"
            );
        }
    }
}

/// tests/test_settings_validation.py::test_report_contact_mail_rejects_other_types
#[test]
fn report_contact_mail_rejects_other_types() {
    for current in [Value::Null, json!("a@b.c")] {
        for value in [json!(1), json!(true), json!(["a"])] {
            assert!(
                !profile_settings_type_ok("report_contact_mail", &value, &current),
                "{value} over {current}"
            );
        }
    }
}

/// tests/test_settings_validation.py::test_report_contact_mail_normalizes_missing_values_to_none
#[test]
fn report_contact_mail_normalizes_missing_values_to_none() {
    for value in [Value::Null, json!(""), json!("   "), json!("\t\n")] {
        assert_eq!(
            normalize_report_contact_mail(&value),
            Ok(Value::Null),
            "{value}"
        );
    }
}

/// tests/test_settings_validation.py::test_report_contact_mail_accepts_longest_allowed_address
#[test]
fn report_contact_mail_accepts_longest_allowed_address() {
    let address = format!("{}@b.cd", "a".repeat(249));
    assert_eq!(address.len(), 254);
    assert_eq!(
        normalize_report_contact_mail(&json!(address)),
        Ok(json!(address))
    );
}

/// tests/test_settings_validation.py::test_report_contact_mail_rejects_malformed_addresses
#[test]
fn report_contact_mail_rejects_malformed_addresses() {
    let too_long = format!("{}@b.cd", "a".repeat(250));
    for value in [
        "userexample.com",
        "@example.com",
        "user@",
        "@",
        "user@@example.com",
        "user@exa@mple.com",
        "us er@example.com",
        "user@exam ple.com",
        too_long.as_str(),
    ] {
        assert!(
            normalize_report_contact_mail(&json!(value)).is_err(),
            "{value}"
        );
    }
}

/// tests/test_settings_validation.py::test_report_contact_mail_rejects_non_strings
#[test]
fn report_contact_mail_rejects_non_strings() {
    for value in [json!(123), json!(true), json!(["user@example.com"])] {
        let error = normalize_report_contact_mail(&value).unwrap_err();
        assert!(error.contains("string or null"), "{value}: {error}");
    }
}
