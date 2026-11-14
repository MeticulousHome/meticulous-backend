//! Port of `tests/test_log_redaction_filter.py`.
//!
//! The Python tests drive a real `logging.Logger`; here a record goes through
//! [`LogRedactor::filter`] exactly once, the way the backend's logging layer
//! calls it, and the assertions read what a handler would receive.

use std::time::{Duration, Instant};

use redactor::{
    LogRecord, LogRedactor, REFRESH_INTERVAL, RedactionFailure, SweepValues, pseudonym,
};
use regex::Regex;

/// `TEST_KEY = bytes(range(32))`.
fn test_key() -> Vec<u8> {
    (0..32).collect()
}

const LOGGER: &str = "redaction_test_1";

struct Harness {
    filter: LogRedactor,
    now: Instant,
    records: Vec<LogRecord>,
}

impl Harness {
    fn new() -> Harness {
        Harness::with_filter(LogRedactor::new(test_key()))
    }

    fn with_filter(filter: LogRedactor) -> Harness {
        Harness {
            filter,
            now: Instant::now(),
            records: Vec::new(),
        }
    }

    fn seed(&mut self, ssids: &[&str], credentials: &[&str]) {
        self.filter.set_values(
            ssids.iter().map(|s| (*s).to_owned()).collect(),
            credentials.iter().map(|s| (*s).to_owned()).collect(),
        );
    }

    fn handle(&mut self, mut record: LogRecord) {
        self.filter.filter(&mut record, self.now);
        self.records.push(record);
    }

    fn info(&mut self, msg: &str) {
        self.handle(LogRecord::new(LOGGER, msg));
    }

    fn last(&self) -> &str {
        &self.records.last().expect("a record was logged").msg
    }

    fn last_record(&self) -> &LogRecord {
        self.records.last().expect("a record was logged")
    }
}

/// `logging.Formatter().format(record)` without the time: the message, then
/// the exception text (rendered from `exc_info` if absent), then the stack.
fn format(record: &LogRecord) -> String {
    let mut out = record.msg.clone();
    let exc_text = record
        .exc_text
        .clone()
        .filter(|t| !t.is_empty())
        .or_else(|| record.exc_info.clone());
    for extra in [exc_text, record.stack_info.clone()].into_iter().flatten() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&extra);
    }
    out
}

fn assert_matches(pattern: &str, text: &str) {
    assert!(
        Regex::new(pattern)
            .expect("pattern compiles")
            .is_match(text),
        "{pattern:?} does not match {text:?}"
    );
}

fn broken_provider() -> Result<SweepValues, RedactionFailure> {
    Err(RedactionFailure::new("RuntimeError"))
}

/// `ShapeRuleTests.test_credential_key_value_is_redacted`.
#[test]
fn credential_key_value_is_redacted() {
    let mut h = Harness::new();
    h.info("CONF:   root_password: s3cr3tvalue");
    assert_eq!(h.last(), "CONF:   root_password: [REDACTED]");
}

/// `ShapeRuleTests.test_mac_and_ipv6_become_tokens`.
#[test]
fn mac_and_ipv6_become_tokens() {
    let mut h = Harness::new();
    h.info("authenticate with aa:bb:cc:dd:ee:ff via fe80::1122:3344:5566:7788");
    assert!(!h.last().contains("aa:bb:cc:dd:ee:ff"));
    assert!(!h.last().contains("fe80::1122:3344:5566:7788"));
    assert_matches(
        r"with \[MAC_[0-9a-f]{8}\] via \[IPV6_[0-9a-f]{8}\]",
        h.last(),
    );
}

/// `ShapeRuleTests.test_anchored_ssid_is_redacted_without_seeding`.
#[test]
fn anchored_ssid_is_redacted_without_seeding() {
    let mut h = Harness::new();
    h.info("Trying to associate with SSID 'HomeNet'");
    assert!(!h.last().contains("HomeNet"));
}

/// `ShapeRuleTests.test_timezone_is_coarsened`.
#[test]
fn timezone_is_coarsened() {
    let mut h = Harness::new();
    h.info("Changed time zone to 'America/Mexico_City' (CST).");
    assert_eq!(h.last(), "Changed time zone to 'America/*****' (*****).");
}

