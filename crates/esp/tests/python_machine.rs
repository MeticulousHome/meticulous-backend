//! The pure parts of the machine-level Python tests:
//! `tests/test_machine_esp_observability.py`,
//! `tests/test_machine_esp_handshake.py`, `tests/test_machine_device_uuid.py`
//! and `tests/test_machine_setting_sync.py`.
//!
//! The reader tests drive `Machine._read_data` in Python; here [`Reader`]
//! composes this crate's pieces in the order `esp::line` documents for the
//! backend, so the decisions (diagnostics, info requests, updates) are the
//! same. What needs the backend runtime (Sentry scopes, alarms, threads,
//! subprocess, the real port) is listed in the crate's report.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use esp::encode;
use esp::line::{ClassifiedLine, EspLine};
use esp::observability::{EspCommunicationPhase as Phase, EspDiagnostic, EspObservability};
use esp::sync::{
    DEVICE_UUID_MAX_ASSIGNMENT_ATTEMPTS, DEVICE_UUID_RETRY_DELAY_SECONDS, DeviceUuidEnrollment,
    DeviceUuidLog, DeviceUuidStep, ESP_INFO_RETRY_INTERVAL_SECONDS, TareBehaviorPlan,
    TareBehaviorResponse, TareBehaviorSync, esp_info_request_due,
};
use esp::version::{FirmwareVersion, parse_version_string};
use esp::{EspInfo, should_start_firmware_update};

/// The observability and handshake decisions of one `_read_data` loop.
struct Reader {
    monitor: EspObservability,
    clock: Box<dyn FnMut() -> f64>,
    available: Option<FirmwareVersion>,
    running: Option<FirmwareVersion>,
    esp_info: Option<EspInfo>,
    info_ready: bool,
    info_requested_at: Option<f64>,
    writes: Vec<Vec<u8>>,
    diagnostics: Vec<EspDiagnostic>,
    updates_started: usize,
}

impl Reader {
    fn new(
        monitor: EspObservability,
        available: Option<&str>,
        clock: impl FnMut() -> f64 + 'static,
    ) -> Self {
        Reader {
            monitor,
            clock: Box::new(clock),
            available: parse_version_string(available),
            running: None,
            esp_info: None,
            info_ready: false,
            info_requested_at: None,
            writes: Vec::new(),
            diagnostics: Vec::new(),
            updates_started: 0,
        }
    }

    /// `Machine._greet`: the two writes, and the request time.
    fn greet(&mut self) {
        self.writes.extend(encode::greeting());
        self.info_requested_at = Some((self.clock)());
    }

    fn feed(&mut self, line: &str) {
        let now = (self.clock)();
        let classified = ClassifiedLine::new(line);
        if classified.boot_banner {
            self.diagnostics
                .extend(self.monitor.observe_raw_line(line, now));
            self.esp_info = None;
            self.info_requested_at = None;
            self.info_ready = false;
        }
        if self.info_ready && self.esp_info.is_none() {
            let check_now = match self.info_requested_at {
                Some(_) => (self.clock)(),
                None => now,
            };
            if esp_info_request_due(true, false, self.info_requested_at, check_now) {
                self.writes.push(encode::action("info"));
                self.info_requested_at = Some((self.clock)());
            }
        }
        if classified.observe_as_raw_line() {
            self.diagnostics
                .extend(self.monitor.observe_raw_line(line, now));
        }
        if let EspLine::EspBoot { reason, code } = &classified.line {
            self.diagnostics
                .extend(self.monitor.observe_boot_reason(reason, code, now));
        }
        match &classified.line {
            EspLine::Data(Some(_)) => self.info_ready = true,
            EspLine::EspInfo(Some(info)) => {
                self.esp_info = Some(info.clone());
                self.info_ready = true;
                self.running = parse_version_string(Some(&info.firmware_v));
                if should_start_firmware_update(
                    self.available.as_ref(),
                    self.running.as_ref(),
                    self.monitor.update_in_progress(),
                    false,
                ) {
                    self.updates_started += 1;
                }
            }
            _ => {}
        }
        let after = (self.clock)();
        if let Some(message_type) = classified.valid_message_type() {
            let firmware = match &classified.line {
                EspLine::EspInfo(Some(info)) => Some(info.firmware_v.as_str()),
                _ => None,
            };
            self.diagnostics.extend(self.monitor.observe_valid_message(
                message_type,
                after,
                firmware,
            ));
        }
        self.diagnostics.extend(self.monitor.check_timeouts(after));
    }

