//! `shot_debug_manager.py`: everything about a brew for support and bug
//! reports: the samples with their timing, the machine and its settings, the
//! node program sent to the ESP32 and the backend's logs during the brew.
//!
//! The Python guards the current debug shot with two locks and spins in
//! `send_json_with_hash` until the reader thread opened the shot; here too
//! the shot's data and its log records have a lock each: code holding the
//! data lock logs, and a record must neither wait for that lock nor be lost
//! while the UART thread holds it. The node program waits in
//! `pending_node_json` for the shot to start.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Map, Value, json};

use esp::{MachineStatus, SensorData, ShotData};

use crate::app::App;
use crate::config::keys;
use crate::logging::Record;
use crate::notifications::now_seconds;
use crate::shots::{Shot, local_time};

pub const MAX_DEBUG_SHOT_RETENTION_DAYS: i64 = 365;
const FOLDER_FORMAT: &str = "%Y-%m-%d";
const FILE_FORMAT: &str = "%H:%M:%S";

/// The profile engine's status labels while it waits for the user (its
/// `ClickToStart`, `AskForWater` and `WaitContinueAtTop` states, and the
/// stages of the same names in the node program): nothing moves until the
/// user acts, which can take hours.
const WAIT_STATUSES: [&str; 4] = [
    crate::shots::PUSH_TO_BREW_STATUS,
    "Pour water and click to continue",
    "click to purge",
    "remove cup",
];
/// While waiting, one sample per this many milliseconds is kept.
const WAIT_SAMPLE_INTERVAL_MS: i64 = 1000;
/// Samples kept while waiting, over the whole shot: 30 minutes at one per
/// second. Past it, only each wait's first and latest samples are.
const MAX_WAIT_SAMPLES: usize = 1800;

/// `DebugShot`.
#[derive(Debug, Clone)]
pub struct DebugShot {
    pub start_time: f64,
    pub shot_type: String,
    pub profile_name: Option<String>,
    pub profile: Option<Value>,
    pub node_json: Option<Value>,
    pub config: Value,
    pub machine: Map<String, Value>,
    pub data: Vec<Value>,
    pub logs: Vec<Value>,
    /// Samples appended while waiting for the user, against
    /// [`MAX_WAIT_SAMPLES`].
    wait_samples: usize,
}

impl DebugShot {
    fn new(app: &App) -> DebugShot {
        let mut config = app.config.section(keys::USER);
        if let Value::Object(map) = &mut config {
            map.insert(keys::WIFI.into(), json!({}));
        }
        let esp_info = app.machine.esp_info().unwrap_or_default().to_sio();
        let mut machine = crate::machine_info::identity(app);
        machine.insert(
            "manufacturing".into(),
            json!(app.machine.enable_manufacturing()),
        );
        machine.insert("upgrade_first_boot".into(), json!(app.ota.is_changed()));
        let history = match app.config.get(keys::SYSTEM, keys::LAST_SYSTEM_VERSIONS) {
            Value::Null => json!([]),
            other => other,
        };
        machine.insert("version_history".into(), history);
        if let Value::Object(info) = esp_info {
            for (k, v) in info {
                machine.insert(k, v);
            }
        }
        DebugShot {
            start_time: now_seconds(),
            shot_type: "shot".into(),
            profile_name: None,
            profile: None,
            node_json: None,
            config,
            machine,
            data: Vec::new(),
            logs: Vec::new(),
            wait_samples: 0,
        }
    }

    /// `DebugShot.to_json`, consuming the shot: the samples and the log
    /// records move into the JSON instead of being copied, since a shot
    /// left open for long holds hundreds of megabytes of them.
    pub fn into_json(self) -> Map<String, Value> {
        let mut out = Map::new();
        out.insert("time".into(), json!(self.start_time));
        out.insert("type".into(), json!(self.shot_type));
        out.insert(
            "profile_name".into(),
            self.profile_name.map_or(Value::Null, Value::String),
        );
        out.insert("machine".into(), Value::Object(self.machine));
        out.insert("profile".into(), self.profile.unwrap_or(Value::Null));
        out.insert("nodeJSON".into(), self.node_json.unwrap_or(Value::Null));
        out.insert("config".into(), self.config);
        out.insert("data".into(), Value::Array(self.data));
        out.insert("logs".into(), Value::Array(self.logs));
        out
    }