/// `ConfigSeededSweepTests.test_bare_known_ssid_is_redacted`: the case no
/// shape rule can reach.
#[test]
fn bare_known_ssid_is_redacted() {
    let mut h = Harness::new();
    h.seed(&["HomeNet"], &[]);
    h.info("Found known WIFI HomeNet. Connecting");
    assert_eq!(
        h.last(),
        format!(
            "Found known WIFI {}. Connecting",
            pseudonym("SSID", "HomeNet", &test_key())
        )
    );
}

/// `ConfigSeededSweepTests.test_bare_credential_is_destroyed_not_pseudonymised`.
#[test]
fn bare_credential_is_destroyed_not_pseudonymised() {
    let mut h = Harness::new();
    h.seed(&[], &["sup3rs3cret"]);
    h.info("nmcli failed: connection needs sup3rs3cret to activate");
    assert!(!h.last().contains("sup3rs3cret"));
    assert!(h.last().contains("[REDACTED]"));
}

/// `ConfigSeededSweepTests.test_credential_wins_when_a_value_is_both`.
#[test]
fn credential_wins_when_a_value_is_both() {
    let mut h = Harness::new();
    h.seed(&["sharedvalue"], &["sharedvalue"]);
    h.info("saw sharedvalue in the wild");
    assert_eq!(h.last(), "saw [REDACTED] in the wild");
}

/// `ConfigSeededSweepTests.test_longest_value_wins_at_the_same_position`.
#[test]
fn longest_value_wins_at_the_same_position() {
    let mut h = Harness::new();
    h.seed(&["Coffee", "Coffee Bar"], &[]);
    h.info("joined Coffee Bar now");
    assert_eq!(
        h.last(),
        format!(
            "joined {} now",
            pseudonym("SSID", "Coffee Bar", &test_key())
        )
    );
}

/// `ConfigSeededSweepTests.test_short_values_are_not_swept`.
#[test]
fn short_values_are_not_swept() {
    let mut h = Harness::new();
    h.seed(&["up"], &[]);
    h.info("link is up and running");
    assert_eq!(h.last(), "link is up and running");
}

/// `ConfigSeededSweepTests.test_word_boundaries_are_respected`.
#[test]
fn word_boundaries_are_respected() {
    let mut h = Harness::new();
    h.seed(&["Home"], &[]);
    h.info("Homeless value and Home itself");
    assert!(h.last().contains("Homeless"));
    assert!(!h.last().contains(" Home itself"));
}

/// Not in the Python suite: PARITY. The Python sweep pattern is
/// `(?<![\w-])a|b|c(?![\w-])` without a group, so with several values only
/// the longest checks the character before it and only the shortest the one
/// after it. Expected values from the Python filter.
#[test]
fn sweep_boundaries_bind_to_the_outer_alternatives_only() {
    let mut h = Harness::new();
    h.seed(&["Home", "HomeNet", "Office"], &[]);
    h.info("xHomeNetx xOfficex Homeless xHome");
    let token = |v: &str| pseudonym("SSID", v, &test_key());
    assert_eq!(
        h.last(),
        format!(
            "xHomeNetx x{}x Homeless x{}",
            token("Office"),
            token("Home")
        )
    );
}

/// `ConfigSeededSweepTests.test_invalidate_picks_up_a_new_network`.
#[test]
fn invalidate_picks_up_a_new_network() {
    use std::sync::{Arc, Mutex};

    let networks = Arc::new(Mutex::new(vec!["FirstNet".to_owned()]));
    let mut h = Harness::new();
    let shared = Arc::clone(&networks);
    h.filter.set_value_provider(move || {
        Ok(SweepValues {
            ssids: shared.lock().unwrap().clone(),
            credentials: Vec::new(),
        })
    });
    h.info("saw FirstNet and SecondNet");
    assert!(!h.last().contains("FirstNet"));
    assert!(h.last().contains("SecondNet"));

    networks.lock().unwrap().push("SecondNet".to_owned());
    h.filter.invalidate();
    h.info("saw FirstNet and SecondNet");
    assert!(!h.last().contains("SecondNet"));
}

