//! Port of `tests/test_esp_observability.py`: the ESP observability state
//! machine, case by case (parametrized cases become loops).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use esp::observability::{EspCommunicationPhase as Phase, EspDiagnostic, EspObservability};
use serde_json::Value;

const BOOT: &str = "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)";

fn titles(events: &[EspDiagnostic]) -> Vec<&str> {
    events.iter().map(|e| e.title.as_str()).collect()
}

fn capture_guru(monitor: &mut EspObservability, reason: &str, now: f64) {
    monitor.observe_raw_line(
        &format!("Guru Meditation Error: Core 1 panic'ed ({reason})."),
        now,
    );
    monitor.observe_raw_line("Core 1 register dump:", now + 0.1);
    monitor.observe_raw_line("Backtrace: 0x40381234:0x3fceabcd", now + 0.2);
}

fn with_info(firmware: &str) -> EspObservability {
    let mut monitor = EspObservability::new(0.0);
    monitor.observe_valid_message("ESPInfo", 0.1, Some(firmware));
    monitor
}

fn ctx<'a>(event: &'a EspDiagnostic, key: &str) -> &'a Value {
    event
        .context
        .get(key)
        .unwrap_or_else(|| panic!("no context {key}"))
}

fn ctx_str<'a>(event: &'a EspDiagnostic, key: &str) -> &'a str {
    ctx(event, key).as_str().unwrap()
}

fn tag<'a>(event: &'a EspDiagnostic, key: &str) -> &'a str {
    event.tags.get(key).unwrap()
}

/// `test_stale_pre_update_info_does_not_finish_update`.
#[test]
fn stale_pre_update_info_does_not_finish_update() {
    let mut monitor = with_info("old");
    monitor.begin_update(Some("new"), Some("old"), 1.0);
    monitor.finish_flashing(10.0);
    assert!(
        monitor
            .observe_valid_message("ESPInfo", 10.1, Some("old"))
            .is_empty()
    );
    assert_eq!(monitor.phase, Phase::WaitingForBoot);
    assert!(monitor.check_timeouts(12.5).is_empty());
}

/// `test_update_requires_boot_and_exact_expected_esp_info_without_false_timeout`.
#[test]
fn update_requires_boot_and_exact_expected_esp_info() {
    let mut monitor = with_info("old");
    monitor.begin_update(Some("new"), Some("old"), 1.0);
    monitor.finish_flashing(10.0);
    assert!(monitor.observe_raw_line(BOOT, 10.2).is_empty());
    assert_eq!(monitor.phase, Phase::WaitingForProtocol);
    assert!(monitor.check_timeouts(15.0).is_empty());
    assert!(monitor.observe_boot_reason("SW", "3", 12.0).is_empty());
    assert!(monitor.observe_valid_message("Data", 12.1, None).is_empty());
    assert!(
        monitor
            .observe_valid_message("ESPInfo", 12.15, Some("new-dirty"))
            .is_empty()
    );
    assert_eq!(monitor.phase, Phase::WaitingForProtocol);
    assert!(
        monitor
            .observe_valid_message("ESPInfo", 12.2, Some("new"))
            .is_empty()
    );
    assert_eq!(monitor.phase, Phase::Normal);
}

/// `test_update_without_expected_artifact_version_stays_in_recovery`.
#[test]
fn update_without_expected_artifact_version_stays_in_recovery() {
    let mut monitor = EspObservability::new(0.0);
    monitor.begin_update(None, Some("old"), 1.0);
    monitor.finish_flashing(2.0);
    assert!(monitor.observe_raw_line(BOOT, 2.1).is_empty());
    assert!(
        monitor
            .observe_valid_message("ESPInfo", 2.2, Some("new"))
            .is_empty()
    );
    assert_eq!(monitor.phase, Phase::WaitingForProtocol);
    assert_eq!(
        titles(&monitor.check_timeouts(32.1)),
        ["ESP32 did not recover after firmware update"]
    );
}

/// `test_update_recovery_failure_is_specific_and_bounded`.
#[test]
fn update_recovery_failure_is_specific_and_bounded() {
    let mut monitor = EspObservability::new(0.0);
    monitor.begin_update(Some("new"), Some("old"), 1.0);
    monitor.finish_flashing(2.0);
    let events = monitor.check_timeouts(32.1);
    assert_eq!(
        titles(&events),
        ["ESP32 did not recover after firmware update"]
    );
    assert_eq!(tag(&events[0], "recovery_phase"), "waiting_for_boot");
    assert_eq!(ctx(&events[0], "timeout_seconds"), 30.0);
}

