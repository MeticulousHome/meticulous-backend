//! `esp_serial/data.py`: the messages the ESP32 sends, parsed from their
//! comma-separated fields, and the shapes the backend emits from them.

use std::sync::LazyLock;

use pyfmt::{object, py_float, py_int, strip_crlf, unquote};
use regex::Regex;
use serde_json::Value;

use crate::pynum::{PyNum, float_json};

/// `esp_serial.data.MachineStatus`: the status labels the ESP32 reports.
#[derive(Debug, Clone, Copy)]
pub struct MachineStatus;

impl MachineStatus {
    pub const IDLE: &'static str = "idle";
    pub const HEATING: &'static str = "heating";
    pub const PURGE: &'static str = "purge";
    pub const RETRACTING: &'static str = "retracting";
    pub const CLOSING_VALVE: &'static str = "closing valve";
    pub const HOME: &'static str = "home";
    pub const BOOT: &'static str = "boot";
    pub const STARTING: &'static str = "starting...";
}

/// `esp_serial.data.MachineStatusToProfile`.
pub fn machine_status_to_profile(status: &str) -> Option<&'static str> {
    match status {
        MachineStatus::PURGE => Some("Purge"),
        MachineStatus::HOME => Some("Home"),
        MachineStatus::BOOT => Some("Bootup"),
        _ => None,
    }
}

/// `esp_serial.data.MachineState`: the state the backend reports outwards.
#[derive(Debug, Clone, Copy)]
pub struct MachineState;

impl MachineState {
    pub const IDLE: &'static str = "idle";
    pub const PURGE: &'static str = "purge";
    pub const HOME: &'static str = "home";
    pub const BREWING: &'static str = "brewing";
    pub const ERROR: &'static str = "error";
}

/// `esp_serial.data.ControlTypes`.
#[derive(Debug, Clone, Copy)]
pub struct ControlTypes;

impl ControlTypes {
    pub const FLOW: &'static str = "Flow";
    pub const PRESSURE: &'static str = "Pressure";
    pub const PISTON: &'static str = "Piston";
    pub const POWER: &'static str = "Power";
}

static DEVICE_UUID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
        .expect("the device UUID pattern is a valid regex")
});

/// `deviceUUIDRegex.fullmatch` in `data.py`, and `is_valid_device_uuid` in
/// `device_identity.py` (the same pattern): a lowercase canonical UUIDv4.
pub fn is_valid_device_uuid(s: &str) -> bool {
    DEVICE_UUID_REGEX.is_match(s)
}

fn arg<S: AsRef<str>>(args: &[S], index: usize) -> Option<&str> {
    args.get(index).map(AsRef::as_ref)
}

fn safe_float_arg<S: AsRef<str>>(args: &[S], index: usize) -> Option<PyNum> {
    PyNum::safe_float(arg(args, index)?)
}

/// `SensorData`: the state of all sensors, sent after every `Data` line.
#[derive(Debug, Clone, PartialEq)]
pub struct SensorData {
    pub external_1: PyNum,
    pub external_2: PyNum,
    pub bar_up: PyNum,
    pub bar_mid_up: PyNum,
    pub bar_mid_down: PyNum,
    pub bar_down: PyNum,
    pub tube: PyNum,
    pub motor_temp: PyNum,
    pub lam_temp: PyNum,
    pub motor_position: PyNum,
    pub motor_speed: PyNum,
    pub motor_power: PyNum,
    pub motor_current: PyNum,
    pub bandheater_power: PyNum,
    pub bandheater_current: PyNum,
    pub pressure_sensor: PyNum,
    pub adc_0: PyNum,
    pub adc_1: PyNum,
    pub adc_2: PyNum,
    pub adc_3: PyNum,
    pub water_status: bool,
    pub motor_thermistor: PyNum,
    pub weight_prediction: PyNum,
}