/// `ConfigSeededSweepTests.test_sweep_refreshes_without_an_explicit_invalidate`:
/// the cache has a ceiling, so a missed invalidate cannot persist.
#[test]
fn sweep_refreshes_without_an_explicit_invalidate() {
    use std::sync::{Arc, Mutex};

    let networks: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut h = Harness::new();
    let shared = Arc::clone(&networks);
    h.filter.set_value_provider(move || {
        Ok(SweepValues {
            ssids: shared.lock().unwrap().clone(),
            credentials: Vec::new(),
        })
    });
    h.info("nothing yet");

    networks.lock().unwrap().push("LateNet".to_owned());
    h.info("saw LateNet");
    // Within the interval the stale sweep is still in use.
    assert!(h.last().contains("LateNet"));
    // Reach past the refresh interval rather than sleeping through it.
    h.now += REFRESH_INTERVAL + Duration::from_millis(1);
    h.info("saw LateNet");
    assert!(!h.last().contains("LateNet"));
}

/// `ConfigSeededSweepTests.test_provider_failure_does_not_leak_or_raise`.
#[test]
fn provider_failure_does_not_leak_or_raise() {
    let mut h = Harness::new();
    h.filter.set_value_provider(broken_provider);
    h.info("Found known WIFI HomeNet. Connecting");
    assert!(!h.last().contains("HomeNet"));
    assert!(h.last().contains("redaction failed"));
}

/// `ConfigSeededSweepTests.test_provider_failure_keeps_failing_instead_of_degrading`:
/// a broken provider must not fall back to the shape rules alone.
#[test]
fn provider_failure_keeps_failing_instead_of_degrading() {
    let mut h = Harness::new();
    h.filter.set_value_provider(broken_provider);
    for _ in 0..3 {
        h.info("Found known WIFI HomeNet. Connecting");
    }
    for record in &h.records {
        assert!(!record.msg.contains("HomeNet"));
        assert!(record.msg.contains("redaction failed"));
    }
}

/// `ConfigSeededSweepTests.test_recovery_after_a_provider_failure`.
#[test]
fn recovery_after_a_provider_failure() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let broken = Arc::new(AtomicBool::new(true));
    let mut h = Harness::new();
    let flag = Arc::clone(&broken);
    h.filter.set_value_provider(move || {
        if flag.load(Ordering::SeqCst) {
            return Err(RedactionFailure::new("RuntimeError"));
        }
        Ok(SweepValues {
            ssids: vec!["HomeNet".to_owned()],
            credentials: Vec::new(),
        })
    });
    h.info("Found known WIFI HomeNet. Connecting");
    assert!(h.last().contains("redaction failed"));

    broken.store(false, Ordering::SeqCst);
    h.info("Found known WIFI HomeNet. Connecting");
    assert!(!h.last().contains("HomeNet"));
    assert!(!h.last().contains("redaction failed"));
}

/// `RecordShapeTests.test_percent_args_are_formatted_then_redacted`: the
/// caller formats `%` arguments before the filter sees the message, so there
/// are no `args` left for a handler to re-apply.
#[test]
fn percent_args_are_formatted_then_redacted() {
    let mut h = Harness::new();
    h.seed(&["HomeNet"], &[]);
    h.info(&format!("connecting to {} on {}", "HomeNet", "wlan0"));
    assert!(!h.last().contains("HomeNet"));
    assert!(h.last().contains("on wlan0"));
}

/// `RecordShapeTests.test_non_string_msg_survives`: `logger.info(dict)` logs
/// `str(dict)`.
#[test]
fn non_string_msg_survives() {
    let mut h = Harness::new();
    h.info("{'root_password': 's3cr3tvalue'}");
    assert!(!h.last().contains("s3cr3tvalue"));
}

/// `RecordShapeTests.test_multiline_record_is_redacted_line_by_line`.
#[test]
fn multiline_record_is_redacted_line_by_line() {
    let mut h = Harness::new();
    h.info("first: aa:bb:cc:dd:ee:ff\nsecond: root_password: s3cr3tvalue");
    assert!(!h.last().contains("aa:bb:cc:dd:ee:ff"));
    assert!(!h.last().contains("s3cr3tvalue"));
    assert_eq!(h.last().lines().count(), 2);
}

