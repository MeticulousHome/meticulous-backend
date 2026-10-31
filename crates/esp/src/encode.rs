//! The bytes the backend writes to the ESP32. Every message ends with
//! `\x03`; the firmware treats anything before a message as part of it, so
//! nothing else may be written.

use md5::{Digest, Md5};
use pyfmt::{dumps_default, repr_f64};
use serde_json::Value;

use crate::pynum::PyNum;

/// The terminator of every message to the ESP32.
pub const TERMINATOR: &str = "\x03";

/// `Machine.ALLOWED_ESP_ACTIONS`: the actions Socket.IO `action` forwards.
pub const ALLOWED_ESP_ACTIONS: [&str; 10] = [
    "start",
    "stop",
    "tare",
    "scale_master_calibration",
    "preheat",
    "continue",
    "finish",
    "home",
    "purge",
    "continue",
];

/// `Machine.ALLOWED_BACKEND_ACTIONS`: the actions the backend handles itself.
pub const ALLOWED_BACKEND_ACTIONS: [&str; 2] = ["reset", "abort"];

fn message(text: String) -> Vec<u8> {
    let mut bytes = text.into_bytes();
    bytes.extend_from_slice(TERMINATOR.as_bytes());
    bytes
}

/// `Machine.action`: `action,<action>\x03`.
pub fn action(action_event: &str) -> Vec<u8> {
    message(format!("action,{action_event}"))
}

/// `Machine._greet`: two writes, after the input buffer was flushed. The
/// lone terminator ends whatever partial message the ESP32 holds.
pub fn greeting() -> [Vec<u8>; 2] {
    [TERMINATOR.as_bytes().to_vec(), action("info")]
}

/// `esp_nvs_keys` in `machine.py`: the NVS keys the backend reads and writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NvsKey {
    Color,
    SerialNumber,
    BatchNumber,
    BuildDate,
    PartialRetraction,
    AutoPurgeAfterShot,
    TareBehavior,
    DisturbanceResilience,
    Mileage,
}

impl NvsKey {
    pub const ALL: [NvsKey; 9] = [
        NvsKey::Color,
        NvsKey::SerialNumber,
        NvsKey::BatchNumber,
        NvsKey::BuildDate,
        NvsKey::PartialRetraction,
        NvsKey::AutoPurgeAfterShot,
        NvsKey::TareBehavior,
        NvsKey::DisturbanceResilience,
        NvsKey::Mileage,
    ];

    /// The key's value on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            NvsKey::Color => "color_key",
            NvsKey::SerialNumber => "serial_number_key",
            NvsKey::BatchNumber => "batch_number_key",
            NvsKey::BuildDate => "build_date_key",
            NvsKey::PartialRetraction => "partial_retraction_key",
            NvsKey::AutoPurgeAfterShot => "auto_purge_after_shot_key",
            NvsKey::TareBehavior => "tare_behavior_key",
            NvsKey::DisturbanceResilience => "disturbance_resilience_key",
            NvsKey::Mileage => "mileage_key",
        }
    }

    pub fn from_wire(key: &str) -> Option<NvsKey> {
        NvsKey::ALL.into_iter().find(|k| k.as_str() == key)
    }
}

/// `nvs_request,write,<key>,<value>\x03`.
pub fn nvs_write(key: NvsKey, value: &str) -> Vec<u8> {
    message(format!("nvs_request,write,{},{value}", key.as_str()))
}

/// `nvs_request,read,<key>\x03`.
pub fn nvs_read(key: NvsKey) -> Vec<u8> {
    message(format!("nvs_request,read,{}", key.as_str()))
}

/// `Machine.setPartialRetraction`: `str(float(value))`.
pub fn partial_retraction_write(value: f64) -> Vec<u8> {
    nvs_write(NvsKey::PartialRetraction, &repr_f64(value))
}

/// `Machine.setAutoPurgeAfterShot`: `true`/`false`.
pub fn auto_purge_write(value: bool) -> Vec<u8> {
    nvs_write(
        NvsKey::AutoPurgeAfterShot,
        if value { "true" } else { "false" },
    )
}

/// `Machine.setTareBehavior`.
pub fn tare_behavior_write(value: &str) -> Vec<u8> {
    nvs_write(NvsKey::TareBehavior, value)
}

/// The disturbance resilience write the settings handler waits on:
/// `true`/`false`.
pub fn disturbance_resilience_write(value: bool) -> Vec<u8> {
    nvs_write(
        NvsKey::DisturbanceResilience,
        if value { "true" } else { "false" },
    )
}

/// `Machine._handleMileageRequest`: the shot count that seeds the counter.
pub fn mileage_seed_write(seed: i64) -> Vec<u8> {
    nvs_write(NvsKey::Mileage, &seed.to_string())
}

/// `Machine.requestMileage`.
pub fn mileage_read() -> Vec<u8> {
    nvs_read(NvsKey::Mileage)
}

