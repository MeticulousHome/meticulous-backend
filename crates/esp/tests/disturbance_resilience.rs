//! The UART side of disturbance resilience, as the firmware's
//! `contracts/uart/protocol.json` defines it: the two Kalman readings after
//! `predicted_weight` in `Sensors`, the `disturbance_resilience` field after
//! the device UUID in `ESPInfo`, the NVS write and its replies. Every case
//! with fields from older firmware guards the step where a freshly updated
//! backend still talks to the previous firmware.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use esp::encode::{self, NvsKey};
use esp::line::{EspLine, classify_line};
use esp::sync::{disturbance_resilience_status_ok, is_keyless_nvs_error};
use esp::{EspInfo, PyNum, SensorData};
use serde_json::{Value, json};

/// The 23 fields every firmware sends, ending with `predicted_weight`.
const SENSORS_23: &str = "90.1,91.2,92.0,93.0,94.0,95.0,85.0,40.0,50.0,10.5,100.0,5.0,2.5,1.0,3.0,9.0,0.1,0.2,0.3,0.4,true,35.0,18.5";

fn sensors(line: &str) -> SensorData {
    match classify_line(line) {
        EspLine::Sensors(Some(sensors)) => sensors,
        other => panic!("{line:?} dispatched as {other:?}"),
    }
}

/// New firmware: `kalman_weight` and `predicted_kalman_weight` reach the
/// `sensors` event as `weight_kalman` and `weight_pred_kalman`, right after
/// `weight_pred`. Without it the Dial and the app never see the weight that
/// ends the shot when disturbance resilience is on.
#[test]
fn sensors_with_kalman_fields_reach_the_sensors_event() {
    let parsed = sensors(&format!("Sensors,{SENSORS_23},17.25,18.75,\n"));
    assert_eq!(parsed.weight_prediction, PyNum::Float(18.5));
    assert_eq!(parsed.kalman_weight, PyNum::Float(17.25));
    assert_eq!(parsed.predicted_kalman_weight, PyNum::Float(18.75));

    let payload = parsed.to_sio_sensors();
    let keys: Vec<&str> = payload
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys[keys.len() - 3..],
        ["weight_pred", "weight_kalman", "weight_pred_kalman"]
    );
    assert_eq!(payload["weight_kalman"], json!(17.25));
    assert_eq!(payload["weight_pred_kalman"], json!(18.75));
}

/// The firmware sends `nan` while the filter has no estimate; it is passed
/// on as `"NaN"`, like `weight_pred`, instead of dropping the line.
#[test]
fn sensors_nan_kalman_readings_are_nan_strings() {
    let parsed = sensors(&format!("Sensors,{SENSORS_23},nan,nan,\n"));
    let payload = parsed.to_sio_sensors();
    assert_eq!(payload["weight_kalman"], json!("NaN"));
    assert_eq!(payload["weight_pred_kalman"], json!("NaN"));
}

/// Firmware before the feature sends 23 fields and a trailing comma: the
/// line still parses and the new readings are 0.0, not `"NaN"` from the
/// empty field after the comma.
#[test]
fn sensors_from_older_firmware_still_parse() {
    for line in [
        format!("Sensors,{SENSORS_23},\n"),
        format!("Sensors,{SENSORS_23}\n"),
    ] {
        let parsed = sensors(&line);
        assert_eq!(parsed.weight_prediction, PyNum::Float(18.5), "{line:?}");
        assert_eq!(parsed.kalman_weight, PyNum::Float(0.0), "{line:?}");
        assert_eq!(
            parsed.predicted_kalman_weight,
            PyNum::Float(0.0),
            "{line:?}"
        );
        let payload = parsed.to_sio_sensors();
        assert_eq!(payload["weight_kalman"], json!(0.0));
        assert_eq!(payload["weight_pred_kalman"], json!(0.0));
    }
}

/// `to_args` (the emulator's `Sensors` lines) writes the new fields in the
/// firmware's order, so they read back.
#[test]
fn sensors_round_trip_through_to_args() {
    let parsed = sensors(&format!("Sensors,{SENSORS_23},17.25,18.75,\n"));
    let args = parsed.to_args();
    assert_eq!(args.len(), 25);
    assert_eq!(SensorData::from_args(&args), Some(parsed));
}

