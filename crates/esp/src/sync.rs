//! The decisions `Machine` takes when it keeps the ESP32's settings and
//! identity in step: which replies mean what, when to write a setting, and
//! the device UUID enrollment. Timers, files and logging stay with the
//! caller; each step says what to do.

use std::collections::VecDeque;

use crate::data::is_valid_device_uuid;
use crate::encode::{device_uuid_assign, tare_behavior_write};

/// `Machine.DEVICE_UUID_MAX_ASSIGNMENT_ATTEMPTS`.
pub const DEVICE_UUID_MAX_ASSIGNMENT_ATTEMPTS: u32 = 3;
/// `Machine.DEVICE_UUID_RETRY_DELAY_SECONDS`.
pub const DEVICE_UUID_RETRY_DELAY_SECONDS: f64 = 2.0;
/// `Machine.HAWKBIT_UPDATER_RESTART_TIMEOUT_SECONDS`.
pub const HAWKBIT_UPDATER_RESTART_TIMEOUT_SECONDS: u64 = 5;
/// `ESP_INFO_RETRY_INTERVAL_SECONDS`: ask `action,info` again this often
/// until ESPInfo arrives.
pub const ESP_INFO_RETRY_INTERVAL_SECONDS: f64 = 2.0;
/// `MILEAGE_RETRY_INTERVAL_SECONDS`: ask for the mileage again this often
/// until the ESP32 answers.
pub const MILEAGE_RETRY_INTERVAL_SECONDS: f64 = 5.0;

/// `_read_data` asks `action,info` again (before dispatching the line) when
/// the ESP32 talks but has not sent ESPInfo, and the last request is
/// `ESP_INFO_RETRY_INTERVAL_SECONDS` old or there was none since a boot
/// banner. `info_ready` is `Machine.infoReady` (set by `Data` and ESPInfo,
/// cleared by a boot banner); times are monotonic.
pub fn esp_info_request_due(
    info_ready: bool,
    esp_info_known: bool,
    info_requested_at: Option<f64>,
    now: f64,
) -> bool {
    info_ready
        && !esp_info_known
        && info_requested_at.is_none_or(|at| now - at >= ESP_INFO_RETRY_INTERVAL_SECONDS)
}

/// `_read_data` reads the mileage again (after the dispatch) until the
/// ESP32 answers. `last_request` is wall-clock time (`time.time()`), 0 before
/// the first request.
pub fn mileage_request_due(
    info_ready: bool,
    mileage_known: bool,
    last_request: f64,
    now: f64,
) -> bool {
    info_ready && !mileage_known && now - last_request >= MILEAGE_RETRY_INTERVAL_SECONDS
}

/// `Machine.setPartialRetraction` skips the write when the ESP32 already
/// holds the value (finite and within 1e-6).
pub fn partial_retraction_in_sync(esp_value: Option<f64>, desired: f64) -> bool {
    esp_value.is_some_and(|current| current.is_finite() && (current - desired).abs() <= 1e-6)
}

/// `Machine.setAutoPurgeAfterShot` skips the write when the ESP32 already
/// holds the value.
pub fn auto_purge_in_sync(esp_value: Option<bool>, desired: bool) -> bool {
    esp_value == Some(desired)
}

/// What `Machine._handleNvsResponse` makes of `nvs_response,<key>,<value>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NvsReply {
    /// Not the mileage key: other keys are written blind.
    Ignored,
    /// `SUCCESS` to the seed write: log it and read the counter back.
    MileageWriteAccepted,
    /// `ERROR` or `ERROR...`: logged as a warning.
    MileageWriteRejected(String),
    /// The counter; the backend's `mileage`.
    Mileage(i64),
    /// Not an int: logged as a warning.
    MileageUnparseable(String),
}

