//! `timezone_manager.py`: the system timezone through `timedatectl`, the
//! country/city table the Dial's timezone picker reads
//! (`UI_timezones.json`), the IP-based lookup for automatic sync, and setting
//! the clock.
//!
//! Every call here blocks (commands, HTTP); async callers run them in
//! `spawn_blocking`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde_json::{Map, Value, json};

use crate::config::{AUTOMATIC_TIMEZONE_SYNC, ConfigStore, keys};
use crate::http_client::{HttpClient, HttpError};
use crate::py::{CommandRunner, Completed, PyError, argv, called_process_error, py_str};

pub const TIMEZONE_STATUS_COMMAND: &str =
    "timedatectl status | grep 'Time zone' | awk -F'[:()]' '{print $2}'";
pub const TZ_GETTER_URL: &str = "https://analytics.meticulousespresso.com/timezone_ip";
const MAX_TIMEZONE_FETCH_ATTEMPTS: u32 = 5;
const SYNC_TIMEOUT: Duration = Duration::from_secs(20);
const SYNC_TIMEOUT_MESSAGE: &str =
    "time out error, server could not be contacted or took too long to answer";

/// `TimezoneManagerError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TimezoneError(pub String);

/// The exception `request_and_sync_tz()` lets escape, as `str(e)`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TimezoneSyncError(pub String);

#[derive(Default)]
struct State {
    system_timezone: Option<String>,
    system_synced: bool,
    fetch_attempts: u32,
}

pub struct TimezoneManager {
    config: Arc<ConfigStore>,
    json_path: PathBuf,
    zoneinfo: PathBuf,
    runner: RwLock<Arc<dyn CommandRunner>>,
    http: RwLock<Arc<dyn HttpClient>>,
    state: Mutex<State>,
    sleep: RwLock<fn(Duration)>,
}

/// `redact_timezone`: only the region survives ("America/*****").
pub fn redact_timezone(timezone: Option<&str>) -> String {
    let Some(timezone) = timezone.filter(|t| !t.is_empty()) else {
        return String::new();
    };
    let mut parts = timezone.split('/');
    let region = parts.next().unwrap_or("");
    if parts.next().is_some() {
        format!("{region}/*****")
    } else {
        region.to_string()
    }
}

/// `_redact_timezone_in`: strip a raw timezone out of free-form text.
fn redact_timezone_in(text: &str, timezone: Option<&str>) -> String {
    match timezone.filter(|t| !t.is_empty()) {
        Some(tz) if !text.is_empty() => text.replace(tz, &redact_timezone(Some(tz))),
        _ => text.to_string(),
    }
}

impl TimezoneManager {
    pub fn new(
        config: Arc<ConfigStore>,
        json_path: PathBuf,
        runner: Arc<dyn CommandRunner>,
        http: Arc<dyn HttpClient>,
    ) -> TimezoneManager {
        TimezoneManager {
            config,
            json_path,
            zoneinfo: PathBuf::from("/usr/share/zoneinfo"),
            runner: RwLock::new(runner),
            http: RwLock::new(http),
            state: Mutex::new(State::default()),
            sleep: RwLock::new(std::thread::sleep),
        }
    }

    #[cfg(test)]
    pub fn set_runner(&self, runner: Arc<dyn CommandRunner>) {
        *self.runner.write() = runner;
    }

    #[cfg(test)]
    pub fn set_http(&self, http: Arc<dyn HttpClient>) {
        *self.http.write() = http;
    }

    #[cfg(test)]
    pub fn set_zoneinfo(&mut self, dir: PathBuf) {
        self.zoneinfo = dir;
    }

    #[cfg(test)]
    pub fn set_sleep(&self, sleep: fn(Duration)) {
        *self.sleep.write() = sleep;
    }

    fn runner(&self) -> Arc<dyn CommandRunner> {
        self.runner.read().clone()
    }

