//! The line dispatch of `Machine._read_data` in `machine.py`: what each line
//! the ESP32 sends is, with every parsing decision and none of the side
//! effects. The backend acts on the returned [`EspLine`].
//!
//! The Python loop, per line, in this order (the backend keeps it):
//!
//! 1. `data_bytes.decode("utf-8")`; on failure the line is skipped entirely,
//!    `check_timeouts` included ([`decode_line`]).
//! 2. A boot banner ([`is_boot_banner`]) goes to
//!    `ESPObservability::observe_raw_line` first and resets the handshake
//!    state (`esp_info`, `infoReady`, pending tare writes, device UUID
//!    enrollment).
//! 3. The line is dispatched ([`classify_line`]) and its handler runs.
//! 4. A line that is not valid traffic and not a boot banner goes to
//!    `observe_raw_line` ([`ClassifiedLine::observe_as_raw_line`]).
//! 5. `ESPBoot` goes to `observe_boot_reason`.
//! 6. Valid traffic goes to `observe_valid_message`
//!    ([`ClassifiedLine::valid_message_type`]), with the firmware version
//!    when the line was a parsed `ESPInfo`.
//! 7. `check_timeouts` runs after every read, with or without a line.

use indexmap::IndexMap;
use pyfmt::strip_crlf;

use crate::data::{
    ButtonEventData, EspInfo, HeaterTimeoutError, HeaterTimeoutInfo, MachineNotify, SensorData,
    ShotData,
};

/// `data_bytes.decode("utf-8")`: `None` where Python logs "decoding fails"
/// and skips the rest of the loop iteration.
pub fn decode_line(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes).ok()
}

/// The boot-banner check of `_read_data`, on the decoded line as read
/// (terminator included): the ROM bootloader's first line after a reset.
///
/// PARITY: it looks at the raw line, so a line with a leading `\r` is not a
/// banner here, while `ESPObservability::observe_raw_line` (which strips
/// first) treats it as one.
pub fn is_boot_banner(data_str: &str) -> bool {
    data_str.starts_with("rst:0x")
        && data_str.contains("boot:0x")
        && data_str.contains(" (SPI_FAST_FLASH_BOOT)")
}

/// `data_str.strip("\r\n").split(",")`: the fields `_read_data` matches on.
pub fn split_fields(data_str: &str) -> Vec<&str> {
    strip_crlf(data_str).split(',').collect()
}

/// The parsed `Log,<level>,<message>[,<key>=<value>...]` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EspLog {
    /// `log_data[0].lower()`: `debug`, `info` and `warning` are logged at
    /// that level, `error` as `ESP error: <message>` at ERROR (and to
    /// Sentry), anything else not at all.
    pub level: String,
    /// `log_data[1]`, which goes to `SmokeValidationManager.observe_esp_log`
    /// and is the Sentry event message.
    pub message: String,
    /// `",".join(log_data[1:])`, the text logged for debug/info/warning.
    pub full_message: String,
    /// `items_filtered`: the `key=value` items without `sentry`, first
    /// occurrence of a key wins; `None` when the line has no item fields.
    /// It is the `esp-data` Sentry context.
    pub items: Option<IndexMap<String, String>>,
    /// Item fields without `=`, each logged as a warning
    /// `Error parsing ESP log item: <item>`.
    pub malformed_items: Vec<String>,
    /// Report the line to Sentry: an item `sentry=true`, or level `error`.
    pub send_to_sentry: bool,
}

/// The `Log` branch raised; Python logs `Error processing ESP log (IndexError)`
/// at ERROR.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Error processing ESP log (IndexError)")]
pub struct LogWithoutMessage;

impl EspLog {
    /// The `Log` branch of `_read_data`, on the fields after `Log`.
    pub fn from_args(log_data: &[&str]) -> Result<EspLog, LogWithoutMessage> {
        let (Some(level), Some(message)) = (log_data.first(), log_data.get(1)) else {
            return Err(LogWithoutMessage);
        };
        let level = level.to_lowercase();
        let full_message = log_data[1..].join(",");
        let mut items = None;
        let mut malformed_items = Vec::new();
        let mut send_to_sentry = false;
        if log_data.len() > 2 {
            let mut all: IndexMap<String, String> = IndexMap::new();
            for item in &log_data[2..] {
                let parts: Vec<&str> = item.split('=').collect();
                let (Some(key), Some(value)) = (parts.first(), parts.get(1)) else {
                    malformed_items.push((*item).to_string());
                    continue;
                };
                // PARITY: the value ends at the next '=' ("a=b=c" gives "b").
                all.entry((*key).to_string())
                    .or_insert_with(|| (*value).to_string());
            }
            send_to_sentry = all.get("sentry").map(String::as_str) == Some("true");
            all.shift_remove("sentry");
            items = Some(all);
        }
        send_to_sentry = send_to_sentry || level == "error";
        Ok(EspLog {
            level,
            message: (*message).to_string(),
            full_message,
            items,
            malformed_items,
            send_to_sentry,
        })
    }
}