    fn run(&mut self, lines: &[&str]) {
        for line in lines {
            self.feed(line);
        }
    }
}

/// `itertools.count(3, 0.1)`, the fixture's default clock.
fn counting_clock() -> impl FnMut() -> f64 {
    let mut next = 3.0;
    move || {
        let now = next;
        next += 0.1;
        now
    }
}

fn monitor_after_info(firmware: &str) -> EspObservability {
    let mut monitor = EspObservability::new(0.0);
    monitor.observe_valid_message("ESPInfo", 0.1, Some(firmware));
    monitor
}

// --- test_machine_esp_observability.py ------------------------------------

/// `test_beta_protocol_messages_are_valid_observability_traffic`.
#[test]
fn beta_protocol_messages_are_valid_observability_traffic() {
    for line in [
        "MileageRequest\n",
        "nvs_response,mileage_key,123\n",
        "nvs_response,ERROR: key not found\n",
        "device_uuid_response,accepted\n",
    ] {
        let mut reader = Reader::new(EspObservability::new(0.0), Some("1.2.3"), counting_clock());
        reader.run(&[line]);
        assert!(reader.diagnostics.is_empty(), "{line}");
        assert_eq!(reader.updates_started, 0);
        assert_eq!(reader.monitor.phase, Phase::Normal, "{line}");
    }
}

/// `test_stale_esp_info_during_update_recovery_does_not_start_another_update`.
#[test]
fn stale_esp_info_during_update_recovery_does_not_start_another_update() {
    let mut monitor = monitor_after_info("1.0.0");
    monitor.begin_update(Some("2.0.0"), Some("1.0.0"), 1.0);
    monitor.finish_flashing(2.0);
    assert!(
        monitor
            .observe_valid_message("ESPInfo", 3.0, Some("1.0.0"))
            .is_empty()
    );
    assert_eq!(monitor.phase, Phase::WaitingForBoot);
    assert_eq!(monitor.expected_firmware.as_deref(), Some("2.0.0"));
    assert!(!should_start_firmware_update(
        Some(&"2.0.0"),
        Some(&"1.0.0"),
        monitor.update_in_progress(),
        false
    ));
}

/// `test_machine_update_recovery_requires_boot_and_exact_version`.
#[test]
fn machine_update_recovery_requires_boot_and_exact_version() {
    let mut monitor = monitor_after_info("1.0.0");
    monitor.begin_update(Some("2.0.0"), Some("1.0.0"), 1.0);
    monitor.finish_flashing(2.0);
    let mut reader = Reader::new(monitor, Some("2.0.0"), counting_clock());
    reader.run(&[
        "ESPInfo,1.0.0,1,24.0\n",
        "rst:0x1 (POWERON_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        "ESPInfo,2.0.0-dirty,1,24.0\n",
        "ESPInfo,2.0.0,1,24.0\n",
    ]);
    assert!(reader.diagnostics.is_empty());
    assert_eq!(reader.updates_started, 0);
    assert_eq!(reader.monitor.phase, Phase::Normal);
    assert_eq!(reader.monitor.expected_firmware, None);
    assert_eq!(reader.monitor.previous_firmware.as_deref(), Some("2.0.0"));
    assert_eq!(reader.running, parse_version_string(Some("2.0.0")));
}

/// `test_machine_explicit_reset_ignores_stale_protocol_until_boot`.
#[test]
fn machine_explicit_reset_ignores_stale_protocol_until_boot() {
    let mut monitor = monitor_after_info("1.2.3");
    monitor.begin_expected_reset(1.0);
    let mut reader = Reader::new(monitor, Some("1.2.3"), counting_clock());
    reader.run(&[
        "Log,info,stale pre-reset message\n",
        "ESPInfo,1.2.3,1,24.0\n",
        "rst:0x3 (SW_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        "ESPBoot,SW,3\n",
        "ESPInfo,1.2.3,1,24.0\n",
    ]);
    assert!(reader.diagnostics.is_empty());
    assert_eq!(reader.updates_started, 0);
    // No diagnostic, so no alarm.
    assert_eq!(reader.monitor.phase, Phase::Normal);
}

