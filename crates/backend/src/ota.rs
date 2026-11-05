//! `ota.py`: what the image says about itself (build date, channel, version,
//! the repositories it was built from), the version history kept in the
//! settings, and the update channel rauc-hawkbit-updater follows.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::NaiveDateTime;
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::config::{ConfigStore, keys};
use crate::env::SystemPaths;

const HAWKBIT_CHANNEL_FILE: &str = "channel";
const MAX_SYSTEM_VERSIONS: usize = 30;

/// `/opt/ROOTFS_BUILD_DATE` parsed with `"%a, %d %b %Y %H:%M:%S %z"`: the
/// wall time as written and its UTC offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildTimestamp {
    pub local: NaiveDateTime,
    pub offset_seconds: i32,
}

impl BuildTimestamp {
    /// `strftime("%Y-%m-%d %H:%M:%S")`, the machine info's `software_version`.
    pub fn software_version(&self) -> String {
        use chrono::{Datelike, Timelike};
        let t = &self.local;
        // glibc's %Y does not pad years below 1000.
        format!(
            "{}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.year(),
            t.month(),
            t.day(),
            t.hour(),
            t.minute(),
            t.second()
        )
    }

    /// What sentry-sdk writes for the `build-timestamp` tag of this
    /// `datetime` (`format_timestamp`): in UTC, with microseconds and `Z`.
    pub fn sentry_tag(&self) -> String {
        use chrono::{Datelike, Timelike};
        let t = self.local - chrono::TimeDelta::seconds(i64::from(self.offset_seconds));
        format!(
            "{}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
            t.year(),
            t.month(),
            t.day(),
            t.hour(),
            t.minute(),
            t.second(),
            t.nanosecond() / 1000
        )
    }

    /// `strftime("%Y%m%d_%H%M%S")`, the version history's stamp.
    pub fn compact(&self) -> String {
        use chrono::{Datelike, Timelike};
        let t = &self.local;
        format!(
            "{}{:02}{:02}_{:02}{:02}{:02}",
            t.year(),
            t.month(),
            t.day(),
            t.hour(),
            t.minute(),
            t.second()
        )
    }
}

pub struct UpdateManager {
    config: Arc<ConfigStore>,
    build_date_file: PathBuf,
    repo_info_file: PathBuf,
    build_channel_file: PathBuf,
    build_version_file: PathBuf,
    hawkbit_dir: PathBuf,
    channel: Mutex<Option<String>>,
    version: Mutex<Option<String>>,
    repo_info: Mutex<Option<Value>>,
    is_changed: AtomicBool,
}

impl UpdateManager {
    pub fn new(config: Arc<ConfigStore>, paths: &SystemPaths) -> UpdateManager {
        UpdateManager {
            config,
            build_date_file: paths.image_build_date.clone(),
            repo_info_file: paths.image_summary.clone(),
            build_channel_file: paths.image_build_channel.clone(),
            build_version_file: paths.image_build_version.clone(),
            hawkbit_dir: paths.hawkbit_dir.clone(),
            channel: Mutex::new(None),
            version: Mutex::new(None),
            repo_info: Mutex::new(None),
            is_changed: AtomicBool::new(false),
        }
    }

    /// `UpdateManager.is_changed`: this boot is the first of a new image.
    pub fn is_changed(&self) -> bool {
        self.is_changed.load(Ordering::Relaxed)
    }

