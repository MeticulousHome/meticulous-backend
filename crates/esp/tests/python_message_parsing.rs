//! Port of `tests/test_machine_message_parsing.py`: the parsers of
//! `esp_serial/data.py`, case by case, with the Python test's values.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// 3.14 is the Python test's value, not an approximation of pi.
#![allow(clippy::approx_constant)]

use esp::{
    ButtonEvent, ButtonEventData, EspInfo, HeaterTimeoutInfo, MachineNotify, MachineState, PyNum,
    SensorData, ShotData,
};

fn f(v: f64) -> PyNum {
    PyNum::Float(v)
}

// --- TestSafeFloat / TestSafeFloatWithNan --------------------------------

/// `TestSafeFloat`: `safeFloat`.
#[test]
fn safe_float() {
    assert_eq!(PyNum::safe_float("3.14"), Some(f(3.14)));
    assert_eq!(PyNum::safe_float("0"), Some(f(0.0)));
    assert_eq!(PyNum::safe_float("-1.5"), Some(f(-1.5)));
    // test_nan_returns_zero: the int 0.
    assert_eq!(PyNum::safe_float("nan"), Some(PyNum::Int(0)));
    assert_eq!(PyNum::safe_float("42"), Some(f(42.0)));
    // test_invalid_raises
    assert_eq!(PyNum::safe_float("notanumber"), None);
}

/// `TestSafeFloatWithNan`: `safe_float_with_nan`.
#[test]
fn safe_float_with_nan() {
    assert_eq!(PyNum::safe_float_with_nan("3.14"), f(3.14));
    assert_eq!(PyNum::safe_float_with_nan("nan"), PyNum::NanString);
    assert_eq!(PyNum::safe_float_with_nan("notanumber"), PyNum::NanString);
    assert_eq!(PyNum::safe_float_with_nan("0"), f(0.0));
    assert_eq!(PyNum::safe_float_with_nan("-2.5"), f(-2.5));
}

// --- TestSensorData -------------------------------------------------------

