//! Helpers shared by the integration tests: the goldens' value encoding.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use esp::{
    ButtonEventData, EspInfo, HeaterTimeoutInfo, MachineNotify, PyNum, SensorData, ShotData,
};
use pyfmt::{dumps_default, object, repr_f64};
use serde_json::{Number, Value};

pub fn load(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/goldens")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad golden {name}: {e}"))
}

/// A float as the generator writes it: a JSON number, or
/// `{"nonfinite": "inf" | "-inf" | "nan"}`.
pub fn float(v: f64) -> Value {
    match Number::from_f64(v) {
        Some(n) => Value::Number(n),
        None => object([("nonfinite", Value::String(repr_f64(v)))]),
    }
}

pub fn num(v: &PyNum) -> Value {
    match v {
        PyNum::Int(i) => Value::from(*i),
        PyNum::Float(f) => float(*f),
        PyNum::NanString => Value::String("NaN".to_string()),
        PyNum::None => Value::Null,
    }
}

pub fn opt_str(v: &Option<String>) -> Value {
    v.clone().map_or(Value::Null, Value::String)
}

/// Reads back what [`num`] writes.
pub fn to_pynum(v: &Value) -> PyNum {
    match v {
        Value::Null => PyNum::None,
        Value::String(s) if s == "NaN" => PyNum::NanString,
        Value::Number(n) if n.is_i64() => PyNum::Int(n.as_i64().unwrap()),
        Value::Number(n) => PyNum::Float(n.as_f64().unwrap()),
        Value::Object(_) => PyNum::Float(to_f64(v)),
        other => panic!("not a number: {other}"),
    }
}

pub fn to_f64(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap(),
        Value::Object(map) => match map["nonfinite"].as_str().unwrap() {
            "inf" => f64::INFINITY,
            "-inf" => f64::NEG_INFINITY,
            "nan" => f64::NAN,
            other => panic!("bad nonfinite {other}"),
        },
        other => panic!("not a float: {other}"),
    }
}

pub fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

pub fn sensor_fields(s: &SensorData) -> Value {
    object([
        ("external_1", num(&s.external_1)),
        ("external_2", num(&s.external_2)),
        ("bar_up", num(&s.bar_up)),
        ("bar_mid_up", num(&s.bar_mid_up)),
        ("bar_mid_down", num(&s.bar_mid_down)),
        ("bar_down", num(&s.bar_down)),
        ("tube", num(&s.tube)),
        ("motor_temp", num(&s.motor_temp)),
        ("lam_temp", num(&s.lam_temp)),
        ("motor_position", num(&s.motor_position)),
        ("motor_speed", num(&s.motor_speed)),
        ("motor_power", num(&s.motor_power)),
        ("motor_current", num(&s.motor_current)),
        ("bandheater_power", num(&s.bandheater_power)),
        ("bandheater_current", num(&s.bandheater_current)),
        ("pressure_sensor", num(&s.pressure_sensor)),
        ("adc_0", num(&s.adc_0)),
        ("adc_1", num(&s.adc_1)),
        ("adc_2", num(&s.adc_2)),
        ("adc_3", num(&s.adc_3)),
        ("water_status", Value::Bool(s.water_status)),
        ("motor_thermistor", num(&s.motor_thermistor)),
        ("weight_prediction", num(&s.weight_prediction)),
    ])
}

pub fn shot_fields(s: &ShotData) -> Value {
    object([
        ("pressure", num(&s.pressure)),
        ("flow", num(&s.flow)),
        ("weight", num(&s.weight)),
        ("stable_weight", Value::Bool(s.stable_weight)),
        ("temperature", num(&s.temperature)),
        ("status", opt_str(&s.status)),
        ("profile", opt_str(&s.profile)),
        ("time", Value::from(s.time)),
        ("profile_time", Value::from(s.profile_time)),
        ("state", Value::String(s.state.clone())),
        ("is_extracting", Value::Bool(s.is_extracting)),
        ("gravimetric_flow", num(&s.gravimetric_flow)),
        ("main_controller_kind", opt_str(&s.main_controller_kind)),
        ("main_setpoint", num(&s.main_setpoint)),
        ("aux_controller_kind", opt_str(&s.aux_controller_kind)),
        ("aux_setpoint", num(&s.aux_setpoint)),
        (
            "is_aux_controller_active",
            Value::Bool(s.is_aux_controller_active),
        ),
    ])
}