/// A line on which Python raises out of `_read_data`: the exception ends the
/// coroutine and with it the `MachineSerial` thread, so the Python backend
/// stops reading the UART until it restarts. The Rust backend cannot sensibly
/// copy that; it should log the line at ERROR and treat it as free text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Uncaught {
    /// `Data` with fewer than four fields: `ShotData.from_args` reads
    /// `args[3]` outside its try.
    #[error("Data line with {0} fields: IndexError in ShotData.from_args")]
    ShortData(usize),
    /// `Notify` without fields: `notifyArgs[0]`.
    #[error("Notify line without fields: IndexError")]
    EmptyNotify,
}

/// What a line from the ESP32 is: the cases of the `match` in `_read_data`,
/// in its order.
#[derive(Debug, Clone, PartialEq)]
pub enum EspLine {
    /// A bare `CCW`, `CW`, `push`, `pu_d`, `elng`, `ta_d`, `ta_l` or `strt`,
    /// or `Event,<token>[,<ms>]`. `None` when `ButtonEventData.from_args`
    /// failed (no token, unknown token); the line is still valid traffic.
    Event(Option<ButtonEventData>),
    /// `Data,...`; `None` when `ShotData.from_args` returned `None`.
    Data(Option<ShotData>),
    /// `Sensors,...`, either the color-coded single field or the plain list.
    Sensors(Option<SensorData>),
    /// `ESPInfo,...`.
    EspInfo(Option<EspInfo>),
    /// Exactly `ESPBoot,<reason>,<code>`.
    EspBoot { reason: String, code: String },
    /// Exactly `device_uuid_response,<response>`, for
    /// `Machine.handleDeviceUUIDResponse` (see `sync::DeviceUuidResponse`).
    DeviceUuidResponse(String),
    /// Exactly `nvs_response,tare_behavior_key,<status>`, for
    /// `Machine.handleTareBehaviorNVSResponse`.
    TareBehaviorNvsResponse(String),
    /// Exactly `nvs_response,disturbance_resilience_key,<status>`: the
    /// answer to the write a settings request waits on.
    DisturbanceResilienceNvsResponse(String),
    /// `Notify,<type>[,<message>...]`: the message is the remaining fields
    /// joined with `,`, with every `;` turned into a newline.
    Notify(MachineNotify),
    /// `HeaterTimeoutInfo,...`. On `Err` Python logs
    /// `Error processing HeaterTimeoutInfo: <e>` at ERROR; the line is still
    /// valid traffic.
    HeaterTimeoutInfo(Result<HeaterTimeoutInfo, HeaterTimeoutError>),
    /// `Log,...`. `Err` is logged at ERROR; the line is still valid traffic.
    Log(Result<EspLog, LogWithoutMessage>),
    /// Exactly `MileageRequest`.
    MileageRequest,
    /// Exactly `nvs_response,<key>,<value>` (any key but
    /// `tare_behavior_key` and `disturbance_resilience_key`), for `Machine._handleNvsResponse`
    /// (see `sync::NvsReply`).
    NvsResponse { key: String, value: String },
    /// Any other `nvs_response` line, logged as a warning
    /// `ESP nvs_response error: <text>` with the fields after the first
    /// joined by `,`.
    NvsError(String),
    /// Free text: logged at INFO, not valid traffic.
    Unknown,
    /// Python raises here; see [`Uncaught`].
    Uncaught(Uncaught),
}

const BARE_BUTTON_TOKENS: [&str; 8] = ["CCW", "CW", "push", "pu_d", "elng", "ta_d", "ta_l", "strt"];

/// The `match (data_str_sensors)` of `_read_data`, on the decoded line.
pub fn classify_line(data_str: &str) -> EspLine {
    let fields = split_fields(data_str);
    let owned = |s: &str| s.to_string();
    match fields.as_slice() {
        [token] if BARE_BUTTON_TOKENS.contains(token) => {
            EspLine::Event(ButtonEventData::from_args(&[*token]))
        }
        ["Event", event_data @ ..] => EspLine::Event(ButtonEventData::from_args(event_data)),
        ["Data", data_args @ ..] => match ShotData::from_args(data_args) {
            Ok(data) => EspLine::Data(data),
            Err(error) => EspLine::Uncaught(Uncaught::ShortData(error.fields)),
        },
        ["Sensors", color_coded] => {
            EspLine::Sensors(SensorData::from_color_coded_args(color_coded))
        }
        ["Sensors", sensor_args @ ..] => EspLine::Sensors(SensorData::from_args(sensor_args)),
        ["ESPInfo", info_args @ ..] => EspLine::EspInfo(EspInfo::from_args(info_args)),
        ["ESPBoot", reason, code] => EspLine::EspBoot {
            reason: owned(reason),
            code: owned(code),
        },
        ["device_uuid_response", response] => EspLine::DeviceUuidResponse(owned(response)),
        ["nvs_response", "tare_behavior_key", status] => {
            EspLine::TareBehaviorNvsResponse(owned(status))
        }
        ["nvs_response", "disturbance_resilience_key", status] => {
            EspLine::DisturbanceResilienceNvsResponse(owned(status))
        }
        ["Notify", notify_args @ ..] => match notify_args.split_first() {
            Some((notification_type, rest)) => EspLine::Notify(MachineNotify {
                notification_type: owned(notification_type),
                message: rest.join(",").replace(';', "\n"),
            }),
            None => EspLine::Uncaught(Uncaught::EmptyNotify),
        },
        ["HeaterTimeoutInfo", timeout_args @ ..] => {
            EspLine::HeaterTimeoutInfo(HeaterTimeoutInfo::from_args(timeout_args))
        }
        ["Log", log_data @ ..] => EspLine::Log(EspLog::from_args(log_data)),
        ["MileageRequest"] => EspLine::MileageRequest,
        // PARITY: exactly three fields; a value with a comma in it is an
        // nvs_response error, although the firmware sends it as the rest of
        // the line.
        ["nvs_response", key, value] => EspLine::NvsResponse {
            key: owned(key),
            value: owned(value),
        },
        ["nvs_response", nvs_error @ ..] => EspLine::NvsError(nvs_error.join(",")),
        _ => EspLine::Unknown,
    }
}

