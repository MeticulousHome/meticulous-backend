//! The Rust ports against the Python they replace, case by case.
//!
//! Every expected value in `tests/data/` was produced by running the Python
//! backend's code (`tools/gen_goldens.py`) on a corpus of profiles (the
//! bundled ones, the schema's example, the test fixtures, every profile the
//! old `default_profiles` submodule ever shipped) and on thousands of mutated
//! copies.

mod common;

use common::{Mismatches, TestEnv, apply_ops, dumps, golden, md5_hex};
use profile::manual_mode::build_profile_from_manual_shot;
use profile::pour_over::{
    b64decode_strict, is_oversized_pour_over_body, serialize_pour_over_profile,
    validate_pour_over_profile,
};
use profile::profiles::espresso_validator;
use profile::pyrepr::{is_printable, py_repr, py_repr_str, py_strip};
use profile::pyround::{py_format_fixed, py_round};
use profile::schema::{best_match, is_uuid};
use profile::{
    ManualModeSeed, ProfileError, ProgramSettings, ReferenceIds, build_manual_mode_profile,
    manual_mode_stages, normalize_loaded_profile, plan_manual_mode_profile, prepare_for_esp32,
    process_variables, validate_profile,
};
use serde_json::{Value, json};

/// The outcome the generator records for a `ProfileError`.
fn profile_error_outcome(error: &ProfileError) -> Value {
    if let ProfileError::Schema(e) = error {
        return json!({"kind": "schema", "message": e.message});
    }
    let kind = if error.is_variable_error() {
        "preprocess"
    } else {
        "other"
    };
    json!({"kind": kind, "class": error.class_name(), "message": error.to_string()})
}

/// `ProfileManager.validate_profile`, and the espresso schema's
/// `iter_errors` and `best_match` on their own.
#[test]
fn espresso_validation_matches_python() {
    let data = golden("validation.json");
    let bases = data["bases"].as_array().expect("bases");
    let mut validate = Mismatches::new("validate_profile");
    let mut schema = Mismatches::new("espresso schema iter_errors/best_match");
    for case in data["cases"].as_array().expect("cases") {
        let doc = apply_ops(
            &bases[case["base"].as_u64().expect("base") as usize],
            &case["ops"],
        );

        let actual = match validate_profile(&doc) {
            Ok(()) => Value::Null,
            Err(error) => profile_error_outcome(&error),
        };
        validate.check(actual == case["validate"], || {
            format!(
                "ops {}\n  python {}\n  rust   {}",
                case["ops"], case["validate"], actual
            )
        });

        let errors = espresso_validator().iter_errors(&doc);
        let listed: Vec<Value> = errors
            .iter()
            .map(|e| json!([e.path_json(), e.message, e.validator()]))
            .collect();
        let best = best_match(errors)
            .map(|e| json!([e.path_json(), e.message]))
            .unwrap_or(Value::Null);
        let actual = json!({"errors": listed, "best": best});
        schema.check(actual == case["schema"], || {
            format!(
                "ops {}\n  python {}\n  rust   {}",
                case["ops"], case["schema"], actual
            )
        });
    }
    validate.finish();
    schema.finish();
}

fn preprocess_outcome(doc: &Value) -> Value {
    match process_variables(doc) {
        Ok(out) => {
            let text = dumps(&out);
            json!({"md5": md5_hex(&text), "len": text.chars().count()})
        }
        Err(error) => json!({"error": {"class": error.class_name(), "message": error.to_string()}}),
    }
}

/// `ProfilePreprocessor.processVariables` on well-formed and malformed
/// documents: the output (its `json.dumps` hash) or the exception.
#[test]
fn process_variables_matches_python() {
    let data = golden("preprocessor.json");
    let bases = data["bases"].as_array().expect("bases");
    let mut mismatches = Mismatches::new("processVariables");
    for case in data["cases"].as_array().expect("cases") {
        let doc = apply_ops(
            &bases[case["base"].as_u64().expect("base") as usize],
            &case["ops"],
        );
        let actual = preprocess_outcome(&doc);
        mismatches.check(actual == case["expected"], || {
            format!(
                "ops {}\n  python {}\n  rust   {}",
                case["ops"], case["expected"], actual
            )
        });
    }
    for case in data["roots"].as_array().expect("roots") {
        let actual = preprocess_outcome(&case["input"]);
        mismatches.check(actual == case["expected"], || {
            format!(
                "input {}\n  python {}\n  rust   {}",
                case["input"], case["expected"], actual
            )
        });
    }
    mismatches.finish();
}

