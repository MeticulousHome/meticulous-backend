//! `tests/test_profile_uart_payload.py`: what reaches the ESP32, in which
//! bytes. The expected bodies and hashes in `tests/data/uart.json` were
//! written by `Machine.send_json_with_hash`'s `json.dumps` in the Python
//! backend (`tools/gen_goldens.py`).

mod common;

use common::{dumps, golden, md5_hex};
use profile::{
    ESP32_PROFILE_FIELDS, ProgramSettings, ReferenceIds, prepare_for_esp32, process_variables,
    profile_for_esp32,
};
use serde_json::{Value, json};

/// `adaptive_profile()` of the Python test, with its 296,904-character
/// embedded image and 10,000 characters of unknown metadata.
fn adaptive_profile() -> Value {
    json!({
        "id": "a188c420-4952-4b33-8489-a082e9500ecb",
        "name": "Adaptive (Gagn\u{e9} Method)",
        "author": "Corb3t",
        "author_id": "550e8400-e29b-41d4-a716-446655449999",
        "version": 9,
        "last_changed": 1776608894.8666232,
        "image": format!("data:image/png;base64,{}", "A".repeat(296_904)),
        "display": {
            "image": "/api/v1/profile/image/ae3e6ef14875bad88cad00f09945e681.png",
            "graphs": [{"x": "time", "y": "flow", "label": "Flow"}],
            "description": "Display-only recipe guidance",
        },
        "temperature": 90,
        "final_weight": 36,
        "variables": [
            {"key": "pressure_PeakLimit", "name": "Peak Pressure Limit", "type": "pressure", "value": 8.6},
            {"key": "flow_TargetExtract", "name": "Target Extraction Flow", "type": "flow", "value": 4},
            {"key": "pressure_PILimit", "name": "Pre-infusion Pressure Limit", "type": "pressure", "value": 4},
            {"key": "time_FillPause", "name": "Fill Pause Duration", "type": "time", "value": 4},
        ],
        "stages": [
            {
                "key": "fast_fill_1",
                "name": "Fast Fill",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PILimit"}],
                "dynamics": {"over": "time", "points": [[0, 8]], "interpolation": "linear"},
                "exit_triggers": [
                    {"type": "pressure", "value": "$pressure_PILimit", "relative": false, "comparison": ">="}
                ],
            },
            {
                "key": "fill_pause_2",
                "name": "Fill Pause",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PILimit"}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, 0.5], ["$time_FillPause", 0.5]],
                    "interpolation": "linear",
                },
                "exit_triggers": [
                    {"type": "time", "value": "$time_FillPause", "relative": true, "comparison": ">="}
                ],
            },
            {
                "key": "pressure_ramp_3",
                "name": "Pressure Ramp",
                "type": "pressure",
                "limits": [{"type": "flow", "value": 6}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, "$pressure_PILimit"], [4, "$pressure_PeakLimit"]],
                    "interpolation": "linear",
                },
                "exit_triggers": [{"type": "time", "value": 4, "relative": true, "comparison": ">="}],
            },
            {
                "key": "adaptive_extraction_4",
                "name": "Adaptive Extraction",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PeakLimit"}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, "$flow_TargetExtract"], [35, 0.5]],
                    "interpolation": "linear",
                },
                "exit_triggers": [{"type": "time", "value": 45, "relative": true, "comparison": ">="}],
            },
        ],
        "community": {"downloads": 123, "featured": true},
        "unknown_future_metadata": {"large": "B".repeat(10_000)},
    })
}

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect()
}

