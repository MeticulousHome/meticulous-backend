//! The pure parts of `tests/test_pour_over_profiles.py`: validation
//! outcomes, the stored serialisation, the image decoding and the error
//! answers. Storage, listing and the HTTP routes are the backend's.

mod common;

use profile::pour_over::{
    MAX_POUR_OVER_PROFILE_BYTES, POUR_OVER_PROFILE_SCHEMA, PourOverProfileError,
    is_oversized_pour_over_body, pour_over_profile_image, pour_over_summary,
    serialize_pour_over_profile, validate_pour_over_profile,
};
use serde_json::{Value, json};

fn valid_pour_over_profile() -> Value {
    json!({
        "version": 1,
        "brew_type": "pour_over",
        "id": "11111111-2222-4333-8444-555555555555",
        "name": "Lance 2 pour",
        "author": "Recipe author",
        "author_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "display": {
            "accentColor": "#23383F",
            "shortDescription": "A bright three-stage recipe",
            "description": "Two smaller pours followed by the main pour.",
        },
        "recipe": {
            "coffee_dose_g": 15,
            "total_water_g": 225,
            "water_temperature_c": 92,
            "target_total_time_s": 135,
            "target_total_time_max_s": 180,
        },
        "stages": [
            {
                "key": "pour-1",
                "name": "Pour 1",
                "starts_at_s": 0,
                "pour": {
                    "water_g": 35,
                    "duration_s": 10,
                    "target_cumulative_water_g": 35,
                    "flow_rate_g_s": 3.5,
                    "pattern": "spiral_out",
                },
            },
            {
                "key": "pour-2",
                "name": "Pour 2",
                "starts_at_s": 30,
                "pour": {
                    "water_g": 35,
                    "duration_s": 10,
                    "target_cumulative_water_g": 70,
                    "flow_rate_g_s": 3.5,
                    "flow_range_g_s": [3, 4],
                },
            },
            {
                "key": "pour-3",
                "name": "Pour 3",
                "starts_at_s": 60,
                "pour": {
                    "water_g": 155,
                    "duration_s": 31,
                    "target_cumulative_water_g": 225,
                    "flow_rate_g_s": 5,
                },
            },
        ],
    })
}

type Mutation = Box<dyn Fn(&mut Value)>;