/// `Machine.setSerial`: four writes, in this order.
pub fn serial_writes(
    color: &str,
    serial: &str,
    batch_number: &str,
    build_date: &str,
) -> [Vec<u8>; 4] {
    [
        nvs_write(NvsKey::Color, color),
        nvs_write(NvsKey::SerialNumber, serial),
        nvs_write(NvsKey::BatchNumber, batch_number),
        nvs_write(NvsKey::BuildDate, build_date),
    ]
}

/// `Machine._sendDeviceUUIDAssignment`: `device_uuid,assign,<uuid>\x03`.
pub fn device_uuid_assign(uuid: &str) -> Vec<u8> {
    message(format!("device_uuid,assign,{uuid}"))
}

/// `HeaterActuator.set_timeout` rejected the value with `ValueError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Timeout must be between 0 and 60 minutes")]
pub struct HeaterTimeoutOutOfRange(pub i64);

/// `HeaterActuator.set_timeout`: `heater_timeout,<minutes>\x03`, 0 to 60.
pub fn heater_timeout(minutes: i64) -> Result<Vec<u8>, HeaterTimeoutOutOfRange> {
    if !(0..=60).contains(&minutes) {
        return Err(HeaterTimeoutOutOfRange(minutes));
    }
    Ok(message(format!("heater_timeout,{minutes}")))
}

/// The Socket.IO `calibrate` handler in `backend.py`:
/// `action,calibration,100.0,<str(current weight)>\x03`. PARITY: the weight
/// is `Machine.data_sensors.weight`, so it is written as `NaN` while the load
/// cell is inactive.
pub fn calibration(current_weight: &PyNum) -> Vec<u8> {
    message(format!(
        "action,calibration,100.0,{}",
        current_weight.to_py_string()
    ))
}

/// A profile as `Machine.send_json_with_hash` streams it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileUpload {
    /// `json.dumps(profile)`.
    pub json: String,
    /// The md5 of the JSON text, lowercase hex.
    pub md5_hex: String,
    /// Everything written, in order: `hash,<md5>\x03json\n<json>\x03`.
    /// Python writes it in four calls (`hash,`, the hash, `\x03`, the rest);
    /// on a serial port that is the same byte stream.
    pub bytes: Vec<u8>,
}

/// `Machine.send_json_with_hash`. The hash covers `json_data[5:-1]`, which
/// is exactly the JSON text.
pub fn send_json_with_hash(profile: &Value) -> ProfileUpload {
    let json = dumps_default(profile);
    let md5_hex: String = Md5::digest(json.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut bytes = Vec::with_capacity(json.len() + 48);
    bytes.extend_from_slice(b"hash,");
    bytes.extend_from_slice(md5_hex.as_bytes());
    bytes.extend_from_slice(TERMINATOR.as_bytes());
    bytes.extend_from_slice(b"json\n");
    bytes.extend_from_slice(json.as_bytes());
    bytes.extend_from_slice(TERMINATOR.as_bytes());
    ProfileUpload {
        json,
        md5_hex,
        bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected bytes are the literals `machine.py`, `backend.py` and
    /// `heater_actuator.py` build.
    #[test]
    fn messages_are_terminated_and_formatted_like_python() {
        assert_eq!(action("info"), b"action,info\x03");
        assert_eq!(greeting(), [b"\x03".to_vec(), b"action,info\x03".to_vec()]);
        assert_eq!(
            partial_retraction_write(45.0),
            b"nvs_request,write,partial_retraction_key,45.0\x03"
        );
        assert_eq!(
            auto_purge_write(false),
            b"nvs_request,write,auto_purge_after_shot_key,false\x03"
        );
        assert_eq!(mileage_read(), b"nvs_request,read,mileage_key\x03");
        assert_eq!(
            mileage_seed_write(12),
            b"nvs_request,write,mileage_key,12\x03"
        );
        assert_eq!(heater_timeout(0).unwrap(), b"heater_timeout,0\x03");
        assert!(heater_timeout(61).is_err());
        assert!(heater_timeout(-1).is_err());
        assert_eq!(
            calibration(&PyNum::NanString),
            b"action,calibration,100.0,NaN\x03"
        );
        assert_eq!(
            calibration(&PyNum::Int(100)),
            b"action,calibration,100.0,100\x03"
        );
    }

    /// The md5 is the one `hashlib.md5(json.dumps(obj).encode())` gives.
    #[test]
    fn profile_upload_hashes_the_json_text() {
        let upload = send_json_with_hash(&serde_json::json!({"a": 1, "b": [1.5, null]}));
        assert_eq!(upload.json, r#"{"a": 1, "b": [1.5, null]}"#);
        assert_eq!(upload.md5_hex, "1eccb94b3a72b663820c29b70e1e4169");
        assert_eq!(
            upload.bytes,
            b"hash,1eccb94b3a72b663820c29b70e1e4169\x03json\n{\"a\": 1, \"b\": [1.5, null]}\x03"
        );
    }
}
