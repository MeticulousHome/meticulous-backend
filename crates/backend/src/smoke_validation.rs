//! `smoke_validation.py`: after an OS update, the first complete espresso
//! shot (heating, extraction, retracting, purge, then the ESP32's final log
//! line) is recorded per image version and reported to Hawkbit.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Map, Value, json};

use crate::host::Host;

const FINAL_COFFEE_LOG: &str = "coffee profile finished. Heater will be kept on until timeout.";
const PUBLISH_SERVICE: &str = "meticulous-smoke-report.service";

/// `MachineStatus` values the sequence looks at.
mod status {
    pub const IDLE: &str = "idle";
    pub const HEATING: &str = "heating";
    pub const PURGE: &str = "purge";
    pub const RETRACTING: &str = "retracting";
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ShotSequence {
    active: bool,
    saw_heating: bool,
    saw_extraction: bool,
    saw_retracting: bool,
    saw_purge: bool,
}

impl ShotSequence {
    fn to_value(self) -> Value {
        json!({
            "active": self.active,
            "saw_heating": self.saw_heating,
            "saw_extraction": self.saw_extraction,
            "saw_retracting": self.saw_retracting,
            "saw_purge": self.saw_purge,
        })
    }
}

/// What `get_status` reads off `Machine`.
#[derive(Debug, Clone, PartialEq)]
pub struct MachineSnapshot {
    /// `Machine.infoReady`.
    pub info_ready: bool,
    /// `Machine.esp_info is not None`.
    pub esp_info_present: bool,
    /// `Machine.esp_info.firmwareV`.
    pub firmware_version: Option<String>,
    /// `Machine.data_sensors.state`.
    pub machine_state: Option<String>,
    /// `Machine.data_sensors.status`.
    pub machine_status: Option<String>,
}

impl MachineSnapshot {
    /// What `get_status(Machine)` reads now.
    pub fn of(machine: &crate::machine::Machine) -> MachineSnapshot {
        let esp_info = machine.esp_info();
        let data = machine.data_sensors();
        MachineSnapshot {
            info_ready: machine.info_ready(),
            esp_info_present: esp_info.is_some(),
            firmware_version: esp_info.map(|info| info.firmware_v),
            machine_state: Some(data.state),
            machine_status: data.status,
        }
    }
}

pub struct SmokeValidationManager {
    state_file: PathBuf,
    version_file: PathBuf,
    publish_command: PathBuf,
    host: Arc<Host>,
    lock: Mutex<()>,
    sequence: Mutex<ShotSequence>,
}

/// An exception the Python lets escape, as `str(exc)`.
pub type PyError = String;

impl SmokeValidationManager {
    pub fn new(
        state_file: PathBuf,
        version_file: PathBuf,
        publish_command: PathBuf,
        host: Arc<Host>,
    ) -> SmokeValidationManager {
        SmokeValidationManager {
            state_file,
            version_file,
            publish_command,
            host,
            lock: Mutex::new(()),
            sequence: Mutex::new(ShotSequence::default()),
        }
    }

    fn now() -> String {
        let now = chrono::Utc::now().naive_utc();
        format!("{}+00:00", crate::notifications::python_isoformat(&now))
    }

    fn current_version(&self) -> String {
        match std::fs::read(&self.version_file) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => {
                    let version = crate::ota::py_strip(&text);
                    if version.is_empty() {
                        "unknown".to_string()
                    } else {
                        version.to_string()
                    }
                }
                Err(error) => {
                    tracing::warn!("Failed to read image version for smoke validation: {error}");
                    "unknown".to_string()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "unknown".to_string(),
            Err(error) => {
                tracing::warn!("Failed to read image version for smoke validation: {error}");
                "unknown".to_string()
            }
        }
    }

    fn default_state(&self) -> Value {
        json!({
            "image_version": self.current_version(),
            "post_update_shot_completed": false,
            "post_update_shot_completed_at": null,
            "post_update_shot_completed_version": null,
        })
    }

