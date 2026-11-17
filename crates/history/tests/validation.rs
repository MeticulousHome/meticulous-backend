//! `SearchParams` and `PourOverSession` validation against pydantic.
//!
//! `tests/data/validation.json` holds pydantic's answer for every input
//! (made by `generate_validation.py`): the validated model or the errors
//! `ValidationError.json()` lists. Also
//! test_session_validation_rejects_non_finite_and_reversed_ranges and the
//! 422 shapes of tests/test_api_history.py.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::load_json;
use history::SearchParams;
use history::pour_over::PourOverSession;
use history::search::SearchId;
use serde_json::{Map, Value, json};

/// The reference marks non-finite floats as {"$float": "NaN"}.
fn decode(value: &Value) -> Value {
    match value {
        Value::Object(map) if map.contains_key("$float") => Value::Null,
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), decode(v))).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(decode).collect()),
        other => other.clone(),
    }
}

fn is_non_finite(value: &Value) -> bool {
    matches!(value, Value::Object(map) if map.contains_key("$float"))
}

fn contains_non_finite(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.contains_key("$float") || map.values().any(contains_non_finite),
        Value::Array(items) => items.iter().any(contains_non_finite),
        _ => false,
    }
}

fn params_json(params: &SearchParams) -> Value {
    let float = |f: Option<f64>| match f {
        None => Value::Null,
        Some(f) if !f.is_finite() => Value::Null,
        Some(f) => json!(f),
    };
    json!({
        "query": params.query,
        "ids": params.ids.iter().map(|id| match id {
            SearchId::Str(s) => json!(s),
            SearchId::Int(i) => json!(i),
        }).collect::<Vec<_>>(),
        "start_date": float(params.start_date),
        "end_date": float(params.end_date),
        "order_by": params.order_by.iter().map(|o| o.as_str()).collect::<Vec<_>>(),
        "sort": params.sort.as_str(),
        "max_results": params.max_results,
        "dump_data": params.dump_data,
    })
}

/// Python's json.dumps of a value, so 0 and 0.0 differ like in the reference.
fn same(a: &Value, b: &Value) -> bool {
    pyfmt::dumps_default(a) == pyfmt::dumps_default(b)
}

/// HistoryHandler: `SearchParams(**data)` accepts, coerces and rejects the
/// same inputs as pydantic, with the same errors (`e.json()`), including
/// test_invalid_search_parameters_are_rejected_with_422.
#[test]
fn search_params_match_pydantic() {
    let reference = load_json("validation.json");
    let (mut accepted, mut rejected) = (0, 0);
    for case in reference["search_params"].as_array().unwrap() {
        let input: Map<String, Value> = case["input"].as_object().unwrap().clone();
        let result = SearchParams::from_json(&input);
        if let Some(ok) = case.get("ok") {
            let params = result.unwrap_or_else(|e| panic!("{input:?} rejected: {e}"));
            assert!(
                same(&params_json(&params), &decode(ok)),
                "{input:?}: {:?} vs {ok}",
                params_json(&params)
            );
            if is_non_finite(&ok["start_date"]) {
                assert!(params.start_date.is_some_and(|f| !f.is_finite()));
            }
            accepted += 1;
        } else {
            rejected += 1;
            let errors = result.err().unwrap_or_else(|| panic!("{input:?} accepted"));
            let actual: Vec<Value> = errors.to_json(false).as_array().unwrap().clone();
            let expected = case["errors"].as_array().unwrap();
            assert_eq!(actual.len(), expected.len(), "{input:?}: {actual:?}");
            for (a, e) in actual.iter().zip(expected) {
                assert!(same(a, e), "{input:?}:\n  rust   {a}\n  python {e}");
            }
            // e.json() carries the documentation URL of the pydantic version.
            let with_url = errors.to_json(true);
            let url = with_url[0]["url"].as_str().unwrap();
            assert_eq!(
                url,
                format!(
                    "https://errors.pydantic.dev/2.12/v/{}",
                    expected[0]["type"].as_str().unwrap()
                )
            );
        }
    }
    assert!(
        accepted > 20 && rejected > 15,
        "{accepted} accepted, {rejected} rejected"
    );
}