    /// `UpdateManager.init`: follow the image's channel after an update,
    /// tell rauc-hawkbit-updater, and remember the image in the history.
    pub fn init(&self) {
        let Some(build_channel) = self.image_channel() else {
            tracing::error!("Could not get build channel");
            return;
        };

        let configured = self.config.get(keys::USER, keys::UPDATE_CHANNEL);
        let mut update_channel = configured.clone();
        if configured == json!("") {
            update_channel = if build_channel == "stable" || build_channel == "factory" {
                json!(build_channel)
            } else {
                json!("stable")
            };
        }

        let Some(build_time) = self.build_timestamp() else {
            tracing::error!("Could not get build timestamp");
            if self.config.get(keys::USER, keys::UPDATE_CHANNEL) != update_channel {
                self.config
                    .set(keys::USER, keys::UPDATE_CHANNEL, update_channel);
                self.config.save();
            }
            self.set_channel(&python_str(
                &self.config.get(keys::USER, keys::UPDATE_CHANNEL),
            ));
            return;
        };

        let this_version = format!("{build_channel}-{}", build_time.compact());
        let history = self.config.get(keys::SYSTEM, keys::LAST_SYSTEM_VERSIONS);
        // PARITY: the last entry is compared as stored, so an entry of
        // another shape (a dict) always counts as a different image.
        let (is_changed, last_known) = match history.as_array().and_then(|h| h.last()) {
            Some(last) => (*last != json!(this_version), python_str(last)),
            None => (true, "NONE".to_string()),
        };
        self.is_changed.store(is_changed, Ordering::Relaxed);
        tracing::info!("Last known version image: {last_known}");
        tracing::info!("This image version: {this_version}");

        if is_changed {
            tracing::info!("System was updated to {this_version} from {last_known}");
            update_channel = json!(build_channel);
        }

        let previous = self.config.get(keys::USER, keys::UPDATE_CHANNEL);
        if previous != update_channel {
            self.config
                .set(keys::USER, keys::UPDATE_CHANNEL, update_channel.clone());
            self.config.save();
            tracing::warn!(
                "Set update channel to {} based on image (previous: {})",
                python_str(&update_channel),
                python_str(&previous)
            );
        }

        self.set_channel(&python_str(
            &self.config.get(keys::USER, keys::UPDATE_CHANNEL),
        ));

        if is_changed {
            // A history that is not a list (hand-edited) would make the
            // Python raise and stop the boot; start a new one instead.
            let mut versions = match history {
                Value::Array(versions) => versions,
                _ => Vec::new(),
            };
            versions.push(json!(this_version));
            while versions.len() > MAX_SYSTEM_VERSIONS {
                versions.remove(0);
            }
            self.config.set(
                keys::SYSTEM,
                keys::LAST_SYSTEM_VERSIONS,
                Value::Array(versions),
            );
            self.config.save();
        }
    }

    /// `UpdateManager.setChannel`: write the channel rauc-hawkbit-updater
    /// polls, when the updater is installed and the channel differs.
    pub fn set_channel(&self, channel: &str) {
        let channel = py_strip(channel);
        if !self.hawkbit_dir.exists() {
            tracing::info!(
                "{} does not exist, not changing update channel",
                self.hawkbit_dir.display()
            );
            return;
        }
        let channel_file = self.hawkbit_dir.join(HAWKBIT_CHANNEL_FILE);
        let current = match std::fs::read_to_string(&channel_file) {
            Ok(text) => Some(py_strip(&text).to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                tracing::warn!("Could not read {}: {error}", channel_file.display());
                None
            }
        };
        if current.as_deref() != Some(channel) {
            match std::fs::write(&channel_file, format!("{channel}\n")) {
                Ok(()) => tracing::info!(
                    "Changed update channel from {} to {channel}",
                    current.as_deref().unwrap_or("None")
                ),
                Err(error) => tracing::error!("Failed to change update channel: {error}"),
            }
        }
    }

    /// `UpdateManager.getBuildTimestamp`. Read on every call, like the Python.
    pub fn build_timestamp(&self) -> Option<BuildTimestamp> {
        let text = match std::fs::read_to_string(&self.build_date_file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!("{} file not found", self.build_date_file.display());
                return None;
            }
            Err(error) => {
                tracing::error!("Error reading {}: {error}", self.build_date_file.display());
                return None;
            }
        };
        let parsed = parse_build_date(py_strip(&text));
        if parsed.is_none() {
            tracing::error!("Invalid date format in BUILD_DATE file");
        }
        parsed
    }

    /// The machine info's `software_version`.
    pub fn software_version(&self) -> Option<String> {
        self.build_timestamp().map(|t| t.software_version())
    }

    /// `UpdateManager.getImageChannel`, cached once read.
    pub fn image_channel(&self) -> Option<String> {
        read_cached(
            &self.channel,
            &self.build_channel_file,
            "Read image build channel",
            "Error reading image build channel",
        )
    }

    /// `UpdateManager.getImageVersion`, cached once read.
    pub fn image_version(&self) -> Option<String> {
        read_cached(
            &self.version,
            &self.build_version_file,
            "Read image build Version",
            "Error reading image build Version",
        )
    }

    /// `UpdateManager.getRepositoryInfo`: `/opt/summary.txt` parsed, cached
    /// once read.
    pub fn repository_info(&self) -> Option<Value> {
        let mut cached = self.repo_info.lock();
        if let Some(info) = cached.as_ref() {
            return Some(info.clone());
        }
        match std::fs::read(&self.repo_info_file) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => {
                    let info = parse_summary_file(py_strip(&universal_newlines(&text)));
                    tracing::info!("Read repository info: {info}");
                    *cached = Some(info.clone());
                    Some(info)
                }
                Err(_) => {
                    tracing::error!("Error reading repository info: UnicodeDecodeError");
                    None
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!("{} file not found", self.repo_info_file.display());
                None
            }
            Err(error) => {
                tracing::error!(
                    "Error reading repository info: {}",
                    python_os_error_name(&error)
                );
                None
            }
        }
    }

    /// The machine info's `repository_info`: branch and last commit per repo.
    pub fn repository_summary(&self) -> Value {
        let mut summary = Map::new();
        if let Some(Value::Object(repos)) = self.repository_info() {
            for (repo, info) in repos {
                summary.insert(
                    repo,
                    json!({
                        "branch": info.get("branch").cloned().unwrap_or(Value::Null),
                        "commit": info.get("last_commit").cloned().unwrap_or(Value::Null),
                    }),
                );
            }
        }
        Value::Object(summary)
    }
}

