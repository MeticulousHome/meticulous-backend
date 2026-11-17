//! Port of `log_redactor/tests/test_log_redactor.py`.

use std::cell::Cell;
use std::time::Instant;

use redactor::{
    KEY_SIZE, RedactionCancelled, RedactionState, Redactor, check_key, is_already_redacted,
};
use regex::Regex;

/// `TEST_KEY = bytes(range(32))`.
fn test_key() -> Vec<u8> {
    (0..32).collect()
}

fn redact(text: &str) -> String {
    Redactor::new(test_key()).redact(text).text
}

fn assert_matches(pattern: &str, text: &str) {
    assert!(
        Regex::new(pattern)
            .expect("pattern compiles")
            .is_match(text),
        "{pattern:?} does not match {text:?}"
    );
}

/// `LogRedactorTests.test_must_redact_vectors`.
#[test]
fn must_redact_vectors() {
    let text = [
        "Config: added 'ssid' value 'HomeNet'",
        "wlan0: Trying to associate with SSID 'HomeNet'",
        "Connected to wireless network \"HomeNet\"",
        "SME: Trying to authenticate with aa:bb:cc:dd:ee:ff (SSID='HomeNet' freq=2437 MHz)",
        "policy: set 'HomeNet' (wlan0) as default",
        "selected access point 'HomeNet'",
        "Activation: starting connection 'HomeNet'",
        "ssid: HomeNet",
        "set-hw-addr: set MAC address to AA:BB:CC:DD:EE:FF (scanning)",
        "Registering new address record for fe80::1122:3344:5566:7788 on wlan0.*.",
        "CONF:   root_password: s3cr3tvalue",
        "CONF:   APPassword: '123456789012'",
        "{\"root_password\": \"abc123\", \"serial\": \"332233\"}",
        "user renamed network to HomeNet today",
        "systemd-timedated - Changed time zone to 'America/Mexico_City' (CST).",
        "timezone_manager INFO new system time zone: Europe/Berlin",
        "Time zone: America/Argentina/Buenos_Aires (ART, -0300)",
        "Etc/GMT-6 carries an offset",
    ]
    .join("\n");

    let output = redact(&text);

    for sensitive in [
        "HomeNet",
        "aa:bb:cc:dd:ee:ff",
        "AA:BB:CC:DD:EE:FF",
        "fe80::1122:3344:5566:7788",
        "s3cr3tvalue",
        "123456789012",
        "abc123",
        "Mexico_City",
        "Berlin",
        "Buenos_Aires",
        "Argentina",
        "(CST)",
        "ART",
        "-0300",
        "GMT-6",
    ] {
        assert!(!output.contains(sensitive), "{sensitive:?} survived");
    }
    assert!(output.contains("root_password: [REDACTED]"));
    assert!(output.contains("APPassword: '[REDACTED]'"));
    assert!(output.contains("\"root_password\": \"[REDACTED]\""));
    assert_matches(r"SSID='\[SSID_[0-9a-f]{8}\]'", &output);
    assert_matches(r"with \[MAC_[0-9a-f]{8}\]", &output);
    assert_matches(r"for \[IPV6_[0-9a-f]{8}\]", &output);
    assert!(output.contains("to 'America/*****' (*****)."));
    assert!(output.contains("new system time zone: Europe/*****"));
    assert!(output.contains("Time zone: America/***** (*****)"));
    assert!(output.contains("Etc/***** carries an offset"));
}

/// `LogRedactorTests.test_network_manager_credential_form_is_redacted`.
#[test]
fn network_manager_credential_form_is_redacted() {
    assert_eq!(
        redact("Config: added 'psk' value '<hidden>'"),
        "Config: added 'psk' value '[REDACTED]'"
    );
}

/// `LogRedactorTests.test_credentials_run_before_identifier_rules`.
#[test]
fn credentials_run_before_identifier_rules() {
    assert_eq!(
        redact("password: aa:bb:cc:dd:ee:ff"),
        "password: [REDACTED]"
    );
}

/// `LogRedactorTests.test_known_wifis_keys_are_redacted_and_attributes_are_not_ssids`.
#[test]
fn known_wifis_keys_are_redacted_and_attributes_are_not_ssids() {
    let text = "config DEBUG CONF:   wifi:\r\n\
                config DEBUG CONF:     KnownWifis:\r\n\
                config DEBUG CONF:       Some Network 5G:\r\n\
                config DEBUG CONF:         password: example-pass\r\n\
                config DEBUG CONF:         last_used: yesterday";

    let output = redact(text);

    assert!(!output.contains("Some Network 5G"));
    assert_matches(r"CONF:       \[SSID_[0-9a-f]{8}\]:\r\n", &output);
    assert!(output.contains("password: [REDACTED]"));
    assert!(output.contains("last_used: yesterday"));
    assert!(!output.ends_with(['\n', '\r']));
    assert_eq!(redact(&output), output);
}