impl Default for SensorData {
    fn default() -> Self {
        let zero = PyNum::Float(0.0);
        SensorData {
            external_1: zero,
            external_2: zero,
            bar_up: zero,
            bar_mid_up: zero,
            bar_mid_down: zero,
            bar_down: zero,
            tube: zero,
            motor_temp: zero,
            lam_temp: zero,
            motor_position: zero,
            motor_speed: zero,
            motor_power: zero,
            motor_current: zero,
            bandheater_power: zero,
            bandheater_current: zero,
            pressure_sensor: zero,
            adc_0: zero,
            adc_1: zero,
            adc_2: zero,
            adc_3: zero,
            water_status: false,
            motor_thermistor: zero,
            weight_prediction: zero,
        }
    }
}

static COLOR_SENSOR_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("\x1b\\[1;(31|32|33|34|35|36)m [a-z0-9_]*\x1b\\[0m")
        .expect("the color code pattern is a valid regex")
});

impl SensorData {
    /// `SensorData.from_color_coded_args`: the debug format where each value
    /// is preceded by a colored label instead of a comma.
    pub fn from_color_coded_args(color_separated: &str) -> Option<SensorData> {
        let replaced = COLOR_SENSOR_REGEX.replace_all(color_separated, ",");
        let mut args: Vec<&str> = replaced.split(',').collect();
        if args.first() == Some(&"") {
            args.remove(0);
        }
        SensorData::from_args(&args)
    }

    /// `SensorData.from_args`: `None` when a field is missing or one of the
    /// first twenty does not parse as a float.
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Option<SensorData> {
        Some(SensorData {
            external_1: safe_float_arg(args, 0)?,
            external_2: safe_float_arg(args, 1)?,
            bar_up: safe_float_arg(args, 2)?,
            bar_mid_up: safe_float_arg(args, 3)?,
            bar_mid_down: safe_float_arg(args, 4)?,
            bar_down: safe_float_arg(args, 5)?,
            tube: safe_float_arg(args, 6)?,
            motor_temp: safe_float_arg(args, 7)?,
            lam_temp: safe_float_arg(args, 8)?,
            motor_position: safe_float_arg(args, 9)?,
            motor_speed: safe_float_arg(args, 10)?,
            motor_power: safe_float_arg(args, 11)?,
            motor_current: safe_float_arg(args, 12)?,
            bandheater_current: safe_float_arg(args, 13)?,
            bandheater_power: safe_float_arg(args, 14)?,
            pressure_sensor: safe_float_arg(args, 15)?,
            adc_0: safe_float_arg(args, 16)?,
            adc_1: safe_float_arg(args, 17)?,
            adc_2: safe_float_arg(args, 18)?,
            adc_3: safe_float_arg(args, 19)?,
            water_status: arg(args, 20)?.to_lowercase() == "true",
            motor_thermistor: PyNum::safe_float_with_nan(arg(args, 21)?),
            weight_prediction: PyNum::safe_float_with_nan(arg(args, 22)?),
        })
    }

    /// `SensorData.to_args`: the fields of a `Sensors,` line.
    pub fn to_args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            &self.external_1,
            &self.external_2,
            &self.bar_up,
            &self.bar_mid_up,
            &self.bar_mid_down,
            &self.bar_down,
            &self.tube,
            &self.motor_temp,
            &self.lam_temp,
            &self.motor_position,
            &self.motor_speed,
            &self.motor_power,
            &self.motor_current,
            &self.bandheater_current,
            &self.bandheater_power,
            &self.pressure_sensor,
            &self.adc_0,
            &self.adc_1,
            &self.adc_2,
            &self.adc_3,
        ]
        .iter()
        .map(|v| v.to_py_string())
        .collect();
        args.push(bool_arg(self.water_status));
        args.push(self.motor_thermistor.to_py_string());
        args.push(self.weight_prediction.to_py_string());
        args
    }

    /// `SensorData.to_sio_sensors`: the payload of the `sensors` event.
    pub fn to_sio_sensors(&self) -> Value {
        object([
            ("t_ext_1", self.external_1.to_json()),
            ("t_ext_2", self.external_2.to_json()),
            ("t_bar_up", self.bar_up.to_json()),
            ("t_bar_mu", self.bar_mid_up.to_json()),
            ("t_bar_md", self.bar_mid_down.to_json()),
            ("t_bar_down", self.bar_down.to_json()),
            ("t_tube", self.tube.to_json()),
            ("t_motor_temp", self.motor_temp.to_json()),
            ("lam_temp", self.lam_temp.to_json()),
            ("p", self.pressure_sensor.to_json()),
            ("a_0", self.adc_0.to_json()),
            ("a_1", self.adc_1.to_json()),
            ("a_2", self.adc_2.to_json()),
            ("a_3", self.adc_3.to_json()),
            ("m_pos", self.motor_position.to_json()),
            ("m_spd", self.motor_speed.to_json()),
            ("m_pwr", self.motor_power.to_json()),
            ("m_cur", self.motor_current.to_json()),
            ("bh_pwr", self.bandheater_power.to_json()),
            ("bh_cur", self.bandheater_current.to_json()),
            ("w_stat", Value::Bool(self.water_status)),
            ("motor_temp", self.motor_thermistor.to_json()),
            ("weight_pred", self.weight_prediction.to_json()),
        ])
    }
}

