//! `esp_observability.py`: what the UART traffic says about the ESP32's
//! health. A pure state machine: the caller feeds it lines, boot reasons,
//! valid messages and the monotonic clock, and reports the diagnostics it
//! returns (Sentry event, alarm; see [`EspDiagnostic`]).

use std::collections::VecDeque;
use std::sync::LazyLock;

use indexmap::IndexMap;
use pyfmt::strip_crlf;
use regex::Regex;
use serde_json::{Map, Value};

use crate::pynum::{float_json, py_round, py_strip};

/// `ESPCommunicationPhase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EspCommunicationPhase {
    Normal,
    ExpectedReset,
    Flashing,
    WaitingForBoot,
    WaitingForProtocol,
}

impl EspCommunicationPhase {
    /// The enum value (`Machine.esp_restart_request` is
    /// `phase.value != "normal"`).
    pub fn value(self) -> &'static str {
        match self {
            EspCommunicationPhase::Normal => "normal",
            EspCommunicationPhase::ExpectedReset => "expected_reset",
            EspCommunicationPhase::Flashing => "flashing",
            EspCommunicationPhase::WaitingForBoot => "waiting_for_boot",
            EspCommunicationPhase::WaitingForProtocol => "waiting_for_protocol",
        }
    }
}

pub const TITLE_UPDATE_FAILED: &str = "ESP32 firmware update failed";
pub const TITLE_BOOT_LOOP: &str = "ESP32 firmware boot loop detected";
pub const TITLE_UNEXPECTED_RESET: &str = "ESP32 unexpected reset detected";
pub const TITLE_UPDATE_RECOVERY_FAILED: &str = "ESP32 did not recover after firmware update";
pub const TITLE_VALID_MESSAGE_TIMEOUT: &str = "ESP32 valid-message timeout";
pub const TITLE_PANIC: &str = "ESP32 firmware panic detected";

/// The alarm `Machine._report_esp_diagnostics` raises for a diagnostic
/// when it is not set yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticAlarm {
    /// `AlarmType.ESP_RESTART`, `force=False, quiet=True`.
    EspRestart,
    /// `AlarmType.ESP_DISCONNECTED`, `force=True, quiet=True`.
    EspDisconnected,
}

/// `ESPDiagnostic`. `Machine._report_esp_diagnostics` logs it as a warning
/// `<title>: tags=<tags>`, sends it to the ESP Sentry project (see
/// [`EspDiagnostic::firmware_version`]) and raises [`EspDiagnostic::alarm`].
#[derive(Debug, Clone, PartialEq)]
pub struct EspDiagnostic {
    pub title: String,
    pub level: String,
    pub fingerprint: String,
    pub tags: IndexMap<String, String>,
    pub context: Map<String, Value>,
}

impl EspDiagnostic {
    fn new(
        title: &str,
        level: &str,
        fingerprint: impl Into<String>,
        tags: IndexMap<String, String>,
        context: Map<String, Value>,
    ) -> Self {
        EspDiagnostic {
            title: title.to_string(),
            level: level.to_string(),
            fingerprint: fingerprint.into(),
            tags,
            context,
        }
    }

    /// The firmware `Machine._capture_esp_diagnostic` names in the event:
    /// the context's `previous_firmware`, else the monitor's. The event is
    /// `{"message": title, "level": level, "fingerprint": [fingerprint]}`,
    /// plus `"release": "espresso-firmware@<version>"` and the tag
    /// `firmware-version` when there is a version; tags with empty values
    /// are skipped and a non-empty context goes in as `esp-diagnostic`.
    pub fn firmware_version(&self, monitor_previous_firmware: Option<&str>) -> Option<String> {
        match self.context.get("previous_firmware") {
            Some(Value::String(version)) if !version.is_empty() => Some(version.clone()),
            _ => monitor_previous_firmware
                .filter(|v| !v.is_empty())
                .map(str::to_string),
        }
    }

