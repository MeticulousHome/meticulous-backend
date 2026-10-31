//! Parity with the current Python, case by case. The goldens are written by
//! `crates/esp/tools/gen_goldens.py`, which runs `esp_serial/data.py`, the
//! line dispatch cut out of `machine.py`, `esp_observability.py`,
//! `Machine._parseVersionString`, `monitoring/motor_power_monitoring.py`,
//! the real `Machine` UART writers and `get_version_from_firmware`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use esp::encode;
use esp::line::{ClassifiedLine, EspLine, Uncaught};
use esp::motor::EnergyCalculator;
use esp::observability::EspObservability;
use esp::version::{Pep440Version, parse_version_string};
use esp::{
    ButtonEventData, EspInfo, HeaterTimeoutError, HeaterTimeoutInfo, MachineNotify, PyNum,
    SensorData, ShotData, firmware_image_app_version, should_start_firmware_update,
};
use pyfmt::{dumps_default, object};
use serde_json::{Map, Value};

fn args_of(case: &Value) -> Vec<String> {
    strs(&case["args"])
}

#[test]
fn parsed_messages_match_data_py() {
    let golden = load("data");
    let cases = golden["parsed"].as_array().unwrap();
    assert!(cases.len() > 1000);
    for case in cases {
        let kind = case["kind"].as_str().unwrap();
        let ctx = format!("{kind} {}", case["args"]);
        let expected = &case["result"];
        let actual = match kind {
            "sensors" => render_opt(
                SensorData::from_args(&args_of(case)).as_ref(),
                render_sensors,
            ),
            "sensors_color" => render_opt(
                SensorData::from_color_coded_args(case["args"].as_str().unwrap()).as_ref(),
                render_sensors,
            ),
            "shot" => match (ShotData::from_args(&args_of(case)), case.get("raises")) {
                (Err(_), Some(_)) => continue,
                (Ok(shot), None) => render_opt(shot.as_ref(), render_shot),
                (result, raises) => panic!("{ctx}: rust {result:?}, python raises {raises:?}"),
            },
            "esp_info" => render_opt(EspInfo::from_args(&args_of(case)).as_ref(), render_info),
            "button" => render_opt(
                ButtonEventData::from_args(&args_of(case)).as_ref(),
                render_button,
            ),
            "notify" => match MachineNotify::from_args(&args_of(case)) {
                Some(n) => object([("fields", notify_fields(&n))]),
                None => Value::Null,
            },
            "heater" => match (
                HeaterTimeoutInfo::from_args(&args_of(case)),
                case.get("raises"),
            ) {
                (Ok(info), None) => render_heater(&info),
                (Err(error), Some(message)) => {
                    assert_heater_error(&error, message.as_str().unwrap(), &ctx);
                    continue;
                }
                (result, raises) => panic!("{ctx}: rust {result:?}, python raises {raises:?}"),
            },
            other => panic!("unknown kind {other}"),
        };
        assert_same(&actual, expected, &ctx);
    }
}

fn assert_heater_error(error: &HeaterTimeoutError, python: &str, ctx: &str) {
    match error {
        HeaterTimeoutError::WrongArgCount(_) => assert_eq!(error.to_string(), python, "{ctx}"),
        // Python's message quotes the text with repr(); only the kind is compared.
        HeaterTimeoutError::NotAFloat(_) => assert!(
            python.starts_with("could not convert string to float"),
            "{ctx}: {python}"
        ),
    }
}

fn build_shot(kwargs: &Map<String, Value>) -> ShotData {
    let mut shot = ShotData::default();
    for (key, value) in kwargs {
        match key.as_str() {
            "pressure" => shot.pressure = to_pynum(value),
            "flow" => shot.flow = to_pynum(value),
            "weight" => shot.weight = to_pynum(value),
            "temperature" => shot.temperature = to_pynum(value),
            "gravimetric_flow" => shot.gravimetric_flow = to_pynum(value),
            "main_setpoint" => shot.main_setpoint = to_pynum(value),
            "aux_setpoint" => shot.aux_setpoint = to_pynum(value),
            "status" => shot.status = value.as_str().map(str::to_string),
            "profile" => shot.profile = value.as_str().map(str::to_string),
            "main_controller_kind" => {
                shot.main_controller_kind = value.as_str().map(str::to_string)
            }
            "aux_controller_kind" => shot.aux_controller_kind = value.as_str().map(str::to_string),
            "state" => shot.state = value.as_str().unwrap().to_string(),
            "time" => shot.time = value.as_i64().unwrap(),
            "profile_time" => shot.profile_time = value.as_i64().unwrap(),
            "is_extracting" => shot.is_extracting = value.as_bool().unwrap(),
            "is_aux_controller_active" => shot.is_aux_controller_active = value.as_bool().unwrap(),
            other => panic!("ShotData kwarg {other}"),
        }
    }
    shot
}