    /// `TimezoneManager.init()`: make the system timezone and the user's
    /// setting agree, preferring the user's.
    pub fn init(&self) {
        self.validate_timezones_json();
        let system = self.get_system_timezone();
        self.state.lock().system_timezone = system.clone();
        let mut target = self.config.get(keys::USER, keys::TIME_ZONE);
        let differs = match (&target, &system) {
            (Value::Null, None) => false,
            (Value::String(t), Some(s)) => t != s,
            _ => true,
        };
        if !differs {
            return;
        }
        if target.is_null() {
            tracing::info!("No timezone was ever configured by user");
            tracing::info!("Setting system timezone to Etc/UTC as a default");
            target = json!("Etc/UTC");
            self.config.set(keys::USER, keys::TIME_ZONE, target.clone());
        } else {
            tracing::warn!(
                "user config and system timezones confilct, updating system config to {}",
                redact_timezone(Some(&py_str(&target)))
            );
        }
        let target = py_str(&target);
        if let Err(error) = self.set_system_timezone(&target) {
            let text = redact_timezone_in(&error.0, Some(&target));
            let text = redact_timezone_in(&text, system.as_deref());
            tracing::error!(
                "failed to set system TZ, syncing user TZ with system to {}. Error: {text}",
                redact_timezone(system.as_deref())
            );
            self.config.set(
                keys::USER,
                keys::TIME_ZONE,
                system.map(Value::String).unwrap_or(Value::Null),
            );
        }
        self.config.save();
    }

    /// `update_timezone(new_timezone)`: apply a zone the user picked. The
    /// caller stores it in the settings.
    pub fn update_timezone(&self, new_timezone: &str) -> Result<(), TimezoneError> {
        let stripped = new_timezone.trim_end_matches('"').trim_start_matches('"');
        let current = self.config.get(keys::USER, keys::TIME_ZONE);
        if current.as_str() == Some(stripped) {
            return Ok(());
        }
        match self.set_system_timezone(stripped) {
            Ok(()) => {
                tracing::debug!("update timezone status: Success");
                Ok(())
            }
            Err(error) => {
                tracing::error!("Error updating timezone: {error}");
                Err(TimezoneError(format!("Error updating timezone: {error}")))
            }
        }
    }

    /// `set_system_timezone(new_timezone)`.
    ///
    /// The Python runs `f"timedatectl set-timezone {tz}"` through a shell; the
    /// zone comes from the settings API, so here it is a single argument.
    /// Messages still quote the command line the Python would have run.
    pub fn set_system_timezone(&self, new_timezone: &str) -> Result<(), TimezoneError> {
        tracing::debug!("setting system timezone");
        let command = format!("timedatectl set-timezone {new_timezone}");
        let result = self
            .runner()
            .run(&argv(&["timedatectl", "set-timezone", new_timezone]), None);
        let failure = match result {
            Ok(done) if done.returncode != 0 => {
                // CalledProcessError: raised without logging here.
                let message = redact_timezone_in(
                    &format!(
                        "Error setting system time zone: {} [ Out: {} | Err: {} ]",
                        called_process_error(&command, done.returncode),
                        done.stdout,
                        done.stderr
                    ),
                    Some(new_timezone),
                );
                return Err(TimezoneError(message));
            }
            Ok(done) if !done.stdout.is_empty() || !done.stderr.is_empty() => {
                format!("[ Out:{} | Err: {} ]", done.stdout, done.stderr)
            }
            Ok(_) => {
                let now = self.get_system_timezone();
                tracing::info!("new system time zone: {}", redact_timezone(now.as_deref()));
                return Ok(());
            }
            Err(error) => error.to_string(),
        };
        let message = redact_timezone_in(
            &format!("Error setting system time zone: {failure}"),
            Some(new_timezone),
        );
        tracing::error!("{message}");
        Err(TimezoneError(message))
    }

    /// `get_system_timezone()`: `None` when the pipeline cannot run.
    pub fn get_system_timezone(&self) -> Option<String> {
        match self.runner().run_shell(TIMEZONE_STATUS_COMMAND) {
            Ok(Completed {
                returncode: 0,
                stdout,
                stderr,
            }) => {
                if !stderr.is_empty() {
                    tracing::error!("error getting system time zone, using last saved by user");
                }
                Some(stdout.trim().to_string())
            }
            _ => {
                tracing::error!("error getting system time zone, using last saved by user");
                None
            }
        }
    }

    /// The zone read at start, before any change.
    #[cfg(test)]
    pub fn startup_system_timezone(&self) -> Option<String> {
        self.state.lock().system_timezone.clone()
    }

    /// `validate_timezones_json()`: generate the picker's table on first use.
    pub fn validate_timezones_json(&self) {
        if self.json_path.is_file() {
            return;
        }
        tracing::warn!("Json file for UI timezones does not exist, generating");
        // PARITY: the file is opened (and so created empty) before the table
        // is built; a failure leaves the empty file behind.
        let file = match std::fs::File::create(&self.json_path) {
            Ok(file) => file,
            Err(error) => {
                tracing::error!(
                    "Error generating json file: {}",
                    os_error(&error, &self.json_path)
                );
                return;
            }
        };
        match self.get_organized_timezones() {
            Ok(table) => {
                use std::io::Write;
                let mut file = file;
                if let Err(error) = file.write_all(pyfmt::dumps_default(&table).as_bytes()) {
                    tracing::error!("Error generating json file: {error}");
                }
            }
            Err(error) => tracing::error!("Error generating json file: {error}"),
        }
    }