fn read_cached(
    cache: &Mutex<Option<String>>,
    path: &std::path::Path,
    read_message: &str,
    error_message: &str,
) -> Option<String> {
    let mut cached = cache.lock();
    if cached.is_some() {
        return cached.clone();
    }
    match std::fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => {
                let value = py_strip(&text).to_string();
                tracing::info!("{read_message}: {value}");
                *cached = Some(value);
            }
            Err(error) => tracing::error!("{error_message}: {error}"),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!("{} file not found", path.display());
        }
        Err(error) => tracing::error!("{error_message}: {error}"),
    }
    cached.clone()
}

/// The name of the `OSError` subclass Python raises for an IO error.
fn python_os_error_name(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        std::io::ErrorKind::IsADirectory => "IsADirectoryError",
        std::io::ErrorKind::NotADirectory => "NotADirectoryError",
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        _ => "OSError",
    }
}

/// `str(value)` for the settings values these modules print.
pub fn python_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => i.to_string(),
            (_, Some(u), _) => u.to_string(),
            (_, _, Some(f)) => pyfmt::repr_f64(f),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// `str.isspace()`: Unicode whitespace plus the ASCII separators U+001C..U+001F.
pub fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()`.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// Reading a file in text mode: `\r\n` and `\r` become `\n`.
pub fn universal_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `str.splitlines()`.
pub fn py_splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let boundary = matches!(
            c,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !boundary {
            continue;
        }
        lines.push(&text[start..i]);
        let mut end = i + c.len_utf8();
        if c == '\r'
            && let Some(&(j, '\n')) = chars.peek()
        {
            chars.next();
            end = j + 1;
        }
        start = end;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// `UpdateManager.parse_summary_file`: meticulous-machine's `summary.txt`,
/// one `## component ##` block per repository.
pub fn parse_summary_file(summary: &str) -> Value {
    static HEADER: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let header = HEADER.get_or_init(|| Regex::new(r"^##\s+(.*?)\s+##").expect("valid regex"));

    let mut data = Map::new();
    let mut current: Option<String> = None;
    let mut modified_files_section = false;
    let lines = py_splitlines(summary);
    for (i, raw) in lines.iter().enumerate() {
        let line = py_strip(raw);
        if let Some(caps) = header.captures(line) {
            let key = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            data.insert(
                key.clone(),
                json!({
                    "repo": null,
                    "url": null,
                    "branch": null,
                    "commit": null,
                    "last_commit": null,
                    "modified_files": [],
                }),
            );
            current = Some(key);
            modified_files_section = false;
            continue;
        }
        let Some(entry) = current.as_ref().and_then(|k| data.get_mut(k)) else {
            continue;
        };
        let field = |prefix: &str| py_strip(&line.replace(prefix, "")).to_string();
        if line.starts_with("Repository:") {
            entry["repo"] = json!(field("Repository:"));
            modified_files_section = false;
        } else if line.starts_with("URL:") {
            entry["url"] = json!(field("URL:"));
            modified_files_section = false;
        } else if line.starts_with("Branch:") {
            entry["branch"] = json!(field("Branch:"));
            modified_files_section = false;
        } else if line.starts_with("Commit:") {
            entry["commit"] = json!(field("Commit:"));
            modified_files_section = false;
        } else if line.starts_with("Last commit details:") {
            if let Some(next) = lines.get(i + 1) {
                entry["last_commit"] = json!(py_strip(next));
            }
            modified_files_section = false;
        } else if line.starts_with("Modified files:") {
            modified_files_section = true;
        } else if modified_files_section
            && !line.is_empty()
            && let Some(files) = entry["modified_files"].as_array_mut()
        {
            files.push(json!(line));
        }
    }
    Value::Object(data)
}

