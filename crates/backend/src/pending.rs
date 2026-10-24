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

pub mod network {
    use serde_json::Value;

    use crate::app::App;
    use crate::config::keys;

    /// `HostnameManager.generateDeviceName()`.
    pub fn device_name(app: &App) -> String {
        let ident = match app.config.get(keys::SYSTEM, keys::DEVICE_IDENTIFIER) {
            Value::Array(parts) if parts.len() == 2 => {
                let first = parts[0].as_str().unwrap_or_default();
                let mut chars = first.chars();
                let capitalized: String = chars
                    .next()
                    .map(|c| c.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect())
                    .unwrap_or_default();
                format!("{capitalized}{}", parts[1].as_str().unwrap_or_default())
            }
            _ => "Espresso".into(),
        };
        format!("Meticulous{ident}")
    }

    /// `WifiManager.getCurrentConfig().hostname`.
    pub fn hostname(_app: &App) -> String {
        nix::unistd::gethostname()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}