    /// `get_organized_timezones()`: zones grouped by country name, city
    /// names with spaces, the `Etc/` zones under `ETC` and the rest under
    /// `LINKS`.
    pub fn get_organized_timezones(&self) -> Result<Value, String> {
        let iso_path = self.zoneinfo.join("iso3166.tab");
        let iso = std::fs::read_to_string(&iso_path).map_err(|e| os_error(&e, &iso_path))?;
        let mut code_to_name: Vec<(String, String)> = Vec::new();
        for line in iso.split_inclusive('\n') {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            let Some(name) = parts.get(1) else {
                return Err("list index out of range".into());
            };
            let code = parts[0].to_string();
            if !code_to_name.iter().any(|(c, _)| *c == code) {
                code_to_name.push((code, name.trim_end_matches('\n').to_string()));
            }
        }

        let zone_path = self.zoneinfo.join("zone.tab");
        let zones = std::fs::read_to_string(&zone_path).map_err(|e| os_error(&e, &zone_path))?;
        let mut country_to_timezones: Vec<(String, Vec<String>)> = Vec::new();
        for line in zones.split_inclusive('\n') {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            let Some(timezone) = parts.get(2) else {
                return Err("list index out of range".into());
            };
            let timezone = timezone.trim_end().to_string();
            let country = code_to_name
                .iter()
                .find(|(c, _)| c == parts[0])
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| "Unknown Country".into());
            match country_to_timezones.iter_mut().find(|(c, _)| *c == country) {
                Some((_, list)) => list.push(timezone),
                None => country_to_timezones.push((country, vec![timezone])),
            }
        }