/// `send_profile_to_esp32` up to `Machine.send_json_with_hash`: the bytes
/// sent (as `json.dumps` and its md5) or the exception, and where the
/// converter's reference id counter ends up.
#[test]
fn esp32_payloads_match_python_byte_for_byte() {
    let data = golden("esp32.json");
    let bases = data["bases"].as_array().expect("bases");
    let mut mismatches = Mismatches::new("prepare_for_esp32");
    for case in data["cases"].as_array().expect("cases") {
        let mut doc = apply_ops(
            &bases[case["base"].as_u64().expect("base") as usize],
            &case["ops"],
        );
        // The backend gives the profile an id before preparing it.
        if let Value::Object(map) = &mut doc {
            map.entry("id")
                .or_insert_with(|| Value::from("00000000-0000-4000-8000-00000000abcd"));
        }
        let settings = &case["settings"];
        let settings = ProgramSettings {
            auto_purge: settings["auto_purge"].as_bool().expect("auto_purge"),
            allow_stage_skipping: settings["allow_stage_skipping"].as_bool().expect("skip"),
            partial_retraction: settings["partial_retraction"].as_f64().expect("retraction"),
        };
        let mut ids = ReferenceIds::starting_at(case["ref_start"].as_i64().expect("ref_start"));
        let expected = &case["expected"];
        let (actual, payload) = match prepare_for_esp32(&doc, &settings, &mut ids) {
            Ok(payload) => {
                let text = dumps(&payload);
                (
                    json!({"md5": md5_hex(&text), "len": text.chars().count(), "ref_end": ids.peek()}),
                    Some(text),
                )
            }
            Err(error) => (
                json!({"error": profile_error_outcome(&error), "ref_end": ids.peek()}),
                None,
            ),
        };
        let mut comparable = expected.clone();
        if let Value::Object(map) = &mut comparable {
            map.remove("payload");
        }
        mismatches.check(actual == comparable, || {
            let diff = match (expected.get("payload").and_then(Value::as_str), &payload) {
                (Some(py), Some(rs)) => first_difference(py, rs),
                _ => String::new(),
            };
            format!(
                "base {} ops {} settings {}\n  python {}\n  rust   {}{diff}",
                case["base"], case["ops"], case["settings"], comparable, actual
            )
        });
        if let (Some(py), Some(rs)) = (expected.get("payload").and_then(Value::as_str), &payload) {
            mismatches.check(py == rs, || first_difference(py, rs));
        }
    }
    mismatches.finish();
}

fn first_difference(python: &str, rust: &str) -> String {
    let at = python
        .chars()
        .zip(rust.chars())
        .position(|(a, b)| a != b)
        .unwrap_or(python.len().min(rust.len()));
    let from = at.saturating_sub(80);
    let py: String = python.chars().skip(from).take(200).collect();
    let rs: String = rust.chars().skip(from).take(200).collect();
    format!("\n  first difference at char {at}:\n  python ...{py}\n  rust   ...{rs}")
}