fn build_sensors(kwargs: &Map<String, Value>) -> SensorData {
    let mut s = SensorData::default();
    for (key, value) in kwargs {
        if key == "water_status" {
            s.water_status = value.as_bool().unwrap();
            continue;
        }
        let v = to_pynum(value);
        let field = match key.as_str() {
            "external_1" => &mut s.external_1,
            "external_2" => &mut s.external_2,
            "bar_up" => &mut s.bar_up,
            "bar_mid_up" => &mut s.bar_mid_up,
            "bar_mid_down" => &mut s.bar_mid_down,
            "bar_down" => &mut s.bar_down,
            "tube" => &mut s.tube,
            "motor_temp" => &mut s.motor_temp,
            "lam_temp" => &mut s.lam_temp,
            "motor_position" => &mut s.motor_position,
            "motor_speed" => &mut s.motor_speed,
            "motor_power" => &mut s.motor_power,
            "motor_current" => &mut s.motor_current,
            "bandheater_power" => &mut s.bandheater_power,
            "bandheater_current" => &mut s.bandheater_current,
            "pressure_sensor" => &mut s.pressure_sensor,
            "adc_0" => &mut s.adc_0,
            "adc_1" => &mut s.adc_1,
            "adc_2" => &mut s.adc_2,
            "adc_3" => &mut s.adc_3,
            "motor_thermistor" => &mut s.motor_thermistor,
            "weight_prediction" => &mut s.weight_prediction,
            other => panic!("SensorData kwarg {other}"),
        };
        *field = v;
    }
    s
}

fn build_info(kwargs: &Map<String, Value>) -> EspInfo {
    let mut info = EspInfo::default();
    for (key, value) in kwargs {
        match key.as_str() {
            "tareBehavior" => info.tare_behavior = value.as_str().map(str::to_string),
            "deviceUUID" => info.device_uuid = value.as_str().unwrap().to_string(),
            "deviceUUIDSupported" => info.device_uuid_supported = value.as_bool().unwrap(),
            "mainVoltage" => info.main_voltage = to_f64(value),
            other => panic!("ESPInfo kwarg {other}"),
        }
    }
    info
}

/// The objects the emulator builds (`EmulationData.shotToEmulation`,
/// `EmulationData.init`) and their wire fields.
#[test]
fn constructed_messages_match_data_py() {
    let golden = load("data");
    for case in golden["constructed"].as_array().unwrap() {
        let kwargs = case["kwargs"].as_object().unwrap();
        let ctx = format!("{} {}", case["type"], case["kwargs"]);
        let (to_args, to_sio) = match case["type"].as_str().unwrap() {
            "ShotData" => {
                let shot = build_shot(kwargs);
                (shot.to_args(), shot.to_sio())
            }
            "SensorData" => {
                let sensors = build_sensors(kwargs);
                without_disturbance_resilience(
                    sensors.to_args(),
                    23,
                    &sensors.to_sio_sensors(),
                    &["weight_kalman", "weight_pred_kalman"],
                )
            }
            "ESPInfo" => {
                let info = build_info(kwargs);
                without_disturbance_resilience(
                    info.to_args(),
                    12,
                    &info.to_sio(),
                    &["disturbance_resilience", "disturbance_resilience_supported"],
                )
            }
            other => panic!("type {other}"),
        };
        assert_eq!(to_args, strs(&case["to_args"]), "{ctx}");
        assert_eq!(
            dumps_default(&to_sio),
            case["to_sio"].as_str().unwrap(),
            "{ctx}"
        );
    }
    for case in golden["clones"].as_array().unwrap() {
        let shot = ShotData::from_args(&args_of(case)).unwrap().unwrap();
        let clone = case["clone"].as_array().unwrap();
        let cloned = shot.clone_with_time_and_state(
            clone[0].as_i64().unwrap(),
            clone[1].as_bool().unwrap(),
            clone[2].as_i64().unwrap(),
        );
        assert_eq!(
            dumps_default(&cloned.to_sio()),
            case["to_sio"].as_str().unwrap()
        );
    }
}

