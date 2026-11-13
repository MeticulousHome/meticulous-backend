//! The pure parts of `tests/test_manual_mode.py`: the Manual mode profile,
//! its boot-time seeding, profiles built from a recorded manual brew, the
//! node program, and what loading a manual profile sends.

mod common;

use std::collections::HashSet;

use common::TestEnv;
use profile::manual_mode::{
    ManualShotError, build_profile_from_manual_shot, manual_shot_search_ids, pick_manual_shot,
};
use profile::manual_program::{
    END_NODE_HEAD, INIT_NODE_TAIL, MANUAL_CONTROLLER_KINDS, build_manual_program, is_manual_stage,
    validate_program,
};
use profile::profiles::{
    MANUAL_MODE_AUTHOR, MANUAL_MODE_AUTHOR_ID, MANUAL_MODE_FINAL_WEIGHT_SENTINEL,
    MANUAL_MODE_FLOW_STAGE_KEY, MANUAL_MODE_FLOW_STAGE_NAME,
    MANUAL_MODE_LEGACY_FINAL_WEIGHT_SENTINEL, MANUAL_MODE_PRESSURE_STAGE_KEY,
    MANUAL_MODE_PRESSURE_STAGE_NAME, MANUAL_MODE_PROFILE_ID, MANUAL_MODE_PROFILE_NAME,
    MANUAL_MODE_TEMPERATURE, espresso_validator,
};
use profile::{
    ManualModeSeed, ProfileError, ProgramError, ProgramSettings, ReferenceIds,
    build_manual_mode_profile, manual_mode_stages, manual_profile_has_seeded_stages,
    plan_manual_mode_profile, prepare_for_esp32, profile_for_esp32,
};
use serde_json::{Value, json};

fn assert_schema_valid(profile: &Value) {
    if let Err(error) = espresso_validator().validate(profile) {
        panic!("not schema-valid: {}", error.message);
    }
}

/// `test_build_manual_mode_profile_matches_the_contract`.
#[test]
fn build_manual_mode_profile_matches_the_contract() {
    let profile = build_manual_mode_profile();

    assert_schema_valid(&profile);

    assert_eq!(profile["id"], json!(MANUAL_MODE_PROFILE_ID));
    assert_eq!(profile["name"], json!(MANUAL_MODE_PROFILE_NAME));
    assert_eq!(profile["author"], json!(MANUAL_MODE_AUTHOR));
    assert_eq!(profile["author_id"], json!(MANUAL_MODE_AUTHOR_ID));
    assert_eq!(profile["previous_authors"], json!([]));
    assert_eq!(profile["temperature"], json!(MANUAL_MODE_TEMPERATURE));
    assert_eq!(
        profile["final_weight"],
        json!(MANUAL_MODE_FINAL_WEIGHT_SENTINEL)
    );
    assert_eq!(profile["variables"], json!([]));
    assert_eq!(profile["display"], json!({}));
    assert_eq!(profile["manual"], json!(true));
    assert_eq!(profile["final_weight"], json!(2000.0));

    let stages = profile["stages"].as_array().expect("stages");
    assert_eq!(stages.len(), 2);
    assert_eq!(
        (&stages[0]["name"], &stages[0]["key"], &stages[0]["type"]),
        (
            &json!(MANUAL_MODE_PRESSURE_STAGE_NAME),
            &json!(MANUAL_MODE_PRESSURE_STAGE_KEY),
            &json!("pressure")
        )
    );
    assert_eq!(
        (&stages[1]["name"], &stages[1]["key"], &stages[1]["type"]),
        (
            &json!(MANUAL_MODE_FLOW_STAGE_NAME),
            &json!(MANUAL_MODE_FLOW_STAGE_KEY),
            &json!("flow")
        )
    );
    for stage in stages {
        assert_eq!(
            stage["dynamics"],
            json!({"points": [[0, 0]], "over": "time", "interpolation": "none"})
        );
        assert_eq!(
            stage["exit_triggers"],
            json!([{"type": "user_interaction", "value": 1}])
        );
        assert_eq!(stage["limits"], json!([]));
    }
}

/// `test_build_manual_mode_profile_returns_a_fresh_copy`.
#[test]
fn build_manual_mode_profile_returns_a_fresh_copy() {
    let mut first = build_manual_mode_profile();
    first["stages"][0]["dynamics"]["points"][0][1] = json!(9.0);
    first["name"] = json!("renamed");

    let second = build_manual_mode_profile();

    assert_eq!(second["name"], json!(MANUAL_MODE_PROFILE_NAME));
    assert_eq!(second["stages"][0]["dynamics"]["points"], json!([[0, 0]]));
}

fn seeded_stages(order: [&str; 2]) -> Value {
    let stages = manual_mode_stages();
    let by_type = |kind: &str| {
        stages
            .as_array()
            .expect("stages")
            .iter()
            .find(|s| s["type"] == kind)
            .cloned()
            .expect("stage")
    };
    json!([by_type(order[0]), by_type(order[1])])
}

/// `test_ensure_manual_mode_profile_seeds_when_absent`.
#[test]
fn ensure_manual_mode_profile_seeds_when_absent() {
    let ManualModeSeed::Seed(profile) = plan_manual_mode_profile(None) else {
        panic!("expected a seed");
    };
    assert_eq!(profile["id"], json!(MANUAL_MODE_PROFILE_ID));
    assert_eq!(profile["manual"], json!(true));
}

/// `test_ensure_manual_mode_profile_is_a_no_op_when_already_seeded`.
#[test]
fn ensure_manual_mode_profile_is_a_no_op_when_already_seeded() {
    let existing =
        json!({"id": MANUAL_MODE_PROFILE_ID, "stages": seeded_stages(["pressure", "flow"])});
    assert_eq!(
        plan_manual_mode_profile(Some(&existing)),
        ManualModeSeed::UpToDate
    );
}

/// `test_ensure_manual_mode_profile_keeps_a_reordered_stage_pair`.
#[test]
fn ensure_manual_mode_profile_keeps_a_reordered_stage_pair() {
    let existing =
        json!({"id": MANUAL_MODE_PROFILE_ID, "stages": seeded_stages(["flow", "pressure"])});
    assert_eq!(
        plan_manual_mode_profile(Some(&existing)),
        ManualModeSeed::UpToDate
    );
}

/// `test_ensure_manual_mode_profile_keeps_edited_stage_values`.
#[test]
fn ensure_manual_mode_profile_keeps_edited_stage_values() {
    let mut stages = seeded_stages(["pressure", "flow"]);
    stages[0]["dynamics"]["points"] = json!([[0, 6.5]]);
    let existing = json!({"id": MANUAL_MODE_PROFILE_ID, "stages": stages});
    assert_eq!(
        plan_manual_mode_profile(Some(&existing)),
        ManualModeSeed::UpToDate
    );
}