const ESP_INFO_12: [&str; 12] = [
    "1.2.3",
    "off",
    "120",
    "black",
    "SN123",
    "B456",
    "2024-01-01",
    "scale1",
    "45.33",
    "false",
    "after_retraction",
    "123e4567-e89b-42d3-a456-426614174000",
];

fn info(extra: Option<&str>) -> EspInfo {
    let mut fields = ESP_INFO_12.join(",");
    if let Some(extra) = extra {
        fields.push(',');
        fields.push_str(extra);
    }
    match classify_line(&format!("ESPInfo,{fields}\n")) {
        EspLine::EspInfo(Some(info)) => info,
        other => panic!("dispatched as {other:?}"),
    }
}

/// ESPInfo's thirteenth field is the value the ESP32 stores; the payload
/// says so with `disturbance_resilience` and `_supported`, which the Dial
/// uses to show or hide the setting.
#[test]
fn esp_info_reports_disturbance_resilience() {
    for (field, value) in [("true", true), ("false", false)] {
        let parsed = info(Some(field));
        assert_eq!(parsed.disturbance_resilience, Some(value));
        assert_eq!(parsed.tare_behavior.as_deref(), Some("after_retraction"));
        assert!(parsed.device_uuid_supported);
        let payload = parsed.to_sio();
        assert_eq!(payload["disturbance_resilience"], Value::Bool(value));
        assert_eq!(payload["disturbance_resilience_supported"], true);
    }
}

/// Firmware with 12 fields does not support the setting: `null` and
/// `false`, and everything it does send still parses.
#[test]
fn esp_info_from_older_firmware_does_not_support_it() {
    let parsed = info(None);
    assert_eq!(parsed.disturbance_resilience, None);
    assert_eq!(parsed.device_uuid, "123e4567-e89b-42d3-a456-426614174000");
    let payload = parsed.to_sio();
    assert_eq!(payload["disturbance_resilience"], Value::Null);
    assert_eq!(payload["disturbance_resilience_supported"], false);
}

/// `to_args` keeps the field at index 12, filling the tare and UUID slots
/// before it, so the emulator's ESPInfo reads back the same.
#[test]
fn esp_info_round_trips_through_to_args() {
    let with_field = info(Some("true"));
    assert_eq!(
        EspInfo::from_args(&with_field.to_args()),
        Some(with_field.clone())
    );
    // Firmware with the field always has the UUID slot before it.
    let bare = EspInfo {
        device_uuid_supported: true,
        disturbance_resilience: Some(false),
        ..EspInfo::default()
    };
    let args = bare.to_args();
    assert_eq!(args.len(), 13);
    assert_eq!(args[12], "false");
    assert_eq!(EspInfo::from_args(&args), Some(bare));
}

/// The bytes the firmware's NVS parser expects for the bool key.
#[test]
fn nvs_write_bytes() {
    assert_eq!(
        encode::disturbance_resilience_write(true),
        b"nvs_request,write,disturbance_resilience_key,true\x03"
    );
    assert_eq!(
        encode::disturbance_resilience_write(false),
        b"nvs_request,write,disturbance_resilience_key,false\x03"
    );
    assert_eq!(
        NvsKey::from_wire("disturbance_resilience_key"),
        Some(NvsKey::DisturbanceResilience)
    );
}

/// The replies a settings request waits on: the keyed reply goes to the
/// waiting write, only `SUCCESS` confirms it, and old firmware's key-less
/// `ERROR: key not found` is recognised as a refusal.
#[test]
fn nvs_replies() {
    for status in ["SUCCESS", "ERROR"] {
        assert_eq!(
            classify_line(&format!(
                "nvs_response,disturbance_resilience_key,{status}\r\n"
            )),
            EspLine::DisturbanceResilienceNvsResponse(status.to_string())
        );
    }
    assert!(disturbance_resilience_status_ok("SUCCESS"));
    for status in ["ERROR", "success", "true", ""] {
        assert!(!disturbance_resilience_status_ok(status), "{status:?}");
    }

    let old_firmware = classify_line("nvs_response,ERROR: key not found\n");
    assert_eq!(
        old_firmware,
        EspLine::NvsError("ERROR: key not found".to_string())
    );
    assert_eq!(old_firmware.message_type(), Some("NVSResponse"));
    assert!(is_keyless_nvs_error("ERROR: key not found"));
    assert!(!is_keyless_nvs_error("mileage_key,1,2"));
}