/// `RecordShapeTests.test_traceback_text_is_redacted`.
#[test]
fn traceback_text_is_redacted() {
    let mut h = Harness::new();
    h.seed(&["HomeNet"], &[]);
    let mut record = LogRecord::new(LOGGER, "wifi failed");
    record.exc_info = Some(
        "Traceback (most recent call last):\n  \
         File \"/app/tests/test_log_redaction_filter.py\", line 223, in test_traceback_text_is_redacted\n    \
         raise ValueError(\"could not reach HomeNet at aa:bb:cc:dd:ee:ff\")\n\
         ValueError: could not reach HomeNet at aa:bb:cc:dd:ee:ff\n"
            .to_owned(),
    );
    h.handle(record);

    let record = h.last_record();
    let rendered = format(record);
    assert!(!rendered.contains("HomeNet"), "{rendered}");
    assert!(!rendered.contains("aa:bb:cc:dd:ee:ff"));
    assert!(rendered.contains("ValueError"));
    // exc_info stays for the structured Sentry exception; exc_text is what
    // every formatter renders.
    assert!(record.exc_info.is_some());
    assert!(!record.exc_text.as_deref().unwrap().ends_with('\n'));
}

/// `RecordShapeTests.test_stack_info_is_redacted`.
#[test]
fn stack_info_is_redacted() {
    let mut h = Harness::new();
    h.seed(&[], &["s3cr3tvalue"]);
    let mut record = LogRecord::new(LOGGER, "saving s3cr3tvalue");
    record.stack_info = Some(
        "Stack (most recent call last):\n  File \"/app/x.py\", line 1, in save\n    \
         save('s3cr3tvalue')"
            .to_owned(),
    );
    h.handle(record);
    let record = h.last_record();
    assert!(
        !record
            .stack_info
            .as_deref()
            .unwrap()
            .contains("s3cr3tvalue")
    );
    assert!(!format(record).contains("s3cr3tvalue"));
}

/// `RecordShapeTests.test_output_is_idempotent`.
#[test]
fn output_is_idempotent() {
    let mut h = Harness::new();
    h.seed(&["HomeNet"], &[]);
    h.info("Found known WIFI HomeNet. Connecting");
    let once = h.last().to_owned();
    h.info(&once);
    assert_eq!(h.last(), once);
}

/// `RecordShapeTests.test_an_already_redacted_record_is_not_reprocessed`:
/// `redact_ssid()` output survives the filter unchanged.
#[test]
fn an_already_redacted_record_is_not_reprocessed() {
    let mut h = Harness::new();
    let token = pseudonym("SSID", "HomeNet", &test_key());
    h.info(&format!("Connecting to wifi: {token}"));
    assert_eq!(h.last(), format!("Connecting to wifi: {token}"));
}

/// Not in the Python suite: the `_meticulous_redacted` mark. A record that
/// reaches the filter twice (logger filter and root handler filter) is only
/// worked once.
#[test]
fn a_marked_record_is_only_worked_once() {
    let mut h = Harness::new();
    let mut record = LogRecord::new(LOGGER, "root_password: s3cr3tvalue");
    record.redacted = true;
    h.handle(record);
    assert_eq!(h.last(), "root_password: s3cr3tvalue");

    let mut record = LogRecord::new(LOGGER, "root_password: s3cr3tvalue");
    h.filter.filter(&mut record, h.now);
    assert!(record.redacted);
    // A second pass over an already redacted record would be a no-op anyway,
    // but the mark is what stops it.
    record.msg.push_str(" root_password: again");
    h.filter.filter(&mut record, h.now);
    assert!(record.msg.ends_with("root_password: again"));
}