/// `test_ensure_manual_mode_profile_migrates_the_v1_single_stage`.
#[test]
fn ensure_manual_mode_profile_migrates_the_v1_single_stage() {
    let existing = json!({
        "id": MANUAL_MODE_PROFILE_ID,
        "name": "My manual",
        "author": "Someone",
        "author_id": "11111111-1111-1111-1111-111111111111",
        "previous_authors": [],
        "temperature": 93.5,
        "final_weight": MANUAL_MODE_LEGACY_FINAL_WEIGHT_SENTINEL,
        "variables": [],
        "display": {"accentColor": "#abcdef"},
        "manual": true,
        "stages": [
            {
                "name": "Manual",
                "key": MANUAL_MODE_PRESSURE_STAGE_KEY,
                "type": "pressure",
                "dynamics": {"points": [[0, 0]], "over": "time", "interpolation": "none"},
                "exit_triggers": [{"type": "user_interaction", "value": 1}],
                "limits": [],
            }
        ],
    });

    let ManualModeSeed::Migrate(written) = plan_manual_mode_profile(Some(&existing)) else {
        panic!("expected a migration");
    };

    assert_schema_valid(&written);
    assert_eq!(written["name"], json!("My manual"));
    assert_eq!(written["temperature"], json!(93.5));
    assert_eq!(written["author"], json!("Someone"));
    assert_eq!(
        written["author_id"],
        json!("11111111-1111-1111-1111-111111111111")
    );
    assert_eq!(written["display"], json!({"accentColor": "#abcdef"}));
    assert_eq!(
        written["final_weight"],
        json!(MANUAL_MODE_FINAL_WEIGHT_SENTINEL)
    );
    assert_eq!(written["final_weight"], json!(2000.0));
    assert_eq!(written["manual"], json!(true));
    let keys: Vec<&Value> = written["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .map(|s| &s["key"])
        .collect();
    assert_eq!(
        keys,
        [
            &json!(MANUAL_MODE_PRESSURE_STAGE_KEY),
            &json!(MANUAL_MODE_FLOW_STAGE_KEY)
        ]
    );
}

/// `test_migration_keeps_a_final_weight_the_user_chose`.
#[test]
fn migration_keeps_a_final_weight_the_user_chose() {
    let existing = json!({"id": MANUAL_MODE_PROFILE_ID, "final_weight": 36.0, "stages": []});
    let ManualModeSeed::Migrate(written) = plan_manual_mode_profile(Some(&existing)) else {
        panic!("expected a migration");
    };
    assert_eq!(written["final_weight"], json!(36.0));
}

/// `test_migration_does_not_mutate_the_known_profile`.
#[test]
fn migration_does_not_mutate_the_known_profile() {
    let stored = json!({"id": MANUAL_MODE_PROFILE_ID, "stages": []});
    assert!(matches!(
        plan_manual_mode_profile(Some(&stored)),
        ManualModeSeed::Migrate(_)
    ));
    assert_eq!(stored["stages"], json!([]));
}

/// `test_ensure_manual_mode_profile_migrates_a_broken_stage_list` (every
/// parametrisation).
#[test]
fn ensure_manual_mode_profile_migrates_a_broken_stage_list() {
    let broken = [
        json!([]),
        json!(null),
        json!("not a list"),
        json!([{"key": MANUAL_MODE_PRESSURE_STAGE_KEY, "type": "pressure"}]),
        json!([
            {"key": MANUAL_MODE_PRESSURE_STAGE_KEY, "type": "flow"},
            {"key": MANUAL_MODE_FLOW_STAGE_KEY, "type": "pressure"},
        ]),
        json!([
            {"key": MANUAL_MODE_PRESSURE_STAGE_KEY, "type": "pressure"},
            {"key": MANUAL_MODE_PRESSURE_STAGE_KEY, "type": "pressure"},
        ]),
        json!([
            {"key": MANUAL_MODE_PRESSURE_STAGE_KEY, "type": "pressure"},
            {"key": MANUAL_MODE_FLOW_STAGE_KEY, "type": "flow"},
            {"key": "extra", "type": "flow"},
        ]),
    ];
    for stages in broken {
        let existing = json!({"id": MANUAL_MODE_PROFILE_ID, "stages": stages});
        let ManualModeSeed::Migrate(written) = plan_manual_mode_profile(Some(&existing)) else {
            panic!("expected a migration for {stages}");
        };
        assert_eq!(written["stages"], manual_mode_stages());
    }
}

/// `test_manual_profile_has_seeded_stages_accepts_either_order`.
#[test]
fn manual_profile_has_seeded_stages_accepts_either_order() {
    assert!(manual_profile_has_seeded_stages(
        &json!({"stages": seeded_stages(["pressure", "flow"])})
    ));
    assert!(manual_profile_has_seeded_stages(
        &json!({"stages": seeded_stages(["flow", "pressure"])})
    ));
}

// --- profile construction from a recorded manual brew -----------------------

const SHOT_TIME: f64 = 1757193600.0;
/// `datetime.fromtimestamp(SHOT_TIME).strftime("Manual %Y-%m-%d %H:%M")` in
/// UTC, the zone the tests run the local clock in.
const SHOT_TIME_NAME: &str = "Manual 2025-09-06 21:20";

fn sample(profile_time: Value, setpoints: Value, weight: Value, use_profile_time: bool) -> Value {
    let mut sample = json!({
        "shot": {
            "pressure": 6.0,
            "flow": 1.2,
            "weight": weight,
            "gravimetric_flow": 1.1,
            "setpoints": setpoints,
        },
        "time": profile_time,
        "status": "Manual",
    });
    if use_profile_time {
        sample["profile_time"] = profile_time;
    }
    sample
}

fn pressure_sample(time: i64, pressure: f64, weight: Value) -> Value {
    sample(
        json!(time),
        json!({"active": "pressure", "pressure": pressure}),
        weight,
        true,
    )
}

fn flow_sample(time: i64, flow: f64, weight: Value) -> Value {
    sample(
        json!(time),
        json!({"active": "flow", "flow": flow}),
        weight,
        true,
    )
}

fn manual_shot(data: Value) -> Value {
    json!({
        "id": "8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6",
        "db_key": 12,
        "time": SHOT_TIME,
        "file": "2026-09-07/10:00:00.shot.json.zst",
        "debug_file": null,
        "name": MANUAL_MODE_PROFILE_NAME,
        "data": data,
        "profile": {
            "id": MANUAL_MODE_PROFILE_ID,
            "db_key": 7,
            "author": "Barista",
            "author_id": "11111111-2222-4333-8444-555555555555",
            "display": {},
            "final_weight": 1000.0,
            "last_changed": SHOT_TIME,
            "name": MANUAL_MODE_PROFILE_NAME,
            "temperature": 93.5,
            "stages": [],
            "variables": [],
            "previous_authors": [],
        },
    })
}

fn mixed_shot() -> Value {
    manual_shot(json!([
        sample(
            json!(0),
            json!({"active": "temperature", "temperature": 93.5}),
            json!(0.0),
            true
        ),
        pressure_sample(1000, 6.0, json!(0.0)),
        pressure_sample(2000, 8.456, json!(12.0)),
        pressure_sample(2000, 9.0, json!(13.0)),
        sample(json!(3000), json!({"active": null}), json!(0.0), true),
        sample(json!(3100), json!({"active": "pressure"}), json!(0.0), true),
        pressure_sample(3500, 4.0, json!(36.24)),
    ]))
}

fn build(shot: &Value, name: Option<&str>) -> Value {
    build_profile_from_manual_shot(shot, name, &mut TestEnv::default()).expect("builds")
}

fn is_uuid_string(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() == 36
            && s.chars().enumerate().all(|(i, c)| match i {
                8 | 13 | 18 | 23 => c == '-',
                _ => c.is_ascii_hexdigit(),
            })
    })
}