fn issues(profile: &Value) -> Vec<Value> {
    match validate_pour_over_profile(profile) {
        Err(PourOverProfileError::Validation(issues)) => {
            issues.iter().map(|issue| issue.to_json()).collect()
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

/// `test_profile_validation_and_atomic_persistence`: the profile validates
/// and what would be stored is the `indent=2` JSON of it (writing it
/// atomically is the backend's).
#[test]
fn profile_validation_and_stored_serialisation() {
    let profile = valid_pour_over_profile();
    let validated = validate_pour_over_profile(&profile).expect("valid");
    assert_eq!(validated["recipe"]["coffee_dose_g"], json!(15));

    let stored = serialize_pour_over_profile(&validated).expect("small enough");
    let reloaded: Value = serde_json::from_str(&stored).expect("json");
    assert_eq!(reloaded["stages"][2]["starts_at_s"], json!(60));
    assert!(stored.starts_with("{\n  \"version\": 1,\n  \"brew_type\": \"pour_over\","));
}

/// `test_documented_example_matches_the_canonical_contract`.
#[test]
fn documented_example_matches_the_canonical_contract() {
    let example = common::repo_json("pour_over_profile_schema/example_profile.json");
    let validated = validate_pour_over_profile(&example).expect("valid");
    assert_eq!(validated["brew_type"], json!("pour_over"));
    assert_eq!(validated["stages"].as_array().map(Vec::len), Some(2));
}

/// `test_rejects_schema_and_semantic_inconsistency` (every parametrisation).
#[test]
fn rejects_schema_and_semantic_inconsistency() {
    let cases: Vec<(Mutation, Value)> = vec![
        (
            Box::new(|p| p["recipe"]["coffee_dose_g"] = json!(4.9)),
            json!(["recipe", "coffee_dose_g"]),
        ),
        (
            Box::new(|p| p["recipe"]["water_temperature_c"] = json!(69)),
            json!(["recipe", "water_temperature_c"]),
        ),
        (
            Box::new(|p| p["stages"][0]["starts_at_s"] = json!(1)),
            json!(["stages", 0, "starts_at_s"]),
        ),
        (
            Box::new(|p| p["stages"][1]["pour"]["target_cumulative_water_g"] = json!(71)),
            json!(["stages", 1, "pour", "target_cumulative_water_g"]),
        ),
        (
            Box::new(|p| p["stages"][2]["pour"]["flow_rate_g_s"] = json!(8)),
            json!(["stages", 2, "pour", "flow_rate_g_s"]),
        ),
        (Box::new(|p| p["unknown"] = json!(true)), json!([])),
    ];
    for (mutate, expected_path) in cases {
        let mut profile = valid_pour_over_profile();
        mutate(&mut profile);
        let paths: Vec<Value> = issues(&profile).iter().map(|i| i["path"].clone()).collect();
        assert!(
            paths.contains(&expected_path),
            "{expected_path} not in {paths:?}"
        );
    }
}

/// The messages of those rejections, as Python words them.
#[test]
fn rejection_messages_match_python() {
    let mut profile = valid_pour_over_profile();
    profile["recipe"]["coffee_dose_g"] = json!(41);
    assert_eq!(
        issues(&profile),
        vec![json!({
            "path": ["recipe", "coffee_dose_g"],
            "message": "41 is greater than the maximum of 40",
            "code": "schema",
        })]
    );

    let mut profile = valid_pour_over_profile();
    profile["unknown"] = json!(true);
    assert_eq!(
        issues(&profile),
        vec![json!({
            "path": [],
            "message": "Additional properties are not allowed ('unknown' was unexpected)",
            "code": "schema",
        })]
    );

    let mut profile = valid_pour_over_profile();
    profile["stages"][2]["pour"]["flow_rate_g_s"] = json!(8);
    assert_eq!(
        issues(&profile),
        vec![json!({
            "path": ["stages", 2, "pour", "flow_rate_g_s"],
            "message": "Flow must equal water divided by duration (5.00 g/s)",
            "code": "flow_consistency",
        })]
    );
}

/// `test_rejects_overlap_and_total_time_before_final_pour`.
#[test]
fn rejects_overlap_and_total_time_before_final_pour() {
    let mut profile = valid_pour_over_profile();
    profile["stages"][1]["starts_at_s"] = json!(5);
    profile["recipe"]["target_total_time_s"] = json!(80);
    let codes: Vec<Value> = issues(&profile).iter().map(|i| i["code"].clone()).collect();
    assert!(codes.contains(&json!("overlap")));
    assert!(codes.contains(&json!("time_consistency")));
}

/// `TestPourOverProfilesAPI.test_existing_profile_save_endpoint_dispatches_pour_over`:
/// the listed summary has no image, and the image route serves the decoded
/// JPEG.
#[test]
fn summary_drops_the_image_and_the_image_decodes() {
    let mut profile = valid_pour_over_profile();
    profile["display"]["image"] = json!("data:image/jpeg;base64,/9j/2Q==");
    validate_pour_over_profile(&profile).expect("valid");

    assert!(
        pour_over_summary(&profile)["display"]
            .get("image")
            .is_none()
    );
    assert_eq!(
        pour_over_profile_image(&profile),
        Ok(Some(vec![0xff, 0xd8, 0xff, 0xd9]))
    );
}

/// `test_profile_image_endpoint_returns_404_without_an_image`.
#[test]
fn profile_without_an_image_has_none() {
    assert_eq!(
        pour_over_profile_image(&valid_pour_over_profile()),
        Ok(None)
    );
}

/// `test_profile_api_returns_503_when_optional_storage_is_unavailable`,
/// `test_invalid_profile_returns_field_specific_422`,
/// `test_oversized_pour_over_body_is_rejected_before_json_parsing`: the
/// answers `write_pour_over_profile_error` sends.
#[test]
fn errors_map_to_the_handler_answers() {
    assert_eq!(
        PourOverProfileError::Unavailable("Pour Over profiles are unavailable".into())
            .http_response(),
        (
            503,
            json!({"status": "error", "error": "Pour Over profiles are unavailable"})
        )
    );

    let mut profile = valid_pour_over_profile();
    profile["recipe"]["coffee_dose_g"] = json!(41);
    let error = validate_pour_over_profile(&profile).expect_err("invalid");
    let (status, body) = error.http_response();
    assert_eq!(status, 422);
    assert_eq!(body["error"], json!("Invalid Pour Over profile"));
    let paths: Vec<&Value> = body["details"]
        .as_array()
        .expect("details")
        .iter()
        .map(|d| &d["path"])
        .collect();
    assert!(paths.contains(&&json!(["recipe", "coffee_dose_g"])));

    let mut body = br#"{"brew_type":"pour_over","padding":""#.to_vec();
    body.extend(std::iter::repeat_n(b'x', MAX_POUR_OVER_PROFILE_BYTES));
    body.extend(br#""}"#);
    assert!(is_oversized_pour_over_body(&body));
    assert_eq!(
        PourOverProfileError::TooLarge.http_response(),
        (
            413,
            json!({"status": "error", "error": "Pour Over profile is too large"})
        )
    );

    assert_eq!(PourOverProfileError::Limit.http_response().0, 409);
    assert_eq!(
        PourOverProfileError::Storage("Could not save profile".into()).http_response(),
        (
            507,
            json!({"status": "error", "error": "Could not access Pour Over profile storage"})
        )
    );
    assert_eq!(
        PourOverProfileError::Other(String::new()).http_response().0,
        500
    );
}

/// `test_schema_endpoint_reports_version_one`.
#[test]
fn schema_reports_version_one() {
    let schema: Value = serde_json::from_str(POUR_OVER_PROFILE_SCHEMA).expect("schema");
    assert_eq!(schema["properties"]["version"]["const"], json!(1));
}

/// Not an object at all.
#[test]
fn a_non_object_is_rejected_with_the_invalid_code() {
    assert_eq!(
        issues(&json!([])),
        vec![json!({"path": [], "message": "Profile must be an object", "code": "invalid"})]
    );
}