fn log_outcome(log: &esp::EspLog) -> Value {
    let items = log.items.as_ref().map_or(Value::Null, |items| {
        Value::Object(
            items
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect(),
        )
    });
    let mut sentry = Vec::new();
    if log.send_to_sentry {
        let mut contexts = Map::new();
        if !items.is_null() {
            contexts.insert("esp-data".to_string(), items.clone());
        }
        sentry.push(object([
            (
                "event",
                object([
                    ("message", Value::String(log.message.clone())),
                    ("level", Value::String(log.level.clone())),
                ]),
            ),
            ("contexts", Value::Object(contexts)),
            ("tags", Value::Object(Map::new())),
        ]));
    }
    object([
        ("level", Value::String(log.level.clone())),
        ("message", Value::String(log.message.clone())),
        ("full_message", Value::String(log.full_message.clone())),
        ("items", items),
        (
            "malformed_items",
            Value::Array(
                log.malformed_items
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        ),
        ("send_to_sentry", Value::Bool(log.send_to_sentry)),
        ("sentry", Value::Array(sentry)),
        (
            "smoke",
            Value::Array(vec![Value::String(log.message.clone())]),
        ),
    ])
}

/// `Machine._read_data`'s `match` and boot-banner check, line by line.
#[test]
fn line_dispatch_matches_machine_py() {
    let cases = load("lines");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() > 1000);
    for case in cases {
        let line = case["line"].as_str().unwrap();
        let ctx = format!("line {line:?}");
        let classified = ClassifiedLine::new(line);
        assert_eq!(
            classified.boot_banner,
            case["boot_banner"].as_bool().unwrap(),
            "{ctx}"
        );
        let kind = case["kind"].as_str().unwrap();
        if kind == "uncaught" {
            assert!(
                matches!(classified.line, EspLine::Uncaught(_)),
                "{ctx}: {:?}",
                classified.line
            );
            assert_eq!(classified.valid_message_type(), None);
            assert!(classified.observe_as_raw_line() || classified.boot_banner);
            continue;
        }
        let python_type = case["type"].as_str();
        let python_valid = case["valid"].as_bool().unwrap();
        assert_eq!(classified.line.message_type(), python_type, "{ctx}");
        let valid_traffic = if python_valid { python_type } else { None };
        assert_eq!(classified.valid_message_type(), valid_traffic, "{ctx}");
        assert_eq!(
            classified.observe_as_raw_line(),
            (!python_valid || python_type.is_none()) && !classified.boot_banner,
            "{ctx}"
        );
        let expected = case.get("result").cloned().unwrap_or(Value::Null);
        let actual = match (&classified.line, kind) {
            (EspLine::Unknown, "unknown") => Value::Null,
            (EspLine::Event(event), "event") => render_opt(event.as_ref(), render_button),
            (EspLine::Data(data), "data") => render_opt(data.as_ref(), render_shot),
            (EspLine::Sensors(sensors), "sensors") => render_opt(sensors.as_ref(), render_sensors),
            (EspLine::EspInfo(info), "esp_info") => render_opt(info.as_ref(), render_info),
            (EspLine::EspBoot { reason, code }, "esp_boot") => {
                Value::Array(vec![reason.clone().into(), code.clone().into()])
            }
            (EspLine::DeviceUuidResponse(response), "device_uuid_response") => {
                response.clone().into()
            }
            (EspLine::TareBehaviorNvsResponse(status), "tare_nvs_response") => {
                status.clone().into()
            }
            (EspLine::NvsResponse { key, value }, "nvs_response") => {
                Value::Array(vec![key.clone().into(), value.clone().into()])
            }
            (EspLine::NvsError(text), "nvs_error") => text.clone().into(),
            (EspLine::Notify(notify), "notify") => Value::Array(vec![
                notify.notification_type.clone().into(),
                notify.message.clone().into(),
            ]),
            (EspLine::HeaterTimeoutInfo(Ok(info)), "heater") => {
                let emits = Value::Array(vec![Value::Array(vec![
                    "heater_status".into(),
                    float(info.preheat_remaining),
                ])]);
                assert_same(&emits, &case["emits"], &ctx);
                render_heater(info)
            }
            (EspLine::HeaterTimeoutInfo(Err(error)), "heater") => {
                let message = case["error"][0].as_str().unwrap();
                let python = message
                    .strip_prefix("Error processing HeaterTimeoutInfo: ")
                    .unwrap();
                assert_heater_error(error, python, &ctx);
                continue;
            }
            (EspLine::Log(Ok(log)), "log") => log_outcome(log),
            (EspLine::Log(Err(error)), "log") => {
                assert_eq!(
                    case["error"],
                    Value::Array(vec![error.to_string().into()]),
                    "{ctx}"
                );
                continue;
            }
            (EspLine::MileageRequest, "mileage_request") => Value::Null,
            (line, kind) => panic!("{ctx}: rust {line:?}, python {kind}"),
        };
        assert_same(&actual, &expected, &ctx);
    }
}