fn sentry_release(reader: &Reader, diagnostic: &EspDiagnostic) -> Option<String> {
    diagnostic
        .firmware_version(reader.monitor.previous_firmware.as_deref())
        .map(|version| format!("espresso-firmware@{version}"))
}

/// `test_machine_routes_one_bounded_panic_with_firmware_context`.
#[test]
fn machine_routes_one_bounded_panic_with_firmware_context() {
    let mut reader = Reader::new(monitor_after_info("1.2.3"), Some("1.2.3"), counting_clock());
    reader.run(&[
        "Guru Meditation Error: Core 1 panic'ed (Unhandled debug exception).\n",
        "Debug exception reason: Stack canary watchpoint triggered (Read ADC)\n",
        "Register dump:\n",
        "Backtrace: 0x42000000:0x3fc00000\n",
        "rst:0x3 (SW_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        "Log,INFO,no loadcell found\n",
        "ESPBoot,PANIC,4\n",
    ]);
    assert_eq!(reader.updates_started, 0);
    assert_eq!(reader.diagnostics.len(), 1);
    let event = &reader.diagnostics[0];
    let context = &event.context;
    assert_eq!(event.title, "ESP32 firmware panic detected");
    assert_eq!(
        sentry_release(&reader, event).as_deref(),
        Some("espresso-firmware@1.2.3")
    );
    assert_eq!(event.fingerprint, "esp32-firmware-panic-stack-canary");
    assert_eq!(context["previous_firmware"], "1.2.3");
    assert_eq!(context["reset_reason"], "PANIC");
    assert_eq!(context["core"], "1");
    assert_eq!(context["panic_reason"], "Unhandled debug exception");
    assert_eq!(
        context["exception_reason"],
        "Stack canary watchpoint triggered"
    );
    assert_eq!(context["panic_location"], "Read ADC");
    assert_eq!(context["backtrace"], "Backtrace: 0x42000000:0x3fc00000");
    assert!(context["panic_output"].as_str().unwrap().len() <= EspObservability::MAX_PANIC_BYTES);
}

/// `test_machine_routes_unexpected_reset_reason_after_startup_log`.
#[test]
fn machine_routes_unexpected_reset_reason_after_startup_log() {
    let mut reader = Reader::new(monitor_after_info("1.2.3"), Some("1.2.3"), counting_clock());
    reader.run(&[
        "rst:0x9 (BROWNOUT_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        "Log,INFO,no loadcell found\n",
        "ESPBoot,BROWNOUT,9\n",
    ]);
    assert_eq!(reader.updates_started, 0);
    assert_eq!(reader.diagnostics.len(), 1);
    let event = &reader.diagnostics[0];
    assert_eq!(event.title, "ESP32 unexpected reset detected");
    assert_eq!(event.fingerprint, "esp32-unexpected-reset-brownout");
    assert_eq!(event.tags["reset_reason"], "BROWNOUT");
    assert_eq!(event.tags["reset_reason_code"], "9");
    assert_eq!(reader.monitor.phase, Phase::Normal);
}

/// `test_machine_routes_one_bounded_abort_with_firmware_context`.
#[test]
fn machine_routes_one_bounded_abort_with_firmware_context() {
    let mut reader = Reader::new(monitor_after_info("1.2.3"), Some("1.2.3"), counting_clock());
    reader.run(&[
        "abort() was called at PC 0x42000000 on core 0\n",
        "Register dump:\n",
        "Backtrace: 0x42000000:0x3fc00000\n",
        "rst:0x3 (SW_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        "ESPBoot,PANIC,4\n",
    ]);
    assert_eq!(reader.updates_started, 0);
    assert_eq!(reader.diagnostics.len(), 1);
    let event = &reader.diagnostics[0];
    let context = &event.context;
    assert_eq!(event.title, "ESP32 firmware panic detected");
    assert_eq!(
        sentry_release(&reader, event).as_deref(),
        Some("espresso-firmware@1.2.3")
    );
    assert_eq!(event.fingerprint, "esp32-firmware-panic-abort");
    assert_eq!(context["previous_firmware"], "1.2.3");
    assert_eq!(context["reset_reason"], "PANIC");
    assert_eq!(context["core"], "0");
    assert_eq!(context["panic_reason"], "abort");
    assert_eq!(context["backtrace"], "Backtrace: 0x42000000:0x3fc00000");
    assert!(
        context["panic_output"]
            .as_str()
            .unwrap()
            .starts_with("abort() was called")
    );
}