/// `Machine._handleNvsResponse`.
pub fn classify_nvs_response(key: &str, value: &str) -> NvsReply {
    if key != crate::encode::NvsKey::Mileage.as_str() {
        return NvsReply::Ignored;
    }
    if value == "SUCCESS" {
        return NvsReply::MileageWriteAccepted;
    }
    if value.starts_with("ERROR") {
        return NvsReply::MileageWriteRejected(value.to_string());
    }
    match pyfmt::py_int(value) {
        Some(mileage) => NvsReply::Mileage(mileage),
        None => NvsReply::MileageUnparseable(value.to_string()),
    }
}

/// `Machine.handleTareBehaviorNVSResponse` accepts `SUCCESS` in any case.
pub fn tare_behavior_status_ok(status: &str) -> bool {
    status.to_uppercase() == "SUCCESS"
}

/// The outcome of `Machine.setTareBehavior`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TareBehaviorPlan {
    /// No ESPInfo yet, or firmware without tare behaviour: logged at INFO.
    Deferred,
    /// The ESP32 holds the value and no write is pending.
    InSync,
    /// The last pending write already asks for the value.
    AlreadyPending,
    /// The serial connection is not ready: logged as a warning.
    NotConnected,
    /// Write these bytes; the value is now pending.
    Write(Vec<u8>),
}

/// The outcome of `Machine.handleTareBehaviorNVSResponse`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TareBehaviorResponse {
    /// Nothing was pending: logged as a warning.
    NoPendingWrite,
    /// The ESP32 refused the oldest pending write: logged at ERROR.
    Rejected { requested: String, status: String },
    /// The ESP32 stored the oldest pending write: set
    /// `esp_info.tare_behavior` to it.
    Confirmed(String),
}

/// `Machine._pending_tare_behavior_writes`: tare behaviour writes the
/// ESP32 has not confirmed yet, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TareBehaviorSync {
    pending: VecDeque<String>,
}

impl TareBehaviorSync {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending(&self) -> Vec<String> {
        self.pending.iter().cloned().collect()
    }

    /// A boot banner drops the pending writes.
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// `Machine.setTareBehavior`. `esp_tare_behavior` is `None` without
    /// ESPInfo or when the firmware does not report tare behaviour;
    /// `connected` is false when the port is missing or flashing.
    pub fn plan(
        &mut self,
        esp_tare_behavior: Option<&str>,
        desired: &str,
        connected: bool,
    ) -> TareBehaviorPlan {
        let Some(current) = esp_tare_behavior else {
            return TareBehaviorPlan::Deferred;
        };
        if current == desired && self.pending.is_empty() {
            return TareBehaviorPlan::InSync;
        }
        if self.pending.back().map(String::as_str) == Some(desired) {
            return TareBehaviorPlan::AlreadyPending;
        }
        if !connected {
            return TareBehaviorPlan::NotConnected;
        }
        self.pending.push_back(desired.to_string());
        TareBehaviorPlan::Write(tare_behavior_write(desired))
    }

    /// `Machine.handleTareBehaviorNVSResponse`: the reply answers the
    /// oldest pending write.
    pub fn on_response(&mut self, status: &str) -> TareBehaviorResponse {
        let Some(requested) = self.pending.pop_front() else {
            return TareBehaviorResponse::NoPendingWrite;
        };
        if !tare_behavior_status_ok(status) {
            return TareBehaviorResponse::Rejected {
                requested,
                status: status.to_string(),
            };
        }
        TareBehaviorResponse::Confirmed(requested)
    }
}

/// `device_uuid_response,<response>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceUuidResponse {
    Success,
    AlreadyAssigned,
    ErrorWriteFailed,
    ErrorInvalidFormat,
    ErrorInvalidUuid,
    Unrecognized(String),
}