#[test]
fn uncaught_lines_name_the_python_exception() {
    assert_eq!(
        esp::classify_line("Data,1,2,3\n"),
        EspLine::Uncaught(Uncaught::ShortData(3))
    );
    assert_eq!(
        esp::classify_line("Notify\r\n"),
        EspLine::Uncaught(Uncaught::EmptyNotify)
    );
}

fn diag_value(d: &esp::EspDiagnostic) -> Value {
    object([
        ("title", Value::String(d.title.clone())),
        ("level", Value::String(d.level.clone())),
        ("fingerprint", Value::String(d.fingerprint.clone())),
        (
            "tags",
            Value::Object(
                d.tags
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect(),
            ),
        ),
        ("context", typed_context(&d.context)),
    ])
}

/// The context with non-finite floats spelled as the generator spells them.
fn typed_context(context: &Map<String, Value>) -> Value {
    Value::Object(context.clone())
}

fn snapshot(m: &EspObservability) -> Value {
    let opt_f = |v: Option<f64>| v.map_or(Value::Null, float);
    let opt_s = |v: &Option<String>| v.clone().map_or(Value::Null, Value::String);
    Value::Array(vec![
        m.phase.value().into(),
        m.update_in_progress().into(),
        float(m.last_valid_message),
        opt_f(m.recovery_deadline),
        opt_s(&m.expected_firmware),
        opt_s(&m.previous_firmware),
        m.boot_seen.into(),
        m.pending_unexpected_reset.into(),
        m.reset_reported.into(),
        m.panic_reported.into(),
        m.timeout_reported.into(),
    ])
}

/// `esp_observability.py` over seeded random scripts: every step's
/// diagnostics and public state.
#[test]
fn observability_matches_esp_observability_py() {
    let scripts = load("observability");
    let scripts = scripts.as_array().unwrap();
    assert!(scripts.len() > 100);
    for (index, script) in scripts.iter().enumerate() {
        let mut monitor = EspObservability::new(script["start"].as_f64().unwrap());
        for (step_index, step) in script["steps"].as_array().unwrap().iter().enumerate() {
            let op = step["op"].as_array().unwrap();
            let s = |i: usize| op[i].as_str();
            let f = |i: usize| op[i].as_f64().unwrap();
            let events = match op[0].as_str().unwrap() {
                "observe_raw_line" => monitor.observe_raw_line(s(1).unwrap(), f(2)),
                "observe_boot_reason" => {
                    monitor.observe_boot_reason(s(1).unwrap(), s(2).unwrap(), f(3))
                }
                "observe_valid_message" => monitor.observe_valid_message(s(1).unwrap(), f(2), s(3)),
                "check_timeouts" => monitor.check_timeouts(f(1)),
                "begin_update" => monitor.begin_update(s(1), s(2), f(3)),
                "finish_flashing" => {
                    monitor.finish_flashing(f(1));
                    Vec::new()
                }
                "fail_flashing" => vec![monitor.fail_flashing(s(1).unwrap(), s(2).unwrap(), f(3))],
                "begin_expected_reset" => {
                    monitor.begin_expected_reset(f(1));
                    Vec::new()
                }
                other => panic!("op {other}"),
            };
            let ctx = format!("script {index} step {step_index} {:?}", op[0]);
            let actual = Value::Array(events.iter().map(diag_value).collect());
            assert_same(&actual, &step["events"], &ctx);
            assert_same(&snapshot(&monitor), &step["state"], &ctx);
        }
    }
}