/// `test_valid_log_marker_text_does_not_start_raw_panic`.
#[test]
fn valid_log_marker_text_does_not_start_raw_panic() {
    for message in [
        "assert failed in old diagnostic text",
        "Backtrace: is a troubleshooting label",
        "heap corruption is not what this log reports",
    ] {
        let mut reader = Reader::new(monitor_after_info("1.2.3"), Some("1.2.3"), counting_clock());
        reader.run(&[&format!("Log,info,{message}\n")]);
        assert!(reader.diagnostics.is_empty(), "{message}");
        assert_eq!(reader.updates_started, 0);
    }
}

/// `test_start_update_reports_pending_panic_before_automatic_reflash_cleanup`:
/// `Machine.startUpdate` calls `begin_update` with the running firmware
/// and, after a flash without error, `finish_flashing`.
#[test]
fn start_update_reports_pending_panic_before_reflash_cleanup() {
    let mut monitor = monitor_after_info("1.2.3");
    monitor.observe_raw_line(
        "Guru Meditation Error: Core 1 panic'ed (LoadProhibited).",
        1.0,
    );
    let captured = monitor.begin_update(Some("2.0.0"), Some("1.2.3"), 2.0);
    monitor.finish_flashing(2.0);
    let titles: Vec<&str> = captured.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, ["ESP32 firmware panic detected"]);
    assert_eq!(monitor.phase, Phase::WaitingForBoot);
}

/// `test_machine_missing_expected_version_cannot_complete_or_reflash`.
#[test]
fn machine_missing_expected_version_cannot_complete_or_reflash() {
    let mut monitor = monitor_after_info("1.0.0");
    monitor.begin_update(None, Some("1.0.0"), 1.0);
    monitor.finish_flashing(2.0);
    let mut times = [3.0, 3.0, 4.0, 4.0, 123.0, 123.0].into_iter();
    let mut reader = Reader::new(monitor, None, move || times.next().unwrap());
    reader.run(&[
        "rst:0x1 (POWERON_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
        "ESPInfo,2.0.0,1,24.0\n",
        "waiting for expected artifact version\n",
    ]);
    assert_eq!(reader.updates_started, 0);
    assert_eq!(reader.diagnostics.len(), 1);
    let event = &reader.diagnostics[0];
    assert_eq!(event.title, "ESP32 did not recover after firmware update");
    assert_eq!(event.fingerprint, "esp32-firmware-update-recovery-failed");
    assert!(event.context["expected_firmware"].is_null());
    assert_eq!(reader.monitor.phase, Phase::Normal);
}

/// `test_should_start_firmware_update`.
#[test]
fn should_start_firmware_update_cases() {
    let cases = [
        (Some("2.0.0"), "1.0.0", false, false, true),
        (Some("2.0.0"), "1.0.0", true, false, false),
        (Some("2.0.0"), "2.0.0", false, false, false),
        (Some("2.0.0"), "1.0.0", false, true, false),
        (None, "1.0.0", false, false, false),
    ];
    for (available, running, in_progress, disallowed, expected) in cases {
        assert_eq!(
            should_start_firmware_update(
                available.as_ref(),
                Some(&running),
                in_progress,
                disallowed
            ),
            expected
        );
    }
}

// --- test_machine_esp_handshake.py ----------------------------------------

const DATA_IDLE: &str = "Data,0.00,0.00,0.00,S,21.93,idle,idle,none,0.00,none,0.00,false,0.00,none,0.00,none,0.00,Idle\n";
const ESP_INFO: &str = "ESPInfo,0.9.10-sil,off,120,black,SIL-TM-0001,0025,2025-11-14,acaia,45.33,false,after_retraction,\n";
const ROM_BOOT_BANNER: &str = "rst:0x3 (SW_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n";
const LINE_PERIOD_S: f64 = 0.5;

/// The fixture's clock: 100 at the greeting, then 0.5 s per line.
fn handshake_reader() -> (Reader, std::rc::Rc<std::cell::Cell<f64>>) {
    let clock = std::rc::Rc::new(std::cell::Cell::new(100.0));
    let shared = clock.clone();
    let reader = Reader::new(EspObservability::new(100.0), None, move || shared.get());
    (reader, clock)
}

