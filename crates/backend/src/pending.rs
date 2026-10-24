//! Stand-ins for services still being ported on other branches, so the
//! machine runtime can be built and run against the emulator meanwhile.
//! Each is replaced by the real module when its branch is merged; nothing
//! here ships.

use std::path::Path;

use serde_json::Value;

use crate::app::App;

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