impl DeviceUuidResponse {
    pub fn parse(response: &str) -> DeviceUuidResponse {
        match response {
            "SUCCESS" => DeviceUuidResponse::Success,
            "ALREADY_ASSIGNED" => DeviceUuidResponse::AlreadyAssigned,
            "ERROR_WRITE_FAILED" => DeviceUuidResponse::ErrorWriteFailed,
            "ERROR_INVALID_FORMAT" => DeviceUuidResponse::ErrorInvalidFormat,
            "ERROR_INVALID_UUID" => DeviceUuidResponse::ErrorInvalidUuid,
            other => DeviceUuidResponse::Unrecognized(other.to_string()),
        }
    }
}

/// A log record the device UUID enrollment produces, with Python's level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceUuidLog {
    /// ERROR `ESP32 reported an invalid device UUID: %r`.
    ReportedInvalid(String),
    /// ERROR: the assignment was sent `DEVICE_UUID_MAX_ASSIGNMENT_ATTEMPTS`
    /// times already.
    AssignmentsExhausted,
    /// INFO: `SUCCESS` or `ALREADY_ASSIGNED`, awaiting ESPInfo.
    Accepted(DeviceUuidResponse),
    /// ERROR: `ERROR_WRITE_FAILED` with nothing pending.
    WriteFailedNothingPending,
    /// ERROR: `ERROR_WRITE_FAILED` after the last attempt.
    WriteFailedExhausted,
    /// WARNING: `ERROR_WRITE_FAILED` while a retry is scheduled.
    WriteFailedRetryPending,
    /// WARNING: `ERROR_WRITE_FAILED`, scheduling attempt `attempt`.
    WriteFailedScheduling { attempt: u32 },
    /// ERROR: `ERROR_INVALID_FORMAT` or `ERROR_INVALID_UUID`.
    Rejected(String),
    /// ERROR: any other response.
    Unrecognized(String),
}

impl DeviceUuidLog {
    /// The Python logging level of the record.
    pub fn level(&self) -> &'static str {
        match self {
            DeviceUuidLog::Accepted(_) => "info",
            DeviceUuidLog::WriteFailedRetryPending
            | DeviceUuidLog::WriteFailedScheduling { .. } => "warning",
            _ => "error",
        }
    }

    /// The text `machine.py` logs.
    pub fn message(&self) -> String {
        const PREFIX: &str = "ESP32 device UUID assignment response:";
        let max = DEVICE_UUID_MAX_ASSIGNMENT_ATTEMPTS;
        match self {
            DeviceUuidLog::ReportedInvalid(uuid) => {
                format!("ESP32 reported an invalid device UUID: '{uuid}'")
            }
            DeviceUuidLog::AssignmentsExhausted => format!(
                "ESP32 device UUID assignment exhausted {max} attempts; the OTA identity cache \
                 will remain unchanged until ESPInfo confirms a valid UUID"
            ),
            DeviceUuidLog::Accepted(DeviceUuidResponse::AlreadyAssigned) => format!(
                "{PREFIX} ALREADY_ASSIGNED; awaiting ESPInfo confirmation of the stored UUID"
            ),
            DeviceUuidLog::Accepted(_) => {
                format!("{PREFIX} SUCCESS; awaiting ESPInfo confirmation")
            }
            DeviceUuidLog::WriteFailedNothingPending => format!(
                "{PREFIX} ERROR_WRITE_FAILED, but no same-boot assignment is pending; no retry scheduled"
            ),
            DeviceUuidLog::WriteFailedExhausted => format!(
                "{PREFIX} ERROR_WRITE_FAILED; exhausted {max} attempts and the OTA identity cache \
                 remains unchanged"
            ),
            DeviceUuidLog::WriteFailedRetryPending => {
                format!("{PREFIX} ERROR_WRITE_FAILED; a bounded retry is already scheduled")
            }
            DeviceUuidLog::WriteFailedScheduling { attempt } => format!(
                "{PREFIX} ERROR_WRITE_FAILED; scheduling attempt {attempt} of {max} in \
                 {DEVICE_UUID_RETRY_DELAY_SECONDS:.1} seconds"
            ),
            DeviceUuidLog::Rejected(response) => {
                format!("{PREFIX} {response}; the OTA identity cache remains unchanged")
            }
            DeviceUuidLog::Unrecognized(response) => {
                format!("{PREFIX} {response}; unrecognized response and no retry scheduled")
            }
        }
    }
}