/// `test_update_without_current_info_retains_last_known_firmware`.
#[test]
fn update_without_current_info_retains_last_known_firmware() {
    let mut monitor = with_info("1.2.3");
    monitor.begin_update(Some("2.0.0"), None, 1.0);
    monitor.finish_flashing(2.0);
    let event = monitor.check_timeouts(32.1).remove(0);
    assert_eq!(monitor.previous_firmware.as_deref(), Some("1.2.3"));
    assert_eq!(ctx_str(&event, "previous_firmware"), "1.2.3");
}

/// `test_flash_error_is_specific_bounded_and_restores_normal_state`.
#[test]
fn flash_error_is_specific_bounded_and_restores_normal_state() {
    let mut monitor = EspObservability::new(0.0);
    monitor.begin_update(Some("new"), Some("old"), 1.0);
    let event = monitor.fail_flashing(&"x".repeat(2000), "flash", 2.0);
    assert_eq!(event.title, "ESP32 firmware update failed");
    assert_eq!(ctx_str(&event, "detail").chars().count(), 1000);
    assert_eq!(monitor.phase, Phase::Normal);
}

/// `test_expected_api_reset_emits_no_error`.
#[test]
fn expected_api_reset_emits_no_error() {
    let mut monitor = with_info("current");
    monitor.begin_expected_reset(1.0);
    assert!(monitor.observe_raw_line(BOOT, 1.2).is_empty());
    assert!(monitor.observe_boot_reason("SW", "3", 3.0).is_empty());
    assert!(
        monitor
            .observe_valid_message("ESPInfo", 3.1, Some("current"))
            .is_empty()
    );
    assert_eq!(monitor.phase, Phase::Normal);
}

/// `test_startup_attachment_accepts_valid_protocol_without_boot_banner`.
#[test]
fn startup_attachment_accepts_valid_protocol_without_boot_banner() {
    let mut monitor = EspObservability::new(0.0);
    assert_eq!(monitor.phase, Phase::ExpectedReset);
    assert!(monitor.observe_valid_message("Data", 0.1, None).is_empty());
    assert_eq!(monitor.phase, Phase::Normal);
}

/// `test_stale_valid_message_cannot_complete_explicit_reset_before_boot`.
#[test]
fn stale_valid_message_cannot_complete_explicit_reset_before_boot() {
    for message_type in ["Data", "Sensors", "Log", "ESPInfo", "ESPBoot"] {
        let mut monitor = with_info("current");
        monitor.begin_expected_reset(1.0);
        let firmware = (message_type == "ESPInfo").then_some("current");
        assert!(
            monitor
                .observe_valid_message(message_type, 2.0, firmware)
                .is_empty()
        );
        assert_eq!(monitor.phase, Phase::WaitingForBoot, "{message_type}");
        assert_eq!(monitor.recovery_deadline, Some(16.0));
        assert!(monitor.observe_raw_line(BOOT, 2.1).is_empty());
        assert!(monitor.observe_boot_reason("SW", "3", 2.2).is_empty());
        assert!(
            monitor
                .observe_valid_message("ESPInfo", 2.3, Some("current"))
                .is_empty()
        );
        assert_eq!(monitor.phase, Phase::Normal);
    }
}

/// `test_exact_debug_exception_is_structured_and_correlated_with_boot_reason`.
#[test]
fn exact_debug_exception_is_structured_and_correlated_with_boot_reason() {
    let mut monitor = with_info("1.2.3");
    monitor.observe_raw_line(
        "Guru Meditation Error: Core 1 panic'ed (Unhandled debug exception).",
        1.0,
    );
    monitor.observe_raw_line(
        "Debug exception reason: Stack canary watchpoint triggered (Read ADC)",
        1.1,
    );
    monitor.observe_raw_line("Backtrace: 0x40381234:0x3fceabcd", 1.2);
    assert!(monitor.observe_raw_line(BOOT, 1.3).is_empty());
    let events = monitor.observe_boot_reason("PANIC", "4", 3.5);
    assert_eq!(titles(&events), ["ESP32 firmware panic detected"]);
    let event = &events[0];
    assert_eq!(tag(event, "reset_reason"), "PANIC");
    assert_eq!(tag(event, "reset_reason_code"), "4");
    assert_eq!(ctx_str(event, "core"), "1");
    assert_eq!(ctx_str(event, "panic_reason"), "Unhandled debug exception");
    assert_eq!(
        ctx_str(event, "exception_reason"),
        "Stack canary watchpoint triggered"
    );
    assert_eq!(ctx_str(event, "panic_location"), "Read ADC");
    assert_eq!(ctx_str(event, "previous_firmware"), "1.2.3");
    assert!(ctx_str(event, "backtrace").starts_with("Backtrace:"));
    assert_eq!(event.fingerprint, "esp32-firmware-panic-stack-canary");
}

