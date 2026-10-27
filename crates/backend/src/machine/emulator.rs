//! `EmulatorSerialConnection` and `EmulationData`: an ESP32 made of recorded
//! shots on a pty, for development and the Docker image.
//!
//! It idles until it reads `action,start`, `action,purge` or `action,home`
//! and then replays the matching recording; `action,stop` returns to idle.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use esp::{EspInfo, PyNum, SensorData, ShotData};

/// `EmulationData.PROFILE_PLACEHOLDER`: the profile name of emulated shots
/// that stand for whatever profile is loaded.
pub const PROFILE_PLACEHOLDER: &str = "$$PROFILE_PLACEHOLDER$$";

/// Data and Sensors every 150 ms: half of that between lines.
const DEFAULT_SLEEP: Duration = Duration::from_millis(75);

const SHOT: &str = include_str!("../../assets/emulation/emulated.shot.json");
const HOME: &str = include_str!("../../assets/emulation/emulated.home.json");
const PURGE: &str = include_str!("../../assets/emulation/emulated.purge.json");

struct Recordings {
    idle: Vec<String>,
    espresso: Vec<String>,
    home: Vec<String>,
    purge: Vec<String>,
}

/// `EmulationData.init`.
fn recordings() -> Result<Recordings, String> {
    let idle_data = ShotData {
        temperature: PyNum::Float(23.2),
        state: "idle".into(),
        status: Some("idle".into()),
        profile: Some("idle".into()),
        ..ShotData::default()
    };
    let idle_sensors = SensorData {
        external_1: PyNum::Float(85.11),
        external_2: PyNum::Float(86.27),
        bar_up: PyNum::Float(68.73),
        bar_mid_up: PyNum::Float(69.02),
        bar_mid_down: PyNum::Float(67.75),
        bar_down: PyNum::Float(65.44),
        tube: PyNum::Float(67.74),
        motor_temp: PyNum::Float(29.96),
        lam_temp: PyNum::Float(32.78),
        motor_position: PyNum::Float(74.0),
        motor_speed: PyNum::Float(0.0),
        motor_power: PyNum::Float(0.0),
        motor_current: PyNum::Float(0.0),
        bandheater_power: PyNum::Float(15.3),
        bandheater_current: PyNum::Float(0.62),
        pressure_sensor: PyNum::Float(306.0),
        adc_0: PyNum::Float(14.0),
        adc_1: PyNum::Float(14.0),
        adc_2: PyNum::Float(14.0),
        adc_3: PyNum::Float(14.0),
        water_status: false,
        motor_thermistor: PyNum::Float(0.0),
        ..SensorData::default()
    };
    Ok(Recordings {
        idle: vec![
            format!("Data,{}", idle_data.to_args().join(",")),
            format!("Sensors,{}", idle_sensors.to_args().join(",")),
            format!("ESPInfo,{}", EspInfo::default().to_args().join(",")),
        ],
        espresso: shot_to_emulation(SHOT)?,
        home: shot_to_emulation(HOME)?,
        purge: shot_to_emulation(PURGE)?,
    })
}

fn num(value: Option<&Value>) -> PyNum {
    match value {
        Some(Value::Number(n)) if n.is_f64() => PyNum::Float(n.as_f64().unwrap_or(0.0)),
        Some(Value::Number(n)) => n.as_i64().map(PyNum::Int).unwrap_or(PyNum::None),
        _ => PyNum::None,
    }
}

/// `EmulationData.shotToEmulation`: a recorded shot as the lines the ESP32
/// sent during it.
fn shot_to_emulation(text: &str) -> Result<Vec<String>, String> {
    let shot: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let profile_name = shot
        .get("profile_name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut lines = Vec::new();
    for sample in shot
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if sample.get("time").and_then(Value::as_f64).unwrap_or(0.0) <= 0.0 {
            continue;
        }
        let sensor = sample.get("sensors").cloned().unwrap_or(Value::Null);
        let data = sample.get("shot").cloned().unwrap_or(Value::Null);
        let s = |key: &str| num(sensor.get(key));
        let sensors = SensorData {
            external_1: s("external_1"),
            external_2: s("external_2"),
            bar_up: s("bar_up"),
            bar_mid_up: s("bar_mid_up"),
            bar_mid_down: s("bar_mid_down"),
            bar_down: s("bar_down"),
            tube: s("tube"),
            motor_temp: s("motor_temp"),
            lam_temp: s("lam_temp"),
            motor_position: s("motor_position"),
            motor_speed: s("motor_speed"),
            motor_power: s("motor_power"),
            motor_current: s("motor_current"),
            bandheater_power: s("bandheater_power"),
            bandheater_current: s("bandheater_current"),
            pressure_sensor: s("pressure_sensor"),
            adc_0: s("adc_0"),
            adc_1: s("adc_1"),
            adc_2: s("adc_2"),
            adc_3: s("adc_3"),
            water_status: sensor
                .get("water_status")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ..SensorData::default()
        };
        let setpoints = data.get("setpoints").cloned().unwrap_or(Value::Null);
        let active = setpoints
            .get("active")
            .and_then(Value::as_str)
            .map(str::to_string);
        let main_setpoint = match &active {
            Some(kind) => num(setpoints.get(kind)),
            None => PyNum::None,
        };
        let temperature = match data.get("temperature") {
            Some(t) => num(Some(t)),
            None => s("tube"),
        };
        let shot_data = ShotData {
            pressure: num(data.get("pressure")),
            flow: num(data.get("flow")),
            weight: num(data.get("weight")),
            gravimetric_flow: num(data.get("gravimetric_flow")),
            temperature,
            profile: profile_name.clone(),
            status: sample
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string),
            main_controller_kind: active,
            main_setpoint,
            ..ShotData::default()
        };
        lines.push(format!("Data,{}", shot_data.to_args().join(",")));
        lines.push(format!("Sensors,{}", sensors.to_args().join(",")));
    }
    Ok(lines)
}