fn sensor_args() -> Vec<String> {
    [
        "90.1", "91.2", "92.0", "93.0", "94.0", "95.0", "85.0", "40.0", "50.0", "10.5", "100.0",
        "5.0", "2.5", "1.0", "3.0", "9.0", "0.1", "0.2", "0.3", "0.4", "true", "35.0", "18.5",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// `TestSensorData.test_parse_all_fields`.
#[test]
fn sensor_parse_all_fields() {
    let data = SensorData::from_args(&sensor_args()).expect("parses");
    assert_eq!(data.external_1, f(90.1));
    assert_eq!(data.external_2, f(91.2));
    assert_eq!(data.bar_up, f(92.0));
    assert_eq!(data.pressure_sensor, f(9.0));
    assert!(data.water_status);
    assert_eq!(data.motor_thermistor, f(35.0));
    assert_eq!(data.weight_prediction, f(18.5));
}

/// `TestSensorData.test_water_status_false`.
#[test]
fn sensor_water_status_false() {
    let mut args = sensor_args();
    args[20] = "false".to_string();
    assert!(!SensorData::from_args(&args).unwrap().water_status);
}

/// `TestSensorData.test_nan_motor_thermistor`.
#[test]
fn sensor_nan_motor_thermistor() {
    let mut args = sensor_args();
    args[21] = "nan".to_string();
    assert_eq!(
        SensorData::from_args(&args).unwrap().motor_thermistor,
        PyNum::NanString
    );
}

/// `TestSensorData.test_too_few_args_returns_none`.
#[test]
fn sensor_too_few_args_returns_none() {
    assert_eq!(SensorData::from_args(&["1.0", "2.0"]), None);
}

/// `TestSensorData.test_roundtrip_to_args`.
#[test]
fn sensor_roundtrip_to_args() {
    let data = SensorData::from_args(&sensor_args()).unwrap();
    let reparsed = SensorData::from_args(&data.to_args()).unwrap();
    assert_eq!(reparsed.external_1, data.external_1);
    assert_eq!(reparsed.pressure_sensor, data.pressure_sensor);
    assert_eq!(reparsed.water_status, data.water_status);
}

/// `TestSensorData.test_to_sio_sensors_keys`.
#[test]
fn sensor_to_sio_sensors_keys() {
    let sio = SensorData::from_args(&sensor_args())
        .unwrap()
        .to_sio_sensors();
    assert!(sio.get("p").is_some());
    assert!(sio.get("t_ext_1").is_some());
    assert!(sio.get("w_stat").is_some());
    assert_eq!(sio["p"], 9.0);
}

// --- TestShotData ---------------------------------------------------------

fn shot_args(with_controllers: bool) -> Vec<String> {
    let mut base = vec!["9.0", "2.5", "18.0", "S", "93.5", "idle", "idle"];
    if with_controllers {
        base.extend(["Pressure", "9.0", "Flow", "2.0", "true", "1.5"]);
    }
    base.into_iter().map(str::to_string).collect()
}

fn parse_shot(args: &[String]) -> ShotData {
    ShotData::from_args(args).unwrap().unwrap()
}

/// `TestShotData.test_parse_with_controllers`.
#[test]
fn shot_parse_with_controllers() {
    let data = parse_shot(&shot_args(true));
    assert_eq!(data.pressure, f(9.0));
    assert_eq!(data.flow, f(2.5));
    assert_eq!(data.weight, f(18.0));
    assert!(data.stable_weight);
    assert_eq!(data.temperature, f(93.5));
    assert_eq!(data.status.as_deref(), Some("idle"));
    assert_eq!(data.profile.as_deref(), Some("idle"));
    assert_eq!(data.main_controller_kind.as_deref(), Some("Pressure"));
    assert_eq!(data.main_setpoint, f(9.0));
    assert_eq!(data.aux_controller_kind.as_deref(), Some("Flow"));
    assert_eq!(data.aux_setpoint, f(2.0));
    assert!(data.is_aux_controller_active);
    assert_eq!(data.gravimetric_flow, f(1.5));
}

/// `TestShotData.test_parse_without_controllers`.
#[test]
fn shot_parse_without_controllers() {
    let data = parse_shot(&shot_args(false));
    assert_eq!(data.main_controller_kind, None);
    assert_eq!(data.aux_controller_kind, None);
    assert_eq!(data.gravimetric_flow, f(0.0));
}

/// `TestShotData.test_unstable_weight`.
#[test]
fn shot_unstable_weight() {
    let mut args = shot_args(true);
    args[3] = "U".to_string();
    assert!(!parse_shot(&args).stable_weight);
}

/// `TestShotData.test_brewing_state`, `test_purge_state`, `test_home_state`.
#[test]
fn shot_states() {
    for (profile, state) in [
        ("MyEspresso", MachineState::BREWING),
        ("Purge", "purge"),
        ("Home", "home"),
    ] {
        let mut args = shot_args(true);
        args[6] = profile.to_string();
        assert_eq!(parse_shot(&args).state, state, "{profile}");
    }
}

/// `TestShotData.test_nan_pressure`.
#[test]
fn shot_nan_pressure() {
    let mut args = shot_args(true);
    args[0] = "nan".to_string();
    assert_eq!(parse_shot(&args).pressure, PyNum::NanString);
}

/// `TestShotData.test_to_sio_structure`.
#[test]
fn shot_to_sio_structure() {
    let sio = parse_shot(&shot_args(true)).to_sio();
    assert!(sio.get("sensors").is_some());
    assert!(sio.get("setpoints").is_some());
    assert_eq!(sio["sensors"]["p"], 9.0);
    assert_eq!(sio["sensors"]["f"], 2.5);
    assert_eq!(sio["setpoints"]["active"], "flow");
    assert_eq!(sio["setpoints"]["pressure"], 9.0);
}

/// `TestShotData.test_roundtrip_to_args`.
#[test]
fn shot_roundtrip_to_args() {
    let data = parse_shot(&shot_args(true));
    let reparsed = parse_shot(&data.to_args());
    assert_eq!(reparsed.pressure, data.pressure);
    assert_eq!(reparsed.flow, data.flow);
    assert_eq!(reparsed.main_controller_kind, data.main_controller_kind);
}

// --- TestESPInfo ----------------------------------------------------------

const DEVICE_UUID: &str = "123e4567-e89b-42d3-a456-426614174000";

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

const BASE: [&str; 8] = [
    "1.2.3",
    "2",
    "24.5",
    "black",
    "SN123",
    "B456",
    "2024-01-01",
    "scale1",
];

fn with(extra: &[&str]) -> Vec<String> {
    let mut args = strings(&BASE);
    args.extend(strings(extra));
    args
}

/// `TestESPInfo.test_parse_full`.
#[test]
fn info_parse_full() {
    let info = EspInfo::from_args(&BASE).unwrap();
    assert_eq!(info.firmware_v, "1.2.3");
    assert_eq!(info.esp_pinout, 2);
    assert_eq!(info.main_voltage, 24.5);
    assert_eq!(info.color, "black");
    assert_eq!(info.serial_number, "SN123");
    assert_eq!(info.batch_number, "B456");
    assert_eq!(info.build_date, "2024-01-01");
    assert_eq!(info.scale_module, "scale1");
    assert!(!info.device_uuid_supported);
    assert_eq!(info.tare_behavior, None);
}

/// `TestESPInfo.test_parse_tare_behavior_from_new_firmware`.
#[test]
fn info_parse_tare_behavior_from_new_firmware() {
    let info = EspInfo::from_args(&with(&["45.33", "false", "before_retraction"])).unwrap();
    assert_eq!(info.tare_behavior.as_deref(), Some("before_retraction"));
    assert!(!info.device_uuid_supported);
    assert_eq!(info.to_sio()["tare_behavior_supported"], true);
}

/// `TestESPInfo.test_parse_device_uuid_from_pre_tare_protocol`.
#[test]
fn info_parse_device_uuid_from_pre_tare_protocol() {
    let info = EspInfo::from_args(&with(&["45.33", "false", DEVICE_UUID])).unwrap();
    assert_eq!(info.tare_behavior, None);
    assert_eq!(info.device_uuid, DEVICE_UUID);
    assert!(info.device_uuid_supported);
    assert!(!info.to_sio().to_string().contains(DEVICE_UUID));
}

/// `TestESPInfo.test_parse_minimal`.
#[test]
fn info_parse_minimal() {
    let info = EspInfo::from_args(&["0.9.1", "1", "23.0"]).unwrap();
    assert_eq!(info.firmware_v, "0.9.1");
    assert_eq!(info.esp_pinout, 1);
    assert_eq!(info.main_voltage, 23.0);
    assert_eq!(info.color, "");
    assert_eq!(info.serial_number, "");
}

/// `TestESPInfo.test_invalid_pinout_defaults_zero`.
#[test]
fn info_invalid_pinout_defaults_zero() {
    let info = EspInfo::from_args(&["1.0.0", "notanint", "24.0", "", "", "", "", ""]).unwrap();
    assert_eq!(info.esp_pinout, 0);
}

/// `TestESPInfo.test_to_sio`.
#[test]
fn info_to_sio() {
    let sio = EspInfo::from_args(&BASE).unwrap().to_sio();
    assert_eq!(sio["firmware_version"], "1.2.3");
    assert_eq!(sio["esp_pinout"], 2);
    assert_eq!(sio["serial_number"], "SN123");
}

/// `TestESPInfo.test_roundtrip_to_args`.
#[test]
fn info_roundtrip_to_args() {
    let info =
        EspInfo::from_args(&with(&["45.0", "false", "before_retraction", DEVICE_UUID])).unwrap();
    let reparsed = EspInfo::from_args(&info.to_args()).unwrap();
    assert_eq!(reparsed.firmware_v, info.firmware_v);
    assert_eq!(reparsed.main_voltage, info.main_voltage);
    assert_eq!(reparsed.color, info.color);
    assert_eq!(reparsed.tare_behavior, info.tare_behavior);
    assert_eq!(reparsed.device_uuid, info.device_uuid);
    assert!(reparsed.device_uuid_supported);
}

/// `TestESPInfo.test_parse_device_uuid_without_exposing_it_to_sio`.
#[test]
fn info_device_uuid_not_exposed_to_sio() {
    let info =
        EspInfo::from_args(&with(&["45.0", "false", "before_retraction", DEVICE_UUID])).unwrap();
    let payload = info.to_sio();
    assert_eq!(info.device_uuid, DEVICE_UUID);
    assert!(info.device_uuid_supported);
    let keys: Vec<&str> = payload
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    let mut expected = vec![
        "firmware_version",
        "esp_pinout",
        "main_voltage",
        "color",
        "serial_number",
        "batch_number",
        "build_date",
        "scale_module",
        "partial_retraction",
        "auto_purge_after_shot",
        "tare_behavior",
        "tare_behavior_supported",
        // Added with disturbance resilience (the firmware's ESPInfo field 12).
        "disturbance_resilience",
        "disturbance_resilience_supported",
    ];
    expected.sort_unstable();
    assert_eq!(sorted, expected);
    assert!(
        payload
            .as_object()
            .unwrap()
            .values()
            .all(|v| v != DEVICE_UUID)
    );
}

/// `TestESPInfo.test_empty_device_uuid_still_reports_protocol_support`.
#[test]
fn info_empty_device_uuid_still_reports_protocol_support() {
    let info = EspInfo::from_args(&with(&["45.0", "false", "before_retraction", ""])).unwrap();
    assert_eq!(info.device_uuid, "");
    assert!(info.device_uuid_supported);
}

/// `TestESPInfo.test_roundtrip_to_args_with_tare_behavior`.
#[test]
fn info_roundtrip_to_args_with_tare_behavior() {
    let info = EspInfo {
        tare_behavior: Some("after_retraction".to_string()),
        ..EspInfo::default()
    };
    let reparsed = EspInfo::from_args(&info.to_args()).unwrap();
    assert_eq!(reparsed.tare_behavior.as_deref(), Some("after_retraction"));
}

/// `TestESPInfo.test_roundtrip_device_uuid_without_tare_behavior_uses_distinct_field`.
#[test]
fn info_roundtrip_device_uuid_without_tare_behavior() {
    let info = EspInfo {
        device_uuid: DEVICE_UUID.to_string(),
        device_uuid_supported: true,
        ..EspInfo::default()
    };
    let output = info.to_args();
    let reparsed = EspInfo::from_args(&output).unwrap();
    assert_eq!(output[10], "");
    assert_eq!(output[11], info.device_uuid);
    assert_eq!(reparsed.tare_behavior, None);
    assert_eq!(reparsed.device_uuid, info.device_uuid);
    assert!(reparsed.device_uuid_supported);
}

// --- TestButtonEventData --------------------------------------------------

fn button(args: &[&str]) -> Option<ButtonEventData> {
    ButtonEventData::from_args(args)
}

/// `TestButtonEventData.test_parse_encoder_clockwise`.
#[test]
fn button_parse_encoder_clockwise() {
    let event = button(&["CW", "150"]).unwrap();
    assert_eq!(event.event, ButtonEvent::EncoderClockwise);
    assert_eq!(event.time_since_last_event, 150);
}

/// `TestButtonEventData.test_parse_tare`, `test_parse_context`,
/// `test_parse_cntx_alias`.
#[test]
fn button_aliases() {
    assert_eq!(button(&["tare", "500"]).unwrap().event, ButtonEvent::Tare);
    assert_eq!(button(&["strt", "0"]).unwrap().event, ButtonEvent::Context);
    assert_eq!(
        button(&["cntx", "100"]).unwrap().event,
        ButtonEvent::Context
    );
}

/// `TestButtonEventData.test_overflow_time`.
#[test]
fn button_overflow_time() {
    assert_eq!(
        button(&["push", "9999+++"]).unwrap().time_since_last_event,
        10000
    );
}

/// `TestButtonEventData.test_no_time_arg`.
#[test]
fn button_no_time_arg() {
    let event = button(&["push"]).unwrap();
    assert_eq!(event.event, ButtonEvent::EncoderPush);
    assert_eq!(event.time_since_last_event, 0);
}

/// `TestButtonEventData.test_unknown_event_returns_none`.
#[test]
fn button_unknown_event_returns_none() {
    assert_eq!(button(&["UNKNOWN_EVENT", "0"]), None);
}

/// `TestButtonEventData.test_button_pressed_released`.
#[test]
fn button_pressed_released() {
    assert_eq!(
        button(&["encoder_button_pressed", "0"]).unwrap().event,
        ButtonEvent::EncoderPressed
    );
    assert_eq!(
        button(&["encoder_button_released", "50"]).unwrap().event,
        ButtonEvent::EncoderReleased
    );
}

/// `TestButtonEventData.test_to_sio`.
#[test]
fn button_to_sio() {
    let sio = button(&["tare", "200"]).unwrap().to_sio();
    assert_eq!(sio["type"], "TARE");
    assert_eq!(sio["time_since_last_event"], 200);
}

// --- TestHeaterTimeoutInfo / TestMachineNotify ----------------------------

/// `TestHeaterTimeoutInfo.test_parse_valid`.
#[test]
fn heater_parse_valid() {
    let info = HeaterTimeoutInfo::from_args(&["300.0", "600.0", "120.0", "180.0"]).unwrap();
    assert_eq!(info.coffe_profile_end_remaining, 300.0);
    assert_eq!(info.coffe_profile_end_timeout, 600.0);
    assert_eq!(info.preheat_remaining, 120.0);
    assert_eq!(info.preheat_timeout, 180.0);
}

/// `TestHeaterTimeoutInfo.test_wrong_arg_count_raises`.
#[test]
fn heater_wrong_arg_count_raises() {
    assert!(HeaterTimeoutInfo::from_args(&["300.0", "600.0"]).is_err());
}

/// `TestHeaterTimeoutInfo.test_to_dict`.
#[test]
fn heater_to_dict() {
    let d = HeaterTimeoutInfo::from_args(&["300.0", "600.0", "120.0", "180.0"])
        .unwrap()
        .to_dict();
    assert_eq!(d["coffe_profile_end"]["remaining"], 300.0);
    assert_eq!(d["coffe_profile_end"]["timeout"], 600.0);
    assert_eq!(d["preheat"]["remaining"], 120.0);
    assert_eq!(d["preheat"]["timeout"], 180.0);
}

/// `TestMachineNotify.test_parse_valid`, `test_parse_empty_args_returns_none`.
#[test]
fn notify_from_args() {
    let notify = MachineNotify::from_args(&["warning", "Low water"]).unwrap();
    assert_eq!(notify.notification_type, "warning");
    assert_eq!(notify.message, "Low water");
    assert_eq!(MachineNotify::from_args::<&str>(&[]), None);
}