/// `test_abort_waits_for_boot_reason_and_emits_once`.
#[test]
fn abort_waits_for_boot_reason_and_emits_once() {
    let mut monitor = with_info("1.2.3");
    monitor.observe_raw_line("abort() was called at PC 0x420037e1 on core 1", 1.0);
    monitor.observe_raw_line("Backtrace: 0x4037801a:0x3fcebce0", 1.1);
    assert!(monitor.observe_raw_line(BOOT, 1.2).is_empty());
    let events = monitor.observe_boot_reason("PANIC", "4", 3.0);
    assert_eq!(titles(&events), ["ESP32 firmware panic detected"]);
    assert_eq!(ctx_str(&events[0], "panic_reason"), "abort");
    assert_eq!(ctx_str(&events[0], "core"), "1");
    assert!(ctx_str(&events[0], "panic_output").starts_with("abort() was called"));
    assert_eq!(events[0].fingerprint, "esp32-firmware-panic-abort");
    assert!(
        monitor
            .observe_valid_message("ESPBoot", 3.1, None)
            .is_empty()
    );
}

/// `test_silent_panic_is_finalized_without_duplicate_timeout` (guru, abort).
#[test]
fn silent_panic_is_finalized_without_duplicate_timeout() {
    for panic_line in [
        "Guru Meditation Error: Core 1 panic'ed (LoadProhibited).",
        "abort() was called at PC 0x420037e1 on core 0",
    ] {
        let mut monitor = with_info("1.2.3");
        monitor.observe_raw_line(panic_line, 1.0);
        assert_eq!(monitor.phase, Phase::Normal);
        let mut events = monitor.check_timeouts(3.4);
        events.extend(monitor.check_timeouts(4.0));
        assert_eq!(
            titles(&events),
            ["ESP32 firmware panic detected"],
            "{panic_line}"
        );
        assert!(monitor.observe_boot_reason("PANIC", "4", 5.0).is_empty());
    }
}

/// `test_delayed_boot_reason_after_silent_panic_remains_deduplicated`.
#[test]
fn delayed_boot_reason_after_silent_panic_remains_deduplicated() {
    let mut monitor = with_info("1.2.3");
    capture_guru(&mut monitor, "LoadProhibited", 1.0);
    let mut events = monitor.check_timeouts(3.3);
    events.extend(monitor.observe_raw_line(BOOT, 4.0));
    events.extend(monitor.observe_boot_reason("PANIC", "4", 4.1));
    events.extend(monitor.observe_valid_message("ESPBoot", 4.1, None));
    assert_eq!(titles(&events), ["ESP32 firmware panic detected"]);
    assert_eq!(monitor.phase, Phase::Normal);
}

/// `test_normal_timeout_resumes_after_panic_recovery_traffic`.
#[test]
fn normal_timeout_resumes_after_panic_recovery_traffic() {
    let mut monitor = with_info("1.2.3");
    capture_guru(&mut monitor, "LoadProhibited", 1.0);
    assert_eq!(
        titles(&monitor.check_timeouts(3.3)),
        ["ESP32 firmware panic detected"]
    );
    assert!(monitor.observe_valid_message("Data", 4.0, None).is_empty());
    assert_eq!(
        titles(&monitor.check_timeouts(6.1)),
        ["ESP32 valid-message timeout"]
    );
}