/// `LogRedactorTests.test_leading_compressed_ipv6_is_redacted`.
#[test]
fn leading_compressed_ipv6_is_redacted() {
    let output = redact("peer ::abcd:1234 connected; loopback ::1; unspecified ::");

    assert!(!output.contains("::abcd:1234"));
    assert_matches(r"peer \[IPV6_[0-9a-f]{8}\] connected", &output);
    assert!(output.contains("loopback ::1"));
    assert!(output.contains("unspecified ::"));
}

/// `LogRedactorTests.test_allowlisted_addresses_are_unchanged`.
#[test]
fn allowlisted_addresses_are_unchanged() {
    let text = "ff:ff:ff:ff:ff:ff 00:00:00:00:00:00 33:33:00:00:00:01 01:00:5e:00:00:fb ::1 ::";
    assert_eq!(redact(text), text);
}

/// `LogRedactorTests.test_diagnostic_preservation_vectors`.
#[test]
fn diagnostic_preservation_vectors() {
    for vector in [
        "Firmware: BCM4339/2 version 6.37.39.141 (73212ff CY)",
        "tornado.access INFO 304 GET /api/v1/settings/ (10.10.0.79) 4.10ms",
        "Serial_number: 332233 Batch_number: 0009",
        "Config: added 'key_mgmt' value 'WPA-PSK WPA-PSK-SHA256 FT-PSK'",
        "ssh_manager INFO Password already set: True",
        "pam_unix(chpasswd:chauthtok): password changed for root",
        "Listening on gpg-agent.socket - passphrase cache.",
        "device (lo): Activation: starting connection 'lo'",
        "Joining mDNS multicast group on interface lo.IPv6 with address ::1.",
        "2026-07-28 01:42:05.388326+00:00",
        "systemd-timedated - Changed time zone to 'Etc/UTC' (UTC).",
        "CONF:   time_zone: Etc/UTC",
        "kernel: Out of memory (OOM) killer invoked",
        "kernel: page allocation failure (NAND) on mmcblk0",
        "kernel: enabled Kernel Page Table Isolation (KPTI)",
        "ESP sensor bundle: -2404 -9815 -9252 -8911",
    ] {
        assert_eq!(redact(vector), vector);
    }
}

/// `LogRedactorTests.test_output_is_idempotent_and_preserves_line_count`.
#[test]
fn output_is_idempotent_and_preserves_line_count() {
    let text = "Config: added 'ssid' value 'HomeNet'\n\
                Associated with aa:bb:cc:dd:ee:ff\n\
                root_password: synthetic-password\n";
    let once = redact(text);
    let twice = redact(&once);

    assert_eq!(twice, once);
    assert_eq!(text.lines().count(), once.lines().count());
}

/// `LogRedactorTests.test_json_quotes_and_structure_are_preserved`.
#[test]
fn json_quotes_and_structure_are_preserved() {
    let output = redact("{\"root_password\": \"abc123\", \"serial\": \"332233\"}");
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(
        parsed,
        serde_json::json!({"root_password": "[REDACTED]", "serial": "332233"})
    );
}

/// `LogRedactorTests.test_short_ssid_only_gets_anchored_coverage`.
#[test]
fn short_ssid_only_gets_anchored_coverage() {
    let output = redact("SSID='up'\nnetwork up is still visible");
    assert_matches(r"SSID='\[SSID_[0-9a-f]{8}\]'", &output);
    assert!(output.contains("network up is still visible"));
}

/// `LogRedactorTests.test_key_is_persistent_and_private` and
/// `test_invalid_existing_key_fails_closed`: creating, persisting and
/// chmodding the key file is IO and belongs to the backend's `load_key`; the
/// crate keeps the size check that makes a bad key file fail closed.
#[test]
fn key_file_contents_must_be_exactly_32_bytes() {
    assert_eq!(KEY_SIZE, 32);
    assert!(check_key("/root/.redaction_key", &[0u8; 32]).is_ok());
    let error = check_key("/tmp/x/.redaction_key", b"too-short").unwrap_err();
    assert_eq!(error.python_exception(), "ValueError");
    assert_eq!(
        error.to_string(),
        "redaction key at '/tmp/x/.redaction_key' must be exactly 32 bytes"
    );
}