fn bool_arg(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

/// `ESPInfo`: the firmware version and identity, the answer to `action,info`.
///
/// Field names are snake_case versions of the Python attributes
/// (`firmwareV` is `firmware_v`, `deviceUUID` is `device_uuid`, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct EspInfo {
    pub firmware_v: String,
    pub esp_pinout: i64,
    pub main_voltage: f64,
    pub color: String,
    pub serial_number: String,
    pub batch_number: String,
    pub build_date: String,
    pub scale_module: String,
    pub partial_retraction: f64,
    pub auto_purge_after_shot: bool,
    pub tare_behavior: Option<String>,
    pub device_uuid: String,
    pub device_uuid_supported: bool,
}

impl Default for EspInfo {
    fn default() -> Self {
        EspInfo {
            firmware_v: "0.0.0".to_string(),
            esp_pinout: 0,
            main_voltage: 0.0,
            color: String::new(),
            serial_number: String::new(),
            batch_number: String::new(),
            build_date: String::new(),
            scale_module: String::new(),
            partial_retraction: 45.33,
            auto_purge_after_shot: false,
            tare_behavior: None,
            device_uuid: String::new(),
            device_uuid_supported: false,
        }
    }
}

impl EspInfo {
    /// `ESPInfo.from_args`: the field count says which firmware generation
    /// sent the line. `None` when fewer than three fields arrive or a float
    /// field does not parse.
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Option<EspInfo> {
        // This field used to be the fan status; old firmware sends a word.
        let esp_pinout = arg(args, 1).and_then(py_int).unwrap_or(0);
        let text = |index: usize| arg(args, index).unwrap_or_default().to_string();
        let float = |index: usize| arg(args, index).and_then(py_float);
        let n = args.len();

        let mut info = EspInfo {
            firmware_v: arg(args, 0)?.to_string(),
            esp_pinout,
            main_voltage: float(2)?,
            ..EspInfo::default()
        };
        if n >= 8 {
            info.color = text(3);
            info.serial_number = text(4);
            info.batch_number = text(5);
            info.build_date = text(6);
            info.scale_module = text(7);
        }
        if n >= 9 {
            info.partial_retraction = float(8)?;
        }
        if n >= 10 {
            info.auto_purge_after_shot = text(9).to_lowercase() == "true";
        }
        if n >= 12 {
            let tare = text(10);
            info.tare_behavior = (!tare.is_empty()).then_some(tare);
            info.device_uuid = text(11);
            info.device_uuid_supported = true;
        } else if n == 11 {
            // The device UUID arrived before tare behaviour existed: an
            // eleventh field that is a UUID is the UUID.
            let tenth = text(10);
            if is_valid_device_uuid(&tenth) {
                info.device_uuid = tenth;
                info.device_uuid_supported = true;
            } else {
                // PARITY: unlike the twelve-field case, an empty field is
                // kept as an empty tare behaviour, not None.
                info.tare_behavior = Some(tenth);
            }
        }
        Some(info)
    }