/// `test_standalone_panic_evidence_does_not_suppress_timeout`.
#[test]
fn standalone_panic_evidence_does_not_suppress_timeout() {
    for line in ["Register dump:", "Backtrace: 0x1:0x2"] {
        let mut monitor = with_info("1.2.3");
        assert!(monitor.observe_raw_line(line, 1.0).is_empty());
        assert_eq!(
            titles(&monitor.check_timeouts(2.2)),
            ["ESP32 valid-message timeout"],
            "{line}"
        );
    }
}

/// `test_garbled_boot_after_panic_is_bounded_and_finalized_on_silence`.
#[test]
fn garbled_boot_after_panic_is_bounded_and_finalized_on_silence() {
    let mut monitor = with_info("1.2.3");
    capture_guru(&mut monitor, "LoadProhibited", 1.0);
    monitor.observe_raw_line("rst:garbled boot output", 1.3);
    let events = monitor.check_timeouts(3.4);
    assert_eq!(titles(&events), ["ESP32 firmware panic detected"]);
    assert!(ctx_str(&events[0], "panic_output").contains("rst:garbled boot output"));
    assert!(monitor.check_timeouts(4.0).is_empty());
}

/// `test_independent_panics_are_reported_after_esp_info_recovery`.
#[test]
fn independent_panics_are_reported_after_esp_info_recovery() {
    let mut monitor = with_info("1.2.3");
    capture_guru(&mut monitor, "LoadProhibited", 1.0);
    monitor.observe_raw_line(BOOT, 1.3);
    let first = monitor.observe_boot_reason("PANIC", "4", 2.0);
    assert!(monitor.observe_boot_reason("PANIC", "4", 2.1).is_empty());
    assert!(
        monitor
            .observe_valid_message("ESPInfo", 2.2, Some("1.2.3"))
            .is_empty()
    );
    capture_guru(&mut monitor, "StoreProhibited", 3.0);
    monitor.observe_raw_line(BOOT, 3.3);
    let second = monitor.observe_boot_reason("PANIC", "4", 4.0);
    assert_eq!(titles(&first), ["ESP32 firmware panic detected"]);
    assert_eq!(titles(&second), ["ESP32 firmware panic detected"]);
    let fingerprints: Vec<&str> = first
        .iter()
        .chain(&second)
        .map(|e| e.fingerprint.as_str())
        .collect();
    assert_eq!(
        fingerprints,
        [
            "esp32-firmware-panic-load-prohibited",
            "esp32-firmware-panic-store-prohibited"
        ]
    );
}

/// `test_valid_traffic_finalizes_panic_and_clears_evidence_for_next_incident`.
#[test]
fn valid_traffic_finalizes_panic_and_clears_evidence_for_next_incident() {
    let mut monitor = with_info("1.2.3");
    capture_guru(&mut monitor, "LoadProhibited", 1.0);
    let first = monitor.observe_valid_message("Data", 1.5, None);
    capture_guru(&mut monitor, "StoreProhibited", 2.0);
    let second = monitor.observe_valid_message("Sensors", 2.5, None);
    let fingerprints: Vec<&str> = first
        .iter()
        .chain(&second)
        .map(|e| e.fingerprint.as_str())
        .collect();
    assert_eq!(
        fingerprints,
        [
            "esp32-firmware-panic-load-prohibited",
            "esp32-firmware-panic-store-prohibited"
        ]
    );
    assert!(ctx_str(&first[0], "panic_output").contains("LoadProhibited"));
    assert!(!ctx_str(&second[0], "panic_output").contains("LoadProhibited"));
}

/// `test_begin_update_emits_pending_panic_before_clearing_state`.
#[test]
fn begin_update_emits_pending_panic_before_clearing_state() {
    let mut monitor = with_info("1.2.3");
    capture_guru(&mut monitor, "LoadProhibited", 1.0);
    let events = monitor.begin_update(Some("2.0.0"), Some("1.2.3"), 1.5);
    assert_eq!(titles(&events), ["ESP32 firmware panic detected"]);
    assert!(ctx_str(&events[0], "backtrace").starts_with("Backtrace:"));
    assert_eq!(monitor.phase, Phase::Flashing);
}

/// `test_older_firmware_valid_message_provides_one_unknown_reset_fallback`.
#[test]
fn older_firmware_valid_message_provides_one_unknown_reset_fallback() {
    let mut monitor = with_info("1.2.3");
    capture_guru(&mut monitor, "LoadProhibited", 1.0);
    assert!(monitor.observe_raw_line(BOOT, 1.3).is_empty());
    let events = monitor.observe_valid_message("Data", 3.0, None);
    assert_eq!(titles(&events), ["ESP32 firmware panic detected"]);
    assert_eq!(tag(&events[0], "reset_reason"), "UNKNOWN");
    assert!(
        monitor
            .observe_valid_message("Sensors", 3.1, None)
            .is_empty()
    );
}

