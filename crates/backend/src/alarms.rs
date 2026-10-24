//! `api/alarms.py`: conditions that block brewing for a while, persisted
//! across restarts as one file per alarm holding its end time.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::images::WARNING_TRIANGLE_IMAGE;
use crate::notifications::{Notification, NotificationManager, now_seconds, responses};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlarmType {
    MotorStressed,
    EspRestart,
    EspDisconnected,
}

impl AlarmType {
    pub fn file_name(self) -> &'static str {
        match self {
            AlarmType::MotorStressed => "motor_stress_alarm",
            AlarmType::EspRestart => "esp_restart_alarm",
            AlarmType::EspDisconnected => "esp_disconected_alarm",
        }
    }

    fn from_file_name(name: &str) -> Option<AlarmType> {
        [
            AlarmType::MotorStressed,
            AlarmType::EspRestart,
            AlarmType::EspDisconnected,
        ]
        .into_iter()
        .find(|t| t.file_name() == name)
    }
}

pub struct AlarmManager {
    dir: PathBuf,
    /// End time per alarm; `-inf` for an alarm that lasts the session.
    alarms: Mutex<HashMap<AlarmType, f64>>,
    notifications: Arc<NotificationManager>,
}

impl AlarmManager {
    pub fn new(dir: PathBuf, notifications: Arc<NotificationManager>) -> AlarmManager {
        AlarmManager {
            dir,
            alarms: Mutex::new(HashMap::new()),
            notifications,
        }
    }

    fn path(&self, alarm: AlarmType) -> PathBuf {
        self.dir.join(alarm.file_name())
    }

    /// `AlarmManager.init`: load the alarms on disk and start the expiry task.
    pub fn init(self: &Arc<Self>) {
        if !self.dir.exists() {
            tracing::info!("Creating alarms directory");
            if let Err(error) = std::fs::create_dir_all(&self.dir) {
                tracing::error!(
                    "Cannot create alarms directory, alarms set will be lost on backend restart: {error}"
                );
            }
        }
        tracing::info!("Loading alarms from disk");
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let Some(alarm) = AlarmType::from_file_name(&name) else {
                    continue;
                };
                tracing::debug!("Alarm file found for {name}");
                let parsed = std::fs::read_to_string(entry.path())
                    .ok()
                    .and_then(|t| pyfmt::py_float(t.trim()));
                match parsed {
                    Some(end) => {
                        self.alarms.lock().entry(alarm).or_insert(end);
                    }
                    None => {
                        tracing::warn!("Cannot read file {name}, clearing alarm");
                        if let Err(error) = std::fs::remove_file(entry.path()) {
                            tracing::error!("Cannot remove file {name}: {error}");
                        }
                        self.alarms.lock().remove(&alarm);
                    }
                }
            }
        }
        let manager = Arc::clone(self);
        std::thread::Builder::new()
            .name("alarm manager".into())
            .spawn(move || {
                loop {
                    manager.expire(now_seconds());
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .map(|_| ())
            .unwrap_or_else(|e| tracing::error!("Could not start the alarm task: {e}"));
    }

    fn expire(&self, now: f64) {
        let expired: Vec<AlarmType> = self
            .alarms
            .lock()
            .iter()
            .filter(|(_, end)| end.is_finite() && **end < now)
            .map(|(t, _)| *t)
            .collect();
        for alarm in expired {
            self.remove_file(alarm);
            self.alarms.lock().remove(&alarm);
        }
    }

    fn remove_file(&self, alarm: AlarmType) {
        let path = self.path(alarm);
        tracing::debug!("clearing {} file", alarm.file_name());
        if !path.exists() {
            tracing::warn!("{} files does not exist", alarm.file_name());
            return;
        }
        if let Err(error) = std::fs::remove_file(&path) {
            tracing::error!("Cannot remove {} file: {error}", alarm.file_name());
        }
    }

    /// `set_alarm`: `end_time` `None` means for this session only (no file).
    pub fn set(&self, alarm: AlarmType, end_time: Option<f64>, force: bool, quiet: bool) {
        let end = end_time.unwrap_or(f64::NEG_INFINITY);
        let path = self.path(alarm);
        if !path.exists() || force {
            if end_time.is_some() {
                if let Err(error) = std::fs::create_dir_all(&self.dir)
                    .and_then(|_| std::fs::write(&path, pyfmt::repr_f64(end)))
                {
                    tracing::warn!(
                        "Cannot write {} file, the alarm will be lost if the backend stops: {error}",
                        alarm.file_name()
                    );
                }
            } else {
                tracing::info!(
                    "setting alarm {} with the duration of the session",
                    alarm.file_name()
                );
            }
            self.alarms.lock().insert(alarm, end);
            if !quiet {
                self.notify(&alarm_message(alarm, end), WARNING_TRIANGLE_IMAGE);
            }
        } else {
            match std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| pyfmt::py_float(t.trim()))
            {
                Some(stored) => {
                    self.alarms.lock().insert(alarm, stored);
                    let remaining = if stored.is_infinite() {
                        "Never".to_string()
                    } else {
                        format!("{:.2}s", stored - now_seconds())
                    };
                    tracing::warn!("Alarm was already set, will end in: {remaining}");
                    tracing::warn!("You can force the alarm to overwrite it");
                }
                None => {
                    tracing::warn!("Cannot read {} file, clearing alarm", alarm.file_name());
                    self.remove_file(alarm);
                    self.alarms.lock().remove(&alarm);
                }
            }
        }
    }

    pub fn clear(&self, alarm: AlarmType) {
        let present = self.alarms.lock().remove(&alarm).is_some();
        if present {
            tracing::debug!("Removing {}", alarm.file_name());
            self.remove_file(alarm);
        }
    }

    /// `is_alarm_set`: the end time, or `None`.
    pub fn end_time(&self, alarm: AlarmType) -> Option<f64> {
        self.alarms.lock().get(&alarm).copied()
    }

    /// `AlarmManager._notify_user`.
    pub fn notify(&self, message: &str, image: &str) {
        self.notifications
            .add(Notification::with_responses(message, &[responses::OK]).image(image));
    }
}

/// Minutes left, rounded up, or 10 for an alarm without an end.
pub fn minutes_left(end: f64) -> i64 {
    if end.is_finite() {
        ((end - now_seconds()) / 60.0).ceil() as i64
    } else {
        10
    }
}

fn alarm_message(alarm: AlarmType, end: f64) -> String {
    match alarm {
        AlarmType::MotorStressed => format!(
            "Brewing paused because of high strain in the motor. Let the machine rest for {} mins and use a coarser grind before trying again",
            minutes_left(end)
        ),
        AlarmType::EspRestart => {
            "Digital controller seems to be unresponsive, buttons are disabled.".to_string()
        }
        AlarmType::EspDisconnected => {
            "Digital controller seems disconnected, buttons are disabled".to_string()
        }
    }
}