    /// `ESPInfo.to_args`: the fields of an `ESPInfo,` line.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![
            self.firmware_v.clone(),
            self.esp_pinout.to_string(),
            pyfmt::repr_f64(self.main_voltage),
            self.color.clone(),
            self.serial_number.clone(),
            self.batch_number.clone(),
            self.build_date.clone(),
            self.scale_module.clone(),
            pyfmt::repr_f64(self.partial_retraction),
            bool_arg(self.auto_purge_after_shot),
        ];
        if let Some(tare) = &self.tare_behavior {
            args.push(tare.clone());
        }
        if self.device_uuid_supported {
            if self.tare_behavior.is_none() {
                args.push(String::new());
            }
            args.push(self.device_uuid.clone());
        }
        args
    }

    /// `ESPInfo.to_sio`. The device UUID is deliberately absent.
    pub fn to_sio(&self) -> Value {
        object([
            ("firmware_version", Value::String(self.firmware_v.clone())),
            ("esp_pinout", Value::from(self.esp_pinout)),
            ("main_voltage", float_json(self.main_voltage)),
            ("color", Value::String(self.color.clone())),
            ("serial_number", Value::String(self.serial_number.clone())),
            ("batch_number", Value::String(self.batch_number.clone())),
            ("build_date", Value::String(self.build_date.clone())),
            ("scale_module", Value::String(self.scale_module.clone())),
            ("partial_retraction", float_json(self.partial_retraction)),
            (
                "auto_purge_after_shot",
                Value::Bool(self.auto_purge_after_shot),
            ),
            (
                "tare_behavior",
                self.tare_behavior
                    .clone()
                    .map_or(Value::Null, Value::String),
            ),
            (
                "tare_behavior_supported",
                Value::Bool(self.tare_behavior.is_some()),
            ),
        ])
    }
}

/// `ShotData`: one telemetry point of the machine, sent every period.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotData {
    pub pressure: PyNum,
    pub flow: PyNum,
    pub weight: PyNum,
    pub stable_weight: bool,
    pub temperature: PyNum,
    /// Reported as `name` over Socket.IO.
    pub status: Option<String>,
    pub profile: Option<String>,
    pub time: i64,
    pub profile_time: i64,
    pub state: String,
    pub is_extracting: bool,
    pub gravimetric_flow: PyNum,
    pub main_controller_kind: Option<String>,
    pub main_setpoint: PyNum,
    pub aux_controller_kind: Option<String>,
    pub aux_setpoint: PyNum,
    pub is_aux_controller_active: bool,
}

impl Default for ShotData {
    fn default() -> Self {
        ShotData {
            pressure: PyNum::Float(0.0),
            flow: PyNum::Float(0.0),
            weight: PyNum::Float(0.0),
            stable_weight: false,
            temperature: PyNum::Float(20.0),
            status: Some(String::new()),
            profile: Some(String::new()),
            time: -1,
            profile_time: -1,
            state: String::new(),
            is_extracting: false,
            gravimetric_flow: PyNum::Float(0.0),
            main_controller_kind: None,
            main_setpoint: PyNum::Int(-1),
            aux_controller_kind: None,
            aux_setpoint: PyNum::Int(-1),
            is_aux_controller_active: false,
        }
    }
}

/// `ShotData.from_args` raised where `machine.py` does not catch it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("ShotData.from_args got {fields} fields; Python raises IndexError reading the fourth")]
pub struct ShotDataTooShort {
    pub fields: usize,
}

impl ShotData {
    /// `ShotData.from_args`.
    ///
    /// `Ok(None)` where Python returns `None` (four fields), and
    /// `Err(ShotDataTooShort)` with fewer than four, where Python raises
    /// `IndexError` out of `from_args` (`args[3]` is read outside its try).
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Result<Option<ShotData>, ShotDataTooShort> {
        let status = arg(args, 5).map(|s| unquote(strip_crlf(s)));
        let profile = arg(args, 6).map(|s| unquote(strip_crlf(s)));

        let mut main_controller_kind = None;
        let mut main_setpoint = PyNum::Float(0.0);
        let mut aux_controller_kind = None;
        let mut aux_setpoint = PyNum::Float(0.0);
        let mut is_aux_controller_active = false;
        let mut gravimetric_flow = PyNum::Float(0.0);
        let stable_weight =
            strip_crlf(arg(args, 3).ok_or(ShotDataTooShort { fields: args.len() })?) == "S";

        if args.len() > 12 {
            // PARITY: the fields are assigned one by one inside one try, so
            // a setpoint that does not parse keeps what was assigned before
            // it and leaves the rest at their defaults.
            let kind = |s: &str| {
                let s = strip_crlf(s);
                (s != "none").then(|| s.to_string())
            };
            let all_args = args.iter().map(AsRef::as_ref).collect::<Vec<&str>>();
            'parse: {
                main_controller_kind = kind(all_args[7]);
                let Some(main) = PyNum::safe_float(strip_crlf(all_args[8])) else {
                    break 'parse;
                };
                main_setpoint = main;
                aux_controller_kind = kind(all_args[9]);
                let Some(aux) = PyNum::safe_float(strip_crlf(all_args[10])) else {
                    break 'parse;
                };
                aux_setpoint = aux;
                is_aux_controller_active = strip_crlf(all_args[11]) == "true";
                gravimetric_flow = PyNum::safe_float_with_nan(all_args[12]);
            }
        }