/// `test_watchdog_reset_is_unexpected_reset_without_raw_backtrace`.
#[test]
fn watchdog_reset_is_unexpected_reset_without_raw_backtrace() {
    let mut monitor = with_info("1.2.3");
    monitor.observe_raw_line(BOOT, 1.0);
    let events = monitor.observe_boot_reason("TASK_WDT", "6", 3.0);
    assert_eq!(titles(&events), ["ESP32 unexpected reset detected"]);
    assert_eq!(tag(&events[0], "reset_reason"), "TASK_WDT");
}

/// `test_update_panic_is_deduplicated_across_recovery_boots`.
#[test]
fn update_panic_is_deduplicated_across_recovery_boots() {
    let mut monitor = with_info("old");
    monitor.begin_update(Some("new"), Some("old"), 1.0);
    monitor.finish_flashing(2.0);
    capture_guru(&mut monitor, "LoadProhibited", 2.05);
    assert!(monitor.observe_raw_line(BOOT, 2.4).is_empty());
    assert!(monitor.observe_raw_line(BOOT, 4.0).is_empty());
    let events = monitor.observe_boot_reason("PANIC", "4", 6.0);
    assert_eq!(titles(&events), ["ESP32 firmware panic detected"]);
    assert!(
        monitor
            .observe_valid_message("ESPBoot", 6.1, None)
            .is_empty()
    );
    assert_eq!(monitor.phase, Phase::WaitingForProtocol);
}

/// `test_normal_valid_message_timeout_is_deduplicated`.
#[test]
fn normal_valid_message_timeout_is_deduplicated() {
    let mut monitor = with_info("1.2.3");
    assert!(monitor.check_timeouts(0.6).is_empty());
    let events = monitor.check_timeouts(0.61);
    assert_eq!(titles(&events), ["ESP32 valid-message timeout"]);
    assert_eq!(ctx(&events[0], "threshold_seconds"), 0.5);
    assert!(monitor.check_timeouts(5.0).is_empty());
}

/// `test_expected_reset_timeout_is_distinct`.
#[test]
fn expected_reset_timeout_is_distinct() {
    let mut monitor = with_info("1.2.3");
    monitor.begin_expected_reset(1.0);
    let events = monitor.check_timeouts(16.1);
    assert_eq!(titles(&events), ["ESP32 valid-message timeout"]);
    assert_eq!(tag(&events[0], "operation"), "expected_reset");
    assert_eq!(ctx_str(&events[0], "recovery_phase"), "waiting_for_boot");
}

/// `test_unexpected_boot_without_protocol_reaches_reset_recovery_timeout`.
#[test]
fn unexpected_boot_without_protocol_reaches_reset_recovery_timeout() {
    let mut monitor = with_info("1.2.3");
    assert!(monitor.observe_raw_line(BOOT, 1.0).is_empty());
    assert_eq!(monitor.phase, Phase::WaitingForProtocol);
    assert!(monitor.check_timeouts(16.0).is_empty());
    let events = monitor.check_timeouts(16.1);
    assert_eq!(titles(&events), ["ESP32 valid-message timeout"]);
    assert_eq!(tag(&events[0], "operation"), "unexpected_reset");
    assert_eq!(ctx(&events[0], "threshold_seconds"), 15.0);
    assert!(monitor.check_timeouts(20.0).is_empty());
}