/// `datetime.strptime(s, "%a, %d %b %Y %H:%M:%S %z")` with CPython's
/// `_strptime` patterns (case-insensitive names, one or more spaces where the
/// format has one), then `datetime`'s range checks.
pub fn parse_build_date(s: &str) -> Option<BuildTimestamp> {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        Regex::new(concat!(
            r"^(?i:mon|tue|wed|thu|fri|sat|sun),\s+",
            r"(3[01]|[12][0-9]|0[1-9]|[1-9]| [1-9])\s+",
            r"(?i:(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec))\s+",
            r"([0-9]{4})\s+",
            r"(2[0-3]|[01][0-9]|[0-9]):([0-5][0-9]|[0-9]):(6[01]|[0-5][0-9]|[0-9])\s+",
            r"(Z|[+-][0-9]{2}:?[0-5][0-9](?::?[0-5][0-9](?:\.[0-9]{1,6})?)?)$",
        ))
        .expect("valid regex")
    });
    let caps = pattern.captures(s)?;
    let day: u32 = caps[1].trim().parse().ok()?;
    let month = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|m| caps[2].eq_ignore_ascii_case(m))? as u32
        + 1;
    let year: i32 = caps[3].parse().ok()?;
    let hour: u32 = caps[4].parse().ok()?;
    let minute: u32 = caps[5].parse().ok()?;
    let second: u32 = caps[6].parse().ok()?;
    if year < 1 || second > 59 {
        return None;
    }
    let offset_seconds = parse_utc_offset(&caps[7])?;
    let local =
        chrono::NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hour, minute, second)?;
    Some(BuildTimestamp {
        local,
        offset_seconds,
    })
}