        let state = match &profile {
            None => MachineState::IDLE.to_string(),
            Some(p) if p == MachineStatus::IDLE || p == "Purge" || p == "Home" => p.to_lowercase(),
            Some(_) => MachineState::BREWING.to_string(),
        };

        let Some(temperature) = arg(args, 4) else {
            return Ok(None);
        };
        let first = |index: usize| PyNum::safe_float_with_nan(arg(args, index).unwrap_or_default());
        Ok(Some(ShotData {
            pressure: first(0),
            flow: first(1),
            weight: first(2),
            stable_weight,
            temperature: PyNum::safe_float_with_nan(temperature),
            status,
            profile,
            state,
            main_controller_kind,
            main_setpoint,
            aux_controller_kind,
            aux_setpoint,
            is_aux_controller_active,
            gravimetric_flow,
            ..ShotData::default()
        }))
    }

    /// `ShotData.clone_with_time_and_state`.
    pub fn clone_with_time_and_state(
        &self,
        shot_start_time: i64,
        is_brewing: bool,
        profile_time: i64,
    ) -> ShotData {
        ShotData {
            time: shot_start_time,
            is_extracting: is_brewing,
            profile_time,
            ..self.clone()
        }
    }

    /// `ShotData.to_args`: the fields of a `Data,` line.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![
            self.pressure.to_py_string(),
            self.flow.to_py_string(),
            self.weight.to_py_string(),
            if self.stable_weight { "S" } else { "U" }.to_string(),
            self.temperature.to_py_string(),
            self.status.clone().unwrap_or_default(),
            self.profile.clone().unwrap_or_default(),
        ];
        match &self.main_controller_kind {
            Some(kind) => {
                args.push(kind.clone());
                args.push(self.main_setpoint.to_py_string());
            }
            None => {
                args.push("none".to_string());
                args.push("0.0".to_string());
            }
        }
        match &self.aux_controller_kind {
            Some(kind) => {
                args.push(kind.clone());
                args.push(self.aux_setpoint.to_py_string());
                args.push(bool_arg(self.is_aux_controller_active));
            }
            None => {
                args.push("none".to_string());
                args.push("0.0".to_string());
                args.push("false".to_string());
            }
        }
        args.push(self.gravimetric_flow.to_py_string());
        args
    }

    /// `ShotData.to_sio`: the machine part of the `status` event.
    pub fn to_sio(&self) -> Value {
        let mut setpoints = serde_json::Map::new();
        setpoints.insert("active".to_string(), Value::Null);
        if let Some(kind) = &self.main_controller_kind {
            let kind = kind.to_lowercase();
            setpoints.insert(kind.clone(), self.main_setpoint.to_json());
            setpoints.insert("active".to_string(), Value::String(kind));
        }
        if let Some(kind) = &self.aux_controller_kind {
            let kind = kind.to_lowercase();
            setpoints.insert(kind.clone(), self.aux_setpoint.to_json());
            if self.is_aux_controller_active {
                setpoints.insert("active".to_string(), Value::String(kind));
            }
        }
        let sensors = object([
            ("p", self.pressure.to_json()),
            ("f", self.flow.to_json()),
            ("w", self.weight.to_json()),
            ("t", self.temperature.to_json()),
            ("g", self.gravimetric_flow.to_json()),
        ]);
        let text = |s: &Option<String>| s.clone().map_or(Value::Null, Value::String);
        object([
            ("name", text(&self.status)),
            ("sensors", sensors),
            ("setpoints", Value::Object(setpoints)),
            ("time", Value::from(self.time)),
            ("profile", text(&self.profile)),
            ("profile_time", Value::from(self.profile_time)),
            ("state", Value::String(self.state.clone())),
            ("extracting", Value::Bool(self.is_extracting)),
        ])
    }
}