    fn read_state(&self) -> Result<Map<String, Value>, PyError> {
        let state = match std::fs::read(&self.state_file) {
            Ok(bytes) => match std::str::from_utf8(&bytes)
                .map_err(|e| e.to_string())
                .and_then(|text| pyfmt::loads_lenient(text).map_err(|e| e.to_string()))
            {
                Ok(state) => state,
                Err(error) => {
                    tracing::warn!("Failed to read smoke validation state: {error}");
                    self.default_state()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.default_state(),
            Err(error) => {
                tracing::warn!("Failed to read smoke validation state: {error}");
                self.default_state()
            }
        };
        let Value::Object(mut state) = state else {
            // PARITY: `state.get(...)` on a JSON document that is not an
            // object raises AttributeError, which the handler reports.
            return Err(format!(
                "'{}' object has no attribute 'get'",
                python_type_name(&state)
            ));
        };
        let current = self.current_version();
        if state.get("image_version") != Some(&json!(current)) {
            state.insert("image_version".into(), json!(current));
            state.insert("post_update_shot_completed".into(), json!(false));
            state.insert("post_update_shot_completed_at".into(), Value::Null);
            state.insert("post_update_shot_completed_version".into(), Value::Null);
            self.write_state(&state)?;
        }
        Ok(state)
    }

    /// `_write_state`: `json.dumps(state, indent=2, sort_keys=True)` plus a
    /// newline, through a `.tmp` sibling and a rename.
    fn write_state(&self, state: &Map<String, Value>) -> Result<(), PyError> {
        let opts = pyfmt::DumpOpts {
            sort_keys: true,
            ..pyfmt::DumpOpts::indented(2)
        };
        let text = pyfmt::dumps(&Value::Object(state.clone()), &opts) + "\n";
        let write = || -> std::io::Result<()> {
            if let Some(parent) = self.state_file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let temporary = self.state_file.with_extension("tmp");
            std::fs::write(&temporary, text.as_bytes())?;
            std::fs::rename(&temporary, &self.state_file)
        };
        write().map_err(|e| e.to_string())
    }

    /// `get_state`.
    pub fn get_state(&self) -> Result<Map<String, Value>, PyError> {
        let _guard = self.lock.lock();
        self.read_state()
    }

    /// `get_status`: what `GET /smoke-validation` answers.
    pub fn status(&self, machine: &MachineSnapshot) -> Result<Value, PyError> {
        let state = self.get_state()?;
        let field = |key: &str| state.get(key).cloned().ok_or_else(|| format!("'{key}'"));
        Ok(json!({
            "backend_initialized": true,
            "esp32_responding": machine.info_ready && machine.esp_info_present,
            "machine_state": machine.machine_state,
            "machine_status": machine.machine_status,
            "firmware_version": machine.firmware_version,
            "image_version": field("image_version")?,
            "post_update_shot_completed": field("post_update_shot_completed")?,
            "post_update_shot_completed_at": field("post_update_shot_completed_at")?,
            "post_update_shot_completed_version": field("post_update_shot_completed_version")?,
        }))
    }

    /// `observe_status`: called by the machine with each status sample and
    /// whether the profile is extracting.
    pub fn observe_status(&self, machine_status: &str, extracting: bool) {
        let mut sequence = self.sequence.lock();
        if machine_status == status::HEATING {
            *sequence = ShotSequence {
                active: true,
                saw_heating: true,
                ..ShotSequence::default()
            };
            return;
        }
        if !sequence.active {
            return;
        }
        let is_extraction_stage = extracting
            && ![status::HEATING, status::RETRACTING, status::PURGE].contains(&machine_status);
        if is_extraction_stage {
            sequence.saw_extraction = true;
        }
        if machine_status == status::RETRACTING && sequence.saw_extraction {
            sequence.saw_retracting = true;
        }
        if machine_status == status::PURGE && sequence.saw_retracting {
            sequence.saw_purge = true;
        }
        if machine_status == status::IDLE {
            *sequence = ShotSequence::default();
        }
    }

    /// `observe_esp_log`: called by the machine with each ESP32 log message.
    pub fn observe_esp_log(&self, message: &str) {
        if message != FINAL_COFFEE_LOG {
            return;
        }
        let sequence = std::mem::take(&mut *self.sequence.lock());
        if sequence.saw_heating
            && sequence.saw_extraction
            && sequence.saw_retracting
            && sequence.saw_purge
        {
            if let Err(error) = self.mark_post_update_shot_completed() {
                tracing::warn!("Failed to record the post-update shot: {error}");
            }
        } else {
            tracing::info!(
                "Shot smoke validation sequence incomplete: {}",
                sequence.to_value()
            );
        }
    }

    /// `mark_post_update_shot_completed`: `Ok(true)` when this is the first
    /// complete shot on this image.
    pub fn mark_post_update_shot_completed(&self) -> Result<bool, PyError> {
        let current = {
            let _guard = self.lock.lock();
            let mut state = self.read_state()?;
            let current = self.current_version();
            if state.get("post_update_shot_completed") == Some(&json!(true))
                && state.get("post_update_shot_completed_version") == Some(&json!(current))
            {
                return Ok(false);
            }
            state.insert("image_version".into(), json!(current));
            state.insert("post_update_shot_completed".into(), json!(true));
            state.insert("post_update_shot_completed_at".into(), json!(Self::now()));
            state.insert("post_update_shot_completed_version".into(), json!(current));
            self.write_state(&state)?;
            current
        };
        tracing::info!("Post-update shot smoke validation completed for {current}");
        self.request_publish();
        Ok(true)
    }

    /// `request_publish`: meticulous-machine's reporter, or its systemd unit.
    fn request_publish(&self) {
        if self.publish_command.exists() {
            let command = self.publish_command.to_string_lossy().into_owned();
            if let Err(error) = self.host.spawn(&[&command, "--shot"]) {
                tracing::warn!("Failed to request Hawkbit shot smoke validation publish: {error}");
            }
            return;
        }
        if self.host.which("systemctl").is_none() {
            return;
        }
        if let Err(error) = self.host.spawn(&["systemctl", "start", PUBLISH_SERVICE]) {
            tracing::warn!("Failed to request Hawkbit smoke validation publish: {error}");
        }
    }
}

fn python_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::fake::FakeRunner;

    struct Fixture {
        _dir: tempfile::TempDir,
        version_file: PathBuf,
        state_file: PathBuf,
        runner: Arc<FakeRunner>,
        manager: SmokeValidationManager,
    }

    /// `configure_state` in tests/test_smoke_validation.py.
    fn configure_state() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("smoke-validation.json");
        let version_file = dir.path().join("image-build-version");
        std::fs::write(&version_file, "2026M1234-nightly\n").expect("version");
        let host = Arc::new(Host::default());
        let runner = Arc::new(FakeRunner::default());
        host.replace(runner.clone());
        let manager = SmokeValidationManager::new(
            state_file.clone(),
            version_file.clone(),
            dir.path().join("missing-publisher"),
            host,
        );
        Fixture {
            _dir: dir,
            version_file,
            state_file,
            runner,
            manager,
        }
    }

