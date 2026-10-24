//! `shot_manager.py`: the shot being brewed and its record once it ends.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Map, Value, json};

use esp::{PyNum, SensorData, ShotData};

use crate::app::App;
use crate::notifications::now_seconds;

pub const PUSH_TO_BREW_STATUS: &str = "click to start";

/// How long the machine waited on "click to start" before the shot.
#[derive(Debug, Clone, Default)]
pub struct PushToBrewTimer {
    started_at: Option<f64>,
    pub duration_ms: i64,
}

impl PushToBrewTimer {
    pub fn reset(&mut self) {
        self.started_at = None;
        self.duration_ms = 0;
    }

    pub fn observe(&mut self, status: &str, timestamp: f64) {
        if status == PUSH_TO_BREW_STATUS {
            if self.started_at.is_none() {
                self.started_at = Some(timestamp);
            }
            return;
        }
        if let Some(started) = self.started_at.take() {
            self.duration_ms = (((timestamp - started) * 1000.0) as i64).max(0);
        }
    }
}

/// `dict(sensor_data.__dict__)`: the dataclass fields in their declared order.
pub fn sensor_dict(s: &SensorData) -> Value {
    let mut map = Map::new();
    for (key, value) in [
        ("external_1", &s.external_1),
        ("external_2", &s.external_2),
        ("bar_up", &s.bar_up),
        ("bar_mid_up", &s.bar_mid_up),
        ("bar_mid_down", &s.bar_mid_down),
        ("bar_down", &s.bar_down),
        ("tube", &s.tube),
        ("motor_temp", &s.motor_temp),
        ("lam_temp", &s.lam_temp),
        ("motor_position", &s.motor_position),
        ("motor_speed", &s.motor_speed),
        ("motor_power", &s.motor_power),
        ("motor_current", &s.motor_current),
        ("bandheater_power", &s.bandheater_power),
        ("bandheater_current", &s.bandheater_current),
        ("pressure_sensor", &s.pressure_sensor),
        ("adc_0", &s.adc_0),
        ("adc_1", &s.adc_1),
        ("adc_2", &s.adc_2),
        ("adc_3", &s.adc_3),
    ] {
        map.insert(key.into(), value.to_json());
    }
    map.insert("water_status".into(), Value::Bool(s.water_status));
    map.insert("motor_thermistor".into(), s.motor_thermistor.to_json());
    map.insert("weight_prediction".into(), s.weight_prediction.to_json());
    Value::Object(map)
}

/// `Shot`: one brew's samples.
#[derive(Debug, Clone)]
pub struct Shot {
    pub data: Vec<Value>,
    pub profile: Option<Value>,
    pub profile_name: Option<String>,
    pub start_time: f64,
    pub extraction_time: Option<i64>,
    pub id: String,
    pub push_to_brew_time: i64,
}

/// Python `max(value, 0)` on a number from the ESP: a negative value gives
/// the int `0`, a tie keeps the original.
pub fn py_max_zero(value: &PyNum) -> Value {
    match value {
        PyNum::Int(v) => json!((*v).max(0)),
        PyNum::Float(v) if *v >= 0.0 || v.is_nan() => value.to_json(),
        PyNum::Float(_) => json!(0),
        // Deliberate fix: max("NaN", 0) raises in Python and the exception
        // ends the UART thread; the string is recorded as is.
        PyNum::NanString | PyNum::None => value.to_json(),
    }
}

impl Shot {
    pub fn new(push_to_brew_time: i64) -> Shot {
        Shot {
            data: Vec::new(),
            profile: None,
            profile_name: None,
            start_time: now_seconds(),
            extraction_time: None,
            id: uuid::Uuid::new_v4().to_string(),
            push_to_brew_time,
        }
    }

    /// `addSensorData`: the sensors join the last sample.
    pub fn add_sensor_data(&mut self, sensors: &SensorData) {
        if let Some(Value::Object(last)) = self.data.last_mut() {
            last.insert("sensors".into(), sensor_dict(sensors));
        }
    }

    /// The sample `addShotData` records, also used by the debug shot.
    pub fn format_sample(shot: &ShotData) -> Map<String, Value> {
        let setpoints = shot
            .to_sio()
            .get("setpoints")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let mut sample = Map::new();
        sample.insert(
            "shot".into(),
            json!({
                "pressure": py_max_zero(&shot.pressure),
                "flow": py_max_zero(&shot.flow),
                "weight": shot.weight.to_json(),
                "gravimetric_flow": shot.gravimetric_flow.to_json(),
                "setpoints": setpoints,
            }),
        );
        sample.insert("time".into(), json!(shot.time));
        sample.insert("profile_time".into(), json!(shot.profile_time));
        sample.insert(
            "status".into(),
            shot.status.clone().map_or(Value::Null, Value::String),
        );
        sample
    }