/// `manual_mode.build_profile_from_manual_shot` with uuid4 as a counter and
/// the clock in UTC.
#[test]
fn profiles_from_manual_shots_match_python() {
    let data = golden("manual_shots.json");
    let mut mismatches = Mismatches::new("build_profile_from_manual_shot");
    for case in data["cases"].as_array().expect("cases") {
        let mut env = TestEnv::default();
        let name = case["name"].as_str();
        let actual = match build_profile_from_manual_shot(&case["shot"], name, &mut env) {
            Ok(profile) => json!({"profile": profile, "uuids": env.uuids as u64}),
            Err(profile::ManualShotError::NoTargets) => {
                json!({"error": {"class": "ManualShotHasNoTargets"}})
            }
            Err(error) => {
                json!({"error": {"class": error.class_name(), "message": error.to_string()}})
            }
        };
        let expected = &case["expected"];
        // Compare the serialisation too: Value equality ignores key order.
        mismatches.check(
            actual == *expected && dumps(&actual) == dumps(expected),
            || {
                format!(
                    "shot {}\n name {:?}\n  python {}\n  rust   {}",
                    dumps(&case["shot"]),
                    name,
                    expected,
                    actual
                )
            },
        );
    }
    mismatches.finish();
}

/// `build_manual_mode_profile`, `manual_mode_stages`,
/// `ensure_manual_mode_profile` and the `refresh_profile_list` defaults.
#[test]
fn profile_helpers_match_python() {
    let data = golden("profiles_helpers.json");
    assert_eq!(
        dumps(&build_manual_mode_profile()),
        dumps(&data["manual_mode_profile"])
    );
    assert_eq!(
        dumps(&manual_mode_stages()),
        dumps(&data["manual_mode_stages"])
    );

    let mut seeds = Mismatches::new("ensure_manual_mode_profile");
    for case in data["seeds"].as_array().expect("seeds") {
        let existing = case.get("existing").filter(|e| !e.is_null());
        let (written, result) = match plan_manual_mode_profile(existing) {
            ManualModeSeed::UpToDate => (Value::Null, false),
            ManualModeSeed::Seed(profile) | ManualModeSeed::Migrate(profile) => (profile, true),
        };
        seeds.check(
            dumps(&written) == dumps(&case["written"]) && case["result"] == result,
            || {
                format!(
                    "existing {}\n  python {}\n  rust   {}",
                    case["existing"], case["written"], written
                )
            },
        );
    }
    seeds.finish();

    let mut normalize = Mismatches::new("refresh_profile_list defaults");
    for case in data["normalize"].as_array().expect("normalize") {
        let mut doc = case["input"].clone();
        let mut uuids = 0u128;
        let mut next = || {
            uuids += 1;
            common::counter_uuid(uuids)
        };
        let actual = match normalize_loaded_profile(&mut doc, &mut next) {
            Ok(changed) => {
                json!({"input": case["input"], "output": doc, "changed": changed, "uuids": uuids as u64})
            }
            Err(exc) => json!({
                "input": case["input"],
                "error": {"class": exc.class_name(), "message": exc.message()},
            }),
        };
        normalize.check(dumps(&actual) == dumps(case), || {
            format!("  python {case}\n  rust   {actual}")
        });
    }
    normalize.finish();
}

/// `PourOverProfileManager.validate_profile` (sorted schema issues, then the
/// semantic rules), the stored serialisation, and the oversized body check.
#[test]
fn pour_over_validation_matches_python() {
    let data = golden("pour_over.json");
    let bases = data["bases"].as_array().expect("bases");
    let mut mismatches = Mismatches::new("Pour Over validate_profile");
    let outcome = |doc: &Value, serialise: bool| match validate_pour_over_profile(doc) {
        Ok(_) if serialise => {
            let text = serialize_pour_over_profile(doc).unwrap_or_default();
            json!({"ok": true, "md5": md5_hex(&text), "bytes": text.len()})
        }
        Ok(_) => json!({"ok": true}),
        Err(profile::PourOverProfileError::Validation(issues)) => {
            json!({"issues": issues.iter().map(|i| i.to_json()).collect::<Vec<_>>()})
        }
        Err(other) => json!({"unexpected": other.to_string()}),
    };
    for case in data["cases"].as_array().expect("cases") {
        let doc = apply_ops(
            &bases[case["base"].as_u64().expect("base") as usize],
            &case["ops"],
        );
        let actual = outcome(&doc, true);
        mismatches.check(dumps(&actual) == dumps(&case["expected"]), || {
            format!(
                "base {} ops {}\n  python {}\n  rust   {}",
                case["base"], case["ops"], case["expected"], actual
            )
        });
    }
    for case in data["roots"].as_array().expect("roots") {
        let actual = outcome(&case["input"], false);
        mismatches.check(dumps(&actual) == dumps(&case["expected"]), || {
            format!(
                "input {}\n  python {}\n  rust   {}",
                case["input"], case["expected"], actual
            )
        });
    }
    for case in data["bodies"].as_array().expect("bodies") {
        let body = format!(
            "{}{}{}",
            case["prefix"].as_str().expect("prefix"),
            "x".repeat(case["pad"].as_u64().expect("pad") as usize),
            case["suffix"].as_str().expect("suffix")
        );
        let actual = is_oversized_pour_over_body(body.as_bytes());
        mismatches.check(case["expected"] == actual, || {
            format!("body {} pad {}", case["prefix"], case["pad"])
        });
    }
    mismatches.finish();
}

