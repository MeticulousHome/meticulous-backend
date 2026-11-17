//! The UART lines of firmware with the Kalman filter, as the firmware's
//! `contracts/uart/protocol.json` defines them: the two Kalman readings after
//! `predicted_weight` in `Sensors`, and the ESPInfo of the firmware that still
//! has the retired disturbance resilience setting. Every case with fields from
//! older firmware guards the step where a freshly updated backend still talks
//! to the previous firmware.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use esp::line::{EspLine, classify_line};
use esp::{EspInfo, PyNum, SensorData};
use serde_json::json;

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
/// ends the shot.
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

/// Firmware from before the setting was dropped still sends it after the
/// device UUID (index 12 in the firmware's ESPInfo); after an update the
/// backend talks to that firmware until it flashes the new one. The line must
/// read exactly like the twelve fields of current firmware: if the extra field
/// shifted anything, the tare behaviour and device UUID enrolment would be
/// wrong, and if it reached the payload the Dial would see the setting again.
#[test]
fn esp_info_ignores_the_retired_field_of_older_firmware() {
    let current = info(None);
    assert_eq!(current.tare_behavior.as_deref(), Some("after_retraction"));
    assert_eq!(current.device_uuid, "123e4567-e89b-42d3-a456-426614174000");
    assert!(current.device_uuid_supported);
    assert_eq!(current.to_args().len(), 12);

    for field in ["true", "false"] {
        let older = info(Some(field));
        assert_eq!(older, current, "{field}");
        assert_eq!(older.to_sio(), current.to_sio(), "{field}");
        let payload = older.to_sio();
        assert_eq!(payload.get("disturbance_resilience"), None);
        assert_eq!(payload.get("disturbance_resilience_supported"), None);
    }
}