    fn add_sensor_data(&mut self, sensors: &SensorData) {
        if let Some(Value::Object(last)) = self.data.last_mut() {
            last.insert("sensors".into(), crate::shots::sensor_dict(sensors));
        }
    }

    /// `Shot.addShotData` plus `profile_ms` (`DebugShot.append_shot_data`),
    /// for a sample received at `now`.
    fn add_shot_data(&mut self, app: &App, shot: &ShotData, now: f64) {
        // The profile naming of Shot.addShotData, shared through a Shot.
        let mut naming = Shot::new(0);
        naming.profile_name = self.profile_name.clone();
        naming.profile = self.profile.clone();
        naming.add_shot_data(app, shot);
        self.profile_name = naming.profile_name;
        self.profile = naming.profile;
        let profile_ms = ((now - self.start_time) * 1000.0) as i64;
        if self.replaces_latest_sample(shot.status.as_deref()) {
            self.data.pop();
        }
        let mut sample = Shot::format_sample(shot);
        sample.insert("profile_ms".into(), json!(profile_ms));
        self.data.push(Value::Object(sample));
    }

    /// Deliberate fix: the Python appends every sample (about 7 a second)
    /// for as long as the debug shot is open, and a shot left on "click to
    /// purge" for 80 minutes grew the backend past what a 1 GB machine has,
    /// so the kernel killed it while saving the shot. While the profile
    /// engine waits for the user, a new sample replaces the latest one while
    /// the latest is less than a second after the one before it, so the
    /// wait keeps one sample a second and always ends with the newest
    /// sample (the state the user found when they acted). Past
    /// [`MAX_WAIT_SAMPLES`] the latest sample is always replaced. Each
    /// wait's first sample is always kept, as is every sample outside a
    /// wait.
    fn replaces_latest_sample(&mut self, status: Option<&str>) -> bool {
        let Some(status) = status.filter(|s| WAIT_STATUSES.contains(s)) else {
            return false;
        };
        let waiting = |back: usize| {
            self.data
                .len()
                .checked_sub(back)
                .and_then(|i| self.data.get(i))
                .and_then(|sample| sample.get("status"))
                .and_then(Value::as_str)
                == Some(status)
        };
        if !(waiting(1) && waiting(2)) {
            self.wait_samples += 1;
            return false;
        }
        let profile_ms = |back: usize| {
            self.data[self.data.len() - back]
                .get("profile_ms")
                .and_then(Value::as_i64)
                .unwrap_or_default()
        };
        if profile_ms(1).saturating_sub(profile_ms(2)) < WAIT_SAMPLE_INTERVAL_MS {
            return true;
        }
        if self.wait_samples >= MAX_WAIT_SAMPLES {
            if self.wait_samples == MAX_WAIT_SAMPLES {
                tracing::warn!(
                    "Debug shot kept {MAX_WAIT_SAMPLES} samples waiting for the user, keeping only the latest from now on"
                );
                // Counted once more so the warning is logged once.
                self.wait_samples += 1;
            }
            return true;
        }
        self.wait_samples += 1;
        false
    }
}

#[derive(Default)]
struct State {
    current: Option<DebugShot>,
    pending_node_json: Option<Value>,
}

/// The running shot's log records (`_logs_lock` and `current.logs`), kept
/// apart from its data. Present exactly while a shot is.
struct ShotLog {
    start_time: f64,
    records: Vec<Value>,
}

pub struct ShotDebugManager {
    debug_dir: PathBuf,
    state: Mutex<State>,
    /// Taken after `state` when both are needed; nothing logs while holding
    /// it, so the log sink, which takes only this one, never deadlocks.
    logs: Arc<Mutex<Option<ShotLog>>>,
}

impl ShotDebugManager {
    pub fn new(debug_dir: PathBuf) -> ShotDebugManager {
        ShotDebugManager {
            debug_dir,
            state: Mutex::new(State::default()),
            logs: Arc::new(Mutex::new(None)),
        }
    }