/// `repr`, `str.strip`, `str.isprintable`, `round`, `format(x, ".nf")`,
/// strict base64 and the `uuid` format check, against CPython.
#[test]
fn cpython_primitives_match() {
    let data = golden("primitives.json");
    let mut mismatches = Mismatches::new("CPython primitives");
    for case in data["repr_strings"].as_array().expect("repr_strings") {
        let s = case[0].as_str().expect("string");
        mismatches.check(py_repr_str(s) == case[1].as_str().expect("repr"), || {
            format!("repr({s:?}): python {} rust {}", case[1], py_repr_str(s))
        });
        mismatches.check(py_strip(s) == case[2].as_str().expect("strip"), || {
            format!("strip({s:?}): python {:?} rust {:?}", case[2], py_strip(s))
        });
        let printable = s.chars().all(is_printable);
        mismatches.check(case[3] == printable, || {
            format!("isprintable({s:?}): python {} rust {printable}", case[3])
        });
    }
    for case in data["repr_values"].as_array().expect("repr_values") {
        mismatches.check(py_repr(&case[0]) == case[1].as_str().expect("repr"), || {
            format!(
                "repr({}): python {} rust {}",
                case[0],
                case[1],
                py_repr(&case[0])
            )
        });
    }
    for case in data["round"].as_array().expect("round") {
        let x = case[0].as_f64().expect("x");
        let same = |a: f64, b: &Value| b.as_f64().is_some_and(|b| a.to_bits() == b.to_bits());
        mismatches.check(same(py_round(x, 2), &case[1]), || {
            format!(
                "round({x:?}, 2): python {} rust {:?}",
                case[1],
                py_round(x, 2)
            )
        });
        mismatches.check(same(py_round(x, 1), &case[2]), || {
            format!(
                "round({x:?}, 1): python {} rust {:?}",
                case[2],
                py_round(x, 1)
            )
        });
        mismatches.check(
            py_format_fixed(x, 2) == case[3].as_str().expect(".2f"),
            || {
                format!(
                    "{x:?}:.2f python {} rust {}",
                    case[3],
                    py_format_fixed(x, 2)
                )
            },
        );
        mismatches.check(
            py_format_fixed(x, 1) == case[4].as_str().expect(".1f"),
            || {
                format!(
                    "{x:?}:.1f python {} rust {}",
                    case[4],
                    py_format_fixed(x, 1)
                )
            },
        );
    }
    for case in data["b64"].as_array().expect("b64") {
        let text = case[0].as_str().expect("text");
        let actual = b64decode_strict(text)
            .map(|bytes| Value::from(bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
            .unwrap_or(Value::Null);
        mismatches.check(actual == case[1], || {
            format!("b64decode({text:?}): python {} rust {actual}", case[1])
        });
    }
    for case in data["uuid"].as_array().expect("uuid") {
        let text = case[0].as_str().expect("text");
        mismatches.check(case[1] == is_uuid(text), || {
            format!("uuid {text:?}: python {} rust {}", case[1], is_uuid(text))
        });
    }
    mismatches.finish();
}