/// `test_build_profile_from_manual_shot_replays_the_pressure_targets`.
#[test]
fn build_profile_from_manual_shot_replays_the_pressure_targets() {
    let profile = build(&mixed_shot(), None);

    assert_schema_valid(&profile);

    let stages = profile["stages"].as_array().expect("stages");
    assert_eq!(stages.len(), 1);
    let stage = &stages[0];
    assert_eq!(stage["name"], json!("Pressure 1"));
    assert_eq!(stage["type"], json!("pressure"));
    assert_eq!(stage["limits"], json!([]));
    assert_eq!(stage["dynamics"]["over"], json!("time"));
    assert_eq!(stage["dynamics"]["interpolation"], json!("none"));
    assert_eq!(
        stage["dynamics"]["points"],
        json!([[0.0, 6.0], [1.0, 8.46], [2.5, 4.0]])
    );
    assert_eq!(
        stage["exit_triggers"],
        json!([{"type": "time", "value": 2.5, "relative": true, "comparison": ">="}])
    );
    assert_eq!(profile["final_weight"], json!(36.2));
    assert_eq!(profile["temperature"], json!(93.5));
    assert_eq!(profile["author"], json!("Barista"));
    assert_eq!(
        profile["author_id"],
        json!("11111111-2222-4333-8444-555555555555")
    );
    assert_eq!(profile["previous_authors"], json!([]));
    assert_eq!(profile["variables"], json!([]));
    assert_eq!(profile["display"], json!({}));
    assert!(profile.get("manual").is_none());
    assert!(is_uuid_string(&profile["id"]));
    assert!(is_uuid_string(&stage["key"]));
}

/// `test_build_profile_from_manual_shot_falls_back_to_the_sample_time`.
#[test]
fn build_profile_from_manual_shot_falls_back_to_the_sample_time() {
    let shot = manual_shot(json!([
        sample(
            json!(4000),
            json!({"active": "pressure", "pressure": 5.0}),
            json!(0.0),
            false
        ),
        sample(
            json!(6250),
            json!({"active": "pressure", "pressure": 7.0}),
            json!(0.0),
            false
        ),
    ]));
    assert_eq!(
        build(&shot, None)["stages"][0]["dynamics"]["points"],
        json!([[0.0, 5.0], [2.25, 7.0]])
    );
}

/// `test_build_profile_from_manual_shot_uses_the_last_target_bearing_weight`.
#[test]
fn build_profile_from_manual_shot_uses_the_last_target_bearing_weight() {
    let shot = manual_shot(json!([
        pressure_sample(1000, 6.0, json!(5.0)),
        pressure_sample(2000, 7.0, json!(30.0)),
        pressure_sample(2000, 7.0, json!(31.5)),
    ]));
    let profile = build(&shot, None);
    assert_eq!(
        profile["stages"][0]["dynamics"]["points"],
        json!([[0.0, 6.0], [1.0, 7.0]])
    );
    assert_eq!(profile["final_weight"], json!(31.5));
}

/// `test_build_profile_from_manual_shot_uses_the_weight_sentinel`. Python's
/// `float("nan")` reaches Rust as `null` (`pyfmt::loads_lenient`).
#[test]
fn build_profile_from_manual_shot_uses_the_weight_sentinel() {
    for weight in [json!(null), json!(0.4), json!(null), json!("36.2")] {
        let shot = manual_shot(json!([pressure_sample(1000, 6.0, weight)]));
        assert_eq!(
            build(&shot, None)["final_weight"],
            json!(MANUAL_MODE_FINAL_WEIGHT_SENTINEL)
        );
    }
}

/// `test_build_profile_from_manual_shot_falls_back_to_the_default_temperature`
/// (`float("inf")` arrives as `null`).
#[test]
fn build_profile_from_manual_shot_falls_back_to_the_default_temperature() {
    for temperature in [json!(null), json!(null), json!("93.5")] {
        let mut shot = manual_shot(json!([pressure_sample(1000, 6.0, json!(0.0))]));
        shot["profile"]["temperature"] = temperature;
        assert_eq!(
            build(&shot, None)["temperature"],
            json!(MANUAL_MODE_TEMPERATURE)
        );
    }
}

/// `test_build_profile_from_manual_shot_defaults_the_name_to_the_shot_time`.
#[test]
fn build_profile_from_manual_shot_defaults_the_name_to_the_shot_time() {
    assert_eq!(build(&mixed_shot(), None)["name"], json!(SHOT_TIME_NAME));
}

/// `test_build_profile_from_manual_shot_ignores_a_blank_name`.
#[test]
fn build_profile_from_manual_shot_ignores_a_blank_name() {
    for name in [Some(""), Some("   "), None] {
        assert_eq!(build(&mixed_shot(), name)["name"], json!(SHOT_TIME_NAME));
    }
}

/// `test_build_profile_from_manual_shot_trims_and_caps_the_name`.
#[test]
fn build_profile_from_manual_shot_trims_and_caps_the_name() {
    assert_eq!(
        build(&mixed_shot(), Some("   Morning manual   "))["name"],
        json!("Morning manual")
    );
    let long = "m".repeat(100);
    assert_eq!(
        build(&mixed_shot(), Some(&long))["name"],
        json!("m".repeat(64))
    );
}