    /// The running shot with the records logged so far, under the data lock
    /// (`_copy_current_data`); `take` closes it.
    fn snapshot(&self, take: bool) -> Option<DebugShot> {
        let mut state = self.state.lock();
        let mut shot = if take {
            state.current.take()?
        } else {
            state.current.clone()?
        };
        let mut logs = self.logs.lock();
        let records = if take {
            logs.take().map(|log| log.records)
        } else {
            logs.as_ref().map(|log| log.records.clone())
        };
        shot.logs = records.unwrap_or_default();
        Some(shot)
    }

    /// Holds the shot's data lock, as `clear_current_data_lock` is held
    /// while a shot is built or a sample ingested.
    #[cfg(test)]
    pub fn hold_data_lock(&self) -> impl Sized + '_ {
        self.state.lock()
    }

    pub fn debug_dir(&self) -> &Path {
        &self.debug_dir
    }

    /// `start`: open a debug shot unless one is open, capture the logs.
    pub fn start(&self, app: &App) {
        tracing::info!("Starting debug shot");
        let needs_shot = self.state.lock().current.is_none();
        if needs_shot {
            let mut shot = DebugShot::new(app);
            let mut state = self.state.lock();
            shot.node_json = state.pending_node_json.take();
            if state.current.is_none() {
                *self.logs.lock() = Some(ShotLog {
                    start_time: shot.start_time,
                    records: Vec::new(),
                });
                state.current = Some(shot);
            }
        }
        let sink_logs = Arc::downgrade(&self.logs);
        app.log.set_shot_sink(Some(Arc::new(move |record: &Record| {
            let Some(logs) = sink_logs.upgrade() else {
                return;
            };
            // Only the log list's own lock: a record logged by code that
            // holds the data lock, or while the UART thread does, is kept
            // and never waits. A record logged between shots goes nowhere.
            if let Some(log) = logs.lock().as_mut() {
                let created = record
                    .created
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                log.records.push(json!({
                    "profile_ms": ((created - log.start_time) * 1000.0) as i64,
                    "loglevel": record.level,
                    "caller": record.name,
                    "log_message": record.line,
                }));
            }
        })));
    }

    /// The node program `send_json_with_hash` streamed: the open debug shot
    /// keeps it, or the next one to start.
    pub fn set_node_json(&self, program: Value) {
        let mut state = self.state.lock();
        match state.current.as_mut() {
            Some(shot) => shot.node_json = Some(program),
            None => state.pending_node_json = Some(program),
        }
    }

    /// The node program the open (or the next) debug shot records.
    #[cfg(test)]
    pub fn node_json(&self) -> Option<Value> {
        let state = self.state.lock();
        match state.current.as_ref() {
            Some(shot) => shot.node_json.clone(),
            None => state.pending_node_json.clone(),
        }
    }

    pub fn handle_sensor_data(&self, sensors: &SensorData) {
        if let Some(shot) = self.state.lock().current.as_mut() {
            shot.add_sensor_data(sensors);
        }
    }

    pub fn handle_shot_data(&self, app: &App, shot: &ShotData) {
        self.handle_shot_data_at(app, shot, now_seconds());
    }

    /// [`ShotDebugManager::handle_shot_data`] for a sample received at
    /// `now`, in seconds since the epoch.
    pub fn handle_shot_data_at(&self, app: &App, shot: &ShotData, now: f64) {
        let mut state = self.state.lock();
        let Some(current) = state.current.as_mut() else {
            return;
        };
        current.add_shot_data(app, shot, now);
        let status = shot.status.as_deref().unwrap_or_default();
        if [
            MachineStatus::PURGE,
            MachineStatus::HOME,
            MachineStatus::BOOT,
        ]
        .contains(&status)
            && esp::machine_status_to_profile(status) == shot.profile.as_deref()
        {
            current.shot_type = status.to_string();
        }
    }

    /// `_getRetentionDays`, clamped so shots are never kept forever.
    fn retention_days(&self, app: &App) -> i64 {
        let days = app
            .config
            .get_i64(keys::USER, keys::DEBUG_SHOT_DATA_RETENTION)
            .unwrap_or(31);
        if !(0..=MAX_DEBUG_SHOT_RETENTION_DAYS).contains(&days) {
            tracing::warn!(
                "Debug shot data retention of {days} days is out of range, clamping to {MAX_DEBUG_SHOT_RETENTION_DAYS} days"
            );
            return MAX_DEBUG_SHOT_RETENTION_DAYS;
        }
        days
    }

    /// `deleteOldDebugShotData`.
    pub fn delete_old(&self, app: &App) {
        let days = self.retention_days(app);
        tracing::info!("Debug shot data retention is set to {days} days, deleting old files");
        let cutoff = chrono::Local::now().naive_local() - chrono::Duration::days(days);
        let Ok(entries) = std::fs::read_dir(&self.debug_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(date) = chrono::NaiveDate::parse_from_str(&name, FOLDER_FORMAT) else {
                continue;
            };
            let Some(day) = date.and_hms_opt(0, 0, 0) else {
                continue;
            };
            if day < cutoff {
                if let Ok(files) = std::fs::read_dir(&path) {
                    for file in files.flatten() {
                        let relative = format!("{name}/{}", file.file_name().to_string_lossy());
                        if let Err(error) = app.history.unlink_debug_file(&relative) {
                            tracing::warn!("Could not unlink {relative}: {error}");
                        }
                    }
                }
                let _ = std::fs::remove_dir_all(&path);
                tracing::info!("Deleted all shots in {name}");
            }
        }
    }

    fn file_path(
        root: &Path,
        shot: &DebugShot,
        incomplete: bool,
    ) -> (chrono::DateTime<chrono::Local>, PathBuf) {
        let start = local_time(shot.start_time);
        let kind = if incomplete {
            format!("{}_incomplete", shot.shot_type)
        } else {
            shot.shot_type.clone()
        };
        let name = format!("{}.{kind}.json.zst", start.format(FILE_FORMAT));
        (
            start,
            root.join(start.format(FOLDER_FORMAT).to_string())
                .join(name),
        )
    }

    /// `_prepare_debug_shot_data` up to its `json.dumps`: the JSON of a
    /// shot, which it consumes so the samples are never copied.
    fn prepare(app: &App, mut shot: DebugShot, start: chrono::DateTime<chrono::Local>) -> Value {
        shot.profile.get_or_insert_with(|| json!({}));
        shot.node_json.get_or_insert_with(|| json!({}));
        let mut data = shot.into_json();
        let has_profile = data
            .get("profile")
            .is_some_and(crate::config::python_truthy);
        if !has_profile
            && data.get("type").and_then(Value::as_str) == Some("shot")
            && let Some(last) = app.profiles.last_profile()
        {
            let load_time = last.get("load_time").and_then(Value::as_f64).unwrap_or(0.0);
            let difference = (start.timestamp() as f64
                + f64::from(start.timestamp_subsec_micros()) / 1e6)
                - load_time;
            // PARITY: Python ignores the profile whenever the times differ
            // at all, despite the 30 seconds its message mentions.
            if difference.abs() != 0.0 {
                tracing::warn!(
                    "Profile load time ({}) and shot start time ({}) are more than 30 seconds apart. Ignoring profile",
                    local_time(load_time).format("%Y-%m-%d %H:%M:%S%.6f"),
                    start.format("%Y-%m-%d %H:%M:%S%.6f")
                );
            } else {
                let profile = last.get("profile").cloned().unwrap_or(Value::Null);
                let name = profile
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                tracing::info!(
                    "Using last profile {} for debug shot",
                    name.as_deref().unwrap_or("None")
                );
                data.insert("profile".into(), profile);
                if let Some(name) = name.filter(|n| !n.is_empty()) {
                    data.insert("profile_name".into(), json!(name));
                }
            }
        }
        Value::Object(data)
    }

    /// The rest of `_prepare_debug_shot_data`: `json.dumps(data,
    /// ensure_ascii=False)`, the text the file holds.
    fn dumps(data: &Value) -> String {
        pyfmt::dumps(data, &pyfmt::DumpOpts::unicode())
    }

    /// `write_current_incomplete_debug_shot`: a snapshot of the running shot
    /// for a bug report, relative to `root`.
    pub fn write_current_incomplete(&self, app: &App, root: &Path) -> Option<String> {
        let shot = self.snapshot(false)?;
        let (start, path) = Self::file_path(root, &shot, true);
        let text = Self::dumps(&Self::prepare(app, shot, start));
        tracing::info!("Writing incomplete debug shot snapshot");
        tracing::info!("Writing debug json to {}", path.display());
        if let Err(error) = history::files::write_compressed(&path, text.as_bytes()) {
            tracing::error!("Failed to write the debug shot snapshot: {error}");
            return None;
        }
        path.strip_prefix(root)
            .ok()
            .map(|p| p.display().to_string())
    }

    /// `stop`: close the shot and save it in the background.
    pub fn stop(&self, app: &Arc<App>) {
        tracing::info!("Stopping debug shot");
        let Some(shot) = self.snapshot(true) else {
            return;
        };
        let (start, path) = Self::file_path(&self.debug_dir, &shot, false);
        let shot_type = shot.shot_type.clone();
        let data = Self::prepare(app, shot, start);
        let app = app.clone();
        let _ = std::thread::Builder::new()
            .name("DebugShotCompr".into())
            .spawn(move || {
                tracing::info!("Writing and compressing debug file");
                let started = std::time::Instant::now();
                tracing::info!("Writing debug json to {}", path.display());
                // The text goes once written, and the upload anonymizes the
                // JSON itself instead of parsing the text back: a long shot
                // never has more than its JSON and one text in memory.
                let text = Self::dumps(&data);
                let written = history::files::write_compressed(&path, text.as_bytes());
                drop(text);
                if let Err(error) = written {
                    tracing::error!("Failed to write the debug shot: {error}");
                    return;
                }
                tracing::info!(
                    "Writing debug json to disc took {} ms",
                    pyfmt::repr_f64(started.elapsed().as_secs_f64() * 1000.0)
                );
                if let Some(history_id) = app.shots.db_history_id.lock().take() {
                    let relative: PathBuf = path
                        .iter()
                        .rev()
                        .take(2)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    if let Err(error) = app
                        .history
                        .link_debug_file(history_id, &relative.display().to_string())
                    {
                        tracing::warn!("Could not link the debug file: {error}");
                    }
                }
                if app.shot_sharing.should_upload(&app, &shot_type) {
                    app.shot_sharing
                        .upload_debug_shot(&app, &data, start, &path);
                }
                drop(data);
                tracing::info!("Debug shot data compressed and saved");
                app.debug_shots.delete_old(&app);
            });
    }

    /// `zipAllDebugShots`: every debug file in one zip, its name returned.
    pub fn zip_all(&self) -> std::io::Result<String> {
        tracing::info!("Zipping all debug files");
        let started = std::time::Instant::now();
        // Deliberate fix: Python fails with a 500 on a machine that has no
        // debug shots yet, because nothing created the folder.
        std::fs::create_dir_all(&self.debug_dir)?;
        for entry in walk(&self.debug_dir) {
            if entry.extension().and_then(|e| e.to_str()) == Some("zip")
                && let Some(name) = entry.file_name()
            {
                tracing::info!("Removing {}", name.to_string_lossy());
                // PARITY: Python removes the name joined to the debug
                // root, whatever folder the zip was found in.
                let _ = std::fs::remove_file(self.debug_dir.join(name));
            }
        }
        let name = chrono::Local::now()
            .format(&format!("debug-{FOLDER_FORMAT}-{FILE_FORMAT}.zip"))
            .to_string();
        let file = std::fs::File::create(self.debug_dir.join(&name))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for entry in walk(&self.debug_dir) {
            if entry.extension().and_then(|e| e.to_str()) != Some("zst") {
                continue;
            }
            let Ok(relative) = entry.strip_prefix(&self.debug_dir) else {
                continue;
            };
            zip.start_file(relative.display().to_string(), options)?;
            std::io::copy(&mut std::fs::File::open(&entry)?, &mut zip)?;
        }
        zip.finish()?;
        tracing::info!(
            "Zipping all debug files disc took {} ms",
            pyfmt::repr_f64(started.elapsed().as_secs_f64() * 1000.0)
        );
        Ok(name)
    }
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 7 samples a second, the rate the ESP32 streams while brewing.
    const RATE: usize = 7;

    /// A phase of the brew: its status and how many samples it lasts.
    struct Phase {
        status: &'static str,
        samples: usize,
    }

    /// The saved debug shot's samples after feeding `phases` through the
    /// UART path, one ShotData then one SensorData per sample. Each sample
    /// is tagged with its index in `external_1`, so a test can tell which
    /// fed sample, and which sensors, each saved one is.
    fn record(phases: &[Phase]) -> Vec<Value> {
        let (dir, app) = crate::testing::app();
        app.wifi.set_host(crate::wifi::fake::FakeNetwork::new());
        app.debug_shots.start(&app);
        let start = now_seconds();
        let mut index = 0;
        for phase in phases {
            for _ in 0..phase.samples {
                let shot = ShotData {
                    status: Some(phase.status.into()),
                    ..ShotData::default()
                };
                let now = start + index as f64 / RATE as f64;
                app.debug_shots.handle_shot_data_at(&app, &shot, now);
                let sensors = SensorData {
                    external_1: esp::PyNum::Float(index as f64),
                    ..SensorData::default()
                };
                app.debug_shots.handle_sensor_data(&sensors);
                index += 1;
            }
        }
        let name = app
            .debug_shots
            .write_current_incomplete(&app, dir.path())
            .expect("a debug shot is running");
        let raw =
            zstd::decode_all(std::fs::read(dir.path().join(name)).unwrap().as_slice()).unwrap();
        let shot: Value = serde_json::from_slice(&raw).unwrap();
        shot["data"].as_array().cloned().unwrap()
    }

    fn tag(sample: &Value) -> usize {
        sample["sensors"]["external_1"].as_f64().unwrap() as usize
    }

    /// A shot left open on "click to start" and then "click to purge" for
    /// hours stays bounded while every sample of the brew itself is kept.
    ///
    /// Regression it prevents: on a 1 GB machine a shot that waited 82
    /// minutes on "click to purge" grew the backend until the kernel killed
    /// it while saving the shot; before this change the brew below saves
    /// all of its 80,290 samples.
    /// Expected values: `WAIT_SAMPLE_INTERVAL_MS` and `MAX_WAIT_SAMPLES` in
    /// this module, the profile engine's status labels in `WAIT_STATUSES`.
    /// Bug it catches: appending wait samples without limit, thinning the
    /// brew's own samples, dropping the newest sample of a wait (what the
    /// user found when they acted) or pairing a sample with another one's
    /// sensors.
    #[test]
    fn a_long_wait_keeps_one_sample_a_second_up_to_a_cap() {
        let phases = [
            Phase {
                status: "click to start",
                samples: 10 * 60 * RATE,
            },
            Phase {
                status: "brewing",
                samples: 60 * RATE,
            },
            Phase {
                status: "click to purge",
                samples: 3 * 60 * 60 * RATE,
            },
            Phase {
                status: "purge",
                samples: 10 * RATE,
            },
        ];
        let data = record(&phases);

        let mut first = 0;
        let mut wait_samples = 0;
        for phase in &phases {
            let kept: Vec<&Value> = data
                .iter()
                .filter(|s| s["status"] == phase.status)
                .collect();
            let last = first + phase.samples - 1;
            if !WAIT_STATUSES.contains(&phase.status) {
                let tags: Vec<usize> = kept.iter().map(|s| tag(s)).collect();
                assert_eq!(tags, (first..=last).collect::<Vec<_>>(), "{}", phase.status);
            } else {
                assert_eq!(tag(kept[0]), first, "{} starts", phase.status);
                assert_eq!(tag(kept[kept.len() - 1]), last, "{} ends", phase.status);
                // Up to the cap one sample a second; the last pair is the
                // newest sample after the last kept one.
                for pair in kept[..kept.len() - 1].windows(2) {
                    let gap = pair[1]["profile_ms"].as_i64().unwrap()
                        - pair[0]["profile_ms"].as_i64().unwrap();
                    assert!(gap >= WAIT_SAMPLE_INTERVAL_MS, "{}: {gap} ms", phase.status);
                }
                wait_samples += kept.len();
            }
            first = last + 1;
        }
        // Ten minutes at one a second, then the rest of the cap.
        assert!(
            wait_samples <= MAX_WAIT_SAMPLES + 2,
            "{wait_samples} samples kept while waiting"
        );
        assert!(data.len() <= 60 * RATE + 10 * RATE + MAX_WAIT_SAMPLES + 2);
    }
}