/// `Machine._parseVersionString` and `packaging.version.Version` ordering.
#[test]
fn versions_match_packaging() {
    let golden = load("versions");
    for case in golden["parsed"].as_array().unwrap() {
        let text = case["text"].as_str();
        let parsed = parse_version_string(text);
        let actual = parsed.as_ref().map_or(Value::Null, |v| {
            object([
                ("release", Value::String(v.release.to_string())),
                (
                    "extra_commits",
                    match &v.extra_commits {
                        esp::ExtraCommits::Default => Value::from(0),
                        esp::ExtraCommits::Text(t) => Value::String(t.clone()),
                    },
                ),
                ("sha", Value::String(v.sha.clone())),
                ("local", Value::String(v.local.clone())),
            ])
        });
        assert_same(&actual, &case["result"], &format!("{text:?}"));
    }
    let valid: Vec<String> = strs(&golden["valid"]);
    let parsed: Vec<_> = valid
        .iter()
        .map(|t| parse_version_string(Some(t)).unwrap())
        .collect();
    let order = strs(&golden["release_order"]);
    let equal = strs(&golden["dict_equal"]);
    for (i, a) in parsed.iter().enumerate() {
        for (j, b) in parsed.iter().enumerate() {
            let expected_order = order[i].as_bytes()[j] as char;
            let actual_order = match a.release.cmp(&b.release) {
                std::cmp::Ordering::Less => '<',
                std::cmp::Ordering::Equal => '=',
                std::cmp::Ordering::Greater => '>',
            };
            assert_eq!(actual_order, expected_order, "{} vs {}", valid[i], valid[j]);
            let expected_equal = equal[i].as_bytes()[j] == b'=';
            assert_eq!(a == b, expected_equal, "{} == {}", valid[i], valid[j]);
            assert_eq!(
                should_start_firmware_update(Some(a), Some(b), false, false),
                !expected_equal
            );
        }
    }
    assert!(Pep440Version::parse("1.0").unwrap() == Pep440Version::parse("1.0.0").unwrap());
}

fn motor_shot(value: &Value) -> Option<(PyNum, PyNum)> {
    let pair = value.as_array()?;
    Some((to_pynum(&pair[0]), to_pynum(&pair[1])))
}

