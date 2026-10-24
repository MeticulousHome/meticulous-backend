//! `config.py`: the YAML settings file every service reads.
//!
//! The tree stays a JSON value rather than a struct: the settings API, the
//! manufacturing API and `met-config` address keys by name and check a new
//! value's type against the stored one (`type(value) is type(current)`), and
//! keys this version does not know must survive a save. Typed accessors sit
//! on top for the code that reads known keys.

mod yaml;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::{Mutex, RwLock};
use serde_json::{Map, Value, json};

pub use yaml::to_yaml;

pub mod keys {
    pub const LOGGING: &str = "logging";
    pub const SYSTEM: &str = "system";
    pub const USER: &str = "user";
    pub const WIFI: &str = "wifi";
    pub const PROFILES: &str = "profiles";
    pub const MANUFACTURING: &str = "manufacturing";

    // system
    pub const HTTP_AUTH_KEY: &str = "auth_key";
    pub const HTTP_ALLOWED_NETWORKS: &str = "always_allowed_networks";
    pub const NOTIFICATION_KEEPALIVE: &str = "notifications_ttl";
    pub const SOUNDS_THEME: &str = "sounds_theme";
    pub const DEVICE_IDENTIFIER: &str = "machine_name";
    pub const MACHINE_SERIAL_NUMBER: &str = "serial";
    pub const MACHINE_BATCH_NUMBER: &str = "batch_number";
    pub const MACHINE_BUILD_DATE: &str = "build_date";
    pub const MACHINE_COLOR: &str = "color";
    pub const SHOT_DATA_SHARING_ID: &str = "shot_data_sharing_id";
    pub const ROOT_PASSWORD: &str = "root_password";
    pub const LAST_SYSTEM_VERSIONS: &str = "last_system_versions";

    // user
    pub const SOUNDS_ENABLED: &str = "enable_sounds";
    pub const DISALLOW_FIRMWARE_FLASHING: &str = "disallow_firmware_flashing";
    pub const DISABLE_UI_FEATURES: &str = "disable_ui_features";
    pub const DEBUG_SHOT_DATA_RETENTION: &str = "debug_shot_data_retention_days";
    pub const PROFILE_AUTO_START: &str = "auto_start_shot";
    pub const PROFILE_AUTO_PURGE: &str = "auto_purge_after_shot";
    pub const PROFILE_TARE_BEHAVIOR: &str = "tare_behavior";
    pub const PROFILE_PARTIAL_RETRACTION: &str = "partial_retraction";
    pub const MACHINE_HEAT_ON_BOOT: &str = "heat_on_boot";
    pub const MACHINE_HEATING_TIMEOUT: &str = "heating_timeout";
    pub const UPDATE_CHANNEL: &str = "update_channel";
    pub const IDLE_SCREEN: &str = "idle_screen";
    pub const REVERSE_SCROLLING: &str = "reverse_scrolling";
    pub const ALLOW_LEGACY_JSON: &str = "allow_legacy_json";
    pub const SHOT_DATA_SHARING: &str = "shot_data_sharing";
    pub const REPORT_CONTACT_MAIL: &str = "report_contact_mail";
    pub const MACHINE_ALLOW_STAGE_SKIPPING: &str = "allow_stage_skipping";
    pub const USB_MODE: &str = "usb_mode";
    pub const TIMEZONE_SYNC: &str = "timezone_sync";
    pub const TIME_ZONE: &str = "time_zone";
    pub const SSH_ENABLED: &str = "ssh_enabled";
    pub const PROFILE_ORDER: &str = "profile_order";
    pub const HOSTNAME_OVERRIDE: &str = "hostname_override";
    pub const CLOCK_FORMAT_24_HOUR: &str = "clock_format_24_hour";

    // logging
    pub const LOGGING_SENSOR_MESSAGES: &str = "log_all_sensor_messages";

    // wifi
    pub const WIFI_MODE: &str = "mode";
    pub const WIFI_AP_NAME: &str = "APName";
    pub const WIFI_AP_PASSWORD: &str = "APPassword";
    pub const WIFI_KNOWN_WIFIS: &str = "KnownWifis";