/// `test_recovery_timeout_reports_retained_panic_once` (reset/update × guru/abort).
#[test]
fn recovery_timeout_reports_retained_panic_once() {
    let cases: [(&[&str], &str, &str); 2] = [
        (
            &[
                "Guru Meditation Error: Core 1 panic'ed (LoadProhibited).",
                "Core 1 register dump:",
                "Backtrace: 0x40381234:0x3fceabcd",
            ],
            "1",
            "esp32-firmware-panic-load-prohibited",
        ),
        (
            &[
                "abort() was called at PC 0x420037e1 on core 0",
                "Register dump:",
                "Backtrace: 0x4037801a:0x3fcebce0",
            ],
            "0",
            "esp32-firmware-panic-abort",
        ),
    ];
    for update_recovery in [false, true] {
        for (panic_lines, core, fingerprint) in cases {
            let mut monitor = with_info("1.2.3");
            if update_recovery {
                monitor.begin_update(Some("2.0.0"), Some("1.2.3"), 1.0);
                monitor.finish_flashing(2.0);
            }
            for (index, line) in panic_lines.iter().enumerate() {
                monitor.observe_raw_line(line, 2.1 + index as f64 / 10.0);
            }
            assert!(monitor.observe_raw_line(BOOT, 3.0).is_empty());

            let timeout = if update_recovery { 32.1 } else { 18.1 };
            let mut events = monitor.check_timeouts(timeout);
            events.extend(monitor.check_timeouts(timeout + 0.1));
            events.extend(monitor.observe_boot_reason("PANIC", "4", timeout + 0.2));
            events.extend(monitor.observe_valid_message("ESPBoot", timeout + 0.3, None));

            let panics: Vec<&EspDiagnostic> = events
                .iter()
                .filter(|e| e.title == "ESP32 firmware panic detected")
                .collect();
            assert_eq!(panics.len(), 1);
            let panic = panics[0];
            assert_eq!(tag(panic, "reset_reason"), "UNKNOWN");
            assert_eq!(ctx_str(panic, "reset_reason"), "UNKNOWN");
            assert_eq!(ctx_str(panic, "core"), core);
            assert_eq!(ctx_str(panic, "previous_firmware"), "1.2.3");
            assert!(ctx_str(panic, "backtrace").starts_with("Backtrace:"));
            let output = ctx_str(panic, "panic_output");
            assert!(output.lines().count() <= EspObservability::MAX_PANIC_LINES);
            assert!(output.len() <= EspObservability::MAX_PANIC_BYTES);
            assert_eq!(panic.fingerprint, fingerprint);
            let recovery_title = if update_recovery {
                "ESP32 did not recover after firmware update"
            } else {
                "ESP32 valid-message timeout"
            };
            assert_eq!(
                titles(&events)
                    .iter()
                    .filter(|t| **t == recovery_title)
                    .count(),
                1
            );

            capture_guru(&mut monitor, "StoreProhibited", timeout + 1.0);
            monitor.observe_raw_line(BOOT, timeout + 1.3);
            let new_events = monitor.observe_boot_reason("PANIC", "4", timeout + 1.4);
            let fingerprints: Vec<&str> =
                new_events.iter().map(|e| e.fingerprint.as_str()).collect();
            assert_eq!(fingerprints, ["esp32-firmware-panic-store-prohibited"]);
        }
    }
}

/// `test_unexpected_boot_protocol_message_clears_recovery_timeout`.
#[test]
fn unexpected_boot_protocol_message_clears_recovery_timeout() {
    let mut monitor = with_info("1.2.3");
    monitor.observe_raw_line(BOOT, 1.0);
    let events = monitor.observe_boot_reason("SW", "3", 2.0);
    assert!(
        monitor
            .observe_valid_message("ESPBoot", 2.0, None)
            .is_empty()
    );
    assert_eq!(titles(&events), ["ESP32 unexpected reset detected"]);
    assert_eq!(monitor.phase, Phase::Normal);
    assert_eq!(monitor.recovery_deadline, None);
    assert!(monitor.check_timeouts(2.5).is_empty());
    assert_eq!(
        titles(&monitor.check_timeouts(2.51)),
        ["ESP32 valid-message timeout"]
    );
}

/// `test_unexpected_boot_loop_is_detected_without_protocol_follow_up`.
#[test]
fn unexpected_boot_loop_is_detected_without_protocol_follow_up() {
    let mut monitor = with_info("1.2.3");
    let mut events = Vec::new();
    for timestamp in [1.0, 10.0, 20.0] {
        events.extend(monitor.observe_raw_line(BOOT, timestamp));
    }
    assert_eq!(titles(&events), ["ESP32 firmware boot loop detected"]);
}