    /// `addShotData`: name the shot after the profile on its first real
    /// sample, then record the sample.
    pub fn add_shot_data(&mut self, app: &App, shot: &ShotData) {
        if self.profile_name.is_none()
            && shot.status.as_deref() != Some("starting...")
            && let Some(profile) = &shot.profile
        {
            let last = app.profiles.last_profile();
            let last_profile = last.as_ref().and_then(|l| l.get("profile")).cloned();
            let last_name = last_profile
                .as_ref()
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string);
            self.profile_name = if app.emulated()
                && profile == crate::machine::emulator::PROFILE_PLACEHOLDER
                && last_name.is_some()
            {
                last_name.clone()
            } else {
                Some(profile.clone())
            };
            if self.profile.is_none()
                && let (Some(p), Some(name)) = (&last_profile, &last_name)
                && Some(name) == self.profile_name.as_ref()
            {
                self.profile = Some(p.clone());
            }
        }
        self.data.push(Value::Object(Self::format_sample(shot)));
    }

    /// `Shot.to_json`.
    pub fn to_json(&self) -> Map<String, Value> {
        let mut shot = Map::new();
        shot.insert("time".into(), json!(self.start_time));
        shot.insert(
            "profile_name".into(),
            self.profile_name.clone().map_or(Value::Null, Value::String),
        );
        shot.insert("data".into(), Value::Array(self.data.clone()));
        shot.insert("id".into(), json!(self.id));
        shot.insert("push_to_brew_time".into(), json!(self.push_to_brew_time));
        if let Some(profile) = &self.profile
            && crate::config::python_truthy(profile)
        {
            shot.insert("profile".into(), profile.clone());
        }
        shot
    }
}

/// `_timestampToFilePaths`: `<date>/<time>.shot.json.zst` in local time.
pub fn file_paths(timestamp: f64) -> (String, String) {
    let start = local_time(timestamp);
    let folder = start.format("%Y-%m-%d").to_string();
    let file = format!("{folder}/{}.shot.json.zst", start.format("%H:%M:%S"));
    (folder, file)
}

pub fn local_time(timestamp: f64) -> chrono::DateTime<chrono::Local> {
    let secs = timestamp.floor() as i64;
    let nanos = ((timestamp - timestamp.floor()) * 1e9) as u32;
    chrono::DateTime::from_timestamp(secs, nanos)
        .map(|t| t.with_timezone(&chrono::Local))
        .unwrap_or_else(chrono::Local::now)
}

#[derive(Default)]
struct State {
    current: Option<Shot>,
    last: Option<Value>,
}

pub struct ShotManager {
    shots_dir: PathBuf,
    state: Mutex<State>,
    /// The history row of the last saved shot, for the debug shot to link
    /// its file to (`ShotManager.db_history_id`).
    pub db_history_id: Mutex<Option<i64>>,
}

impl ShotManager {
    pub fn new(shots_dir: PathBuf) -> ShotManager {
        ShotManager {
            shots_dir,
            state: Mutex::new(State::default()),
            db_history_id: Mutex::new(None),
        }
    }

    pub fn start(&self, push_to_brew_time: i64) {
        self.state.lock().current = Some(Shot::new(push_to_brew_time));
    }

    pub fn handle_sensor_data(&self, sensors: &SensorData) {
        if let Some(shot) = self.state.lock().current.as_mut() {
            shot.add_sensor_data(sensors);
        }
    }

    pub fn handle_shot_data(&self, app: &App, data: &ShotData) {
        let mut state = self.state.lock();
        if let Some(shot) = state.current.as_mut() {
            shot.add_shot_data(app, data);
        }
    }

    /// `handleExtractionEnd`: the first retracting sample's time is the
    /// extraction time.
    pub fn handle_extraction_end(&self, time: i64) -> i64 {
        let mut state = self.state.lock();
        let Some(shot) = state.current.as_mut() else {
            return time;
        };
        if shot.extraction_time.is_none() {
            shot.extraction_time = Some(time);
            tracing::info!("Extraction ended at {time}. Waiting for drippings to stop.");
        }
        shot.extraction_time.unwrap_or(time)
    }

