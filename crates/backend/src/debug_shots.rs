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
        }
    }

    /// `DebugShot.to_json`.
    pub fn to_json(&self) -> Map<String, Value> {
        let mut out = Map::new();
        out.insert("time".into(), json!(self.start_time));
        out.insert("type".into(), json!(self.shot_type));
        out.insert(
            "profile_name".into(),
            self.profile_name.clone().map_or(Value::Null, Value::String),
        );
        out.insert("machine".into(), Value::Object(self.machine.clone()));
        out.insert(
            "profile".into(),
            self.profile.clone().unwrap_or(Value::Null),
        );
        out.insert(
            "nodeJSON".into(),
            self.node_json.clone().unwrap_or(Value::Null),
        );
        out.insert("config".into(), self.config.clone());
        out.insert("data".into(), Value::Array(self.data.clone()));
        out.insert("logs".into(), Value::Array(self.logs.clone()));
        out
    }

    fn add_sensor_data(&mut self, sensors: &SensorData) {
        if let Some(Value::Object(last)) = self.data.last_mut() {
            last.insert("sensors".into(), crate::shots::sensor_dict(sensors));
        }
    }

    /// `Shot.addShotData` plus `profile_ms` (`DebugShot.append_shot_data`).
    fn add_shot_data(&mut self, app: &App, shot: &ShotData) {
        // The profile naming of Shot.addShotData, shared through a Shot.
        let mut naming = Shot::new(0);
        naming.profile_name = self.profile_name.clone();
        naming.profile = self.profile.clone();
        naming.add_shot_data(app, shot);
        self.profile_name = naming.profile_name;
        self.profile = naming.profile;
        let mut sample = Shot::format_sample(shot);
        sample.insert(
            "profile_ms".into(),
            json!(((now_seconds() - self.start_time) * 1000.0) as i64),
        );
        self.data.push(Value::Object(sample));
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

    pub fn handle_sensor_data(&self, sensors: &SensorData) {
        if let Some(shot) = self.state.lock().current.as_mut() {
            shot.add_sensor_data(sensors);
        }
    }

    pub fn handle_shot_data(&self, app: &App, shot: &ShotData) {
        let mut state = self.state.lock();
        let Some(current) = state.current.as_mut() else {
            return;
        };
        current.add_shot_data(app, shot);
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

    /// `_prepare_debug_shot_data`.
    fn prepare(app: &App, shot: &DebugShot, start: chrono::DateTime<chrono::Local>) -> String {
        let mut shot = shot.clone();
        shot.profile.get_or_insert_with(|| json!({}));
        shot.node_json.get_or_insert_with(|| json!({}));
        let mut data = shot.to_json();
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
        pyfmt::dumps(&Value::Object(data), &pyfmt::DumpOpts::unicode())
    }

    /// `write_current_incomplete_debug_shot`: a snapshot of the running shot
    /// for a bug report, relative to `root`.
    pub fn write_current_incomplete(&self, app: &App, root: &Path) -> Option<String> {
        let shot = self.snapshot(false)?;
        let (start, path) = Self::file_path(root, &shot, true);
        let text = Self::prepare(app, &shot, start);
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
        let text = Self::prepare(app, &shot, start);
        let app = app.clone();
        let _ = std::thread::Builder::new()
            .name("DebugShotCompr".into())
            .spawn(move || {
                tracing::info!("Writing and compressing debug file");
                let started = std::time::Instant::now();
                tracing::info!("Writing debug json to {}", path.display());
                if let Err(error) = history::files::write_compressed(&path, text.as_bytes()) {
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
                        .upload_debug_shot(&app, &text, start, &path);
                }
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