pub fn info_fields(i: &EspInfo) -> Value {
    object([
        ("firmwareV", Value::String(i.firmware_v.clone())),
        ("espPinout", Value::from(i.esp_pinout)),
        ("mainVoltage", float(i.main_voltage)),
        ("color", Value::String(i.color.clone())),
        ("serialNumber", Value::String(i.serial_number.clone())),
        ("batchNumber", Value::String(i.batch_number.clone())),
        ("buildDate", Value::String(i.build_date.clone())),
        ("scaleModule", Value::String(i.scale_module.clone())),
        ("partialRetraction", float(i.partial_retraction)),
        ("autoPurgeAfterShot", Value::Bool(i.auto_purge_after_shot)),
        ("tareBehavior", opt_str(&i.tare_behavior)),
        ("deviceUUID", Value::String(i.device_uuid.clone())),
        ("deviceUUIDSupported", Value::Bool(i.device_uuid_supported)),
    ])
}

pub fn button_fields(b: &ButtonEventData) -> Value {
    object([
        ("event", Value::String(b.event.name().to_string())),
        (
            "time_since_last_event",
            Value::from(b.time_since_last_event),
        ),
    ])
}

pub fn notify_fields(n: &MachineNotify) -> Value {
    object([
        (
            "notificationType",
            Value::String(n.notification_type.clone()),
        ),
        ("message", Value::String(n.message.clone())),
    ])
}

pub fn heater_fields(h: &HeaterTimeoutInfo) -> Value {
    object([
        (
            "coffe_profile_end_remaining",
            float(h.coffe_profile_end_remaining),
        ),
        (
            "coffe_profile_end_timeout",
            float(h.coffe_profile_end_timeout),
        ),
        ("preheat_remaining", float(h.preheat_remaining)),
        ("preheat_timeout", float(h.preheat_timeout)),
    ])
}

fn rendered(fields: Value, to_args: Option<Vec<String>>, to_sio: &Value) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("fields".to_string(), fields);
    if let Some(args) = to_args {
        out.insert(
            "to_args".to_string(),
            Value::Array(args.into_iter().map(Value::String).collect()),
        );
    }
    out.insert("to_sio".to_string(), Value::String(dumps_default(to_sio)));
    Value::Object(out)
}

pub fn render_sensors(s: &SensorData) -> Value {
    rendered(sensor_fields(s), Some(s.to_args()), &s.to_sio_sensors())
}

pub fn render_shot(s: &ShotData) -> Value {
    rendered(shot_fields(s), Some(s.to_args()), &s.to_sio())
}

pub fn render_info(i: &EspInfo) -> Value {
    rendered(info_fields(i), Some(i.to_args()), &i.to_sio())
}

pub fn render_button(b: &ButtonEventData) -> Value {
    rendered(button_fields(b), None, &b.to_sio())
}

pub fn render_heater(h: &HeaterTimeoutInfo) -> Value {
    rendered(heater_fields(h), None, &h.to_dict())
}

pub fn render_opt<T>(value: Option<&T>, render: impl Fn(&T) -> Value) -> Value {
    value.map_or(Value::Null, render)
}

/// Equal as Python would print them: same types (1 vs 1.0), same key order.
#[track_caller]
pub fn assert_same(actual: &Value, expected: &Value, context: &str) {
    let (a, e) = (dumps_default(actual), dumps_default(expected));
    assert_eq!(a, e, "{context}");
}