/// One thing the caller does for the enrollment, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceUuidStep {
    /// Cancel the scheduled retry timer.
    CancelRetry,
    /// Write these bytes to the ESP32.
    Write(Vec<u8>),
    /// Start a `DEVICE_UUID_RETRY_DELAY_SECONDS` timer that calls
    /// [`DeviceUuidEnrollment::retry_fired`].
    ScheduleRetry,
    /// The ESP32 confirmed this UUID: `update_device_uuid_cache(uuid)` (an
    /// `OSError` is logged with its traceback and ends the step); when the
    /// cache changed, log it and, unless emulated, restart
    /// `rauc-hawkbit-updater.service` in a background thread
    /// (`systemctl --no-block restart rauc-hawkbit-updater.service`,
    /// timeout `HAWKBIT_UPDATER_RESTART_TIMEOUT_SECONDS`; errors are logged,
    /// a nonzero exit is a warning).
    UpdateCache(String),
    /// Log this record.
    Log(DeviceUuidLog),
}

/// `Machine.pending_device_uuid_assignment`,
/// `device_uuid_assignment_attempts` and `device_uuid_retry_timer`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceUuidEnrollment {
    pub pending: Option<String>,
    pub attempts: u32,
    pub retry_scheduled: bool,
}

impl DeviceUuidEnrollment {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Machine._resetDeviceUUIDEnrollment`, which a boot banner also runs.
    pub fn reset(&mut self) -> Vec<DeviceUuidStep> {
        let steps = self.cancel_retry();
        self.pending = None;
        self.attempts = 0;
        steps
    }

    fn cancel_retry(&mut self) -> Vec<DeviceUuidStep> {
        let was_scheduled = std::mem::take(&mut self.retry_scheduled);
        if was_scheduled {
            vec![DeviceUuidStep::CancelRetry]
        } else {
            Vec::new()
        }
    }

    /// `Machine.syncDeviceUUID`, on every ESPInfo. `new_candidate` is
    /// `generate_device_uuid` (a fresh UUIDv4), called only when an
    /// assignment starts.
    pub fn sync(
        &mut self,
        device_uuid: &str,
        device_uuid_supported: bool,
        new_candidate: impl FnOnce() -> String,
    ) -> Vec<DeviceUuidStep> {
        if is_valid_device_uuid(device_uuid) {
            let mut steps = self.reset();
            steps.push(DeviceUuidStep::UpdateCache(device_uuid.to_string()));
            return steps;
        }
        if !device_uuid_supported {
            return self.reset();
        }
        let mut steps = Vec::new();
        if !device_uuid.is_empty() {
            steps.push(DeviceUuidStep::Log(DeviceUuidLog::ReportedInvalid(
                device_uuid.to_string(),
            )));
        }
        if self.pending.is_none() {
            // get_device_uuid_assignment(reported, True, None) generates a
            // new UUID whenever the reported one is not valid.
            self.pending = Some(new_candidate());
            self.attempts = 0;
            steps.extend(self.send());
        }
        steps
    }

    /// `Machine._sendDeviceUUIDAssignment`.
    fn send(&mut self) -> Vec<DeviceUuidStep> {
        let Some(candidate) = &self.pending else {
            return Vec::new();
        };
        if self.attempts >= DEVICE_UUID_MAX_ASSIGNMENT_ATTEMPTS {
            return vec![DeviceUuidStep::Log(DeviceUuidLog::AssignmentsExhausted)];
        }
        self.attempts += 1;
        vec![DeviceUuidStep::Write(device_uuid_assign(candidate))]
    }

    /// `Machine._retryDeviceUUIDAssignment`: the retry timer fired.
    pub fn retry_fired(&mut self) -> Vec<DeviceUuidStep> {
        self.retry_scheduled = false;
        self.send()
    }

    /// `Machine.handleDeviceUUIDResponse`.
    pub fn handle_response(&mut self, response: &str) -> Vec<DeviceUuidStep> {
        let parsed = DeviceUuidResponse::parse(response);
        match parsed {
            DeviceUuidResponse::Success | DeviceUuidResponse::AlreadyAssigned => {
                let mut steps = self.cancel_retry();
                steps.push(DeviceUuidStep::Log(DeviceUuidLog::Accepted(parsed)));
                steps
            }
            DeviceUuidResponse::ErrorWriteFailed => {
                let log = if self.pending.is_none() {
                    DeviceUuidLog::WriteFailedNothingPending
                } else if self.attempts >= DEVICE_UUID_MAX_ASSIGNMENT_ATTEMPTS {
                    DeviceUuidLog::WriteFailedExhausted
                } else if self.retry_scheduled {
                    DeviceUuidLog::WriteFailedRetryPending
                } else {
                    self.retry_scheduled = true;
                    return vec![
                        DeviceUuidStep::Log(DeviceUuidLog::WriteFailedScheduling {
                            attempt: self.attempts + 1,
                        }),
                        DeviceUuidStep::ScheduleRetry,
                    ];
                };
                vec![DeviceUuidStep::Log(log)]
            }
            DeviceUuidResponse::ErrorInvalidFormat | DeviceUuidResponse::ErrorInvalidUuid => {
                vec![DeviceUuidStep::Log(DeviceUuidLog::Rejected(
                    response.to_string(),
                ))]
            }
            DeviceUuidResponse::Unrecognized(other) => {
                vec![DeviceUuidStep::Log(DeviceUuidLog::Unrecognized(other))]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The branches of `Machine._handleNvsResponse`.
    #[test]
    fn nvs_replies() {
        assert_eq!(classify_nvs_response("color_key", "x"), NvsReply::Ignored);
        assert_eq!(
            classify_nvs_response("mileage_key", "SUCCESS"),
            NvsReply::MileageWriteAccepted
        );
        assert_eq!(
            classify_nvs_response("mileage_key", "ERROR: mileage seed not requested"),
            NvsReply::MileageWriteRejected("ERROR: mileage seed not requested".to_string())
        );
        assert_eq!(
            classify_nvs_response("mileage_key", " 42 "),
            NvsReply::Mileage(42)
        );
        assert_eq!(
            classify_nvs_response("mileage_key", "4.2"),
            NvsReply::MileageUnparseable("4.2".to_string())
        );
        assert!(tare_behavior_status_ok("success"));
        assert!(!tare_behavior_status_ok("ERROR"));
    }

    /// The retry conditions of `_read_data`.
    #[test]
    fn request_timing() {
        assert!(!esp_info_request_due(false, false, None, 10.0));
        assert!(!esp_info_request_due(true, true, None, 10.0));
        assert!(esp_info_request_due(true, false, None, 10.0));
        assert!(!esp_info_request_due(true, false, Some(8.5), 10.0));
        assert!(esp_info_request_due(true, false, Some(8.0), 10.0));
        assert!(mileage_request_due(true, false, 0.0, 5.0));
        assert!(!mileage_request_due(true, false, 1.0, 5.0));
        assert!(!mileage_request_due(true, true, 0.0, 5.0));
    }

    #[test]
    fn partial_retraction_and_auto_purge_skip_written_values() {
        assert!(partial_retraction_in_sync(Some(45.33), 45.330_000_5));
        assert!(!partial_retraction_in_sync(Some(f64::NAN), 45.0));
        assert!(!partial_retraction_in_sync(None, 45.0));
        assert!(auto_purge_in_sync(Some(true), true));
        assert!(!auto_purge_in_sync(None, false));
    }
}