/// `test_three_unexpected_boots_report_boot_loop_once`.
#[test]
fn three_unexpected_boots_report_boot_loop_once() {
    let mut monitor = with_info("1.2.3");
    let mut events = Vec::new();
    for timestamp in [1.0, 10.0, 20.0, 30.0] {
        events.extend(monitor.observe_raw_line(BOOT, timestamp));
        monitor.observe_boot_reason("SW", "3", timestamp + 0.1);
        monitor.observe_valid_message("ESPInfo", timestamp + 0.2, Some("1.2.3"));
    }
    assert_eq!(titles(&events), ["ESP32 firmware boot loop detected"]);
}

/// `test_panic_evidence_is_bounded_by_line_count`.
#[test]
fn panic_evidence_is_bounded_by_line_count() {
    let mut monitor = with_info("1.2.3");
    monitor.observe_raw_line(
        "Guru Meditation Error: Core 1 panic'ed (LoadProhibited).",
        1.0,
    );
    for index in 0..100 {
        monitor.observe_raw_line(&format!("register {index}"), 1.1 + index as f64 / 100.0);
    }
    monitor.observe_raw_line(BOOT, 3.0);
    let event = monitor.observe_boot_reason("PANIC", "4", 4.0).remove(0);
    assert_eq!(ctx_str(&event, "panic_output").lines().count(), 80);
    assert_eq!(ctx(&event, "panic_output_truncated"), true);
}

/// `test_panic_evidence_is_bounded_by_byte_count`.
#[test]
fn panic_evidence_is_bounded_by_byte_count() {
    let mut monitor = EspObservability::new(0.0);
    monitor.observe_raw_line("abort() was called at PC 0x1 on core 0", 1.0);
    for index in 0..20 {
        monitor.observe_raw_line(
            &format!("{index}:{}", "x".repeat(1000)),
            1.1 + index as f64 / 100.0,
        );
    }
    monitor.observe_raw_line(BOOT, 3.0);
    let event = monitor.observe_boot_reason("PANIC", "4", 4.0).remove(0);
    assert!(ctx_str(&event, "panic_output").len() <= 8192);
    assert_eq!(ctx(&event, "panic_output_truncated"), true);
}

/// `test_known_panic_causes_have_separate_fingerprints`.
#[test]
fn known_panic_causes_have_separate_fingerprints() {
    let mut load = EspObservability::default();
    capture_guru(&mut load, "LoadProhibited", 1.0);
    load.observe_raw_line(BOOT, 2.0);
    let load_event = load.observe_boot_reason("PANIC", "4", 3.0).remove(0);

    let mut abort = EspObservability::default();
    abort.observe_raw_line("abort() was called at PC 0x1 on core 0", 1.0);
    abort.observe_raw_line(BOOT, 2.0);
    let abort_event = abort.observe_boot_reason("PANIC", "4", 3.0).remove(0);

    assert_eq!(
        load_event.fingerprint,
        "esp32-firmware-panic-load-prohibited"
    );
    assert_eq!(abort_event.fingerprint, "esp32-firmware-panic-abort");
}

/// `test_other_primary_panic_lines_have_normalized_fingerprints`.
#[test]
fn other_primary_panic_lines_have_normalized_fingerprints() {
    for (line, fingerprint) in [
        ("assert failed: x.cpp:42", "esp32-firmware-panic-assert"),
        (
            "Stack smashing protect failure!",
            "esp32-firmware-panic-stack-smashing",
        ),
        (
            "Heap corruption detected",
            "esp32-firmware-panic-heap-corruption",
        ),
    ] {
        let mut monitor = EspObservability::new(0.0);
        monitor.observe_raw_line(line, 1.0);
        let event = monitor.check_timeouts(3.1).remove(0);
        assert_eq!(event.fingerprint, fingerprint, "{line}");
    }
}

/// `test_variable_unknown_panic_causes_share_bounded_fingerprint`.
#[test]
fn variable_unknown_panic_causes_share_bounded_fingerprint() {
    let mut fingerprints = std::collections::BTreeSet::new();
    for reason in ["Task foo at 0x1234", "Task bar at 0x9876"] {
        let mut monitor = EspObservability::default();
        capture_guru(&mut monitor, reason, 1.0);
        monitor.observe_raw_line(BOOT, 2.0);
        fingerprints.insert(
            monitor
                .observe_boot_reason("PANIC", "4", 3.0)
                .remove(0)
                .fingerprint,
        );
    }
    assert_eq!(
        fingerprints.into_iter().collect::<Vec<_>>(),
        ["esp32-firmware-panic-unknown"]
    );
}