    /// `Machine._report_esp_diagnostics`: the alarm the title raises.
    pub fn alarm(&self) -> Option<DiagnosticAlarm> {
        match self.title.as_str() {
            TITLE_PANIC | TITLE_UNEXPECTED_RESET | TITLE_BOOT_LOOP => {
                Some(DiagnosticAlarm::EspRestart)
            }
            TITLE_VALID_MESSAGE_TIMEOUT | TITLE_UPDATE_RECOVERY_FAILED => {
                Some(DiagnosticAlarm::EspDisconnected)
            }
            _ => None,
        }
    }
}

/// `should_start_firmware_update`: an available firmware that differs from
/// the running one (or the running one is unknown), with no update in
/// progress and flashing allowed.
pub fn should_start_firmware_update<T: PartialEq>(
    available_firmware: Option<&T>,
    running_firmware: Option<&T>,
    update_in_progress: bool,
    flashing_disallowed: bool,
) -> bool {
    available_firmware.is_some()
        && available_firmware != running_firmware
        && !update_in_progress
        && !flashing_disallowed
}

// Python's `\s` also matches U+001C..U+001F, which the regex crate's does not.
static GURU_MEDITATION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)Guru Meditation Error:[\s\x1C-\x1F]*Core[\s\x1C-\x1F]+(\d+)[\s\x1C-\x1F]+panic'ed(?:[\s\x1C-\x1F]+\(([^)]+)\))?",
    )
    .expect("GURU_MEDITATION_PATTERN is a valid regex")
});
static ABORT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)abort\(\) was called at PC[\s\x1C-\x1F]+[^\s\x1C-\x1F]+[\s\x1C-\x1F]+on core[\s\x1C-\x1F]+(\d+)",
    )
    .expect("ABORT_PATTERN is a valid regex")
});
static DEBUG_EXCEPTION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Debug exception reason:[\s\x1C-\x1F]*(.+)")
        .expect("DEBUG_EXCEPTION_PATTERN is a valid regex")
});
static LOCATION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?)[\s\x1C-\x1F]*\(([^()]*)\)$").expect("LOCATION_PATTERN is a valid regex")
});

fn panic_cause_alias(normalized: &str) -> Option<&'static str> {
    Some(match normalized {
        "abort" => "abort",
        "stack canary watchpoint triggered" => "stack-canary",
        "loadprohibited" => "load-prohibited",
        "storeprohibited" => "store-prohibited",
        "illegalinstruction" => "illegal-instruction",
        "integerdividebyzero" => "integer-divide-by-zero",
        "cache error" => "cache-error",
        "interrupt wdt timeout on cpu0" | "interrupt wdt timeout on cpu1" => "interrupt-watchdog",
        "task watchdog got triggered" => "task-watchdog",
        "stack smashing protect failure" => "stack-smashing",
        "heap corruption" => "heap-corruption",
        "assert failed" => "assert",
        _ => return None,
    })
}

fn opt_json(value: &Option<String>) -> Value {
    value.clone().map_or(Value::Null, Value::String)
}