fn uart_golden(name: &str) -> Value {
    golden("uart.json")
        .as_array()
        .expect("list")
        .iter()
        .find(|case| case["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("no uart golden {name}"))
}

/// The frames `Machine.send_json_with_hash` writes: `hash,<md5>\x03` then
/// `json\n<body>\x03`, the md5 taken over the body.
fn serial_frames(payload: &Value) -> Vec<Vec<u8>> {
    let body = dumps(payload);
    let digest = md5_hex(&body);
    vec![
        b"hash,".to_vec(),
        digest.into_bytes(),
        b"\x03".to_vec(),
        format!("json\n{body}\x03").into_bytes(),
    ]
}

/// `test_adaptive_profile_uart_payload_collapses_to_recipe_data`.
#[test]
fn adaptive_profile_uart_payload_collapses_to_recipe_data() {
    let profile = adaptive_profile();

    let preprocessed = process_variables(&profile).expect("variables resolve");
    let payload = profile_for_esp32(&preprocessed);

    assert!(dumps(&profile).len() > 300_000);
    assert!(dumps(&payload).len() < 5_000);
    assert_eq!(keys(&payload), ESP32_PROFILE_FIELDS);
    assert_eq!(payload["stages"][0]["limits"][0]["value"], json!(4));
    assert_eq!(payload["stages"][1]["dynamics"]["points"][1][0], json!(4));
    assert_eq!(payload["stages"][2]["dynamics"]["points"][1][1], json!(8.6));
    assert_eq!(payload["stages"][3]["dynamics"]["points"][0][1], json!(4));
    for absent in [
        "image",
        "id",
        "display",
        "author",
        "community",
        "unknown_future_metadata",
    ] {
        assert!(
            payload.get(absent).is_none(),
            "{absent} reached the payload"
        );
    }

    // The bytes and the hash Python sends for this profile.
    let golden = uart_golden("adaptive");
    assert_eq!(dumps(&payload), golden["body"].as_str().expect("body"));
    assert_eq!(
        md5_hex(&dumps(&payload)),
        golden["md5"].as_str().expect("md5")
    );
    assert_eq!(
        dumps(&profile).len(),
        golden["profile_json_len"].as_u64().expect("len") as usize
    );
}

/// `test_node_profile_keeps_complete_runtime_graph_but_not_metadata`.
#[test]
fn node_profile_keeps_complete_runtime_graph_but_not_metadata() {
    let profile = json!({
        "id": "11111111-2222-4333-8444-555555555555",
        "name": "Diagnostic graph",
        "stages": [
            {
                "name": "initialize",
                "nodes": [
                    {
                        "id": -1,
                        "controllers": [
                            {"kind": "time_reference", "id": 1},
                            {"kind": "log_controller", "message": "loaded"},
                        ],
                        "triggers": [
                            {
                                "kind": "timer_trigger",
                                "timer_reference_id": 1,
                                "operator": ">=",
                                "value": 1,
                                "next_node_id": -2,
                            }
                        ],
                    }
                ],
            }
        ],
        "display": {"image": "data:image/png;base64,unused"},
        "author": "UI metadata",
    });

    let payload = profile_for_esp32(&profile);

    assert_eq!(
        payload,
        json!({"name": profile["name"], "stages": profile["stages"]})
    );
    assert_eq!(keys(&payload), ["name", "stages"]);
}

/// `test_send_preserves_alarm_guard_profile_state_and_load_event`, the part
/// that is not runtime: what `send_profile_to_esp32` hands to the UART. The
/// alarm guard, the recorded last profile and the load event are the
/// backend's.
#[test]
fn send_hands_only_the_runtime_fields_to_the_uart() {
    let profile = adaptive_profile();
    let mut ids = ReferenceIds::default();

    let sent = prepare_for_esp32(&profile, &ProgramSettings::default(), &mut ids)
        .expect("the adaptive profile is valid");

    assert_eq!(keys(&sent), ESP32_PROFILE_FIELDS);
    assert!(dumps(&sent).len() < 5_000);
    let golden = uart_golden("adaptive");
    assert_eq!(
        serial_frames(&sent),
        vec![
            b"hash,".to_vec(),
            golden["md5"].as_str().expect("md5").as_bytes().to_vec(),
            b"\x03".to_vec(),
            format!("json\n{}\x03", golden["body"].as_str().expect("body")).into_bytes(),
        ]
    );
    // A non-manual profile does not touch the converter's id counter.
    assert_eq!(ids, ReferenceIds::default());
}

/// The bundled profiles and the Manual mode program, byte for byte with the
/// bodies and hashes Python sends for them.
#[test]
fn bundled_profiles_and_the_manual_program_hash_like_python() {
    let simple = common::repo_json("simple_profile.json");
    let example = common::repo_json("profile_schema/example_profile.json");
    let manual = profile::build_manual_mode_profile();
    for (name, profile) in [
        ("simple", simple),
        ("example", example),
        ("manual_mode", manual),
    ] {
        let golden = uart_golden(name);
        let mut ids = ReferenceIds::default();
        let sent = prepare_for_esp32(&profile, &ProgramSettings::default(), &mut ids)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let body = dumps(&sent);
        assert_eq!(body, golden["body"].as_str().expect("body"), "{name}");
        assert_eq!(
            md5_hex(&body),
            golden["md5"].as_str().expect("md5"),
            "{name}"
        );
    }
}