/// `KnownWifisContinuationTests.test_yaml_block_spanning_records_is_tracked`:
/// the config dump is one record per line; records from another logger are
/// interleaved.
#[test]
fn yaml_block_spanning_records_is_tracked() {
    let mut h = Harness::new();
    h.handle(LogRecord::new(LOGGER, "CONF: wifi:"));
    h.handle(LogRecord::new(
        "redaction_test_interleaved",
        "an unrelated record arrives mid-dump",
    ));
    h.handle(LogRecord::new(LOGGER, "CONF:   KnownWifis:"));
    h.handle(LogRecord::new(
        "redaction_test_interleaved",
        "and another one",
    ));
    h.handle(LogRecord::new(LOGGER, "CONF:     HomeNet:"));
    h.handle(LogRecord::new(LOGGER, "CONF:       password: s3cr3tvalue"));
    h.handle(LogRecord::new(LOGGER, "CONF:     OfficeNet:"));

    let joined = h
        .records
        .iter()
        .map(|r| r.msg.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!joined.contains("HomeNet"));
    assert!(!joined.contains("OfficeNet"));
    assert!(!joined.contains("s3cr3tvalue"));
    assert!(joined.contains("password: [REDACTED]"));
}

fn unreadable_key() -> Result<Vec<u8>, RedactionFailure> {
    // load_key() on "<tmp>/nope/key" cannot create the file: FileNotFoundError.
    Err(RedactionFailure::new("FileNotFoundError"))
}

/// `FailureModeTests.test_unreadable_key_yields_a_placeholder_and_never_raises`.
#[test]
fn unreadable_key_yields_a_placeholder_and_never_raises() {
    let mut h = Harness::with_filter(LogRedactor::new(unreadable_key()));
    h.info("Connecting to wifi: HomeNet with aa:bb:cc:dd:ee:ff");

    assert!(!h.last().contains("HomeNet"));
    assert!(!h.last().contains("aa:bb:cc:dd:ee:ff"));
    assert!(h.last().contains("redaction failed"));
    assert!(h.last().contains(LOGGER));
    assert_eq!(
        h.last(),
        "<redaction failed for a 'redaction_test_1' record: FileNotFoundError>"
    );
}

/// `FailureModeTests.test_failure_drops_traceback_text`.
#[test]
fn failure_drops_traceback_text() {
    let mut h = Harness::with_filter(LogRedactor::new(unreadable_key()));
    let mut record = LogRecord::new(LOGGER, "wifi failed");
    record.exc_info = Some(
        "Traceback (most recent call last):\nValueError: could not reach HomeNet\n".to_owned(),
    );
    h.handle(record);

    let record = h.last_record();
    assert!(record.exc_info.is_none());
    assert!(!format(record).contains("HomeNet"));
}

/// `OrderingTests.test_record_is_clean_before_handlers_run`: the record is
/// clean by the time anything downstream (handlers, Sentry) reads it.
#[test]
fn record_is_clean_before_handlers_run() {
    let mut h = Harness::new();
    h.seed(&["HomeNet"], &[]);
    h.info("Found known WIFI HomeNet. Connecting");
    assert!(!h.last_record().msg.contains("HomeNet"));
}

/// `ThroughputTests.test_throughput_budget`: DEBUG logging is hot on this
/// device, so a future rule must not quietly make redaction the bottleneck.
/// Same budget as the Python suite: more than 10,000 records per second over
/// 20,000 records.
#[test]
fn throughput_budget() {
    let mut h = Harness::new();
    let ssids: Vec<String> = (0..40).map(|i| format!("Network{i}")).collect();
    let credentials: Vec<String> = (0..10).map(|i| format!("secret{i}")).collect();
    h.filter.set_values(ssids, credentials);
    let message = "tornado.access INFO 304 GET /api/v1/settings/ (10.10.0.79) 4.10ms \
                   state=idle sensor=1234 profile=default";
    // Warm the sweep so compilation is not counted.
    h.info(message);

    let iterations = 20_000;
    let started = Instant::now();
    for _ in 0..iterations {
        let mut record = LogRecord::new(LOGGER, message);
        h.filter.filter(&mut record, h.now);
        assert_eq!(record.msg, message);
    }
    let rate = f64::from(iterations) / started.elapsed().as_secs_f64();
    assert!(
        rate > 10_000.0,
        "redaction throughput fell to {rate:.0} records/s"
    );
}
