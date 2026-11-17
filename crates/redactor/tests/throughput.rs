//! Throughput sanity checks (no criterion). The Python suite guards the
//! filter's records per second (`test_throughput_budget`, ported in
//! `log_redaction_filter.rs`) and the 10 MiB whole-file budget (ported in
//! `log_redactor.rs`); this adds a mixed log where most rules fire, so a slow
//! path in a rule that only runs on a match cannot hide behind benign text.

use std::time::Instant;

use redactor::{LogRecord, LogRedactor, Redactor};

const MIXED: &[&str] = &[
    "2026-07-28 01:42:05.388326+00:00 tornado.access INFO 304 GET /api/v1/settings/ (10.10.0.79) 4.10ms",
    "Jul 28 01:42:05 meticulous332233 wpa_supplicant[790]: wlan0: SME: Trying to authenticate with aa:bb:cc:dd:ee:ff (SSID='HomeNet' freq=2437 MHz)",
    "Jul 28 01:42:05 meticulous332233 avahi-daemon[700]: Registering new address record for fe80::1122:3344:5566:7788 on wlan0.*.",
    "Jul 28 01:42:05 meticulous332233 NetworkManager[812]: <info>  [1722130925.3883] policy: set 'HomeNet' (wlan0) as default",
    "config DEBUG CONF:   root_password: s3cr3tvalue",
    "Jul 28 01:42:05 meticulous332233 systemd-timedated[1]: Changed time zone to 'America/Mexico_City' (CST).",
    "esp DEBUG Sensors: p=8.91 f=2.10 w=36.2 t=93.4 m=-2404 -9815 -9252 -8911 state=brewing profile=default",
    "wifi INFO user renamed network to HomeNet today",
];

/// Floors for an optimized build, where these run at roughly 75 MiB/s and
/// 700k records/s (Python: 4.4 MiB/s on the same log). An unoptimized test
/// build runs the `regex` crate without optimizations too, about 20 times
/// slower, so there the floors only catch an algorithmic regression.
fn floor(release: f64, debug: f64) -> f64 {
    if cfg!(debug_assertions) {
        debug
    } else {
        release
    }
}

/// Redacts a ~4 MiB mixed log; in an optimized build it must beat the spec's
/// whole-file budget (section 10.1: 10 MiB in under 5 s) tenfold.
#[test]
fn whole_file_mixed_log_throughput() {
    let mut text = String::new();
    while text.len() < 4 * 1024 * 1024 {
        for line in MIXED {
            text.push_str(line);
            text.push('\n');
        }
    }
    let redactor = Redactor::new([7u8; 32]);
    let started = Instant::now();
    let out = redactor.redact(&text);
    let seconds = started.elapsed().as_secs_f64();
    let mib_per_s = text.len() as f64 / (1024.0 * 1024.0) / seconds;
    println!("whole file: {mib_per_s:.1} MiB/s");
    assert!(!out.text.contains("HomeNet") && !out.text.contains("s3cr3tvalue"));
    let floor = floor(20.0, 1.0);
    assert!(
        mib_per_s > floor,
        "whole-file redaction fell to {mib_per_s:.2} MiB/s (floor {floor})"
    );
}

/// One record at a time through the filter, with a seeded sweep, on lines
/// where rules fire.
#[test]
fn per_record_mixed_throughput() {
    let mut filter = LogRedactor::new(vec![7u8; 32]);
    filter.set_values(
        (0..40).map(|i| format!("Network{i}")).collect(),
        (0..10).map(|i| format!("secret{i}")).collect(),
    );
    let now = Instant::now();
    let iterations = 20_000;
    let started = Instant::now();
    for i in 0..iterations {
        let mut record = LogRecord::new("wifi", MIXED[i % MIXED.len()]);
        filter.filter(&mut record, now);
    }
    let rate = iterations as f64 / started.elapsed().as_secs_f64();
    println!("per record: {rate:.0} records/s");
    // The Python suite's budget is 10,000 records/s for benign records.
    let floor = floor(100_000.0, 5_000.0);
    assert!(
        rate > floor,
        "redaction throughput fell to {rate:.0} records/s (floor {floor})"
    );
}