fn run_lines(reader: &mut Reader, clock: &std::cell::Cell<f64>, lines: &[&str]) {
    for line in lines {
        clock.set(clock.get() + LINE_PERIOD_S);
        reader.feed(line);
    }
}

fn info_requests(reader: &Reader) -> usize {
    reader
        .writes
        .iter()
        .filter(|w| w.as_slice() == b"action,info\x03")
        .count()
}

/// `test_greeting_flushes_the_port_before_asking_for_info` (the writes; the
/// flush comes first in the backend).
#[test]
fn greeting_writes() {
    let (mut reader, clock) = handshake_reader();
    reader.greet();
    assert_eq!(
        reader.writes,
        vec![b"\x03".to_vec(), b"action,info\x03".to_vec()]
    );
    assert_eq!(reader.info_requested_at, Some(clock.get()));
}

/// `test_the_reader_writes_nothing_before_the_firmware_speaks`.
#[test]
fn reader_writes_nothing_before_the_firmware_speaks() {
    let (mut reader, clock) = handshake_reader();
    reader.greet();
    let greeting = reader.writes.clone();
    run_lines(&mut reader, &clock, &[ESP_INFO]);
    assert_eq!(reader.writes, greeting);
    assert_eq!(
        reader.esp_info.as_ref().unwrap().serial_number,
        "SIL-TM-0001"
    );
}

/// `test_info_is_asked_again_every_interval_until_espinfo_arrives`.
#[test]
fn info_is_asked_again_every_interval_until_espinfo_arrives() {
    let (mut reader, clock) = handshake_reader();
    reader.greet();
    let lines_per_interval = (ESP_INFO_RETRY_INTERVAL_SECONDS / LINE_PERIOD_S) as usize;
    let mut lines = vec![DATA_IDLE; 3 * lines_per_interval];
    lines.push(ESP_INFO);
    lines.extend(vec![DATA_IDLE; 2 * lines_per_interval]);
    run_lines(&mut reader, &clock, &lines);
    assert_eq!(info_requests(&reader), 1 + 3);
    assert_eq!(reader.esp_info.as_ref().unwrap().firmware_v, "0.9.10-sil");
}

/// `test_no_request_is_repeated_once_espinfo_has_arrived`.
#[test]
fn no_request_is_repeated_once_espinfo_has_arrived() {
    let (mut reader, clock) = handshake_reader();
    reader.greet();
    let mut lines = vec![ESP_INFO];
    lines.extend(vec![DATA_IDLE; 20]);
    run_lines(&mut reader, &clock, &lines);
    assert_eq!(info_requests(&reader), 1);
}

/// `test_an_esp_reboot_makes_the_backend_ask_again_at_once`.
#[test]
fn an_esp_reboot_makes_the_backend_ask_again_at_once() {
    let (mut reader, clock) = handshake_reader();
    reader.greet();
    let mut lines = vec![DATA_IDLE; 4];
    lines.extend([ROM_BOOT_BANNER, DATA_IDLE, DATA_IDLE]);
    run_lines(&mut reader, &clock, &lines);
    assert_eq!(info_requests(&reader), 3);
    assert!(clock.get() - 102.0 < ESP_INFO_RETRY_INTERVAL_SECONDS);
    assert!(reader.esp_info.is_none());
}

// --- test_machine_device_uuid.py ------------------------------------------

const FIRST_UUID: &str = "123e4567-e89b-42d3-a456-426614174000";
const SECOND_UUID: &str = "987e6543-e21b-45d3-b654-426614174999";

fn writes(steps: &[DeviceUuidStep]) -> Vec<Vec<u8>> {
    steps
        .iter()
        .filter_map(|s| match s {
            DeviceUuidStep::Write(bytes) => Some(bytes.clone()),
            _ => None,
        })
        .collect()
}

/// `test_unsupported_firmware_receives_no_assignment`.
#[test]
fn unsupported_firmware_receives_no_assignment() {
    let mut enrollment = DeviceUuidEnrollment::new();
    let steps = enrollment.sync("", false, || panic!("no assignment"));
    assert!(steps.is_empty());
    assert_eq!(enrollment.pending, None);
}