    /// tests/test_smoke_validation.py::test_post_update_shot_state_resets_when_image_version_changes
    #[test]
    fn post_update_shot_state_resets_when_image_version_changes() {
        let f = configure_state();
        assert_eq!(f.manager.mark_post_update_shot_completed(), Ok(true));
        let state = f.manager.get_state().expect("state");
        assert_eq!(state["post_update_shot_completed"], json!(true));
        assert_eq!(
            state["post_update_shot_completed_version"],
            json!("2026M1234-nightly")
        );
        assert_eq!(f.manager.mark_post_update_shot_completed(), Ok(false));

        std::fs::write(&f.version_file, "2026M1235-nightly\n").expect("version");
        let state = f.manager.get_state().expect("state");
        assert_eq!(state["image_version"], json!("2026M1235-nightly"));
        assert_eq!(state["post_update_shot_completed"], json!(false));
        assert_eq!(state["post_update_shot_completed_at"], Value::Null);
        assert_eq!(state["post_update_shot_completed_version"], Value::Null);
        // No reporter and no systemctl: nothing was started.
        assert!(f.runner.spawned.lock().is_empty());
    }

    fn run_sequence(f: &Fixture, steps: &[(&str, bool)]) {
        for (machine_status, extracting) in steps {
            f.manager.observe_status(machine_status, *extracting);
        }
        f.manager.observe_esp_log(FINAL_COFFEE_LOG);
    }

    fn completed(f: &Fixture) -> Value {
        f.manager.get_state().expect("state")["post_update_shot_completed"].clone()
    }

    /// tests/test_smoke_validation.py::test_final_coffee_log_marks_completed_after_full_sequence
    #[test]
    fn final_coffee_log_marks_completed_after_full_sequence() {
        let f = configure_state();
        f.runner.on_path.lock().push("systemctl".into());
        run_sequence(
            &f,
            &[
                ("heating", false),
                ("Preinfusion", true),
                ("retracting", true),
                ("purge", false),
            ],
        );
        assert_eq!(completed(&f), json!(true));
        assert_eq!(
            *f.runner.spawned.lock(),
            vec![vec![
                "systemctl".to_string(),
                "start".to_string(),
                PUBLISH_SERVICE.to_string()
            ]]
        );
    }