    // profiles
    pub const PROFILE_LAST: &str = "LastProfile";

    // manufacturing
    pub const FORCE_MANUFACTURING_ENABLED: &str = "enabled";
    pub const LAST_BOOT_MODE: &str = "last_boot_mode";
    pub const SKIP_STAGE: &str = "skip_stage";
}

pub const WIFI_MODE_AP: &str = "AP";
pub const WIFI_MODE_CLIENT: &str = "CLIENT";
pub const AUTOMATIC_TIMEZONE_SYNC: &str = "automatic";
pub const TARE_BEHAVIOR_AFTER_RETRACTION: &str = "after_retraction";
pub const TARE_BEHAVIOR_BEFORE_RETRACTION: &str = "before_retraction";
pub const TARE_BEHAVIORS: [&str; 2] = [
    TARE_BEHAVIOR_AFTER_RETRACTION,
    TARE_BEHAVIOR_BEFORE_RETRACTION,
];
pub const PARTIAL_RETRACTION_LEGACY_DEFAULT: f64 = 45.0;
pub const PARTIAL_RETRACTION_DEFAULT: f64 = 45.33;
pub const PARTIAL_RETRACTION_MIN: f64 = 36.26;
pub const PARTIAL_RETRACTION_MAX: f64 = 67.99;
/// Maximum piston travel in mm.
pub const MAX_PISTON_POSITION: f64 = 75.0;

pub const USB_MODES: [&str; 3] = ["client", "host", "dual_role"];

/// `DefaultConfiguration_V1`, in the Python's key order: the settings API
/// returns the user section in this order.
pub fn defaults() -> Value {
    use keys::*;
    // Python draws the access point password once per process with
    // random.choices(string.digits, k=12).
    let ap_password: String = (0..12)
        .map(|_| char::from(b'0' + rand::random_range(0u8..10)))
        .collect();
    json!({
        "version": 1,
        LOGGING: {LOGGING_SENSOR_MESSAGES: false},
        SYSTEM: {
            HTTP_AUTH_KEY: "AAAABBBBCCCCDDDEEEFFFFGGGG",
            HTTP_ALLOWED_NETWORKS: [],
            NOTIFICATION_KEEPALIVE: 3600,
            SOUNDS_THEME: "default",
            DEVICE_IDENTIFIER: [],
            MACHINE_SERIAL_NUMBER: null,
            MACHINE_BATCH_NUMBER: null,
            MACHINE_BUILD_DATE: null,
            MACHINE_COLOR: null,
            SHOT_DATA_SHARING_ID: null,
            ROOT_PASSWORD: null,
            LAST_SYSTEM_VERSIONS: [],
        },
        USER: {
            SOUNDS_ENABLED: true,
            DISALLOW_FIRMWARE_FLASHING: false,
            DISABLE_UI_FEATURES: false,
            DEBUG_SHOT_DATA_RETENTION: 31,
            PROFILE_AUTO_START: false,
            PROFILE_AUTO_PURGE: false,
            PROFILE_TARE_BEHAVIOR: TARE_BEHAVIOR_AFTER_RETRACTION,
            PROFILE_PARTIAL_RETRACTION: PARTIAL_RETRACTION_DEFAULT,
            MACHINE_HEAT_ON_BOOT: true,
            MACHINE_HEATING_TIMEOUT: 10,
            UPDATE_CHANNEL: "",
            IDLE_SCREEN: "default",
            REVERSE_SCROLLING: {"home": false, "keyboard": false, "menus": false},
            ALLOW_LEGACY_JSON: false,
            SHOT_DATA_SHARING: null,
            REPORT_CONTACT_MAIL: null,
            MACHINE_ALLOW_STAGE_SKIPPING: false,
            USB_MODE: "host",
            TIMEZONE_SYNC: AUTOMATIC_TIMEZONE_SYNC,
            TIME_ZONE: null,
            SSH_ENABLED: true,
            PROFILE_ORDER: [],
            HOSTNAME_OVERRIDE: null,
            CLOCK_FORMAT_24_HOUR: true,
        },
        WIFI: {
            WIFI_MODE: WIFI_MODE_AP,
            WIFI_AP_NAME: "MeticulousEspresso",
            WIFI_AP_PASSWORD: ap_password,
            WIFI_KNOWN_WIFIS: {},
        },
        PROFILES: {PROFILE_LAST: null},
        MANUFACTURING: {
            FORCE_MANUFACTURING_ENABLED: false,
            LAST_BOOT_MODE: null,
            SKIP_STAGE: false,
        },
    })
}

