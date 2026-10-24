//! Stand-ins for services still being ported on other branches, so the
//! machine runtime can be built and run against the emulator meanwhile.
//! Each is replaced by the real module when its branch is merged; nothing
//! here ships.

use std::path::Path;

use serde_json::{Value, json};

use crate::app::App;

pub struct Ssh;
impl Ssh {
    pub fn set_ssh_state(&self, _enabled: bool) -> bool {
        true
    }
}

pub struct Smoke;
impl Smoke {
    pub fn observe_status(&self, _status: &str, _extracting: bool) {}
    pub fn observe_esp_log(&self, _message: &str) {}
}

pub struct Ota;
impl Ota {
    pub fn is_changed(&self) -> bool {
        false
    }
    pub fn set_channel(&self, _app: &App, _channel: &str) {}
}

pub struct Usb;
impl Usb {
    pub fn set_usb_mode(&self, _app: &App, _mode: &str) -> Result<(), String> {
        Ok(())
    }
}

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

pub mod firmware {
    use std::path::{Path, PathBuf};

    pub fn firmware_version(_update_path: &Path) -> Option<String> {
        None
    }

    pub fn flash(
        _port: &mut Box<dyn serialport::SerialPort>,
        _update_path: &Path,
    ) -> Result<(), String> {
        Err("firmware flashing is not ported yet".into())
    }

    #[derive(Debug, Clone, Copy)]
    pub enum SupportedChip {
        Esp32S3,
    }

    impl SupportedChip {
        pub fn from_name(name: &str) -> Option<SupportedChip> {
            (name.to_uppercase() == "ESP32S3").then_some(SupportedChip::Esp32S3)
        }
        pub fn python_names_list() -> String {
            "['ESP32S3']".into()
        }
    }

    pub fn firmware_path(update_path: &Path, _chip: SupportedChip) -> PathBuf {
        update_path.join("esp32-s3")
    }
}

pub mod device_identity {
    use std::path::Path;

    pub fn generate_device_uuid() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    pub fn update_device_uuid_cache(path: &Path, uuid: &str) -> std::io::Result<bool> {
        if std::fs::read_to_string(path).ok().as_deref() == Some(uuid) {
            return Ok(false);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, uuid)?;
        Ok(true)
    }
}

pub mod limited_access {
    use crate::app::App;

    pub fn is_limited_access(_app: &App) -> bool {
        false
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