/// `test_missing_uuid_reuses_pending_assignment_within_boot`.
#[test]
fn missing_uuid_reuses_pending_assignment_within_boot() {
    let mut enrollment = DeviceUuidEnrollment::new();
    let mut generated = 0;
    let mut steps = enrollment.sync("", true, || {
        generated += 1;
        FIRST_UUID.to_string()
    });
    steps.extend(enrollment.sync("invalid", true, || panic!("generated twice")));
    assert_eq!(generated, 1);
    assert_eq!(
        writes(&steps),
        vec![format!("device_uuid,assign,{FIRST_UUID}\x03").into_bytes()]
    );
    assert!(
        !steps
            .iter()
            .any(|s| matches!(s, DeviceUuidStep::UpdateCache(_)))
    );
    assert!(
        steps.contains(&DeviceUuidStep::Log(DeviceUuidLog::ReportedInvalid(
            "invalid".to_string()
        )))
    );
    assert_eq!(enrollment.pending.as_deref(), Some(FIRST_UUID));
}

/// `test_esp_boot_clears_pending_assignment`.
#[test]
fn esp_boot_clears_pending_assignment() {
    let mut enrollment = DeviceUuidEnrollment {
        pending: Some(FIRST_UUID.to_string()),
        ..DeviceUuidEnrollment::default()
    };
    let line = ClassifiedLine::new("rst:0x1 (POWERON_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n");
    assert!(line.boot_banner);
    enrollment.reset();
    assert_eq!(enrollment.pending, None);
}

/// `test_confirmed_uuid_updates_cache_without_restart_when_unchanged`
/// (the cache update is asked for; whether it changed is the backend's).
#[test]
fn confirmed_uuid_updates_cache() {
    let mut enrollment = DeviceUuidEnrollment {
        pending: Some(SECOND_UUID.to_string()),
        ..DeviceUuidEnrollment::default()
    };
    let steps = enrollment.sync(FIRST_UUID, true, || panic!("no assignment"));
    assert_eq!(
        steps,
        vec![DeviceUuidStep::UpdateCache(FIRST_UUID.to_string())]
    );
    assert_eq!(enrollment.pending, None);
}

/// `test_each_firmware_response_is_logged_exactly`.
#[test]
fn each_firmware_response_is_logged_exactly() {
    for (response, level) in [
        ("SUCCESS", "info"),
        ("ALREADY_ASSIGNED", "info"),
        ("ERROR_INVALID_FORMAT", "error"),
        ("ERROR_INVALID_UUID", "error"),
        ("UNEXPECTED_RESPONSE", "error"),
    ] {
        let mut enrollment = DeviceUuidEnrollment {
            pending: Some(FIRST_UUID.to_string()),
            attempts: 1,
            retry_scheduled: false,
        };
        let steps = enrollment.handle_response(response);
        let logs: Vec<&DeviceUuidLog> = steps
            .iter()
            .filter_map(|s| match s {
                DeviceUuidStep::Log(log) => Some(log),
                _ => None,
            })
            .collect();
        assert_eq!(logs.len(), 1, "{response}");
        assert_eq!(logs[0].level(), level, "{response}");
        assert!(logs[0].message().contains(response), "{response}");
    }
}

/// `test_write_failure_retries_same_candidate_after_non_busy_delay`.
#[test]
fn write_failure_retries_same_candidate_after_delay() {
    let mut enrollment = DeviceUuidEnrollment {
        pending: Some(FIRST_UUID.to_string()),
        attempts: 1,
        retry_scheduled: false,
    };
    let steps = enrollment.handle_response("ERROR_WRITE_FAILED");
    assert!(writes(&steps).is_empty());
    assert_eq!(
        steps,
        vec![
            DeviceUuidStep::Log(DeviceUuidLog::WriteFailedScheduling { attempt: 2 }),
            DeviceUuidStep::ScheduleRetry,
        ]
    );
    assert_eq!(DEVICE_UUID_RETRY_DELAY_SECONDS, 2.0);
    assert_eq!(
        DeviceUuidLog::WriteFailedScheduling { attempt: 2 }.level(),
        "warning"
    );

    let retried = enrollment.retry_fired();
    assert_eq!(
        writes(&retried),
        vec![format!("device_uuid,assign,{FIRST_UUID}\x03").into_bytes()]
    );
    assert_eq!(enrollment.attempts, 2);
}