    /// tests/test_smoke_validation.py::test_final_coffee_log_allows_pre_extraction_retracting_and_purge
    #[test]
    fn final_coffee_log_allows_pre_extraction_retracting_and_purge() {
        let f = configure_state();
        run_sequence(
            &f,
            &[
                ("heating", false),
                ("retracting", true),
                ("purge", false),
                ("Preinfusion", true),
                ("retracting", true),
                ("purge", false),
            ],
        );
        assert_eq!(completed(&f), json!(true));
    }

    /// tests/test_smoke_validation.py::test_final_coffee_log_does_not_mark_completed_without_purge
    #[test]
    fn final_coffee_log_does_not_mark_completed_without_purge() {
        let f = configure_state();
        run_sequence(
            &f,
            &[
                ("heating", false),
                ("Preinfusion", true),
                ("retracting", true),
            ],
        );
        assert_eq!(completed(&f), json!(false));
    }

    /// tests/test_smoke_validation.py::test_final_coffee_log_requires_ordered_retracting_and_purge
    #[test]
    fn final_coffee_log_requires_ordered_retracting_and_purge() {
        let f = configure_state();
        run_sequence(
            &f,
            &[
                ("heating", false),
                ("purge", false),
                ("retracting", true),
                ("Preinfusion", true),
            ],
        );
        assert_eq!(completed(&f), json!(false));
    }

    /// The state file is the Python's: sorted keys, two-space indent.
    #[test]
    fn state_file_format() {
        let f = configure_state();
        // Nothing is written while the defaults match the image.
        f.manager.get_state().expect("state");
        assert!(!f.state_file.exists());
        // A state from another image is reset and written back, keeping
        // keys this version does not know.
        std::fs::write(&f.state_file, r#"{"image_version": "old", "extra": 1}"#).expect("state");
        f.manager.get_state().expect("state");
        assert_eq!(
            std::fs::read_to_string(&f.state_file).expect("state"),
            concat!(
                "{\n",
                "  \"extra\": 1,\n",
                "  \"image_version\": \"2026M1234-nightly\",\n",
                "  \"post_update_shot_completed\": false,\n",
                "  \"post_update_shot_completed_at\": null,\n",
                "  \"post_update_shot_completed_version\": null\n",
                "}\n"
            )
        );
    }

    /// The reporter command wins over the systemd unit.
    #[test]
    fn publish_prefers_the_reporter_command() {
        let f = configure_state();
        let reporter = f.state_file.with_file_name("reporter");
        std::fs::write(&reporter, "").expect("reporter");
        let manager = SmokeValidationManager::new(
            f.state_file.clone(),
            f.version_file.clone(),
            reporter.clone(),
            {
                let host = Arc::new(Host::default());
                host.replace(f.runner.clone());
                host
            },
        );
        assert_eq!(manager.mark_post_update_shot_completed(), Ok(true));
        assert_eq!(
            *f.runner.spawned.lock(),
            vec![vec![
                reporter.to_string_lossy().into_owned(),
                "--shot".to_string()
            ]]
        );
    }

    /// GET /smoke-validation: local only, the machine fields and the state.
    #[tokio::test]
    async fn smoke_validation_route() {
        let (dir, app) = crate::testing::app();
        std::fs::create_dir_all(dir.path().join("opt")).expect("opt");
        std::fs::write(
            dir.path().join("opt/image-build-version"),
            "2026M1446-beta\n",
        )
        .expect("version");
        let reply =
            crate::testing::call(&app, http::Method::GET, "/api/v1/smoke-validation", "").await;
        assert_eq!(reply.status_code(), 200);
        assert_eq!(
            crate::testing::json(&reply),
            json!({
                "backend_initialized": true,
                "esp32_responding": false,
                "machine_state": "idle",
                "machine_status": "idle",
                "firmware_version": null,
                "image_version": "2026M1446-beta",
                "post_update_shot_completed": false,
                "post_update_shot_completed_at": null,
                "post_update_shot_completed_version": null,
            })
        );
        let remote = crate::testing::call_with_headers(
            &app,
            http::Method::GET,
            "/api/v1/smoke-validation",
            "",
            &[("X-Real-IP", "192.168.1.50")],
        )
        .await;
        assert_eq!(remote.status_code(), 403);

        // A state that is not an object is the handler's 500.
        std::fs::write(&app.env.paths.smoke_validation_state, "[]").expect("state");
        let broken =
            crate::testing::call(&app, http::Method::GET, "/api/v1/smoke-validation", "").await;
        assert_eq!(broken.status_code(), 500);
        assert_eq!(
            crate::testing::json(&broken),
            json!({"status": "error", "error": "'list' object has no attribute 'get'"})
        );
    }
}
