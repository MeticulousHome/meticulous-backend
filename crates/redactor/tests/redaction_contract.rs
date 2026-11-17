//! Port of `log_redactor/tests/test_redaction_contract.py`: the exact token
//! strings for a fixed key, shared by the watcher, the Python backend and
//! this crate.
//!
//! The pinned values are read from the submodule's own test file rather than
//! copied, so this runs against the commit the backend has checked out: a
//! submodule bump that changes a token fails here, as it fails the Python
//! suites. Do not "fix" a failure by changing the expectations; see the
//! docstring of that file.

use std::path::PathBuf;

use redactor::{Redactor, pseudonym};
use regex::Regex;

fn contract_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../log_redactor/tests/test_redaction_contract.py");
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {} ({e}); run `git submodule update --init log_redactor`",
            path.display()
        )
    })
}

/// A Python string literal body, for the escapes the contract file uses.
fn unescape(body: &str) -> String {
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            other => panic!("unsupported escape \\{other:?} in the contract file"),
        }
    }
    out
}

/// `CONTRACT_KEY = b"\xHH" * N`.
fn contract_key(source: &str) -> Vec<u8> {
    let caps = Regex::new(r#"(?m)^CONTRACT_KEY = b"\\x([0-9a-fA-F]{2})" \* (\d+)$"#)
        .expect("pattern compiles")
        .captures(source)
        .expect("CONTRACT_KEY keeps the b\"\\xHH\" * N form");
    let byte = u8::from_str_radix(&caps[1], 16).expect("two hex digits");
    vec![byte; caps[2].parse().expect("a byte count")]
}

/// `EXPECTED_TOKENS`: (kind, value) -> token.
fn expected_tokens(source: &str) -> Vec<(String, String, String)> {
    let start = source
        .find("EXPECTED_TOKENS = {")
        .expect("EXPECTED_TOKENS is defined");
    let end = start + source[start..].find("\n}").expect("EXPECTED_TOKENS closes");
    let re = Regex::new(r#"\("(\w+)", "((?:[^"\\]|\\.)*)"\): "((?:[^"\\]|\\.)*)""#)
        .expect("pattern compiles");
    let tokens: Vec<_> = re
        .captures_iter(&source[start..end])
        .map(|c| (c[1].to_owned(), unescape(&c[2]), unescape(&c[3])))
        .collect();
    assert!(!tokens.is_empty(), "no EXPECTED_TOKENS entries parsed");
    tokens
}

/// The `(source, expected)` pairs of `test_end_to_end_vectors_are_pinned`.
fn end_to_end_vectors(source: &str) -> Vec<(String, String)> {
    let start = source
        .find("def test_end_to_end_vectors_are_pinned")
        .expect("test_end_to_end_vectors_are_pinned exists");
    let body = &source[start..];
    let body = &body[..body
        .find("for source, expected in vectors")
        .expect("vectors loop")];
    let re = Regex::new(r#"\(\s*"((?:[^"\\]|\\.)*)",\s*"((?:[^"\\]|\\.)*)",\s*\)"#)
        .expect("pattern compiles");
    let vectors: Vec<_> = re
        .captures_iter(body)
        .map(|c| (unescape(&c[1]), unescape(&c[2])))
        .collect();
    assert!(!vectors.is_empty(), "no end-to-end vectors parsed");
    vectors
}

/// `RedactionContractTests.test_token_values_are_pinned`.
#[test]
fn token_values_are_pinned() {
    let source = contract_source();
    let key = contract_key(&source);
    for (kind, value, expected) in expected_tokens(&source) {
        assert_eq!(pseudonym(&kind, &value, &key), expected, "{kind} {value:?}");
    }
}

/// `RedactionContractTests.test_pseudonym_matches_what_redact_emits`: the
/// call-site entry point (`wifi.redact_ssid`) and the rules agree.
#[test]
fn pseudonym_matches_what_redact_emits() {
    let key = contract_key(&contract_source());
    let out = Redactor::new(key.clone()).redact("Config: added 'ssid' value 'HomeNet'");
    assert_eq!(out.learned, ["HomeNet"]);
    assert_eq!(out.text, "Config: added 'ssid' value '[SSID_604cf397]'");
    assert_eq!(pseudonym("SSID", "HomeNet", &key), "[SSID_604cf397]");
}

/// `RedactionContractTests.test_end_to_end_vectors_are_pinned`.
#[test]
fn end_to_end_vectors_are_pinned() {
    let source = contract_source();
    let redactor = Redactor::new(contract_key(&source));
    for (input, expected) in end_to_end_vectors(&source) {
        assert_eq!(redactor.redact(&input).text, expected, "{input:?}");
    }
}

/// `RedactionContractTests.test_case_folding_of_shape_matched_values_is_pinned`:
/// MAC and IPv6 are lowercased before hashing; SSIDs are not.
#[test]
fn case_folding_of_shape_matched_values_is_pinned() {
    let redactor = Redactor::new(contract_key(&contract_source()));
    assert_eq!(
        redactor.redact("set MAC address to AA:BB:CC:DD:EE:FF").text,
        "set MAC address to [MAC_cfe506c4]"
    );
    assert_ne!(
        redactor.redact("ssid: homenet").text,
        "ssid: [SSID_604cf397]"
    );
}