/// `mergedeep.merge(dst, src)`: nested dictionaries merge key by key, any
/// other value in `src` replaces the one in `dst`, new keys go last.
pub fn merge(dst: &mut Value, src: &Value) {
    match (dst, src) {
        (Value::Object(d), Value::Object(s)) => {
            for (key, value) in s {
                match d.get_mut(key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge(existing, value)
                    }
                    _ => {
                        d.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (dst, src) => *dst = src.clone(),
    }
}

type SaveHook = Box<dyn Fn() + Send + Sync>;

pub struct ConfigStore {
    path: PathBuf,
    tree: RwLock<Value>,
    error: AtomicBool,
    save_hooks: Mutex<Vec<SaveHook>>,
}

#[derive(Debug, thiserror::Error)]
enum LoadError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Yaml(#[from] serde_norway::Error),
    #[error("the configuration is not a mapping")]
    NotAMapping,
}

impl ConfigStore {
    /// `MeticulousConfigDict(path, defaults)`: defaults merged with the file,
    /// the load-time migrations applied, and the result written back.
    pub fn open(path: impl Into<PathBuf>, defaults: Value) -> ConfigStore {
        let store = ConfigStore {
            path: path.into(),
            tree: RwLock::new(defaults),
            error: AtomicBool::new(false),
            save_hooks: Mutex::new(Vec::new()),
        };
        store.load();
        tracing::info!("Config initialized");
        store
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn has_error(&self) -> bool {
        self.error.load(Ordering::Relaxed)
    }

    /// Run after every save: the settings event to clients, the log
    /// redaction cache, the Sentry context.
    pub fn on_save(&self, hook: impl Fn() + Send + Sync + 'static) {
        self.save_hooks.lock().push(Box::new(hook));
    }

    fn load(&self) {
        if !self.path.exists() {
            self.save();
            tracing::info!("Created new config");
            return;
        }
        match self.read_disk() {
            Ok(disk) => {
                let mut tree = self.tree.write();
                let own_version = tree.get("version").and_then(Value::as_i64).unwrap_or(1);
                if disk
                    .get("version")
                    .and_then(Value::as_i64)
                    .is_some_and(|v| v > own_version)
                {
                    tracing::warn!("Config on disk is newer than this software");
                }
                merge(&mut tree, &disk);
                normalize(&mut tree);
                tracing::info!("Successfully loaded config from disk");
                self.error.store(false, Ordering::Relaxed);
            }
            Err(error) => {
                tracing::warn!("Failed to load config: {error}");
                let stamp = chrono::Local::now().format("%Y_%m_%d_%H_%M_%S");
                let stem = self.path.with_extension("");
                let ext = self
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("yml");
                let backup = PathBuf::from(format!("{}_broken_{stamp}.{ext}", stem.display()));
                if let Err(error) = std::fs::rename(&self.path, &backup) {
                    tracing::warn!("Could not move the broken config aside: {error}");
                }
                self.error.store(true, Ordering::Relaxed);
            }
        }
        self.save();
    }

    fn read_disk(&self) -> Result<Value, LoadError> {
        let text = std::fs::read_to_string(&self.path)?;
        let disk: Value = serde_norway::from_str(&text)?;
        if !disk.is_object() {
            return Err(LoadError::NotAMapping);
        }
        Ok(disk)
    }

    /// `MeticulousConfig.save()`: write the whole tree, then run the hooks.
    pub fn save(&self) {
        let text = to_yaml(&self.tree.read());
        if let Err(error) = write_atomically(&self.path, text.as_bytes()) {
            tracing::error!("Failed to save config: {error}");
        }
        for hook in self.save_hooks.lock().iter() {
            hook();
        }
    }

    /// A copy of the whole tree.
    pub fn snapshot(&self) -> Value {
        self.tree.read().clone()
    }

    /// A copy of one section (`MeticulousConfig[section]`).
    pub fn section(&self, section: &str) -> Value {
        self.tree
            .read()
            .get(section)
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// A copy of one value (`MeticulousConfig[section][key]`), `Null` if absent.
    pub fn get(&self, section: &str, key: &str) -> Value {
        self.tree
            .read()
            .get(section)
            .and_then(|s| s.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub fn contains(&self, section: &str, key: &str) -> bool {
        self.tree
            .read()
            .get(section)
            .and_then(|s| s.get(key))
            .is_some()
    }

    pub fn get_bool(&self, section: &str, key: &str) -> bool {
        python_truthy(&self.get(section, key))
    }

    pub fn get_f64(&self, section: &str, key: &str) -> Option<f64> {
        self.get(section, key).as_f64()
    }

    pub fn get_i64(&self, section: &str, key: &str) -> Option<i64> {
        self.get(section, key).as_i64()
    }

    pub fn get_str(&self, section: &str, key: &str) -> Option<String> {
        self.get(section, key).as_str().map(str::to_string)
    }

    /// Change the tree without saving.
    pub fn update<R>(&self, f: impl FnOnce(&mut Map<String, Value>) -> R) -> R {
        let mut tree = self.tree.write();
        if !tree.is_object() {
            *tree = Value::Object(Map::new());
        }
        let Value::Object(map) = &mut *tree else {
            unreachable!("the tree was just made an object")
        };
        f(map)
    }

    /// Set `[section][key]`, creating the section if needed, without saving.
    pub fn set(&self, section: &str, key: &str, value: Value) {
        self.update(|tree| {
            let entry = tree
                .entry(section.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if !entry.is_object() {
                *entry = Value::Object(Map::new());
            }
            if let Value::Object(section) = entry {
                section.insert(key.to_string(), value);
            }
        });
    }

    /// Replace a whole section without saving.
    pub fn set_section(&self, section: &str, value: Value) {
        self.update(|tree| {
            tree.insert(section.to_string(), value);
        });
    }

    /// `set` followed by `save`.
    pub fn set_and_save(&self, section: &str, key: &str, value: Value) {
        self.set(section, key, value);
        self.save();
    }
}

/// The load-time migrations in `MeticulousConfigDict.load()`.
fn normalize(tree: &mut Value) {
    let Some(user) = tree.get_mut(keys::USER).and_then(Value::as_object_mut) else {
        return;
    };
    // Retired remote telemetry settings.
    user.shift_remove("telemetry_service_enabled");
    user.shift_remove("allow_debug_sending");

    let stored = user
        .get(keys::PROFILE_PARTIAL_RETRACTION)
        .cloned()
        .unwrap_or(Value::Null);
    let retraction = match &stored {
        Value::Number(n) => match n.as_f64() {
            Some(v) if v.is_finite() => {
                let v = if (v - PARTIAL_RETRACTION_LEGACY_DEFAULT).abs() <= 1e-6 {
                    PARTIAL_RETRACTION_DEFAULT
                } else {
                    v
                };
                v.clamp(PARTIAL_RETRACTION_MIN, PARTIAL_RETRACTION_MAX)
            }
            _ => PARTIAL_RETRACTION_DEFAULT,
        },
        _ => PARTIAL_RETRACTION_DEFAULT,
    };
    let retraction = json!(retraction);
    if !numbers_equal(&stored, &retraction) {
        tracing::info!(
            "Normalized partial_retraction to supported value {:.2} mm",
            retraction.as_f64().unwrap_or(PARTIAL_RETRACTION_DEFAULT)
        );
    }
    user.insert(keys::PROFILE_PARTIAL_RETRACTION.to_string(), retraction);

    let tare_ok = user
        .get(keys::PROFILE_TARE_BEHAVIOR)
        .and_then(Value::as_str)
        .is_some_and(|t| TARE_BEHAVIORS.contains(&t));
    if !tare_ok {
        user.insert(
            keys::PROFILE_TARE_BEHAVIOR.to_string(),
            json!(TARE_BEHAVIOR_AFTER_RETRACTION),
        );
        tracing::warn!("Reset unsupported tare_behavior to after_retraction");
    }
}

/// Python `a != b` for the stored retraction against the normalized float:
/// `45 == 45.0` holds in Python.
fn numbers_equal(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Python truthiness of a config value.
pub fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Write `bytes` to `path` through a temporary file and a rename, so a power
/// cut leaves either the old file or the new one, never half of one.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(text: Option<&str>) -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.yml");
        if let Some(text) = text {
            std::fs::write(&path, text).expect("write");
        }
        let store = ConfigStore::open(&path, defaults());
        (dir, store)
    }

    /// tests/test_config.py: a new machine gets the defaults on disk.
    #[test]
    fn creates_the_file_with_defaults() {
        let (dir, store) = store_with(None);
        assert!(!store.has_error());
        let disk: Value = serde_norway::from_str(
            &std::fs::read_to_string(dir.path().join("config.yml")).expect("read"),
        )
        .expect("yaml");
        assert_eq!(disk["user"]["partial_retraction"], json!(45.33));
        assert_eq!(disk["wifi"]["APPassword"].as_str().map(str::len), Some(12));
    }

    /// tests/test_config_upgrades.py: the legacy 45.0 default becomes 45.33,
    /// out-of-range values clamp, junk resets, and so does tare_behavior.
    #[test]
    fn migrates_partial_retraction_and_tare_behavior() {
        for (stored, expected) in [
            ("45.0", 45.33),
            ("45", 45.33),
            ("100", 67.99),
            ("1.0", 36.26),
            ("'x'", 45.33),
            ("true", 45.33),
            ("50.5", 50.5),
        ] {
            let (_dir, store) = store_with(Some(&format!(
                "user:\n  partial_retraction: {stored}\n  tare_behavior: bogus\n"
            )));
            assert_eq!(
                store.get_f64(keys::USER, keys::PROFILE_PARTIAL_RETRACTION),
                Some(expected),
                "{stored}"
            );
            assert_eq!(
                store
                    .get_str(keys::USER, keys::PROFILE_TARE_BEHAVIOR)
                    .as_deref(),
                Some(TARE_BEHAVIOR_AFTER_RETRACTION)
            );
        }
    }

    /// tests/test_config_upgrades.py: retired keys disappear, unknown keys stay.
    #[test]
    fn drops_retired_keys_and_keeps_unknown_ones() {
        let (_dir, store) = store_with(Some(
            "user:\n  telemetry_service_enabled: true\n  allow_debug_sending: true\n  future_key: 3\n",
        ));
        assert!(!store.contains(keys::USER, "telemetry_service_enabled"));
        assert!(!store.contains(keys::USER, "allow_debug_sending"));
        assert_eq!(store.get(keys::USER, "future_key"), json!(3));
    }

    /// A file that is not a mapping is moved aside and the defaults are used.
    #[test]
    fn broken_file_is_moved_aside() {
        let (dir, store) = store_with(Some("- just\n- a list\n"));
        assert!(store.has_error());
        let broken = std::fs::read_dir(dir.path())
            .expect("dir")
            .filter_map(Result::ok)
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("config_broken_")
            });
        assert!(broken);
        assert_eq!(store.get(keys::USER, keys::SSH_ENABLED), json!(true));
    }

    /// tests/test_config.py "Stop config writes from overwriting
    /// DefaultConfiguration_V1": a store's writes never reach a fresh defaults.
    #[test]
    fn writes_do_not_leak_into_defaults() {
        let (_dir, store) = store_with(None);
        store.set(keys::USER, keys::PROFILE_ORDER, json!(["a"]));
        assert_eq!(defaults()[keys::USER][keys::PROFILE_ORDER], json!([]));
    }

    #[test]
    fn merge_like_mergedeep() {
        let mut dst = json!({"a": {"x": 1, "y": [1]}, "b": 1});
        merge(&mut dst, &json!({"a": {"y": [2], "z": 3}, "c": {"k": 1}}));
        assert_eq!(
            dst,
            json!({"a": {"x": 1, "y": [2], "z": 3}, "b": 1, "c": {"k": 1}})
        );
    }
}
