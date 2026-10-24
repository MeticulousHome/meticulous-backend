//! Port of `tests/test_config.py` and `tests/test_config_upgrades.py`: the
//! settings file a machine already has, loaded by this release
//! (`MeticulousConfigDict(path, DefaultConfiguration_V1)` is
//! `ConfigStore::open(path, defaults())`).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config::{self, ConfigStore, keys};

/// `yaml.safe_load(path.read_text())`.
fn read_yaml(path: &Path) -> Value {
    serde_norway::from_str(&std::fs::read_to_string(path).expect("read config")).expect("yaml")
}

/// The config file with `disk` written into it, and the store loaded from it.
fn load(disk: &str) -> (tempfile::TempDir, PathBuf, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.yml");
    std::fs::write(&path, disk).expect("write config");
    let store = ConfigStore::open(&path, config::defaults());
    (dir, path, store)
}

/// `_load_config`: the defaults on disk with some user settings changed.
fn load_with_user(change: impl FnOnce(&mut serde_json::Map<String, Value>)) -> ConfigStore {
    let mut disk = config::defaults();
    change(disk[keys::USER].as_object_mut().expect("user section"));
    let (_dir, _path, store) = load(&config::to_yaml(&disk));
    store
}

fn broken_backups(dir: &Path) -> Vec<PathBuf> {
    let mut backups: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("config_broken_") && n.ends_with(".yml"))
        })
        .collect();
    backups.sort();
    backups
}

/// tests/test_config.py::test_load_removes_retired_telemetry_settings
#[test]
fn load_removes_retired_telemetry_settings() {
    let mut disk = config::defaults();
    disk[keys::USER]["telemetry_service_enabled"] = json!(true);
    disk[keys::USER]["allow_debug_sending"] = json!(true);
    let (_dir, path, store) = load(&config::to_yaml(&disk));

    assert!(!store.contains(keys::USER, "telemetry_service_enabled"));
    assert!(!store.contains(keys::USER, "allow_debug_sending"));
    let persisted = read_yaml(&path);
    assert_eq!(persisted[keys::USER].get("telemetry_service_enabled"), None);
    assert_eq!(persisted[keys::USER].get("allow_debug_sending"), None);
}

/// tests/test_config.py::test_load_replaces_non_string_tare_behavior
#[test]
fn load_replaces_non_string_tare_behavior() {
    let store = load_with_user(|user| {
        user.insert(
            keys::PROFILE_TARE_BEHAVIOR.into(),
            json!(["before_retraction"]),
        );
    });

    assert_eq!(
        store.get(keys::USER, keys::PROFILE_TARE_BEHAVIOR),
        json!("after_retraction")
    );
}

/// tests/test_config.py::test_load_adds_default_tare_behavior_to_older_config
#[test]
fn load_adds_default_tare_behavior_to_older_config() {
    let store = load_with_user(|user| {
        user.shift_remove(keys::PROFILE_TARE_BEHAVIOR);
    });

    assert_eq!(
        store.get(keys::USER, keys::PROFILE_TARE_BEHAVIOR),
        json!("after_retraction")
    );
}

/// tests/test_config_upgrades.py::test_an_older_config_gains_every_new_default_and_keeps_the_users_values
#[test]
fn an_older_config_gains_every_new_default_and_keeps_the_users_values() {
    let defaults = config::defaults();
    // Values that differ from the defaults, so keeping them is observable.
    let old_user = json!({
        keys::SOUNDS_ENABLED: !defaults[keys::USER][keys::SOUNDS_ENABLED].as_bool().expect("bool"),
        keys::MACHINE_HEAT_ON_BOOT: !defaults[keys::USER][keys::MACHINE_HEAT_ON_BOOT].as_bool().expect("bool"),
    });
    let disk = json!({
        "version": 1,
        keys::USER: old_user,
        keys::WIFI: {keys::WIFI_MODE: config::WIFI_MODE_CLIENT},
    });
    let (_dir, path, store) = load(&config::to_yaml(&disk));

    assert!(!store.has_error());
    for (section, section_defaults) in defaults.as_object().expect("defaults") {
        let Some(section_defaults) = section_defaults.as_object() else {
            continue;
        };
        let loaded = store.section(section);
        for key in section_defaults.keys() {
            assert!(loaded.get(key).is_some(), "{section}.{key}");
        }
    }
    for (key, value) in old_user.as_object().expect("old user") {
        assert_eq!(store.get(keys::USER, key), *value, "{key}");
    }
    assert_eq!(
        store.get(keys::WIFI, keys::WIFI_MODE),
        json!(config::WIFI_MODE_CLIENT)
    );
    let persisted = read_yaml(&path);
    for key in defaults[keys::USER].as_object().expect("user").keys() {
        assert!(persisted[keys::USER].get(key).is_some(), "{key}");
    }
}

/// tests/test_config_upgrades.py::test_a_config_from_a_newer_release_keeps_its_unknown_settings
#[test]
fn a_config_from_a_newer_release_keeps_its_unknown_settings() {
    let mut newer = config::defaults();
    newer["version"] = json!(newer["version"].as_i64().expect("version") + 1);
    newer[keys::USER]["setting_from_the_future"] = json!("kept");
    let (_dir, path, store) = load(&config::to_yaml(&newer));

    assert!(!store.has_error());
    assert_eq!(
        store.get(keys::USER, "setting_from_the_future"),
        json!("kept")
    );
    assert_eq!(
        read_yaml(&path)[keys::USER]["setting_from_the_future"],
        json!("kept")
    );
}

/// tests/test_config_upgrades.py::test_a_damaged_config_is_set_aside_and_replaced_by_defaults
#[test]
fn a_damaged_config_is_set_aside_and_replaced_by_defaults() {
    let damaged = "user: {enable_sounds: [unterminated\n";
    let (dir, path, store) = load(damaged);

    assert!(store.has_error());
    let backups = broken_backups(dir.path());
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(
        std::fs::read_to_string(&backups[0]).expect("backup"),
        damaged
    );
    let defaults = config::defaults();
    assert_eq!(store.section(keys::USER), defaults[keys::USER]);
    let keys_of = |section: &Value| {
        let mut names: Vec<String> = section
            .as_object()
            .expect("section")
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        keys_of(&read_yaml(&path)[keys::SYSTEM]),
        keys_of(&defaults[keys::SYSTEM])
    );
}

/// tests/test_config_upgrades.py::test_an_empty_config_file_is_treated_as_damaged:
/// what a power cut during a write can leave behind.
#[test]
fn an_empty_config_file_is_treated_as_damaged() {
    let (dir, _path, store) = load("");

    assert!(store.has_error());
    assert_eq!(broken_backups(dir.path()).len(), 1);
    assert_eq!(store.section(keys::USER), config::defaults()[keys::USER]);
}