/// `test_persistent_write_failures_stop_after_bounded_attempts`.
#[test]
fn persistent_write_failures_stop_after_bounded_attempts() {
    let mut enrollment = DeviceUuidEnrollment {
        pending: Some(FIRST_UUID.to_string()),
        attempts: DEVICE_UUID_MAX_ASSIGNMENT_ATTEMPTS,
        retry_scheduled: false,
    };
    let steps = enrollment.handle_response("ERROR_WRITE_FAILED");
    assert_eq!(
        steps,
        vec![DeviceUuidStep::Log(DeviceUuidLog::WriteFailedExhausted)]
    );
    assert_eq!(DeviceUuidLog::WriteFailedExhausted.level(), "error");
}

// --- test_machine_setting_sync.py -----------------------------------------

const BEFORE: &[u8] = b"nvs_request,write,tare_behavior_key,before_retraction\x03";
const AFTER: &[u8] = b"nvs_request,write,tare_behavior_key,after_retraction\x03";

/// `test_tare_behavior_sync_is_deferred_for_older_firmware`.
#[test]
fn tare_behavior_sync_is_deferred_for_older_firmware() {
    let mut sync = TareBehaviorSync::new();
    assert_eq!(
        sync.plan(None, "before_retraction", true),
        TareBehaviorPlan::Deferred
    );
    assert!(sync.pending().is_empty());
}

/// `test_tare_behavior_sync_skips_matching_value`.
#[test]
fn tare_behavior_sync_skips_matching_value() {
    let mut sync = TareBehaviorSync::new();
    assert_eq!(
        sync.plan(Some("before_retraction"), "before_retraction", true),
        TareBehaviorPlan::InSync
    );
}

/// `test_tare_behavior_sync_writes_supported_firmware`.
#[test]
fn tare_behavior_sync_writes_supported_firmware() {
    let mut sync = TareBehaviorSync::new();
    let esp_value = "after_retraction";
    assert_eq!(
        sync.plan(Some(esp_value), "before_retraction", true),
        TareBehaviorPlan::Write(BEFORE.to_vec())
    );
    assert_eq!(sync.pending(), ["before_retraction"]);
}

/// `test_tare_behavior_sync_updates_cached_value_only_after_success` and
/// `test_tare_behavior_sync_keeps_cached_value_after_error`.
#[test]
fn tare_behavior_sync_updates_cached_value_only_after_success() {
    let mut sync = TareBehaviorSync::new();
    sync.plan(Some("after_retraction"), "before_retraction", true);
    assert_eq!(
        sync.on_response("SUCCESS"),
        TareBehaviorResponse::Confirmed("before_retraction".to_string())
    );
    assert!(sync.pending().is_empty());

    let mut sync = TareBehaviorSync::new();
    sync.plan(Some("after_retraction"), "before_retraction", true);
    assert_eq!(
        sync.on_response("ERROR"),
        TareBehaviorResponse::Rejected {
            requested: "before_retraction".to_string(),
            status: "ERROR".to_string()
        }
    );
    assert!(sync.pending().is_empty());
}

/// `test_tare_behavior_sync_preserves_rapid_toggle_order`.
#[test]
fn tare_behavior_sync_preserves_rapid_toggle_order() {
    let mut sync = TareBehaviorSync::new();
    let mut esp_value = "after_retraction".to_string();
    let mut written = Vec::new();
    for desired in ["before_retraction", "after_retraction"] {
        if let TareBehaviorPlan::Write(bytes) = sync.plan(Some(&esp_value), desired, true) {
            written.push(bytes);
        }
    }
    assert_eq!(written, vec![BEFORE.to_vec(), AFTER.to_vec()]);

    if let TareBehaviorResponse::Confirmed(value) = sync.on_response("SUCCESS") {
        esp_value = value;
    }
    assert_eq!(esp_value, "before_retraction");
    assert_eq!(sync.pending(), ["after_retraction"]);

    if let TareBehaviorResponse::Confirmed(value) = sync.on_response("SUCCESS") {
        esp_value = value;
    }
    assert_eq!(esp_value, "after_retraction");
    assert!(sync.pending().is_empty());
}
