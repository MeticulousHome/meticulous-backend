//! Differential test against the Python implementation.
//!
//! `tools/gen_goldens.py` runs `log_redactor.redact()` and the backend's
//! `LogRedactionFilter` over a random corpus of realistic log text and stores
//! what Python produced in `tests/data/*.jsonl`. Every case must come out of
//! the Rust port identically. Set `REDACTOR_GOLDENS_DIR` to run a larger,
//! uncommitted corpus generated with `--scale`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use redactor::{LogRecord, RedactionFailure, RedactionState, Redactor, check_key};
use serde_json::Value;

fn data_dir() -> PathBuf {
    std::env::var_os("REDACTOR_GOLDENS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data"))
}

fn cases(name: &str) -> Vec<Value> {
    let path = data_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let cases: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("golden lines are JSON"))
        .collect();
    assert!(!cases.is_empty(), "{} holds no cases", path.display());
    cases
}

fn key(case: &Value) -> Vec<u8> {
    let hex = case["key"].as_str().expect("key is a hex string");
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
        .collect()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("an array of strings")
        .iter()
        .map(|v| v.as_str().expect("a string").to_owned())
        .collect()
}

/// Compiling the rules is the expensive part; share them across keys.
struct Redactors {
    base: Redactor,
    by_key: HashMap<Vec<u8>, Redactor>,
}

impl Redactors {
    fn new() -> Redactors {
        Redactors {
            base: Redactor::new(Vec::new()),
            by_key: HashMap::new(),
        }
    }

    fn get(&mut self, key: Vec<u8>) -> &Redactor {
        let base = &self.base;
        self.by_key
            .entry(key.clone())
            .or_insert_with(|| base.with_key(key))
    }
}

fn report(kind: &str, total: usize, failures: &[String]) {
    assert!(
        failures.is_empty(),
        "{} of {total} {kind} cases differ from Python; first ones:\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// `redact(text, key)` on whole documents, including the learned set.
#[test]
fn redact_matches_python() {
    let cases = cases("redact_goldens.jsonl");
    let mut redactors = Redactors::new();
    let mut failures = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let text = case["text"].as_str().expect("text");
        let got = redactors.get(key(case)).redact(text);
        let expected_text = case["expected"]["text"].as_str().expect("expected text");
        let mut learned = got.learned.clone();
        learned.sort();
        let expected_learned = strings(&case["expected"]["learned"]);
        if got.text != expected_text || learned != expected_learned {
            failures.push(format!(
                "case {index}: input {text:?}\n  python {expected_text:?} {expected_learned:?}\n  rust   {:?} {learned:?}",
                got.text
            ));
        }
    }
    report("redact", cases.len(), &failures);
}

/// `redact(record, key, state=state)` one record at a time, so KnownWifis
/// tracking crosses record boundaries.
#[test]
fn stateful_redact_matches_python() {
    let cases = cases("stateful_goldens.jsonl");
    let mut redactors = Redactors::new();
    let mut failures = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let redactor = redactors.get(key(case));
        let mut state = RedactionState::default();
        let records = strings(&case["records"]);
        let expected = strings(&case["expected"]);
        for (record, expected) in records.iter().zip(&expected) {
            let got = redactor.redact_with_state(record, &mut state).text;
            if &got != expected {
                failures.push(format!(
                    "case {index}: record {record:?}\n  python {expected:?}\n  rust   {got:?}"
                ));
                break;
            }
        }
    }
    report("stateful", cases.len(), &failures);
}

/// `LogRedactionFilter.filter(record)` with a config-seeded sweep.
#[test]
fn log_filter_matches_python() {
    let cases = cases("filter_goldens.jsonl");
    let rules = Redactor::new(Vec::new());
    let mut failures = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        // The Python filter reads the key through load_key(), which rejects a
        // key file that is not exactly 32 bytes with ValueError.
        let key = key(case);
        let key_source = check_key("key", &key)
            .map(|()| key)
            .map_err(RedactionFailure::from);
        let mut filter = rules.log_redactor(key_source);
        filter.set_values(strings(&case["ssids"]), strings(&case["credentials"]));
        let now = Instant::now();
        let records = strings(&case["records"]);
        let expected = strings(&case["expected"]);
        for (text, expected) in records.iter().zip(&expected) {
            let mut record = LogRecord::new("wifi", text.as_str());
            filter.filter(&mut record, now);
            if &record.msg != expected {
                failures.push(format!(
                    "case {index}: record {text:?}\n  python {expected:?}\n  rust   {:?}",
                    record.msg
                ));
                break;
            }
        }
    }
    report("filter", cases.len(), &failures);
}