/// `test_build_profile_from_manual_shot_uses_the_minimum_stage_duration`.
#[test]
fn build_profile_from_manual_shot_uses_the_minimum_stage_duration() {
    let profile = build(
        &manual_shot(json!([pressure_sample(1000, 6.0, json!(0.0))])),
        None,
    );
    assert_eq!(
        profile["stages"][0]["dynamics"]["points"],
        json!([[0.0, 6.0]])
    );
    assert_eq!(
        profile["stages"][0]["exit_triggers"][0]["value"],
        json!(0.1)
    );
}

/// `test_build_profile_from_manual_shot_falls_back_to_the_zero_author`.
#[test]
fn build_profile_from_manual_shot_falls_back_to_the_zero_author() {
    let mut shot = manual_shot(json!([pressure_sample(1000, 6.0, json!(0.0))]));
    shot["profile"]["author"] = json!(null);
    shot["profile"]["author_id"] = json!(null);
    let profile = build(&shot, None);
    assert_eq!(profile["author"], json!(""));
    assert_eq!(profile["author_id"], json!(MANUAL_MODE_AUTHOR_ID));
}

/// `test_build_profile_from_manual_shot_without_targets` (every
/// parametrisation; NaN and infinity targets arrive as `null`).
#[test]
fn build_profile_from_manual_shot_without_targets() {
    let cases = [
        json!([]),
        json!(null),
        json!([sample(
            json!(0),
            json!({"active": "temperature", "temperature": 93.5}),
            json!(0.0),
            true
        )]),
        json!([sample(json!(0), json!({"active": null}), json!(0.0), true)]),
        json!([sample(
            json!(0),
            json!({"active": "pressure"}),
            json!(0.0),
            true
        )]),
        json!([sample(
            json!(0),
            json!({"active": "pressure", "pressure": null}),
            json!(0.0),
            true
        )]),
        json!(["not a sample", {"shot": "not a dict"}, {"shot": {"setpoints": null}}]),
    ];
    for data in cases {
        let result = build_profile_from_manual_shot(
            &manual_shot(data.clone()),
            None,
            &mut TestEnv::default(),
        );
        assert_eq!(result, Err(ManualShotError::NoTargets), "data {data}");
    }
}

fn switching_shot() -> Value {
    manual_shot(json!([
        pressure_sample(0, 6.0, json!(0.0)),
        pressure_sample(1000, 7.0, json!(2.0)),
        flow_sample(2000, 2.5, json!(8.0)),
        flow_sample(3000, 2.0, json!(14.0)),
        pressure_sample(4000, 4.0, json!(20.0)),
    ]))
}

fn stage_field<'a>(profile: &'a Value, field: &str) -> Vec<&'a Value> {
    profile["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .map(|s| &s[field])
        .collect()
}

/// `test_each_run_of_a_control_becomes_its_own_stage`.
#[test]
fn each_run_of_a_control_becomes_its_own_stage() {
    let profile = build(&switching_shot(), None);
    assert_schema_valid(&profile);
    assert_eq!(
        stage_field(&profile, "name"),
        [&json!("Pressure 1"), &json!("Flow 2"), &json!("Pressure 3")]
    );
    assert_eq!(
        stage_field(&profile, "type"),
        [&json!("pressure"), &json!("flow"), &json!("pressure")]
    );
    let keys: HashSet<String> = stage_field(&profile, "key")
        .iter()
        .map(|k| k.to_string())
        .collect();
    assert_eq!(keys.len(), 3);
    for stage in profile["stages"].as_array().expect("stages") {
        assert!(is_uuid_string(&stage["key"]));
        assert_eq!(stage["limits"], json!([]));
        assert_eq!(stage["dynamics"]["over"], json!("time"));
        assert_eq!(stage["dynamics"]["interpolation"], json!("none"));
    }
}

/// `test_each_stage_restarts_its_own_clock`.
#[test]
fn each_stage_restarts_its_own_clock() {
    let profile = build(&switching_shot(), None);
    assert_eq!(
        profile["stages"][0]["dynamics"]["points"],
        json!([[0.0, 6.0], [1.0, 7.0]])
    );
    assert_eq!(
        profile["stages"][1]["dynamics"]["points"],
        json!([[0.0, 2.5], [1.0, 2.0]])
    );
    assert_eq!(
        profile["stages"][2]["dynamics"]["points"],
        json!([[0.0, 4.0]])
    );
}

/// `test_each_stage_exits_on_its_own_duration`.
#[test]
fn each_stage_exits_on_its_own_duration() {
    let profile = build(&switching_shot(), None);
    assert_eq!(
        stage_field(&profile, "exit_triggers"),
        [
            &json!([{"type": "time", "value": 1.0, "relative": true, "comparison": ">="}]),
            &json!([{"type": "time", "value": 1.0, "relative": true, "comparison": ">="}]),
            &json!([{"type": "time", "value": 0.1, "relative": true, "comparison": ">="}]),
        ]
    );
}

/// `test_a_flow_only_brew_builds_a_flow_stage`.
#[test]
fn a_flow_only_brew_builds_a_flow_stage() {
    let profile = build(
        &manual_shot(json!([
            flow_sample(0, 2.0, json!(0.0)),
            flow_sample(1000, 2.4, json!(18.0))
        ])),
        None,
    );
    assert_schema_valid(&profile);
    assert_eq!(stage_field(&profile, "name"), [&json!("Flow 1")]);
    assert_eq!(profile["stages"][0]["type"], json!("flow"));
    assert_eq!(
        profile["stages"][0]["dynamics"]["points"],
        json!([[0.0, 2.0], [1.0, 2.4]])
    );
    assert_eq!(profile["final_weight"], json!(18.0));
}

/// `test_samples_without_a_target_do_not_split_a_run`.
#[test]
fn samples_without_a_target_do_not_split_a_run() {
    let profile = build(
        &manual_shot(json!([
            pressure_sample(0, 6.0, json!(0.0)),
            sample(json!(1000), json!({"active": null}), json!(0.0), true),
            sample(json!(1500), json!({"active": "pressure"}), json!(0.0), true),
            pressure_sample(2000, 7.0, json!(0.0)),
        ])),
        None,
    );
    assert_eq!(stage_field(&profile, "name"), [&json!("Pressure 1")]);
    assert_eq!(
        profile["stages"][0]["dynamics"]["points"],
        json!([[0.0, 6.0], [2.0, 7.0]])
    );
}

/// `test_the_final_weight_comes_from_the_last_target_of_the_last_run`.
#[test]
fn the_final_weight_comes_from_the_last_target_of_the_last_run() {
    assert_eq!(build(&switching_shot(), None)["final_weight"], json!(20.0));
}