fn tags<const N: usize>(pairs: [(&str, &str); N]) -> IndexMap<String, String> {
    pairs
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn context<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// `ESPObservability`.
#[derive(Debug, Clone, PartialEq)]
pub struct EspObservability {
    pub phase: EspCommunicationPhase,
    pub last_valid_message: f64,
    pub recovery_deadline: Option<f64>,
    pub expected_firmware: Option<String>,
    update_active: bool,
    pub previous_firmware: Option<String>,
    pub boot_seen: bool,
    pub pending_unexpected_reset: bool,
    pub reset_reported: bool,
    pub panic_reported: bool,
    resolved_panic_incident: bool,
    pub timeout_reported: bool,
    collecting_panic: bool,
    panic_deadline: Option<f64>,
    panic_lines: Vec<String>,
    panic_bytes: usize,
    panic_core: Option<String>,
    panic_reason: Option<String>,
    exception_reason: Option<String>,
    panic_location: Option<String>,
    recent_unexpected_boots: VecDeque<f64>,
}

impl Default for EspObservability {
    /// `ESPObservability()`: `now=0.0`.
    fn default() -> Self {
        EspObservability::new(0.0)
    }
}

impl EspObservability {
    pub const NORMAL_VALID_MESSAGE_TIMEOUT_SECONDS: f64 = 0.5;
    pub const PANIC_COLLECTION_TIMEOUT_SECONDS: f64 = 2.0;
    pub const UPDATE_RECOVERY_TIMEOUT_SECONDS: f64 = 30.0;
    pub const RESET_RECOVERY_TIMEOUT_SECONDS: f64 = 15.0;
    pub const BOOT_LOOP_WINDOW_SECONDS: f64 = 60.0;
    pub const BOOT_LOOP_THRESHOLD: usize = 3;
    pub const MAX_PANIC_LINES: usize = 80;
    pub const MAX_PANIC_BYTES: usize = 8192;

    /// The backend starts it at boot, expecting the ESP32 to (re)start.
    pub fn new(now: f64) -> Self {
        EspObservability {
            phase: EspCommunicationPhase::ExpectedReset,
            last_valid_message: now,
            recovery_deadline: Some(now + Self::RESET_RECOVERY_TIMEOUT_SECONDS),
            expected_firmware: None,
            update_active: false,
            previous_firmware: None,
            boot_seen: false,
            pending_unexpected_reset: false,
            reset_reported: false,
            panic_reported: false,
            resolved_panic_incident: false,
            timeout_reported: false,
            collecting_panic: false,
            panic_deadline: None,
            panic_lines: Vec::new(),
            panic_bytes: 0,
            panic_core: None,
            panic_reason: None,
            exception_reason: None,
            panic_location: None,
            recent_unexpected_boots: VecDeque::new(),
        }
    }

    /// A firmware update is being flashed or recovered from.
    pub fn update_in_progress(&self) -> bool {
        self.update_active
            && matches!(
                self.phase,
                EspCommunicationPhase::Flashing
                    | EspCommunicationPhase::WaitingForBoot
                    | EspCommunicationPhase::WaitingForProtocol
            )
    }

    /// `Machine.reset` is about to reset the ESP32.
    pub fn begin_expected_reset(&mut self, now: f64) {
        self.phase = EspCommunicationPhase::WaitingForBoot;
        self.update_active = false;
        self.boot_seen = false;
        self.recovery_deadline = Some(now + Self::RESET_RECOVERY_TIMEOUT_SECONDS);
        self.pending_unexpected_reset = false;
        self.reset_reported = false;
        self.panic_reported = false;
        self.timeout_reported = false;
    }

    /// `Machine.startUpdate` is about to flash. A panic still being
    /// collected is reported first.
    pub fn begin_update(
        &mut self,
        expected_firmware: Option<&str>,
        previous_firmware: Option<&str>,
        _now: f64,
    ) -> Vec<EspDiagnostic> {
        if let Some(previous) = clean_version(previous_firmware) {
            self.previous_firmware = Some(previous);
        }
        let events = self.pending_panic_diagnostics();
        self.phase = EspCommunicationPhase::Flashing;
        self.update_active = true;
        self.expected_firmware = clean_version(expected_firmware);
        self.boot_seen = false;
        self.recovery_deadline = None;
        self.pending_unexpected_reset = false;
        self.reset_reported = false;
        self.panic_reported = false;
        self.timeout_reported = false;
        self.clear_panic();
        events
    }

    /// The flash succeeded; the ESP32 has `UPDATE_RECOVERY_TIMEOUT_SECONDS`
    /// to boot the new firmware.
    pub fn finish_flashing(&mut self, now: f64) {
        self.phase = EspCommunicationPhase::WaitingForBoot;
        self.recovery_deadline = Some(now + Self::UPDATE_RECOVERY_TIMEOUT_SECONDS);
    }

    /// The flash failed (`stage` is `flash` or `exception`).
    pub fn fail_flashing(&mut self, detail: &str, stage: &str, now: f64) -> EspDiagnostic {
        let event = EspDiagnostic::new(
            TITLE_UPDATE_FAILED,
            "error",
            "esp32-firmware-update-failed",
            tags([("stage", stage)]),
            context([
                ("detail", Value::String(detail.chars().take(1000).collect())),
                ("expected_firmware", opt_json(&self.expected_firmware)),
                ("previous_firmware", opt_json(&self.previous_firmware)),
            ]),
        );
        self.return_to_normal(now);
        event
    }

    /// A line that is not valid protocol traffic: boot banners, panic output,
    /// free text.
    pub fn observe_raw_line(&mut self, line: &str, now: f64) -> Vec<EspDiagnostic> {
        let mut events = Vec::new();
        let normalized = strip_crlf(line);
        let lowered = normalized.to_lowercase();
        let is_boot_banner = normalized.starts_with("rst:0x")
            && normalized.contains("boot:0x")
            && normalized.contains(" (SPI_FAST_FLASH_BOOT)");

        let starts_panic_incident = [
            "guru meditation error",
            "abort() was called",
            "assert failed",
            "stack smashing protect failure",
            "heap corruption",
        ]
        .iter()
        .any(|marker| lowered.contains(marker));
        if starts_panic_incident {
            if !self.collecting_panic {
                self.resolved_panic_incident = false;
                self.panic_reported = false;
                self.clear_panic();
                self.panic_deadline = Some(now + Self::PANIC_COLLECTION_TIMEOUT_SECONDS);
            }
            self.collecting_panic = true;
            if let Some(guru) = GURU_MEDITATION_PATTERN.captures(normalized) {
                self.panic_core = guru.get(1).map(|m| m.as_str().to_string());
                self.panic_reason = guru.get(2).map(|m| m.as_str().to_string());
            } else if lowered.contains("abort() was called") {
                self.panic_reason = Some("abort".to_string());
            } else if lowered.contains("assert failed") {
                self.panic_reason = Some("assert failed".to_string());
            } else if lowered.contains("stack smashing protect failure") {
                self.panic_reason = Some("stack smashing protect failure".to_string());
            } else if lowered.contains("heap corruption") {
                self.panic_reason = Some("heap corruption".to_string());
            }
            if let Some(abort) = ABORT_PATTERN.captures(normalized) {
                self.panic_core = abort.get(1).map(|m| m.as_str().to_string());
                self.panic_reason = Some("abort".to_string());
            }
        }

        let debug_match = DEBUG_EXCEPTION_PATTERN
            .captures(normalized)
            .filter(|_| self.collecting_panic);
        if let Some(debug) = debug_match {
            let detail = py_strip(debug.get(1).map_or("", |m| m.as_str())).trim_end_matches('.');
            match LOCATION_PATTERN.captures(detail) {
                Some(location) => {
                    self.exception_reason =
                        Some(py_strip(location.get(1).map_or("", |m| m.as_str())).to_string());
                    let place = py_strip(location.get(2).map_or("", |m| m.as_str()));
                    self.panic_location = (!place.is_empty()).then(|| place.to_string());
                }
                None => self.exception_reason = Some(detail.to_string()),
            }
        }

        if self.collecting_panic && !is_boot_banner {
            self.append_panic_line(normalized);
            self.panic_deadline = Some(now + Self::PANIC_COLLECTION_TIMEOUT_SECONDS);
        }

        if !is_boot_banner {
            return events;
        }

        self.boot_seen = true;
        self.collecting_panic = false;
        self.panic_deadline = None;
        if self.update_in_progress() {
            self.phase = EspCommunicationPhase::WaitingForProtocol;
            return events;
        }

        if matches!(
            self.phase,
            EspCommunicationPhase::ExpectedReset | EspCommunicationPhase::WaitingForBoot
        ) || (self.phase == EspCommunicationPhase::WaitingForProtocol
            && !self.pending_unexpected_reset)
        {
            self.phase = EspCommunicationPhase::WaitingForProtocol;
            return events;
        }

        self.pending_unexpected_reset = true;
        self.reset_reported = self.resolved_panic_incident;
        self.phase = EspCommunicationPhase::WaitingForProtocol;
        self.recovery_deadline = Some(now + Self::RESET_RECOVERY_TIMEOUT_SECONDS);
        self.recent_unexpected_boots.push_back(now);
        let cutoff = now - Self::BOOT_LOOP_WINDOW_SECONDS;
        while self
            .recent_unexpected_boots
            .front()
            .is_some_and(|boot| *boot < cutoff)
        {
            self.recent_unexpected_boots.pop_front();
        }
        if self.recent_unexpected_boots.len() == Self::BOOT_LOOP_THRESHOLD {
            events.push(EspDiagnostic::new(
                TITLE_BOOT_LOOP,
                "critical",
                "esp32-firmware-boot-loop",
                IndexMap::new(),
                context([
                    ("resets", Value::from(self.recent_unexpected_boots.len())),
                    ("window_seconds", float_json(Self::BOOT_LOOP_WINDOW_SECONDS)),
                    ("previous_firmware", opt_json(&self.previous_firmware)),
                ]),
            ));
        }
        events
    }

    /// `ESPBoot,<reason>,<code>`, the firmware's first line after a reset.
    pub fn observe_boot_reason(
        &mut self,
        reason: &str,
        code: &str,
        _now: f64,
    ) -> Vec<EspDiagnostic> {
        let mut reason = py_strip(reason).to_uppercase();
        if reason.is_empty() {
            reason = "UNKNOWN".to_string();
        }
        let code = py_strip(code);
        if self.panic_reported {
            return Vec::new();
        }
        if reason == "PANIC" && self.resolved_panic_incident {
            return Vec::new();
        }
        if reason == "PANIC" || !self.panic_lines.is_empty() {
            return self.finalize_panic(&reason, code, reason == "PANIC");
        }
        if !self.pending_unexpected_reset || self.reset_reported {
            return Vec::new();
        }

        self.reset_reported = true;
        vec![EspDiagnostic::new(
            TITLE_UNEXPECTED_RESET,
            "error",
            format!("esp32-unexpected-reset-{}", reason.to_lowercase()),
            tags([("reset_reason", &reason), ("reset_reason_code", code)]),
            context([("previous_firmware", opt_json(&self.previous_firmware))]),
        )]
    }

    /// A valid protocol message of `message_type` (the `valid_message_type`
    /// of the line dispatch); `firmware_version` is the version of a parsed
    /// `ESPInfo`.
    pub fn observe_valid_message(
        &mut self,
        message_type: &str,
        now: f64,
        firmware_version: Option<&str>,
    ) -> Vec<EspDiagnostic> {
        self.last_valid_message = now;
        self.timeout_reported = false;

        if message_type == "Log"
            && self.phase == EspCommunicationPhase::WaitingForProtocol
            && self.boot_seen
        {
            return Vec::new();
        }

        let events = if !self.panic_lines.is_empty() && !self.panic_reported {
            self.pending_panic_diagnostics()
        } else if self.pending_unexpected_reset && !self.reset_reported {
            self.reset_reported = true;
            vec![EspDiagnostic::new(
                TITLE_UNEXPECTED_RESET,
                "error",
                "esp32-unexpected-reset-unknown",
                tags([("reset_reason", "UNKNOWN")]),
                context([("previous_firmware", opt_json(&self.previous_firmware))]),
            )]
        } else {
            Vec::new()
        };

        let recovering_non_update_reset = !self.update_active
            && matches!(
                self.phase,
                EspCommunicationPhase::ExpectedReset | EspCommunicationPhase::WaitingForProtocol
            );
        if message_type != "ESPInfo" {
            if recovering_non_update_reset {
                self.return_to_normal(now);
            }
            return events;
        }

        let observed_firmware = clean_version(firmware_version);
        if self.update_in_progress() {
            if !self.boot_seen {
                return events;
            }
            if self.expected_firmware.is_none() || observed_firmware != self.expected_firmware {
                return events;
            }
            self.return_to_normal(now);
        } else if recovering_non_update_reset
            || matches!(
                self.phase,
                EspCommunicationPhase::ExpectedReset | EspCommunicationPhase::WaitingForProtocol
            )
        {
            self.return_to_normal(now);
        }

        if observed_firmware.is_some() {
            self.previous_firmware = observed_firmware;
        }
        if self.phase == EspCommunicationPhase::Normal {
            self.resolved_panic_incident = false;
        }
        events
    }

    /// Run after every read, with or without a line.
    pub fn check_timeouts(&mut self, now: f64) -> Vec<EspDiagnostic> {
        if self.collecting_panic && self.panic_deadline.is_some_and(|deadline| now > deadline) {
            let events = self.pending_panic_diagnostics();
            self.timeout_reported = true;
            return events;
        }

        if self.update_in_progress()
            && self
                .recovery_deadline
                .is_some_and(|deadline| now > deadline)
        {
            let mut events = self.pending_panic_diagnostics();
            let phase = self.phase.value();
            let event = EspDiagnostic::new(
                TITLE_UPDATE_RECOVERY_FAILED,
                "critical",
                "esp32-firmware-update-recovery-failed",
                tags([("recovery_phase", phase)]),
                context([
                    ("expected_firmware", opt_json(&self.expected_firmware)),
                    ("previous_firmware", opt_json(&self.previous_firmware)),
                    (
                        "timeout_seconds",
                        float_json(Self::UPDATE_RECOVERY_TIMEOUT_SECONDS),
                    ),
                ]),
            );
            self.return_to_normal(now);
            self.timeout_reported = true;
            events.push(event);
            return events;
        }

        if !self.update_active
            && matches!(
                self.phase,
                EspCommunicationPhase::ExpectedReset
                    | EspCommunicationPhase::WaitingForBoot
                    | EspCommunicationPhase::WaitingForProtocol
            )
            && self
                .recovery_deadline
                .is_some_and(|deadline| now > deadline)
        {
            let mut events = self.pending_panic_diagnostics();
            let phase = self.phase.value();
            let operation = if self.pending_unexpected_reset {
                "unexpected_reset"
            } else {
                "expected_reset"
            };
            self.return_to_normal(now);
            self.timeout_reported = true;
            events.push(EspDiagnostic::new(
                TITLE_VALID_MESSAGE_TIMEOUT,
                "error",
                "esp32-valid-message-timeout",
                tags([("operation", operation)]),
                context([
                    ("recovery_phase", Value::String(phase.to_string())),
                    (
                        "threshold_seconds",
                        float_json(Self::RESET_RECOVERY_TIMEOUT_SECONDS),
                    ),
                    ("last_known_firmware", opt_json(&self.previous_firmware)),
                ]),
            ));
            return events;
        }

        if self.phase != EspCommunicationPhase::Normal
            || self.pending_unexpected_reset
            || self.collecting_panic
        {
            return Vec::new();
        }

        let elapsed = now - self.last_valid_message;
        if elapsed > Self::NORMAL_VALID_MESSAGE_TIMEOUT_SECONDS && !self.timeout_reported {
            self.timeout_reported = true;
            return vec![EspDiagnostic::new(
                TITLE_VALID_MESSAGE_TIMEOUT,
                "error",
                "esp32-valid-message-timeout",
                IndexMap::new(),
                context([
                    ("elapsed_seconds", float_json(py_round(elapsed, 3))),
                    (
                        "threshold_seconds",
                        float_json(Self::NORMAL_VALID_MESSAGE_TIMEOUT_SECONDS),
                    ),
                    ("last_known_firmware", opt_json(&self.previous_firmware)),
                ]),
            )];
        }
        Vec::new()
    }

    fn pending_panic_diagnostics(&mut self) -> Vec<EspDiagnostic> {
        self.finalize_panic("UNKNOWN", "", false)
    }

    fn finalize_panic(&mut self, reason: &str, code: &str, force: bool) -> Vec<EspDiagnostic> {
        if (self.panic_lines.is_empty() && !force) || self.panic_reported {
            return Vec::new();
        }
        let event = self.panic_diagnostic(reason, code);
        self.panic_reported = true;
        self.resolved_panic_incident = true;
        self.reset_reported = true;
        self.clear_panic();
        vec![event]
    }

    fn panic_diagnostic(&self, reason: &str, code: &str) -> EspDiagnostic {
        let mut tags = tags([("reset_reason", reason), ("reset_reason_code", code)]);
        let mut context = context([
            ("reset_reason", Value::String(reason.to_string())),
            ("previous_firmware", opt_json(&self.previous_firmware)),
        ]);
        let structured_fields = [
            ("panic_reason", &self.panic_reason),
            ("exception_reason", &self.exception_reason),
            ("panic_location", &self.panic_location),
            ("core", &self.panic_core),
        ];
        for (key, value) in structured_fields {
            if let Some(value) = value.as_deref().filter(|v| !v.is_empty()) {
                context.insert(key.to_string(), Value::String(value.to_string()));
                if key != "panic_location" {
                    tags.insert(key.to_string(), value.to_string());
                }
            }
        }
        if !code.is_empty() {
            context.insert(
                "reset_reason_code".to_string(),
                Value::String(code.to_string()),
            );
        }
        if !self.panic_lines.is_empty() {
            context.insert(
                "panic_output".to_string(),
                Value::String(self.panic_lines.join("\n")),
            );
            let backtrace: Vec<&str> = self
                .panic_lines
                .iter()
                .filter(|line| line.to_lowercase().contains("backtrace"))
                .map(String::as_str)
                .collect();
            context.insert("backtrace".to_string(), Value::String(backtrace.join("\n")));
            context.insert(
                "panic_output_truncated".to_string(),
                Value::Bool(
                    self.panic_lines.len() >= Self::MAX_PANIC_LINES
                        || self.panic_bytes >= Self::MAX_PANIC_BYTES,
                ),
            );
        }
        let cause = self.normalized_panic_cause();
        EspDiagnostic::new(
            TITLE_PANIC,
            "critical",
            format!("esp32-firmware-panic-{cause}"),
            tags,
            context,
        )
    }

    fn normalized_panic_cause(&self) -> &'static str {
        [&self.exception_reason, &self.panic_reason]
            .into_iter()
            .filter_map(|candidate| candidate.as_deref().filter(|c| !c.is_empty()))
            .find_map(|candidate| {
                let lowered = py_strip(candidate).to_lowercase();
                panic_cause_alias(lowered.trim_end_matches('.'))
            })
            .unwrap_or("unknown")
    }

    fn append_panic_line(&mut self, line: &str) {
        if self.panic_lines.len() >= Self::MAX_PANIC_LINES {
            return;
        }
        let mut encoded_size = line.len() + 1;
        let remaining = Self::MAX_PANIC_BYTES.saturating_sub(self.panic_bytes);
        if remaining == 0 {
            return;
        }
        let mut line = line;
        if encoded_size > remaining {
            let mut end = remaining.min(line.len());
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line = &line[..end];
            // PARITY: the cut line is counted without its newline.
            encoded_size = line.len();
        }
        self.panic_lines.push(line.to_string());
        self.panic_bytes += encoded_size;
    }

    fn return_to_normal(&mut self, now: f64) {
        self.phase = EspCommunicationPhase::Normal;
        self.last_valid_message = now;
        self.recovery_deadline = None;
        self.expected_firmware = None;
        self.update_active = false;
        self.boot_seen = false;
        self.pending_unexpected_reset = false;
        self.reset_reported = false;
        self.panic_reported = false;
        self.clear_panic();
    }

    fn clear_panic(&mut self) {
        self.collecting_panic = false;
        self.panic_deadline = None;
        self.panic_lines.clear();
        self.panic_bytes = 0;
        self.panic_core = None;
        self.panic_reason = None;
        self.exception_reason = None;
        self.panic_location = None;
    }
}

/// `ESPObservability._clean_version`: stripped, `None` when empty.
fn clean_version(version: Option<&str>) -> Option<String> {
    version
        .map(py_strip)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Machine._report_esp_diagnostics` and `_capture_esp_diagnostic`.
    #[test]
    fn alarms_and_release_versions() {
        let mut monitor = EspObservability::new(0.0);
        monitor.begin_update(Some("2.0.0"), Some("1.2.3"), 1.0);
        let failed = monitor.fail_flashing("boom", "flash", 2.0);
        assert_eq!(failed.alarm(), None);
        assert_eq!(failed.firmware_version(None).as_deref(), Some("1.2.3"));

        let timeout = monitor.check_timeouts(10.0).remove(0);
        assert_eq!(timeout.title, TITLE_VALID_MESSAGE_TIMEOUT);
        assert_eq!(timeout.alarm(), Some(DiagnosticAlarm::EspDisconnected));
        // This context has no previous_firmware: the monitor's is used.
        assert_eq!(
            timeout.firmware_version(Some("9.9.9")).as_deref(),
            Some("9.9.9")
        );
        assert_eq!(timeout.context["elapsed_seconds"], 8.0);
    }
}