/// `_strptime`'s `%z`: `Z`, `+HHMM`, `+HH:MM`, `+HH:MM:SS[.ffffff]`;
/// `datetime.timezone` wants it strictly inside one day.
fn parse_utc_offset(z: &str) -> Option<i32> {
    if z == "Z" {
        return Some(0);
    }
    let mut digits = z.to_string();
    if digits.as_bytes().get(3) == Some(&b':') {
        digits.remove(3);
        if digits.len() > 5 {
            if digits.as_bytes().get(5) != Some(&b':') {
                return None;
            }
            digits.remove(5);
        }
    }
    let field = |range: std::ops::Range<usize>| -> i32 {
        digits.get(range).and_then(|s| s.parse().ok()).unwrap_or(0)
    };
    let seconds = field(1..3) * 3600 + field(3..5) * 60 + field(5..7);
    if seconds >= 86_400 {
        return None;
    }
    Some(if digits.starts_with('-') {
        -seconds
    } else {
        seconds
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn manager(root: &std::path::Path, config_text: &str) -> (Arc<ConfigStore>, UpdateManager) {
        let config_path = root.join("config.yml");
        std::fs::write(&config_path, config_text).expect("config");
        let config = Arc::new(ConfigStore::open(&config_path, crate::config::defaults()));
        let paths = crate::env::SystemPaths::under(root);
        let manager = UpdateManager::new(config.clone(), &paths);
        (config, manager)
    }

    fn count_saves(config: &ConfigStore) -> Arc<AtomicUsize> {
        let saves = Arc::new(AtomicUsize::new(0));
        let counter = saves.clone();
        config.on_save(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        saves
    }

    fn write(path: &std::path::Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, text).expect("write");
    }

    /// tests/test_ota.py::test_init_updates_channel_to_new_image_channel
    #[test]
    fn init_updates_channel_to_new_image_channel() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (config, ota) = manager(
            dir.path(),
            "user:\n  update_channel: stable\nsystem:\n  last_system_versions:\n  - stable-20260528_120000\n",
        );
        write(&dir.path().join("opt/image-build-channel"), "iw612\n");
        write(
            &dir.path().join("opt/ROOTFS_BUILD_DATE"),
            "Fri, 29 May 2026 12:00:00 +0000\n",
        );
        std::fs::create_dir_all(dir.path().join("etc/hawkbit")).expect("hawkbit");
        let saves = count_saves(&config);

        ota.init();

        assert_eq!(config.get(keys::USER, keys::UPDATE_CHANNEL), json!("iw612"));
        let history = config.get(keys::SYSTEM, keys::LAST_SYSTEM_VERSIONS);
        assert_eq!(
            history.as_array().and_then(|h| h.last()),
            Some(&json!("iw612-20260529_120000"))
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("etc/hawkbit/channel")).expect("channel"),
            "iw612\n"
        );
        assert!(ota.is_changed());
        assert_eq!(saves.load(Ordering::Relaxed), 2);
    }

    /// tests/test_ota.py::test_init_keeps_existing_channel_when_image_is_unchanged
    #[test]
    fn init_keeps_existing_channel_when_image_is_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (config, ota) = manager(
            dir.path(),
            "user:\n  update_channel: stable\nsystem:\n  last_system_versions:\n  - stable-20260529_120000\n",
        );
        write(&dir.path().join("opt/image-build-channel"), "stable\n");
        write(
            &dir.path().join("opt/ROOTFS_BUILD_DATE"),
            "Fri, 29 May 2026 12:00:00 +0000\n",
        );
        std::fs::create_dir_all(dir.path().join("etc/hawkbit")).expect("hawkbit");
        let saves = count_saves(&config);

        ota.init();

        assert_eq!(
            config.get(keys::USER, keys::UPDATE_CHANNEL),
            json!("stable")
        );
        assert_eq!(
            config.get(keys::SYSTEM, keys::LAST_SYSTEM_VERSIONS),
            json!(["stable-20260529_120000"])
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("etc/hawkbit/channel")).expect("channel"),
            "stable\n"
        );
        assert!(!ota.is_changed());
        assert_eq!(saves.load(Ordering::Relaxed), 0);
    }

    /// Boot outside an image (the boot harness): both metadata errors, the
    /// empty channel becomes "stable", no hawkbit dir so nothing is written.
    #[test]
    fn init_without_image_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (config, ota) = manager(dir.path(), "user:\n  update_channel: ''\n");
        ota.init();
        assert_eq!(config.get(keys::USER, keys::UPDATE_CHANNEL), json!(""));

        write(&dir.path().join("opt/image-build-channel"), "nightly\n");
        let (config, ota) = manager(dir.path(), "user:\n  update_channel: ''\n");
        ota.init();
        assert_eq!(
            config.get(keys::USER, keys::UPDATE_CHANNEL),
            json!("stable")
        );
        assert!(!dir.path().join("etc/hawkbit/channel").exists());
    }

    /// The version history keeps the last 30 images.
    #[test]
    fn version_history_is_capped_at_thirty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut text = String::from("system:\n  last_system_versions:\n");
        for i in 0..30 {
            text.push_str(&format!("  - old-{i}\n"));
        }
        let (config, ota) = manager(dir.path(), &text);
        write(&dir.path().join("opt/image-build-channel"), "beta\n");
        write(
            &dir.path().join("opt/ROOTFS_BUILD_DATE"),
            "Tue, 06 Oct 2026 12:34:56 +0000",
        );
        ota.init();
        let history = config.get(keys::SYSTEM, keys::LAST_SYSTEM_VERSIONS);
        let history = history.as_array().expect("list");
        assert_eq!(history.len(), 30);
        assert_eq!(history[0], json!("old-1"));
        assert_eq!(history[29], json!("beta-20261006_123456"));
    }

    /// tests/test_api_settings.py::test_post_update_channel_writes_the_hawkbit_channel_file
    /// and ::test_post_update_channel_without_hawkbit_dir_only_saves_the_setting
    #[test]
    fn set_channel_writes_the_hawkbit_channel_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (_config, ota) = manager(dir.path(), "{}\n");
        ota.set_channel("beta");
        assert!(!dir.path().join("etc/hawkbit").exists());

        std::fs::create_dir_all(dir.path().join("etc/hawkbit")).expect("hawkbit");
        ota.set_channel(" beta ");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("etc/hawkbit/channel")).expect("channel"),
            "beta\n"
        );
    }

    /// tests/test_api_machine.py::TestMachineInfoRoute: the image metadata the
    /// machine info reports.
    #[test]
    fn image_metadata_for_machine_info() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (_config, ota) = manager(dir.path(), "{}\n");
        assert_eq!(ota.software_version(), None);
        assert_eq!(ota.image_channel(), None);
        assert_eq!(ota.image_version(), None);
        assert_eq!(ota.repository_summary(), json!({}));

        write(
            &dir.path().join("opt/ROOTFS_BUILD_DATE"),
            "Tue, 06 Oct 2026 12:34:56 +0000\n",
        );
        write(&dir.path().join("opt/image-build-channel"), "beta\n");
        write(
            &dir.path().join("opt/image-build-version"),
            "2026M1446-beta\n",
        );
        write(
            &dir.path().join("opt/summary.txt"),
            concat!(
                "## meticulous-backend ##\n",
                "Repository: meticulous-backend\n",
                "URL: https://github.com/MeticulousHome/meticulous-backend\n",
                "Branch: beta\n",
                "Commit: abc1234\n",
                "Last commit details:\n",
                "abc1234 Fix wifi repair (2026-10-01)\n",
                "Modified files:\n",
                "\n",
                "## meticulous-dial ##\n",
                "Repository: meticulous-dial\n",
                "Branch: main\n",
                "Commit: def5678\n",
            ),
        );
        assert_eq!(
            ota.software_version().as_deref(),
            Some("2026-10-06 12:34:56")
        );
        assert_eq!(ota.image_channel().as_deref(), Some("beta"));
        assert_eq!(ota.image_version().as_deref(), Some("2026M1446-beta"));
        assert_eq!(
            ota.repository_summary(),
            json!({
                "meticulous-backend": {
                    "branch": "beta",
                    "commit": "abc1234 Fix wifi repair (2026-10-01)",
                },
                "meticulous-dial": {"branch": "main", "commit": null},
            })
        );
    }

    /// tests/test_api_machine.py::test_unparseable_build_date_reports_null_software_version
    #[test]
    fn unparseable_build_date_reports_no_software_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (_config, ota) = manager(dir.path(), "{}\n");
        write(
            &dir.path().join("opt/ROOTFS_BUILD_DATE"),
            "2026-10-06 12:34:56\n",
        );
        assert_eq!(ota.software_version(), None);
    }

    #[test]
    fn build_date_like_strptime() {
        let parsed = parse_build_date("tue,  6 OCT 2026 01:02:03 -05:30").expect("parsed");
        assert_eq!(parsed.software_version(), "2026-10-06 01:02:03");
        assert_eq!(parsed.offset_seconds, -(5 * 3600 + 30 * 60));
        assert_eq!(
            parse_build_date("Tue, 06 Oct 2026 12:34:56 Z").map(|t| t.offset_seconds),
            Some(0)
        );
        assert!(parse_build_date("Tue, 06 Oct 2026 12:34:56 z").is_none());
        assert!(parse_build_date("Tue, 31 Feb 2026 12:34:56 +0000").is_none());
        assert!(parse_build_date("Tue, 06 Oct 2026 12:34:60 +0000").is_none());
        assert!(parse_build_date("Tue, 06 Oct 2026 12:34:56 +2400").is_none());
        assert!(parse_build_date("Tue, 06 Oct 2026 12:34:56 +0000 x").is_none());
    }

    /// sentry-sdk's `format_timestamp` of the parsed build date, the
    /// `build-timestamp` tag a Python machine sends: stable's `Fri, 16 Oct
    /// 2026 23:17:41 +0000` arrives as `2026-10-16T23:17:41.000000Z`.
    #[test]
    fn build_timestamp_as_sentry_writes_it() {
        let tag = |s| parse_build_date(s).expect("parsed").sentry_tag();
        assert_eq!(
            tag("Fri, 16 Oct 2026 23:17:41 +0000"),
            "2026-10-16T23:17:41.000000Z"
        );
        assert_eq!(
            tag("tue,  6 OCT 2026 01:02:03 -05:30"),
            "2026-10-06T06:32:03.000000Z"
        );
        assert_eq!(
            tag("Thu, 31 Dec 2026 23:30:00 -01:00"),
            "2027-01-01T00:30:00.000000Z"
        );
    }

    #[test]
    fn summary_parsing_edge_cases() {
        let parsed = parse_summary_file(
            "noise\n## a ##\nModified files:\n x.py \n\ny.py\nURL: u URL:\n## a ##\nBranch: b",
        );
        assert_eq!(
            parsed,
            json!({"a": {"repo": null, "url": null, "branch": "b", "commit": null,
                         "last_commit": null, "modified_files": []}})
        );
        let parsed = parse_summary_file("## a ##\nModified files:\n x.py \n\ny.py\nURL: u URL:");
        assert_eq!(parsed["a"]["modified_files"], json!(["x.py", "y.py"]));
        assert_eq!(parsed["a"]["url"], json!("u"));
        assert_eq!(py_splitlines("a\r\nb\rc\n"), vec!["a", "b", "c"]);
    }
}