/// The emulator's side of the link: reset makes it return to idle.
#[derive(Default)]
pub struct EmulatorControl {
    reset: AtomicBool,
}

impl EmulatorControl {
    pub fn reset(&self) {
        tracing::info!("Resetting Dummy ESP32");
        self.reset.store(true, Ordering::Relaxed);
    }
}

/// Open the pty, start replaying, and return the device the backend opens.
pub fn start(speed_percent: u32) -> Result<(PathBuf, Arc<EmulatorControl>), String> {
    let pty = nix::pty::openpty(None, None).map_err(|e| e.to_string())?;
    let device = nix::unistd::ttyname(&pty.slave).map_err(|e| e.to_string())?;
    let control = Arc::new(EmulatorControl::default());
    let recordings = recordings()?;
    let thread_control = control.clone();
    let master = pty.master;
    // The slave stays open so the pty survives until the backend opens it.
    let slave = pty.slave;
    std::thread::Builder::new()
        .name("EmulationData".into())
        .spawn(move || replay(master, slave, recordings, thread_control, speed_percent))
        .map_err(|e| e.to_string())?;
    println!("EmulationData initialized");
    Ok((device, control))
}

#[derive(Clone, Copy, PartialEq)]
enum Source {
    Idle,
    Espresso,
    Home,
    Purge,
}

fn replay(
    master: OwnedFd,
    _slave: OwnedFd,
    recordings: Recordings,
    control: Arc<EmulatorControl>,
    speed: u32,
) {
    tracing::info!("Setting emulation pty to non-blocking");
    let raw = master.as_raw_fd();
    if let Ok(flags) = nix::fcntl::fcntl(&master, nix::fcntl::FcntlArg::F_GETFL) {
        let flags = nix::fcntl::OFlag::from_bits_truncate(flags) | nix::fcntl::OFlag::O_NONBLOCK;
        let _ = nix::fcntl::fcntl(&master, nix::fcntl::FcntlArg::F_SETFL(flags));
    }
    let _ = raw;
    let mut file = std::fs::File::from(master);
    tracing::info!("Emulation thread started, sleeping 2 seconds");
    std::thread::sleep(Duration::from_secs(2));
    let sleep = DEFAULT_SLEEP.mul_f64(100.0 / f64::from(speed.max(1)));
    let mut source = Source::Idle;
    let mut counter: usize = 0;
    loop {
        let lines = match source {
            Source::Idle => &recordings.idle,
            Source::Espresso => &recordings.espresso,
            Source::Home => &recordings.home,
            Source::Purge => &recordings.purge,
        };
        if control.reset.swap(false, Ordering::Relaxed) || counter + 1 >= lines.len() {
            if source != Source::Idle {
                tracing::info!("Starting idle simulation!");
            }
            counter = 0;
            source = Source::Idle;
        }
        let mut commands = [0u8; 2048];
        if let Ok(n) = file.read(&mut commands) {
            let commands = &commands[..n];
            let has = |needle: &[u8]| commands.windows(needle.len()).any(|w| w == needle);
            let next = if has(b"action,start") {
                tracing::info!("Starting espresso simulation!");
                Some(Source::Espresso)
            } else if has(b"action,stop") {
                tracing::info!("Stopping all simulations, returning to idle!");
                Some(Source::Idle)
            } else if has(b"action,purge") {
                tracing::info!("Starting purge simulation!");
                Some(Source::Purge)
            } else if has(b"action,home") {
                tracing::info!("Starting purge simulation!");
                Some(Source::Home)
            } else {
                None
            };
            if let Some(next) = next {
                source = next;
                counter = 0;
                continue;
            }
        }
        let lines = match source {
            Source::Idle => &recordings.idle,
            Source::Espresso => &recordings.espresso,
            Source::Home => &recordings.home,
            Source::Purge => &recordings.purge,
        };
        let Some(line) = lines.get(counter) else {
            counter = 0;
            continue;
        };
        counter += 1;
        let line = line.trim_matches(|c| c == ' ' || c == '\t' || c == '\r' || c == '\n');
        if line.is_empty() {
            continue;
        }
        let _ = file.write_all(format!("{line}\r\n").as_bytes());
        std::thread::sleep(sleep);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The recordings convert like EmulationData.init in Python: every line
    /// parses back as the message it claims to be.
    #[test]
    fn recordings_parse_back() {
        let recordings = recordings().expect("recordings");
        assert_eq!(recordings.idle.len(), 3);
        assert!(recordings.espresso.len() > 100);
        for line in recordings
            .espresso
            .iter()
            .chain(&recordings.home)
            .chain(&recordings.purge)
        {
            match esp::classify_line(line) {
                esp::EspLine::Data(Some(_)) | esp::EspLine::Sensors(Some(_)) => {}
                other => panic!("{line} classified as {other:?}"),
            }
        }
    }
}