/// Linux peak RSS in KiB (`VmHWM`), the figure `ru_maxrss` reports.
fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// `LogRedactorTests.test_ten_megabyte_runtime_and_memory_budget`: spec
/// section 10.1, under 5 s and 128 MiB for a 10 MiB log.
#[test]
fn ten_megabyte_runtime_and_memory_budget() {
    let line = "2026-07-28 01:42:05.388326+00:00 tornado.access INFO \
                304 GET /api/v1/settings/ (10.10.0.79) 4.10ms\n";
    let text = line.repeat(10 * 1024 * 1024 / line.len() + 1);

    let started = Instant::now();
    let output = redact(&text);
    let elapsed = started.elapsed();

    assert_eq!(output, text);
    assert!(elapsed.as_secs_f64() < 5.0, "took {elapsed:?}");
    if let Some(peak) = peak_rss_kib() {
        assert!(peak < 128 * 1024, "peak RSS {peak} KiB");
    }
}

const CANCEL_TEXT: &str = "config DEBUG CONF:   wifi:\r\n\
    config DEBUG CONF:     KnownWifis:\r\n\
    config DEBUG CONF:       Some Network 5G:\r\n\
    config DEBUG CONF:         password: example-pass\r\n\
    config DEBUG CONF:         last_used: yesterday\n\
    Config: added 'ssid' value 'HomeNet'\n\
    wlan0: Trying to associate with SSID 'HomeNet'\n\
    Associated with aa:bb:cc:dd:ee:ff\n";

/// `RedactionCancellationTests.test_output_unchanged_when_predicate_never_signals`.
#[test]
fn output_unchanged_when_predicate_never_signals() {
    let redactor = Redactor::new(test_key());
    let baseline = redactor.redact(CANCEL_TEXT);
    let never_signals = redactor
        .redact_cancellable(CANCEL_TEXT, None, &mut || false)
        .unwrap();
    assert_eq!(baseline, never_signals);
}

/// A predicate answering from a script, counting its calls (`mock.Mock`
/// with `side_effect`).
fn scripted(answers: &[bool], calls: &Cell<usize>) -> impl FnMut() -> bool {
    let answers = answers.to_vec();
    move || {
        let index = calls.get();
        calls.set(index + 1);
        answers[index]
    }
}

/// `RedactionCancellationTests.test_predicate_signalling_during_pass_one_raises_and_yields_nothing`:
/// checks at line index 0, 500 and 1000; the third fires.
#[test]
fn predicate_signalling_during_pass_one_raises_and_yields_nothing() {
    let text: String = (0..1200).map(|i| format!("line {i}\n")).collect();
    let calls = Cell::new(0);
    let mut cancelled = scripted(&[false, false, true], &calls);

    let result = Redactor::new(test_key()).redact_cancellable(&text, None, &mut cancelled);

    assert_eq!(result, Err(RedactionCancelled));
    assert_eq!(calls.get(), 3);
}

/// `RedactionCancellationTests.test_predicate_signalling_during_pass_two_raises_and_yields_nothing`.
#[test]
fn predicate_signalling_during_pass_two_raises_and_yields_nothing() {
    let calls = Cell::new(0);
    let mut cancelled = scripted(&[false, true], &calls);

    let result = Redactor::new(test_key()).redact_cancellable(
        "Config: added 'ssid' value 'HomeNet'\n",
        None,
        &mut cancelled,
    );

    assert_eq!(result, Err(RedactionCancelled));
    assert_eq!(calls.get(), 2);
}

/// Not in the Python suite: KnownWifis tracking persists through a caller's
/// state, as `test_yaml_block_spanning_records_is_tracked` relies on.
#[test]
fn state_carries_known_wifis_block_across_calls() {
    let redactor = Redactor::new(test_key());
    let mut state = RedactionState::default();
    for line in ["CONF: wifi:", "CONF:   KnownWifis:"] {
        redactor.redact_with_state(line, &mut state);
    }
    let out = redactor.redact_with_state("CONF:     HomeNet:", &mut state);
    assert_eq!(
        out.text,
        format!("CONF:     {}:", redactor.pseudonym("SSID", "HomeNet"))
    );
    assert_eq!(out.learned, ["HomeNet"]);
    assert!(is_already_redacted(&redactor.pseudonym("SSID", "HomeNet")));
}

/// Not in the Python suite: PARITY. Two learned SSIDs of equal length whose
/// occurrences overlap are swept in an order Python leaves to set iteration
/// (it varies with PYTHONHASHSEED: either token can win). Rust sweeps them in
/// the order they were learned, which is one of Python's outcomes.
#[test]
fn equal_length_overlapping_ssids_sweep_in_learn_order() {
    let redactor = Redactor::new([0u8; 32]);
    let out = redactor.redact("SSID='aa b'\nSSID='b cc'\naa b cc\n");
    assert_eq!(out.text.lines().nth(2), Some("[SSID_ce8f59cd] cc"));
}