/// `EnergyCalculator` with a scripted clock, and its restore from the saved
/// file.
#[test]
fn motor_energy_matches_motor_power_monitoring_py() {
    let golden = load("motor");
    for (index, script) in golden["scripts"].as_array().unwrap().iter().enumerate() {
        let mut calc = EnergyCalculator::new(script["window"].as_f64().unwrap());
        for step in script["steps"].as_array().unwrap() {
            let op = step["op"].as_array().unwrap();
            let ctx = format!("script {index} {op:?}");
            let result = match op[0].as_str().unwrap() {
                "calc" => {
                    let shot = motor_shot(&op[3]);
                    match calc.calculate_motor_energy(
                        &to_pynum(&op[1]),
                        &to_pynum(&op[2]),
                        shot.as_ref().map(|(f, p)| (f, p)),
                        op[4].as_f64().unwrap(),
                    ) {
                        Ok(energy) => float(energy),
                        Err(_) => Value::String("TypeError".to_string()),
                    }
                }
                "restart" => {
                    calc.restart();
                    Value::Null
                }
                "set_window" => {
                    calc.set_window_time(op[1].as_f64().unwrap());
                    Value::Null
                }
                other => panic!("op {other}"),
            };
            assert_same(&result, &step["result"], &ctx);
            assert_same(&float(calc.total_energy()), &step["total"], &ctx);
            assert_eq!(
                calc.history_len() as u64,
                step["history"].as_u64().unwrap(),
                "{ctx}"
            );
            assert_eq!(
                calc.window_seconds(),
                step["window"].as_f64().unwrap(),
                "{ctx}"
            );
        }
    }
    for case in golden["restores"].as_array().unwrap() {
        let ctx = format!("restore {case}");
        let mut calc = EnergyCalculator::new(case["window"].as_f64().unwrap());
        calc.restore(
            case["saved"].as_str().unwrap(),
            case["dt"].as_f64().unwrap(),
            case["now"].as_f64().unwrap(),
        );
        assert_same(&float(calc.total_energy()), &case["total"], &ctx);
        assert_eq!(
            calc.history_len() as u64,
            case["history"].as_u64().unwrap(),
            "{ctx}"
        );
        let energy = calc
            .calculate_motor_energy(
                &PyNum::Float(50.0),
                &PyNum::Float(2.0),
                Some((&PyNum::Float(2.0), &PyNum::Float(9.0))),
                1000.1,
            )
            .unwrap();
        assert_same(&float(energy), &case["after_calc_at_1000_1"], &ctx);
        assert_eq!(
            calc.history_len() as u64,
            case["history_after_calc"].as_u64().unwrap(),
            "{ctx}"
        );
    }
}

/// The bytes the real `Machine` methods (and backend.py's `calibrate`)
/// write to the port.
#[test]
fn uart_writes_match_machine_py() {
    let golden = load("encode");
    for case in golden.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let args = case["args"].as_array().unwrap();
        let ctx = format!("{name} {args:?}");
        let text = |i: usize| args[i].as_str().unwrap();
        let writes: Vec<Vec<u8>> = match name {
            "action" => vec![encode::action(text(0))],
            "greeting" => {
                let mut writes = vec![b"<reset_input_buffer>".to_vec()];
                writes.extend(encode::greeting());
                writes
            }
            "partial_retraction" => {
                let value = match &args[0] {
                    Value::String(s) => pyfmt::py_float(s).unwrap(),
                    other => other.as_f64().unwrap(),
                };
                vec![encode::partial_retraction_write(value)]
            }
            "auto_purge" => vec![encode::auto_purge_write(args[0].as_bool().unwrap())],
            "tare_behavior" => vec![encode::tare_behavior_write(text(0))],
            "mileage_read" => vec![encode::mileage_read()],
            "mileage_seed" => vec![encode::mileage_seed_write(args[0].as_i64().unwrap())],
            "serial" => encode::serial_writes(text(0), text(1), text(2), text(3)).to_vec(),
            "device_uuid_assign" => vec![encode::device_uuid_assign(text(0))],
            "heater_timeout" => match encode::heater_timeout(args[0].as_i64().unwrap()) {
                Ok(bytes) => vec![bytes],
                Err(error) => {
                    assert_eq!(case["writes"]["raises"], error.to_string(), "{ctx}");
                    continue;
                }
            },
            "calibration" => vec![encode::calibration(&to_pynum(&args[0]))],
            "send_json_with_hash" => vec![encode::send_json_with_hash(&args[0]).bytes],
            other => panic!("encoder {other}"),
        };
        let expected = strs(&case["writes"]);
        if name == "send_json_with_hash" {
            // Four writes in Python; the same stream here.
            assert_eq!(writes[0], expected.concat().into_bytes(), "{ctx}");
        } else {
            let actual: Vec<String> = writes
                .into_iter()
                .map(|w| String::from_utf8(w).unwrap())
                .collect();
            assert_eq!(actual, expected, "{ctx}");
        }
    }
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// `ESPToolWrapper.get_version_from_firmware` on synthetic images.
#[test]
fn firmware_image_version_matches_esptool() {
    for case in load("firmware_images").as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let result = firmware_image_app_version(&hex(case["image"].as_str().unwrap()));
        match case.get("raises") {
            Some(_) => assert!(result.is_err(), "{name}: {result:?}"),
            None => assert_eq!(
                result.unwrap(),
                case["version"].as_str().map(str::to_string),
                "{name}"
            ),
        }
    }
}