    /// `getCurrentShot`.
    pub fn current_shot(&self) -> Value {
        let state = self.state.lock();
        let Some(shot) = &state.current else {
            return Value::Null;
        };
        let mut profile = Map::new();
        profile.insert("db_key".into(), Value::Null);
        if let Some(Value::Object(p)) = &shot.profile {
            for (k, v) in p {
                profile.insert(k.clone(), v.clone());
            }
        }
        let (_, file) = file_paths(shot.start_time);
        let mut out = Map::new();
        out.insert("db_key".into(), Value::Null);
        out.insert("file".into(), json!(file));
        for (k, v) in shot.to_json() {
            out.insert(k, v);
        }
        out.insert("profile".into(), Value::Object(profile));
        Value::Object(out)
    }

    /// `getLastShot`: cached until the next shot is saved.
    pub fn last_shot(&self, app: &App) -> Value {
        if let Some(last) = self.state.lock().last.clone() {
            return last;
        }
        match app.history.last_shot() {
            Ok(Some(last)) => {
                self.state.lock().last = Some(last.clone());
                last
            }
            Ok(None) => Value::Null,
            Err(error) => {
                tracing::error!("Failed to read the last shot: {error}");
                Value::Null
            }
        }
    }

    pub fn current_profile_name(&self) -> Option<String> {
        self.state
            .lock()
            .current
            .as_ref()
            .and_then(|s| s.profile_name.clone())
    }

    /// `stop`: write the shot file and the history row in the background.
    pub fn stop(&self, app: &Arc<App>) {
        let Some(shot) = self.state.lock().current.take() else {
            return;
        };
        let mut data = shot.to_json();
        if !data.contains_key("profile")
            && let Some(last) = app.profiles.last_profile()
        {
            data.insert(
                "profile".into(),
                last.get("profile").cloned().unwrap_or(Value::Null),
            );
        }
        let app = app.clone();
        let shots_dir = self.shots_dir.clone();
        let _ = std::thread::Builder::new()
            .name("ShotCompr".into())
            .spawn(move || {
                let (_, file) = file_paths(shot.start_time);
                tracing::info!("Writing and compressing shot file");
                let started = std::time::Instant::now();
                let text = pyfmt::dumps(&Value::Object(data.clone()), &pyfmt::DumpOpts::unicode());
                if let Err(error) = history::files::write_compressed(
                    &shots_dir.join(&file),
                    text.as_bytes(),
                ) {
                    tracing::error!("Failed to write shotfile to disk: {error}");
                    return;
                }
                tracing::info!(
                    "Writing json to disc took {} ms",
                    pyfmt::repr_f64(started.elapsed().as_secs_f64() * 1000.0)
                );
                tracing::info!("Adding shot to sqlite database");
                let started = std::time::Instant::now();
                data.insert("file".into(), json!(file));
                match app.history.insert_history(&Value::Object(data)) {
                    Ok(history_id) => {
                        *app.shots.db_history_id.lock() = Some(history_id);
                        app.shots.state.lock().last = None;
                        app.shots.last_shot(&app);
                        tracing::info!(
                            "Ingesting shot into sqlite took {} ms",
                            pyfmt::repr_f64(started.elapsed().as_secs_f64() * 1000.0)
                        );
                        tracing::info!("Shot ingested with history id: {history_id}");
                    }
                    Err(error) => tracing::error!("Failed to insert shot into sqlite: {error}"),
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tests/test_shot_manager.py: the push-to-brew wait in milliseconds.
    #[test]
    fn push_to_brew_timer() {
        let mut timer = PushToBrewTimer::default();
        timer.observe("heating", 1.0);
        assert_eq!(timer.duration_ms, 0);
        timer.observe(PUSH_TO_BREW_STATUS, 2.0);
        timer.observe(PUSH_TO_BREW_STATUS, 3.0);
        timer.observe("Preinfusion", 4.25);
        assert_eq!(timer.duration_ms, 2250);
        timer.reset();
        assert_eq!(timer.duration_ms, 0);
    }

    #[test]
    fn max_zero_like_python() {
        assert_eq!(py_max_zero(&PyNum::Float(-0.4)), json!(0));
        assert_eq!(py_max_zero(&PyNum::Float(0.0)), json!(0.0));
        assert_eq!(py_max_zero(&PyNum::Float(2.5)), json!(2.5));
        assert_eq!(py_max_zero(&PyNum::Int(-3)), json!(0));
    }
}