/// `ButtonEventEnum`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonEvent {
    EncoderClockwise,
    EncoderCounterclockwise,
    EncoderPush,
    EncoderDouble,
    EncoderLong,
    Tare,
    TareDouble,
    TareLong,
    TareSuperLong,
    Context,
    EncoderPressed,
    EncoderReleased,
    TarePressed,
    TareReleased,
    ContextPressed,
    ContextReleased,
    Unknown,
}

impl ButtonEvent {
    pub const ALL: [ButtonEvent; 17] = [
        ButtonEvent::EncoderClockwise,
        ButtonEvent::EncoderCounterclockwise,
        ButtonEvent::EncoderPush,
        ButtonEvent::EncoderDouble,
        ButtonEvent::EncoderLong,
        ButtonEvent::Tare,
        ButtonEvent::TareDouble,
        ButtonEvent::TareLong,
        ButtonEvent::TareSuperLong,
        ButtonEvent::Context,
        ButtonEvent::EncoderPressed,
        ButtonEvent::EncoderReleased,
        ButtonEvent::TarePressed,
        ButtonEvent::TareReleased,
        ButtonEvent::ContextPressed,
        ButtonEvent::ContextReleased,
        ButtonEvent::Unknown,
    ];

    /// The member name, which is what Socket.IO clients receive.
    pub fn name(self) -> &'static str {
        match self {
            ButtonEvent::EncoderClockwise => "ENCODER_CLOCKWISE",
            ButtonEvent::EncoderCounterclockwise => "ENCODER_COUNTERCLOCKWISE",
            ButtonEvent::EncoderPush => "ENCODER_PUSH",
            ButtonEvent::EncoderDouble => "ENCODER_DOUBLE",
            ButtonEvent::EncoderLong => "ENCODER_LONG",
            ButtonEvent::Tare => "TARE",
            ButtonEvent::TareDouble => "TARE_DOUBLE",
            ButtonEvent::TareLong => "TARE_LONG",
            ButtonEvent::TareSuperLong => "TARE_SUPER_LONG",
            ButtonEvent::Context => "CONTEXT",
            ButtonEvent::EncoderPressed => "ENCODER_PRESSED",
            ButtonEvent::EncoderReleased => "ENCODER_RELEASED",
            ButtonEvent::TarePressed => "TARE_PRESSED",
            ButtonEvent::TareReleased => "TARE_RELEASED",
            ButtonEvent::ContextPressed => "CONTEXT_PRESSED",
            ButtonEvent::ContextReleased => "CONTEXT_RELEASED",
            ButtonEvent::Unknown => "UNKNOWN",
        }
    }

    /// `ButtonEventEnum[name]`; `None` where Python raises `KeyError`.
    pub fn from_name(name: &str) -> Option<ButtonEvent> {
        ButtonEvent::ALL.into_iter().find(|e| e.name() == name)
    }

    /// `ButtonEventEnum.from_str`: the firmware's short token mapped through
    /// the lookup table, anything else upper-cased and looked up by member
    /// name. `None` where Python raises `KeyError`.
    ///
    /// PARITY: `_missing_` only applies to lookups by value, so an unknown
    /// token is not `UNKNOWN` but an error (and `"unknown"` itself is
    /// `UNKNOWN`). The table is case-sensitive while names are not:
    /// `"cw"` fails but `"encoder_clockwise"` works.
    pub fn from_token(type_str: &str) -> Option<ButtonEvent> {
        let mapped = match type_str {
            "CW" => "ENCODER_CLOCKWISE",
            "CCW" => "ENCODER_COUNTERCLOCKWISE",
            "push" => "ENCODER_PUSH",
            "pu_d" => "ENCODER_DOUBLE",
            "elng" => "ENCODER_LONG",
            "tare" => "TARE",
            "ta_d" => "TARE_DOUBLE",
            "ta_l" => "TARE_LONG",
            "ta_sl" => "TARE_SUPER_LONG",
            "strt" | "cntx" => "CONTEXT",
            "encoder_button_pressed" => "ENCODER_PRESSED",
            "encoder_button_released" => "ENCODER_RELEASED",
            "tare_pressed" => "TARE_PRESSED",
            "tare_released" => "TARE_RELEASED",
            "context_pressed" => "CONTEXT_PRESSED",
            "context_released" => "CONTEXT_RELEASED",
            other => other,
        };
        ButtonEvent::from_name(&mapped.to_uppercase())
    }
}