impl EspLine {
    /// The `valid_message_type` `_read_data` sets for the line, which is
    /// what `observe_valid_message` receives.
    pub fn message_type(&self) -> Option<&'static str> {
        match self {
            EspLine::Event(_) => Some("Event"),
            EspLine::Data(_) => Some("Data"),
            EspLine::Sensors(_) => Some("Sensors"),
            EspLine::EspInfo(_) => Some("ESPInfo"),
            EspLine::EspBoot { .. } => Some("ESPBoot"),
            EspLine::DeviceUuidResponse(_) => Some("DeviceUUIDResponse"),
            EspLine::TareBehaviorNvsResponse(_)
            | EspLine::DisturbanceResilienceNvsResponse(_)
            | EspLine::NvsResponse { .. }
            | EspLine::NvsError(_) => Some("NVSResponse"),
            EspLine::Notify(_) => Some("Notify"),
            EspLine::HeaterTimeoutInfo(_) => Some("HeaterTimeoutInfo"),
            EspLine::Log(_) => Some("Log"),
            EspLine::MileageRequest => Some("MileageRequest"),
            EspLine::Unknown | EspLine::Uncaught(_) => None,
        }
    }
}

/// A decoded line with the two decisions `_read_data` takes on it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedLine {
    pub boot_banner: bool,
    pub line: EspLine,
}

impl ClassifiedLine {
    pub fn new(data_str: &str) -> ClassifiedLine {
        ClassifiedLine {
            boot_banner: is_boot_banner(data_str),
            line: classify_line(data_str),
        }
    }

    /// `is_valid_message and valid_message_type is not None`: the type to
    /// pass to `observe_valid_message`, or `None` when the line is not valid
    /// traffic (free text, a boot banner).
    pub fn valid_message_type(&self) -> Option<&'static str> {
        if self.boot_banner {
            return None;
        }
        self.line.message_type()
    }

    /// `(not is_valid_message or valid_message_type is None) and not
    /// is_boot_banner`: the line goes to `observe_raw_line` after the
    /// dispatch (a boot banner went there before it).
    pub fn observe_as_raw_line(&self) -> bool {
        !self.boot_banner && self.line.message_type().is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_is_strict_utf8() {
        assert_eq!(decode_line(b"Data,1\n"), Some("Data,1\n"));
        assert_eq!(decode_line(b"\xff\n"), None);
    }

    /// The `Log` branch of `_read_data`.
    #[test]
    fn log_items() {
        let line = "Log,WARNING,low voltage,voltage=11.2,sentry=true,bad,v=1=2,voltage=0\r\n";
        let EspLine::Log(Ok(log)) = classify_line(line) else {
            panic!("not a log line");
        };
        assert_eq!(log.level, "warning");
        assert_eq!(log.message, "low voltage");
        assert_eq!(
            log.full_message,
            "low voltage,voltage=11.2,sentry=true,bad,v=1=2,voltage=0"
        );
        let items: Vec<(&str, &str)> = log
            .items
            .as_ref()
            .expect("items")
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(items, [("voltage", "11.2"), ("v", "1")]);
        assert_eq!(log.malformed_items, ["bad"]);
        assert!(log.send_to_sentry);
        assert_eq!(
            classify_line("Log,info"),
            EspLine::Log(Err(LogWithoutMessage))
        );
    }

    #[test]
    fn notify_and_nvs_error_join_their_fields() {
        assert_eq!(
            classify_line("Notify,warning,a;b,c\n"),
            EspLine::Notify(MachineNotify {
                notification_type: "warning".to_string(),
                message: "a\nb,c".to_string(),
            })
        );
        assert_eq!(
            classify_line("nvs_response,ERROR: x,y,z"),
            EspLine::NvsError("ERROR: x,y,z".to_string())
        );
        assert_eq!(classify_line("tare"), EspLine::Unknown);
    }
}