/// `test_a_brew_that_never_reached_a_meaningful_weight_uses_the_sentinel`.
#[test]
fn a_brew_that_never_reached_a_meaningful_weight_uses_the_sentinel() {
    let profile = build(
        &manual_shot(json!([
            pressure_sample(0, 6.0, json!(0.0)),
            flow_sample(1000, 2.0, json!(0.2))
        ])),
        None,
    );
    assert_eq!(
        profile["final_weight"],
        json!(MANUAL_MODE_FINAL_WEIGHT_SENTINEL)
    );
    assert_eq!(profile["final_weight"], json!(2000.0));
}

/// `test_the_sentinel_is_the_schema_maximum`.
#[test]
fn the_sentinel_is_the_schema_maximum() {
    let bounds = &espresso_validator().schema()["properties"]["final_weight"];
    assert_eq!(
        bounds["maximum"].as_f64(),
        Some(MANUAL_MODE_FINAL_WEIGHT_SENTINEL)
    );
    const { assert!(MANUAL_MODE_LEGACY_FINAL_WEIGHT_SENTINEL < MANUAL_MODE_FINAL_WEIGHT_SENTINEL) };
}

/// `test_alternating_single_sample_runs_each_get_a_stage`.
#[test]
fn alternating_single_sample_runs_each_get_a_stage() {
    let profile = build(
        &manual_shot(json!([
            pressure_sample(0, 6.0, json!(0.0)),
            flow_sample(500, 2.0, json!(0.0)),
            pressure_sample(1000, 7.0, json!(0.0)),
            flow_sample(1500, 2.4, json!(0.0)),
        ])),
        None,
    );
    assert_eq!(
        stage_field(&profile, "name"),
        [
            &json!("Pressure 1"),
            &json!("Flow 2"),
            &json!("Pressure 3"),
            &json!("Flow 4")
        ]
    );
    for (stage, point) in profile["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .zip([6.0, 2.0, 7.0, 2.4])
    {
        assert_eq!(stage["dynamics"]["points"], json!([[0.0, point]]));
    }
}

// --- node program (contract section 8) --------------------------------------

const HEAD_STAGE_NAMES: [&str; 8] = [
    "prepare",
    "purge",
    "water detection",
    "heating",
    "heating",
    "click to start",
    "retracting",
    "closing valve",
];
const TAIL_STAGE_NAMES: [&str; 5] = [
    "retracting",
    "click to purge",
    "remove cup",
    "purge",
    "END_STAGE",
];

fn program_of(document: &Value) -> Value {
    build_manual_program(
        document,
        &ProgramSettings::default(),
        &mut ReferenceIds::default(),
    )
    .expect("builds")
}

fn program_with(document: &Value, settings: ProgramSettings) -> Value {
    build_manual_program(document, &settings, &mut ReferenceIds::default()).expect("builds")
}

fn stage_names(program: &Value) -> Vec<String> {
    program["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .map(|s| s["name"].as_str().expect("name").to_string())
        .collect()
}

fn program_nodes(program: &Value) -> Vec<&Value> {
    program["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .flat_map(|s| s["nodes"].as_array().expect("nodes"))
        .collect()
}

fn manual_program_stages(program: &Value) -> Vec<&Value> {
    program["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .filter(|s| is_manual_stage(s))
        .collect()
}

/// `_main_nodes(stage)`: the start/resume nodes, everything but `init`.
fn main_nodes(stage: &Value) -> Vec<&Value> {
    stage["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter(|node| {
            node["controllers"]
                .as_array()
                .expect("controllers")
                .iter()
                .any(|c| MANUAL_CONTROLLER_KINDS.contains(&c["kind"].as_str().unwrap_or("")))
        })
        .collect()
}

fn expected_names(manual: &[&str]) -> Vec<String> {
    HEAD_STAGE_NAMES
        .iter()
        .chain(manual)
        .chain(TAIL_STAGE_NAMES.iter())
        .map(|s| s.to_string())
        .collect()
}

fn flow_first_document() -> Value {
    let mut profile = build_manual_mode_profile();
    let reversed: Vec<Value> = profile["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .rev()
        .cloned()
        .collect();
    profile["stages"] = Value::Array(reversed);
    profile
}

/// `test_build_manual_program_keeps_the_head_and_tail_around_the_manual_stages`.
#[test]
fn build_manual_program_keeps_the_head_and_tail_around_the_manual_stages() {
    let program = program_of(&build_manual_mode_profile());
    assert_eq!(
        stage_names(&program),
        expected_names(&["Manual pressure", "Manual flow"])
    );
    assert_eq!(program["id"], json!(MANUAL_MODE_PROFILE_ID));
    assert_eq!(program["name"], json!(MANUAL_MODE_PROFILE_NAME));
}

/// `test_a_flow_first_document_renders_its_stages_in_document_order`.
#[test]
fn a_flow_first_document_renders_its_stages_in_document_order() {
    let program = program_of(&flow_first_document());
    let manual = manual_program_stages(&program);
    let names: Vec<&Value> = manual.iter().map(|s| &s["name"]).collect();
    assert_eq!(names, [&json!("Manual flow"), &json!("Manual pressure")]);

    let start: Vec<&Value> = main_nodes(manual[0])[0]["controllers"]
        .as_array()
        .expect("controllers")
        .iter()
        .filter(|c| c["initial"]["kind"] == "value")
        .collect();
    assert_eq!(start[0]["kind"], json!("manual_flow_controller"));
}

/// `test_the_head_exits_onto_the_first_manual_stages_init_node`.
#[test]
fn the_head_exits_onto_the_first_manual_stages_init_node() {
    let program = program_of(&build_manual_mode_profile());
    let closing_valve = program["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .find(|s| s["name"] == "closing valve")
        .expect("closing valve");
    let exits: HashSet<i64> = closing_valve["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .flat_map(|n| n["triggers"].as_array().expect("triggers"))
        .map(|t| t["next_node_id"].as_i64().expect("id"))
        .collect();
    assert_eq!(exits, HashSet::from([END_NODE_HEAD]));
    assert_eq!(
        manual_program_stages(&program)[0]["nodes"][0]["id"],
        json!(END_NODE_HEAD)
    );
}

/// `test_node_ids_are_unique_across_the_whole_program`.
#[test]
fn node_ids_are_unique_across_the_whole_program() {
    let program = program_of(&build_manual_mode_profile());
    let ids: Vec<i64> = program_nodes(&program)
        .iter()
        .map(|n| n["id"].as_i64().expect("id"))
        .collect();
    let unique: HashSet<i64> = ids.iter().copied().collect();
    assert_eq!(ids.len(), unique.len());
}

/// `test_every_next_node_id_resolves_to_a_node`.
#[test]
fn every_next_node_id_resolves_to_a_node() {
    let program = program_of(&build_manual_mode_profile());
    let known: HashSet<i64> = program_nodes(&program)
        .iter()
        .map(|n| n["id"].as_i64().expect("id"))
        .collect();
    for node in program_nodes(&program) {
        for trigger in node["triggers"].as_array().expect("triggers") {
            let target = trigger["next_node_id"].as_i64().expect("target");
            assert!(known.contains(&target), "unknown target {target}");
        }
    }
}

/// `test_every_manual_main_node_carries_the_five_contract_triggers`.
#[test]
fn every_manual_main_node_carries_the_five_contract_triggers() {
    let program = program_of(&build_manual_mode_profile());
    let manual = manual_program_stages(&program);
    let resume_ids: Vec<i64> = manual
        .iter()
        .map(|s| {
            main_nodes(s).last().expect("resume")["id"]
                .as_i64()
                .expect("id")
        })
        .collect();
    for (index, stage) in manual.iter().enumerate() {
        let other_resume = resume_ids[(index + 1) % manual.len()];
        for node in main_nodes(stage) {
            let expected = json!([
                {
                    "kind": "button_trigger",
                    "source": "Encoder Button",
                    "gesture": "Single Tap",
                    "next_node_id": other_resume,
                },
                {
                    "kind": "button_trigger",
                    "source": "Encoder Button",
                    "gesture": "Long Press",
                    "next_node_id": INIT_NODE_TAIL,
                },
                {
                    "kind": "user_finish_trigger",
                    "next_node_id": INIT_NODE_TAIL,
                },
                {
                    "kind": "weight_value_trigger",
                    "operator": ">=",
                    "value": MANUAL_MODE_FINAL_WEIGHT_SENTINEL,
                    "source": "Weight Predictive",
                    "weight_reference_id": 1,
                    "next_node_id": INIT_NODE_TAIL,
                },
                {
                    "kind": "piston_position_trigger",
                    "operator": ">=",
                    "value": 73,
                    "source": "Piston Position Raw",
                    "position_reference_id": 0,
                    "next_node_id": INIT_NODE_TAIL,
                },
            ]);
            assert_eq!(node["triggers"], expected);
            // Key order matters on the UART.
            assert_eq!(
                pyfmt::dumps_default(&node["triggers"]),
                pyfmt::dumps_default(&expected)
            );
        }
    }
}

/// `test_no_head_or_tail_node_carries_a_user_finish_trigger`.
#[test]
fn no_head_or_tail_node_carries_a_user_finish_trigger() {
    let program = program_of(&build_manual_mode_profile());
    let template: Vec<&Value> = program["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .filter(|s| !is_manual_stage(s))
        .collect();
    let names: Vec<String> = template
        .iter()
        .map(|s| s["name"].as_str().expect("name").to_string())
        .collect();
    assert_eq!(names, expected_names(&[]));
    let kinds: HashSet<&str> = template
        .iter()
        .flat_map(|s| s["nodes"].as_array().expect("nodes"))
        .flat_map(|n| n["triggers"].as_array().expect("triggers"))
        .map(|t| t["kind"].as_str().expect("kind"))
        .collect();
    assert!(!kinds.is_empty());
    assert!(!kinds.contains("user_finish_trigger"));
}

/// `test_a_tap_hands_the_shot_to_the_other_stage`.
#[test]
fn a_tap_hands_the_shot_to_the_other_stage() {
    let program = program_of(&build_manual_mode_profile());
    let manual = manual_program_stages(&program);
    let (pressure, flow) = (manual[0], manual[1]);
    let flow_resume = &main_nodes(flow).last().expect("resume")["id"];
    let pressure_resume = &main_nodes(pressure).last().expect("resume")["id"];
    for node in main_nodes(pressure) {
        assert_eq!(&node["triggers"][0]["next_node_id"], flow_resume);
    }
    for node in main_nodes(flow) {
        assert_eq!(&node["triggers"][0]["next_node_id"], pressure_resume);
    }
}

/// `test_the_manual_controllers_match_the_contract`.
#[test]
fn the_manual_controllers_match_the_contract() {
    let program = program_of(&build_manual_mode_profile());
    let manual = manual_program_stages(&program);
    let pressure_nodes = main_nodes(manual[0]);
    let flow_nodes = main_nodes(manual[1]);

    assert_eq!(pressure_nodes.len(), 2);
    assert_eq!(
        pressure_nodes[0]["controllers"],
        json!([{
            "kind": "manual_pressure_controller",
            "algorithm": "Pressure PID v1.0",
            "step": 0.1,
            "min": 0.0,
            "max": 12.0,
            "initial": {"kind": "value", "value": 0.0},
        }])
    );
    assert_eq!(
        pressure_nodes[1]["controllers"],
        json!([{
            "kind": "manual_pressure_controller",
            "algorithm": "Pressure PID v1.0",
            "step": 0.1,
            "min": 0.0,
            "max": 12.0,
            "initial": {"kind": "sensor", "source": "Pressure Raw", "gain": 1.1},
        }])
    );
    assert_eq!(flow_nodes.len(), 1);
    assert_eq!(
        flow_nodes[0]["controllers"],
        json!([{
            "kind": "manual_flow_controller",
            "algorithm": "Flow PID v1.0",
            "step": 0.1,
            "min": 0.0,
            "max": 12.0,
            "initial": {"kind": "sensor", "source": "Flow Raw", "gain": 1.1},
        }])
    );
}

/// `test_the_start_node_begins_at_the_documents_first_point`.
#[test]
fn the_start_node_begins_at_the_documents_first_point() {
    let mut document = build_manual_mode_profile();
    document["stages"][0]["dynamics"]["points"] = json!([[0, 6.5]]);
    let program = program_of(&document);
    let manual = manual_program_stages(&program);
    let start = main_nodes(manual[0])[0];
    assert_eq!(
        start["controllers"][0]["initial"],
        json!({"kind": "value", "value": 6.5})
    );
}

/// `test_each_manual_stage_opens_with_its_own_references`.
#[test]
fn each_manual_stage_opens_with_its_own_references() {
    let program = program_of(&build_manual_mode_profile());
    let mut reference_ids = Vec::new();
    for stage in manual_program_stages(&program) {
        let init = &stage["nodes"][0];
        let kinds: Vec<&Value> = init["controllers"]
            .as_array()
            .expect("controllers")
            .iter()
            .map(|c| &c["kind"])
            .collect();
        assert_eq!(
            kinds,
            [
                &json!("time_reference"),
                &json!("weight_reference"),
                &json!("position_reference")
            ]
        );
        let triggers: Vec<&Value> = init["triggers"]
            .as_array()
            .expect("triggers")
            .iter()
            .map(|t| &t["kind"])
            .collect();
        assert_eq!(triggers, [&json!("exit")]);
        reference_ids.extend(
            init["controllers"]
                .as_array()
                .expect("controllers")
                .iter()
                .map(|c| c["id"].as_i64().expect("id")),
        );
    }
    let unique: HashSet<i64> = reference_ids.iter().copied().collect();
    assert_eq!(reference_ids.len(), unique.len());
}

/// `test_the_documents_temperature_reaches_the_heating_curve`.
#[test]
fn the_documents_temperature_reaches_the_heating_curve() {
    let mut document = build_manual_mode_profile();
    document["temperature"] = json!(94.0);
    let program = program_of(&document);
    let curves: Vec<&Value> = program["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .filter(|s| s["name"] == "heating")
        .flat_map(|s| s["nodes"].as_array().expect("nodes"))
        .flat_map(|n| n["controllers"].as_array().expect("controllers"))
        .filter(|c| c["kind"] == "temperature_controller")
        .map(|c| &c["curve"]["points"])
        .collect();
    assert!(curves.contains(&&json!([[0, 94.0]])));
}

/// `test_the_documents_final_weight_is_the_weight_stop`.
#[test]
fn the_documents_final_weight_is_the_weight_stop() {
    let mut document = build_manual_mode_profile();
    document["final_weight"] = json!(42.0);
    let program = program_of(&document);
    let mut values = HashSet::new();
    for stage in manual_program_stages(&program) {
        for node in main_nodes(stage) {
            for trigger in node["triggers"].as_array().expect("triggers") {
                if trigger["kind"] == "weight_value_trigger" {
                    values.insert(trigger["value"].to_string());
                }
            }
        }
    }
    assert_eq!(values, HashSet::from([json!(42.0).to_string()]));
}

/// `test_the_auto_purge_setting_picks_the_tail_route` (both
/// parametrisations).
#[test]
fn the_auto_purge_setting_picks_the_tail_route() {
    for (auto_purge, retracting_exit) in [(false, 30), (true, 48)] {
        let settings = ProgramSettings {
            auto_purge,
            ..ProgramSettings::default()
        };
        let program = program_with(&build_manual_mode_profile(), settings);
        let names = stage_names(&program);
        let manual_names: HashSet<String> = manual_program_stages(&program)
            .iter()
            .map(|s| s["name"].as_str().expect("name").to_string())
            .collect();
        let tail_start = manual_names
            .iter()
            .map(|name| names.iter().position(|n| n == name).expect("present"))
            .max()
            .expect("manual stages")
            + 1;
        let tail_retracting = &program["stages"][tail_start];
        assert_eq!(tail_retracting["name"], json!("retracting"));
        let exits: Vec<i64> = tail_retracting["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .flat_map(|n| n["triggers"].as_array().expect("triggers"))
            .map(|t| t["next_node_id"].as_i64().expect("id"))
            .collect();
        assert!(exits.contains(&retracting_exit));
    }
}

/// `test_stage_skipping_never_adds_a_trigger_to_a_manual_node` (both
/// parametrisations).
#[test]
fn stage_skipping_never_adds_a_trigger_to_a_manual_node() {
    for allow_stage_skipping in [false, true] {
        let settings = ProgramSettings {
            allow_stage_skipping,
            ..ProgramSettings::default()
        };
        let program = program_with(&build_manual_mode_profile(), settings);
        for stage in manual_program_stages(&program) {
            for node in main_nodes(stage) {
                assert_eq!(node["triggers"].as_array().map(Vec::len), Some(5));
            }
        }
    }
}

/// `test_repeated_builds_do_not_drift_the_node_ids`.
#[test]
fn repeated_builds_do_not_drift_the_node_ids() {
    let document = build_manual_mode_profile();
    let mut ids = ReferenceIds::default();
    let settings = ProgramSettings::default();
    let first = build_manual_program(&document, &settings, &mut ids).expect("first");
    let second = build_manual_program(&document, &settings, &mut ids).expect("second");
    let node_ids = |program: &Value| -> Vec<Value> {
        program_nodes(program)
            .iter()
            .map(|n| n["id"].clone())
            .collect()
    };
    assert_eq!(node_ids(&first), node_ids(&second));
    // PARITY: the reference ids do drift; Python's counter is never reset.
    assert_eq!(ids.peek(), 100 + 2 * 2 * 3);
}

/// `test_a_document_with_an_unsupported_stage_type_is_rejected`.
#[test]
fn a_document_with_an_unsupported_stage_type_is_rejected() {
    let mut document = build_manual_mode_profile();
    document["stages"][0]["type"] = json!("temperature");
    let error = build_manual_program(
        &document,
        &ProgramSettings::default(),
        &mut ReferenceIds::default(),
    )
    .expect_err("rejected");
    assert_eq!(
        error,
        ProgramError::Value("unsupported manual stage type 'temperature'".into())
    );
}

/// `test_a_document_without_stages_is_rejected`.
#[test]
fn a_document_without_stages_is_rejected() {
    let mut document = build_manual_mode_profile();
    document["stages"] = json!([]);
    let error = build_manual_program(
        &document,
        &ProgramSettings::default(),
        &mut ReferenceIds::default(),
    )
    .expect_err("rejected");
    assert!(error.to_string().contains("at least one stage"));
}

fn manual_stage_index(program: &Value, nth: usize) -> usize {
    program["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .enumerate()
        .filter(|(_, s)| is_manual_stage(s))
        .nth(nth)
        .expect("manual stage")
        .0
}

fn rejection(program: &Value) -> String {
    validate_program(program).expect_err("rejected").to_string()
}

/// `test_validate_program_rejects_a_duplicate_node_id`.
#[test]
fn validate_program_rejects_a_duplicate_node_id() {
    let mut program = program_of(&build_manual_mode_profile());
    let (first, second) = (
        manual_stage_index(&program, 0),
        manual_stage_index(&program, 1),
    );
    program["stages"][second]["nodes"][0]["id"] =
        program["stages"][first]["nodes"][0]["id"].clone();
    assert!(rejection(&program).contains("duplicate node ids"));
}

/// `test_validate_program_rejects_a_manual_id_that_collides_with_the_head`.
#[test]
fn validate_program_rejects_a_manual_id_that_collides_with_the_head() {
    let mut program = program_of(&build_manual_mode_profile());
    let first = manual_stage_index(&program, 0);
    program["stages"][first]["nodes"][0]["id"] = json!(23);
    assert!(rejection(&program).contains("collide with the head or tail"));
}

/// `test_validate_program_rejects_a_dangling_next_node_id`.
#[test]
fn validate_program_rejects_a_dangling_next_node_id() {
    let mut program = program_of(&build_manual_mode_profile());
    let first = manual_stage_index(&program, 0);
    program["stages"][first]["nodes"][0]["triggers"][0]["next_node_id"] = json!(99999);
    assert!(rejection(&program).contains("unknown node 99999"));
}

/// `test_validate_program_rejects_a_moved_tail_entry`.
#[test]
fn validate_program_rejects_a_moved_tail_entry() {
    let mut program = program_of(&build_manual_mode_profile());
    let names = stage_names(&program);
    let tail_start = names
        .iter()
        .position(|n| n == "closing valve")
        .expect("closing valve")
        + 3;
    assert_eq!(
        program["stages"][tail_start]["nodes"][0]["id"],
        json!(INIT_NODE_TAIL)
    );
    program["stages"][tail_start]["nodes"][0]["id"] = json!(6999);
    assert!(rejection(&program).contains("the tail is entered at 6999"));
}

/// `test_validate_program_rejects_an_unreachable_start_node`.
#[test]
fn validate_program_rejects_an_unreachable_start_node() {
    let mut program = program_of(&build_manual_mode_profile());
    let first = manual_stage_index(&program, 0);
    program["stages"][first]["nodes"][0]["triggers"][0]["next_node_id"] = json!(7000);
    assert!(rejection(&program).contains("unreachable from the head exit"));
}

// --- loading a manual profile sends the program ------------------------------

fn loadable_manual_document() -> Value {
    let mut profile = build_manual_mode_profile();
    profile["display"] = json!({"image": "/api/v1/profile/image/manual.png"});
    profile
}

fn sent(document: &Value) -> Value {
    prepare_for_esp32(
        document,
        &ProgramSettings::default(),
        &mut ReferenceIds::default(),
    )
    .expect("sends")
}

/// `test_loading_a_manual_profile_sends_the_node_program`.
#[test]
fn loading_a_manual_profile_sends_the_node_program() {
    let program = sent(&loadable_manual_document());
    assert!(program.get("id").is_none());
    assert_eq!(
        stage_names(&program),
        expected_names(&["Manual pressure", "Manual flow"])
    );
}

/// `test_loading_a_manual_profile_still_records_the_simplified_document`:
/// preparing leaves the document as it was, so the backend records it.
#[test]
fn loading_a_manual_profile_leaves_the_simplified_document() {
    let document = loadable_manual_document();
    let before = document.clone();
    sent(&document);
    assert_eq!(document, before);
    assert_eq!(
        document["stages"][0]["key"],
        json!(MANUAL_MODE_PRESSURE_STAGE_KEY)
    );
    assert!(!pyfmt::dumps_default(&document).contains("nodes"));
}

/// `test_loading_an_ordinary_profile_sends_only_runtime_fields`.
#[test]
fn loading_an_ordinary_profile_sends_only_runtime_fields() {
    let mut document = loadable_manual_document();
    document.as_object_mut().expect("object").remove("manual");
    document["id"] = json!("22222222-3333-4444-8555-666666666666");
    assert_eq!(sent(&document), profile_for_esp32(&document));
}

/// `test_a_profile_marked_manual_with_a_string_is_not_converted`.
#[test]
fn a_profile_marked_manual_with_a_string_is_not_converted() {
    let mut document = loadable_manual_document();
    document["manual"] = json!("true");
    assert_eq!(sent(&document), profile_for_esp32(&document));
}

/// A manual document the program builder rejects fails the load with
/// Python's `ValueError` text (the handler answers `failed to load profile
/// <message>`).
#[test]
fn an_unbuildable_manual_document_fails_with_the_value_error() {
    let mut document = loadable_manual_document();
    document["stages"][0]["type"] = json!("power");
    let error = prepare_for_esp32(
        &document,
        &ProgramSettings::default(),
        &mut ReferenceIds::default(),
    )
    .expect_err("rejected");
    assert_eq!(error.class_name(), "ValueError");
    assert_eq!(error.to_string(), "unsupported manual stage type 'power'");
    assert!(matches!(error, ProfileError::Program(_)));
}

// --- locating the manual shot ----------------------------------------------

/// `test_find_manual_shot_queries_the_latest_manual_profile_shot`: the
/// query's ids (sort descending, one result, with data, are the backend's).
#[test]
fn find_manual_shot_queries_the_latest_manual_profile_shot() {
    assert_eq!(manual_shot_search_ids(None), [MANUAL_MODE_PROFILE_ID]);
    let expected = manual_shot(json!([pressure_sample(1000, 6.0, json!(0.0))]));
    assert_eq!(pick_manual_shot(None, vec![expected.clone()]), Ok(expected));
}

/// `test_find_manual_shot_queries_the_requested_shot`.
#[test]
fn find_manual_shot_queries_the_requested_shot() {
    let id = "8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6";
    assert_eq!(manual_shot_search_ids(Some(id)), [id]);
    let expected = manual_shot(json!([pressure_sample(1000, 6.0, json!(0.0))]));
    assert_eq!(
        pick_manual_shot(Some(id), vec![expected.clone()]),
        Ok(expected)
    );
}

/// `test_find_manual_shot_without_history`.
#[test]
fn find_manual_shot_without_history() {
    assert_eq!(
        pick_manual_shot(None, vec![]),
        Err(ManualShotError::NotFound(
            "no manual brew in history".into()
        ))
    );
}

/// `test_find_manual_shot_rejects_a_shot_from_another_profile`.
#[test]
fn find_manual_shot_rejects_a_shot_from_another_profile() {
    let mut other = manual_shot(json!([pressure_sample(1000, 6.0, json!(0.0))]));
    other["profile"]["id"] = json!("99999999-8888-4777-8666-555555555555");
    assert!(matches!(
        pick_manual_shot(
            Some("8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6"),
            vec![other.clone()]
        ),
        Err(ManualShotError::NotFound(_))
    ));
    assert_eq!(pick_manual_shot(None, vec![other.clone()]), Ok(other));
}

// --- POST /api/v1/profile/from_manual ---------------------------------------

/// `TestCreateProfileFromManualHandler`: the answers for the builder's
/// errors (`test_a_missing_manual_brew_is_reported_as_not_found`,
/// `test_a_brew_without_targets_is_reported_as_a_conflict`,
/// `test_an_unexpected_failure_is_reported_with_its_cause`).
#[test]
fn from_manual_errors_map_to_the_handler_answers() {
    assert_eq!(
        ManualShotError::NotFound("no manual brew in history".into()).http_response(),
        (
            404,
            json!({"status": "error", "error": "no manual brew found"})
        )
    );
    assert_eq!(
        ManualShotError::NoTargets.http_response(),
        (
            409,
            json!({"status": "error", "error": "shot has no target samples"})
        )
    );
    assert_eq!(
        ManualShotError::Other {
            class: "OSError".into(),
            message: "read-only filesystem".into()
        }
        .http_response(),
        (
            400,
            json!({
                "status": "error",
                "error": "failed to create profile from manual brew",
                "cause": "read-only filesystem",
            })
        )
    );
}