/// `ButtonEventData`: a physical button event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonEventData {
    pub event: ButtonEvent,
    pub time_since_last_event: i64,
}

impl ButtonEventData {
    /// `ButtonEventData.from_args`: `None` without a token or with an
    /// unknown one. A time that is not an int counts as 0; `9999+++` (the
    /// firmware's counter overflow) as 10000.
    ///
    /// Python's `int()` has no upper bound; a time beyond `i64` counts as 0
    /// here.
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Option<ButtonEventData> {
        let time_since_last_event = match arg(args, 1) {
            Some("9999+++") => 10000,
            Some(text) => py_int(text).unwrap_or(0),
            None => 0,
        };
        Some(ButtonEventData {
            event: ButtonEvent::from_token(arg(args, 0)?)?,
            time_since_last_event,
        })
    }

    /// `ButtonEventData.to_sio`: the payload of the `button` event.
    pub fn to_sio(&self) -> Value {
        object([
            ("type", Value::String(self.event.name().to_string())),
            (
                "time_since_last_event",
                Value::from(self.time_since_last_event),
            ),
        ])
    }
}

/// `MachineNotify`: a message the ESP32 wants the user to see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineNotify {
    pub notification_type: String,
    pub message: String,
}

impl MachineNotify {
    /// `MachineNotify.from_args`: `None` with fewer than two fields. The
    /// line dispatch does not use it (see `EspLine::Notify`).
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Option<MachineNotify> {
        Some(MachineNotify {
            notification_type: arg(args, 0)?.to_string(),
            message: arg(args, 1)?.to_string(),
        })
    }
}

/// `HeaterTimeoutInfo.from_args` raised `ValueError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HeaterTimeoutError {
    #[error("Expected 4 arguments for HeaterTimeoutInfo")]
    WrongArgCount(usize),
    #[error("could not convert string to float: '{0}'")]
    NotAFloat(String),
}

/// `HeaterTimeoutInfo`: the heater timeouts the ESP32 reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaterTimeoutInfo {
    pub coffe_profile_end_remaining: f64,
    pub coffe_profile_end_timeout: f64,
    pub preheat_remaining: f64,
    pub preheat_timeout: f64,
}

impl HeaterTimeoutInfo {
    /// `HeaterTimeoutInfo.from_args`: exactly four floats.
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Result<HeaterTimeoutInfo, HeaterTimeoutError> {
        if args.len() != 4 {
            return Err(HeaterTimeoutError::WrongArgCount(args.len()));
        }
        let float = |index: usize| {
            let text = arg(args, index).unwrap_or_default();
            py_float(text).ok_or_else(|| HeaterTimeoutError::NotAFloat(text.to_string()))
        };
        Ok(HeaterTimeoutInfo {
            coffe_profile_end_remaining: float(0)?,
            coffe_profile_end_timeout: float(1)?,
            preheat_remaining: float(2)?,
            preheat_timeout: float(3)?,
        })
    }

    /// `HeaterTimeoutInfo.to_dict`.
    pub fn to_dict(&self) -> Value {
        object([
            (
                "coffe_profile_end",
                object([
                    ("remaining", float_json(self.coffe_profile_end_remaining)),
                    ("timeout", float_json(self.coffe_profile_end_timeout)),
                ]),
            ),
            (
                "preheat",
                object([
                    ("remaining", float_json(self.preheat_remaining)),
                    ("timeout", float_json(self.preheat_timeout)),
                ]),
            ),
        ])
    }
}