        Ok(self
            .group_system_timezones(&country_to_timezones)
            .unwrap_or_else(|| {
                tracing::error!("Could not fetch timezones from system");
                Value::Object(Map::new())
            }))
    }

    fn group_system_timezones(
        &self,
        country_to_timezones: &[(String, Vec<String>)],
    ) -> Option<Value> {
        let result = self
            .runner()
            .run(&argv(&["timedatectl", "list-timezones"]), None)
            .ok()
            .filter(|r| r.returncode == 0)?;
        let mut grouped: Map<String, Value> = Map::new();
        let mut put = |group: &str, sub_region: String, tz: &str| {
            let entry = grouped
                .entry(group.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(map) = entry {
                map.insert(sub_region, json!(tz));
            }
        };
        for tz in result.stdout.lines() {
            let segments: Vec<&str> = tz.split('/').collect();
            let fallback = segments[0];
            let zone: Vec<String> = segments[1..].iter().map(|z| z.replace('_', " ")).collect();
            let mut sub_region = if zone.is_empty() {
                fallback.to_string()
            } else {
                zone.join(" - ")
            };
            if tz.starts_with("Etc/") {
                put("ETC", sub_region, tz);
            } else if let Some((country, _)) = country_to_timezones
                .iter()
                .find(|(_, zones)| zones.iter().any(|z| z == tz))
            {
                // A zone listed in zone.tab always has a region, so `zone[0]`
                // exists; Python would fail the whole table otherwise.
                let first = zone.first()?;
                if first == country && zone.len() > 1 {
                    sub_region = zone[1..].join(" - ");
                }
                put(country, sub_region, tz);
            } else {
                put("LINKS", sub_region, tz);
            }
        }
        Some(Value::Object(grouped))
    }

    /// `get_UI_timezones()`: the picker's table, `{}` when it cannot be read.
    #[allow(dead_code, reason = "read by the settings API, ported separately")]
    pub fn get_ui_timezones(&self) -> Value {
        self.validate_timezones_json();
        let parsed = std::fs::read_to_string(&self.json_path)
            .ok()
            .and_then(|text| pyfmt::loads_lenient(&text).ok());
        match parsed {
            Some(value) => value,
            None => {
                tracing::error!(
                    "Json file for UI timezones does not exist and could not be created"
                );
                Value::Object(Map::new())
            }
        }
    }

    /// `tz_background_update()`: one automatic lookup per call while the
    /// zone is not synced, at most five failed attempts in a row.
    pub fn tz_background_update(&self) {
        let automatic = self.config.get(keys::USER, keys::TIMEZONE_SYNC).as_str()
            == Some(AUTOMATIC_TIMEZONE_SYNC);
        {
            let mut state = self.state.lock();
            if !automatic || state.system_synced {
                return;
            }
            if state.fetch_attempts >= MAX_TIMEZONE_FETCH_ATTEMPTS {
                return;
            }
            state.fetch_attempts += 1;
        }
        tracing::info!("Timezone is set to automatic, fetching timezone in the background");
        match self.request_and_sync_tz() {
            Ok(_) => self.state.lock().fetch_attempts = 0,
            Err(error) => {
                tracing::error!("Error while fetching timezone in the background: {error}")
            }
        }
    }

    /// `request_and_sync_tz()`: ask the analytics server which zone our IP is
    /// in and apply it. Retries every second until a 200 or 20 seconds pass.
    pub fn request_and_sync_tz(&self) -> Result<Option<String>, TimezoneSyncError> {
        let http = self.http.read().clone();
        let sleep = *self.sleep.read();
        let deadline = Instant::now() + SYNC_TIMEOUT;
        let timed_out = || {
            tracing::error!("{SYNC_TIMEOUT_MESSAGE}");
            TimezoneSyncError(SYNC_TIMEOUT_MESSAGE.into())
        };
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining <= Duration::from_secs(1) {
                sleep(remaining);
                return Err(timed_out());
            }
            sleep(Duration::from_secs(1));
            let remaining = deadline.saturating_duration_since(Instant::now());
            let response = match http.get(TZ_GETTER_URL, remaining, None) {
                Ok(response) => response,
                Err(HttpError::Timeout) => return Err(timed_out()),
                Err(HttpError::Failed(message)) => return Err(TimezoneSyncError(message)),
            };
            if response.status != 200 {
                tracing::warn!(
                    "timezone fetch failed with status code: {}, re-fetching",
                    response.status
                );
                continue;
            }
            let body = pyfmt::loads_lenient(&response.body)
                .map_err(|_| TimezoneSyncError(json_error_text(&response.body)))?;
            let Value::Object(map) = &body else {
                return Err(TimezoneSyncError(format!(
                    "'{}' object has no attribute 'get'",
                    crate::py::type_name(&body)
                )));
            };
            return match map.get("tz") {
                None | Some(Value::Null) => {
                    tracing::warn!("No timezone known for this IP");
                    self.state.lock().system_synced = true;
                    Ok(None)
                }
                Some(Value::String(tz)) => match self.update_timezone(tz) {
                    Ok(()) => {
                        self.state.lock().system_synced = true;
                        Ok(Some(tz.clone()))
                    }
                    Err(error) => {
                        tracing::error!("failed to set the provided timezone\n\t{error}");
                        Err(TimezoneSyncError(
                            "failed to set the provided timezone".into(),
                        ))
                    }
                },
                Some(other) => Err(TimezoneSyncError(
                    PyError::attribute_error(format!(
                        "'{}' object has no attribute 'rstrip'",
                        crate::py::type_name(other)
                    ))
                    .message,
                )),
            };
        }
    }

    /// `set_system_datetime(newDate)`: `date --set "%Y-%m-%d %H:%M:%S"`.
    #[allow(
        dead_code,
        reason = "called by the machine API and the imager, ported separately"
    )]
    pub fn set_system_datetime(&self, date: &chrono::NaiveDateTime) -> Result<(), PyError> {
        let formatted = date.format("%Y-%m-%d %H:%M:%S").to_string();
        tracing::info!("Setting system date to {formatted}");
        let command = argv(&["date", "--set", &formatted]);
        let result = self.runner().run(&command, None).map_err(|e| e.to_py())?;
        if result.returncode != 0 {
            return Err(PyError::new(
                "CalledProcessError",
                called_process_error(
                    &format!(
                        "[{}]",
                        command
                            .iter()
                            .map(|a| crate::py::repr_str(a))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    result.returncode,
                ),
            ));
        }
        Ok(())
    }
}

/// `str(OSError)` for a file the Python opens.
fn os_error(error: &std::io::Error, path: &Path) -> String {
    let errno = error.raw_os_error().unwrap_or(0);
    let reason = match error.kind() {
        std::io::ErrorKind::NotFound => "No such file or directory".to_string(),
        std::io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        _ => error.to_string(),
    };
    format!("[Errno {errno}] {reason}: '{}'", path.display())
}

/// The `JSONDecodeError` text for a body that is not JSON.
fn json_error_text(body: &str) -> String {
    match crate::http::parse_python_json(body.as_bytes()) {
        Err(error) => error.message,
        Ok(_) => "Expecting value: line 1 column 1 (char 0)".into(),
    }
}

#[cfg(test)]
mod tests;