/// test_invalid_search_parameters_are_rejected_with_422 (the asserted part).
#[test]
fn invalid_search_parameters_errors() {
    let input = json!({"sort": "sideways", "max_results": "x"});
    let errors = SearchParams::from_json(input.as_object().unwrap()).unwrap_err();
    let summary: Vec<(Value, Value)> = errors
        .to_json(true)
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["loc"].clone(), e["type"].clone()))
        .collect();
    assert_eq!(
        summary,
        vec![
            (json!(["sort"]), json!("enum")),
            (json!(["max_results"]), json!("int_parsing"))
        ]
    );
}

/// PourOverSession.model_validate: the stored payload (model_dump(mode="json"))
/// or the 422 details, as pydantic produces them.
#[test]
fn pour_over_sessions_match_pydantic() {
    let reference = load_json("validation.json");
    let mut compared = 0;
    for (index, case) in reference["pour_over"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        compared += 1;
        let input = decode(&case["input"]);
        let result = PourOverSession::validate(&input);
        if let Some(ok) = case.get("ok") {
            let session = result.unwrap_or_else(|e| panic!("case {index} rejected: {e}"));
            assert!(
                same(&session.payload, ok),
                "case {index}:\n  rust   {}\n  python {}",
                pyfmt::dumps_default(&session.payload),
                pyfmt::dumps_default(ok)
            );
            continue;
        }
        let expected = case["errors"].as_array().unwrap();
        if contains_non_finite(&case["input"]) {
            // PARITY: a non-finite value reaches Rust as null (see
            // PourOverSession::validate); it is still rejected at that field.
            let errors = result
                .err()
                .unwrap_or_else(|| panic!("case {index} accepted"));
            assert_eq!(
                errors.errors[0].loc,
                expected[0]["loc"].as_array().unwrap().clone()
            );
            continue;
        }
        let errors = result
            .err()
            .unwrap_or_else(|| panic!("case {index} accepted"));
        let actual = errors.to_json(false);
        let actual = actual.as_array().unwrap();
        assert_eq!(actual.len(), expected.len(), "case {index}: {actual:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!(same(a, e), "case {index}:\n  rust   {a}\n  python {e}");
        }
    }
    assert!(compared > 70, "{compared} cases");
}

/// test_session_validation_rejects_non_finite_and_reversed_ranges and
/// test_compressed_record_reader_bounds_expansion (the validation part).
#[test]
fn session_validation_rejects_reversed_and_out_of_range_values() {
    let reference = load_json("validation.json");
    let base = decode(&reference["pour_over"][0]["input"]);
    let mut duration = base.clone();
    duration["measurements"]["durationMs"] = json!(600_001);
    assert!(PourOverSession::validate(&duration).is_err());
    let mut sample_time = base.clone();
    sample_time["samples"][1]["t"] = json!(600_001);
    assert!(PourOverSession::validate(&sample_time).is_err());
    let mut many = base.clone();
    let samples = many["samples"].as_array().unwrap().clone();
    many["samples"] = Value::Array(
        samples
            .iter()
            .cycle()
            .take(samples.len() * 1_501)
            .cloned()
            .collect(),
    );
    assert!(PourOverSession::validate(&many).is_err());
    let mut reversed = base.clone();
    reversed["recipe"]["pourTargets"] =
        json!([{"number": 1, "startTimeMs": 0, "stopWeightG": 40, "flowRangeGps": [5, 4]}]);
    assert!(PourOverSession::validate(&reversed).is_err());
    let mut non_finite = base;
    non_finite["samples"][0]["f"] = Value::Null;
    assert!(PourOverSession::validate(&non_finite).is_err());
}
