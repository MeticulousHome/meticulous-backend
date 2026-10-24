//! The privacy redaction every log line and Sentry event goes through
//! (`log.py`, `log_redaction_filter.py`, `sentry_privacy.py`).
//!
//! The rules live in the `redactor` crate; this module supplies the
//! per-device key (`log_redactor.load_key`), keeps the values to sweep in
//! step with the settings, and plugs both into logging and Sentry.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use redactor::{KEY_SIZE, KeySource, LogRecord, LogRedactor, RedactionFailure};

use crate::app::App;

/// `load_cached_key(path)`: the key file, created with fresh random bytes
/// on first use, cached once read. A failure is retried on the next record.
pub struct FileKeySource {
    path: PathBuf,
    key: Option<Vec<u8>>,
}

impl FileKeySource {
    pub fn new(path: PathBuf) -> FileKeySource {
        FileKeySource { path, key: None }
    }

    /// `load_key(path)`.
    fn load(&self) -> Result<Vec<u8>, RedactionFailure> {
        match std::fs::read(&self.path) {
            Ok(bytes) => self.validate(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.create(),
            Err(error) => Err(failure_of(&error)),
        }
    }

    fn validate(&self, bytes: Vec<u8>) -> Result<Vec<u8>, RedactionFailure> {
        redactor::check_key(&self.path.display().to_string(), &bytes)?;
        let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        Ok(bytes)
    }

    fn create(&self) -> Result<Vec<u8>, RedactionFailure> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| failure_of(&e))?;
        }
        let key: Vec<u8> = (0..KEY_SIZE).map(|_| rand::random::<u8>()).collect();
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.path)
        {
            Ok(mut file) => {
                file.write_all(&key).map_err(|e| failure_of(&e))?;
                file.sync_all().map_err(|e| failure_of(&e))?;
                Ok(key)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another process created it first: use theirs.
                std::thread::sleep(Duration::from_millis(100));
                let bytes = std::fs::read(&self.path).map_err(|e| failure_of(&e))?;
                self.validate(bytes)
            }
            Err(error) => Err(failure_of(&error)),
        }
    }
}

impl KeySource for FileKeySource {
    fn key(&mut self) -> Result<&[u8], RedactionFailure> {
        if self.key.is_none() {
            self.key = Some(self.load()?);
        }
        Ok(self.key.as_deref().unwrap_or_default())
    }
}

/// The Python exception an `OSError` would be.
fn failure_of(error: &std::io::Error) -> RedactionFailure {
    RedactionFailure::new(match error.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        std::io::ErrorKind::AlreadyExists => "FileExistsError",
        _ => "OSError",
    })
}

/// The one filter all logging goes through.
pub struct Redaction {
    filter: Mutex<LogRedactor>,
}

impl Redaction {
    pub fn new(key_path: PathBuf) -> Arc<Redaction> {
        Arc::new(Redaction {
            filter: Mutex::new(LogRedactor::new(FileKeySource::new(key_path))),
        })
    }

    /// One log message as the filter leaves it.
    pub fn redact_message(&self, logger: &str, message: &str) -> String {
        let mut record = LogRecord::new(logger, message);
        // A record logged while the filter itself logs would deadlock; the
        // filter does not log, so a busy lock means another thread.
        self.filter.lock().filter(&mut record, Instant::now());
        record.msg
    }

    /// Text that leaves the device outside a log record (Sentry messages).
    pub fn redact_text(&self, text: &str) -> String {
        let mut record = LogRecord::new("sentry", text);
        self.filter.lock().filter(&mut record, Instant::now());
        record.msg
    }

    /// The config's network names and secrets, swept from every message.
    pub fn refresh_values(&self, config: &serde_json::Value) {
        let mut filter = self.filter.lock();
        match redactor::sweep_values_from_config(config) {
            Ok(values) => filter.set_values(values.ssids, values.credentials),
            Err(_) => filter.set_values(Vec::new(), Vec::new()),
        }
        filter.invalidate();
    }
}

/// Plug redaction into logging, the settings and Sentry.
pub fn install(app: &Arc<App>) {
    app.redaction.refresh_values(&app.config.snapshot());
    let redaction = app.redaction.clone();
    app.log
        .set_redactor(move |logger, message| redaction.redact_message(logger, message));
    let redaction = app.redaction.clone();
    let config = Arc::downgrade(&app.config);
    app.config.on_save(move || {
        if let Some(config) = config.upgrade() {
            redaction.refresh_values(&config.snapshot());
        }
    });
    let redaction = app.redaction.clone();
    app.reporting.set_scrubber(move |event| {
        let mut event = event;
        // The logging integration hands Sentry the message as logged, so it
        // went through the filter in Python; here it is redacted on its way.
        if let Some(message) = event
            .get("message")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
        {
            event["message"] = serde_json::Value::String(redaction.redact_text(&message));
        }
        if let Some(logentry) = event.get_mut("logentry").and_then(|l| l.get_mut("message")) {
            if let Some(text) = logentry.as_str().map(str::to_string) {
                *logentry = serde_json::Value::String(redaction.redact_text(&text));
            }
        }
        redactor::sanitize_sentry_event(&event)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// test_log_redaction_filter.py: the key is created once, 0600, 32 bytes,
    /// and reused.
    #[test]
    fn key_file_is_created_and_reused() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("root/.redaction_key");
        let mut source = FileKeySource::new(path.clone());
        let first = source.key().expect("key").to_vec();
        assert_eq!(first.len(), KEY_SIZE);
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let mut again = FileKeySource::new(path);
        assert_eq!(again.key().expect("key"), first.as_slice());
    }

    #[test]
    fn wrong_size_key_is_a_value_error() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("key");
        std::fs::write(&path, b"short").expect("write");
        let mut source = FileKeySource::new(path);
        assert_eq!(source.key().unwrap_err().exception_type, "ValueError");
    }

    #[test]
    fn messages_are_redacted() {
        let dir = tempfile::tempdir().expect("tmp");
        let redaction = Redaction::new(dir.path().join("key"));
        redaction.refresh_values(&serde_json::json!({
            "wifi": {"KnownWifis": {"HomeNet": {"password": "hunter2hunter2"}}, "APPassword": "123456789012"},
            "system": {"root_password": null, "auth_key": "AAAABBBBCCCCDDDEEEFFFFGGGG"},
        }));
        let out = redaction.redact_message(
            "wifi",
            "Found known WIFI HomeNet with hunter2hunter2 from 192.168.1.42",
        );
        assert!(!out.contains("HomeNet"), "{out}");
        assert!(!out.contains("hunter2hunter2"), "{out}");
        // REDACTION_SPEC.md keeps IPv4 addresses: they are the machine's own
        // lease and redacting them would break log correlation.
        assert!(out.contains("192.168.1.42"), "{out}");
    }
}
