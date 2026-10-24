//! Stand-ins for services still being ported on other branches, so the
//! machine runtime can be built and run against the emulator meanwhile.
//! Each is replaced by the real module when its branch is merged; nothing
//! here ships.

use std::path::Path;

use serde_json::{Value, json};

use crate::app::App;

pub struct Timezone;
impl Timezone {
    pub async fn request_and_sync_tz(&self, _app: &App) -> Result<Option<String>, String> {
        Ok(None)
    }
    pub fn update_timezone(&self, _app: &App, _tz: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn ui_timezones(&self, _app: &App) -> Value {
        json!({})
    }
}

pub struct ShotSharing;
impl ShotSharing {
    pub fn should_upload(&self, _app: &App, _shot_type: &str) -> bool {
        false
    }
    pub fn upload_debug_shot(
        &self,
        _app: &App,
        _text: &str,
        _start: chrono::DateTime<chrono::Local>,
        _path: &Path,
    ) {
    }
    pub fn on_setting_changed(&self, _app: &App, _value: &Value) {}
}

pub struct History;
impl History {
    pub fn statistics(&self) -> Result<Value, String> {
        Ok(json!({"totalSavedShots": 0}))
    }
    pub fn last_shot(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
    pub fn insert_history(&self, _shot: &Value) -> Result<i64, String> {
        Ok(0)
    }
    pub fn link_debug_file(&self, _history_id: i64, _file: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn unlink_debug_file(&self, _file: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn search_manual_shots(&self, _ids: &[String]) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }
}

pub mod shot_files {
    use std::path::Path;

    /// The shot file: JSON compressed at zstd level 10, like `zstd -10`.
    pub fn write_compressed(path: &Path, json: &[u8]) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let compressed = zstd::encode_all(json, 10)?;
        std::fs::write(path, compressed)
    }
}

pub mod machine_identity {
    use serde_json::{Map, Value, json};

    use crate::app::App;

    /// The identity part of `get_machine_info` shared with debug shots.
    pub fn identity(_app: &App) -> Map<String, Value> {
        let mut machine = Map::new();
        machine.insert("name".into(), json!(""));
        machine.insert("hostname".into(), json!(""));
        machine.insert("software_version".into(), Value::Null);
        machine.insert("image_build_channel".into(), Value::Null);
        machine.insert("image_version".into(), Value::Null);
        machine.insert("repository_info".into(), json!({}));
        machine
    }
}
