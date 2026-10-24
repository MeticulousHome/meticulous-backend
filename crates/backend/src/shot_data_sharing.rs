//! `shot_data_sharing.py`: opt-in, anonymous upload of debug shot files to
//! a Supabase storage bucket.
//!
//! The upload only happens when the user enabled `shot_data_sharing`. The
//! setting is tri-state: `None` until the user answers the prompt on the
//! dial, then `True` (opted in) or `False` (declined). The uploaded copy only
//! holds the fields of [`shared_shot_schema`] (sensor data, brew recipe and
//! software revisions), never the serial number, hostname, device name or
//! network details, and is grouped under a random per-machine sharing id
//! generated when the user opts in.

use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use parking_lot::RwLock;
use serde_json::{Map, Value, json};

use crate::app::App;
use crate::bug_report::watcher::{HttpError, classify};
use crate::config::keys;

pub const PLACEHOLDER_MARKER: &str = "PLACEHOLDER";
pub const UPLOAD_TIMEOUT_SECONDS: u64 = 60;
pub const COMPRESSION_LEVEL: i32 = 10;
pub const SHARED_SHOT_TYPE: &str = "shot";

/// Where shared shots go. The publishable key is a public client key (like
/// the Sentry DSN); the bucket policy only allows anonymous INSERT of `.zst`
/// files under a UUID folder. The environment points a development machine
/// at a different project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharingTarget {
    /// `SHOT_DATA_SHARING_SUPABASE_URL`.
    pub url: String,
    /// `SHOT_DATA_SHARING_SUPABASE_BUCKET`.
    pub bucket: String,
    /// `SHOT_DATA_SHARING_SUPABASE_ANON_KEY`.
    pub anon_key: String,
}

impl SharingTarget {
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> SharingTarget {
        SharingTarget {
            url: lookup("SHOT_DATA_SHARING_SUPABASE_URL")
                .unwrap_or_else(|| "https://szlveuxkohdjujuojwvf.supabase.co".into()),
            bucket: lookup("SHOT_DATA_SHARING_SUPABASE_BUCKET")
                .unwrap_or_else(|| "brew shots".into()),
            anon_key: lookup("SHOT_DATA_SHARING_SUPABASE_ANON_KEY")
                .unwrap_or_else(|| "sb_publishable_5hDHxnYhRLEUeA8wd9m3EQ_SQy9Ks5_".into()),
        }
    }
}

/// A node of the allowlist: copy the value, keep the listed keys of a dict,
/// or apply a schema to every element of a list.
#[derive(Debug)]
pub enum Schema {
    Keep,
    Dict(Vec<(&'static str, Schema)>),
    List(Box<Schema>),
}

fn keep_all(keys: &[&'static str]) -> Schema {
    Schema::Dict(keys.iter().map(|k| (*k, Schema::Keep)).collect())
}

/// `SHARED_SENSOR_FIELDS`.
pub const SHARED_SENSOR_FIELDS: [&str; 23] = [
    "external_1",
    "external_2",
    "bar_up",
    "bar_mid_up",
    "bar_mid_down",
    "bar_down",
    "tube",
    "motor_temp",
    "lam_temp",
    "motor_position",
    "motor_speed",
    "motor_power",
    "motor_current",
    "bandheater_power",
    "bandheater_current",
    "pressure_sensor",
    "adc_0",
    "adc_1",
    "adc_2",
    "adc_3",
    "water_status",
    "motor_thermistor",
    "weight_prediction",
];

/// The settings of `SHARED_SHOT_SCHEMA["config"]`: only those that change
/// how a brew runs.
pub const SHARED_CONFIG_FIELDS: [&str; 7] = [
    "auto_purge_after_shot",
    "auto_start_shot",
    "tare_behavior",
    "partial_retraction",
    "heat_on_boot",
    "heating_timeout",
    "allow_stage_skipping",
];

/// `SHARED_SHOT_SCHEMA`: anything not listed is dropped, so a field added to
/// debug files is not shared until it is added here.
pub fn shared_shot_schema() -> &'static Schema {
    static SCHEMA: LazyLock<Schema> = LazyLock::new(|| {
        Schema::Dict(vec![
            ("time", Schema::Keep),
            ("type", Schema::Keep),
            ("profile_name", Schema::Keep),
            // Software and hardware revision only; serial, batch, colour,
            // build date, hostname and device name stay on the machine.
            (
                "machine",
                keep_all(&[
                    "software_version",
                    "image_build_channel",
                    "image_version",
                    "repository_info",
                    "version_history",
                    "manufacturing",
                    "upgrade_first_boot",
                    "firmware_version",
                    "esp_pinout",
                    "main_voltage",
                    "scale_module",
                    "partial_retraction",
                    "auto_purge_after_shot",
                    "tare_behavior",
                    "tare_behavior_supported",
                ]),
            ),
            // The brew recipe without its author or identifiers.
            (
                "profile",
                keep_all(&[
                    "version",
                    "name",
                    "temperature",
                    "final_weight",
                    "variables",
                    "stages",
                ]),
            ),
            ("nodeJSON", Schema::Keep),
            ("config", keep_all(&SHARED_CONFIG_FIELDS)),
            (
                "data",
                Schema::List(Box::new(Schema::Dict(vec![
                    ("time", Schema::Keep),
                    ("profile_time", Schema::Keep),
                    ("profile_ms", Schema::Keep),
                    ("status", Schema::Keep),
                    (
                        "shot",
                        Schema::Dict(vec![
                            ("pressure", Schema::Keep),
                            ("flow", Schema::Keep),
                            ("weight", Schema::Keep),
                            ("gravimetric_flow", Schema::Keep),
                            (
                                "setpoints",
                                keep_all(&[
                                    "active",
                                    "pressure",
                                    "flow",
                                    "power",
                                    "piston",
                                    "temperature",
                                ]),
                            ),
                        ]),
                    ),
                    ("sensors", keep_all(&SHARED_SENSOR_FIELDS)),
                ]))),
            ),
            // Log lines already went through the redaction filter.
            (
                "logs",
                Schema::List(Box::new(keep_all(&[
                    "profile_ms",
                    "loglevel",
                    "caller",
                    "log_message",
                ]))),
            ),
        ])
    });
    &SCHEMA
}

/// `_select(value, schema)`: the part of `value` the schema allows, `None`
/// for `_OMIT`. Dicts come out in the schema's key order.
fn select(value: &Value, schema: &Schema) -> Option<Value> {
    match schema {
        Schema::Keep => Some(value.clone()),
        Schema::Dict(fields) => {
            let Value::Object(map) = value else {
                return None;
            };
            let mut selected = Map::new();
            for (key, sub_schema) in fields {
                if let Some(sub_value) = map.get(*key).and_then(|v| select(v, sub_schema)) {
                    selected.insert((*key).to_string(), sub_value);
                }
            }
            Some(Value::Object(selected))
        }
        Schema::List(item) => {
            let Value::Array(items) = value else {
                return None;
            };
            Some(Value::Array(
                items.iter().filter_map(|v| select(v, item)).collect(),
            ))
        }
    }
}

/// `quote(text, safe)`: UTF-8 percent-encoding of everything but letters,
/// digits, `_.-~` and `safe`.
fn quote(text: &str, safe: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        let c = byte as char;
        if c.is_ascii_alphanumeric() || "_.-~".contains(c) || (byte.is_ascii() && safe.contains(c))
        {
            out.push(c);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `ShotDataUploadError` and the other exceptions an upload can raise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadError {
    pub type_name: String,
    pub message: String,
}

impl From<HttpError> for UploadError {
    fn from(error: HttpError) -> UploadError {
        UploadError {
            type_name: error.type_name,
            message: error.message,
        }
    }
}

/// The service (`ShotDataSharing`).
pub struct ShotDataSharing {
    target: RwLock<SharingTarget>,
    /// Every failure reported to Sentry, for tests.
    #[cfg(test)]
    pub reported: parking_lot::Mutex<Vec<UploadError>>,
}

impl ShotDataSharing {
    pub fn new(target: SharingTarget) -> ShotDataSharing {
        ShotDataSharing {
            target: RwLock::new(target),
            #[cfg(test)]
            reported: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub fn target(&self) -> SharingTarget {
        self.target.read().clone()
    }

    #[cfg(test)]
    pub fn set_target(&self, target: SharingTarget) {
        *self.target.write() = target;
    }

    /// `is_enabled()`: only an explicit `True` counts.
    pub fn is_enabled(&self, app: &App) -> bool {
        app.config.get(keys::USER, keys::SHOT_DATA_SHARING) == Value::Bool(true)
    }

    /// `is_configured()`.
    pub fn is_configured(&self) -> bool {
        let target = self.target.read();
        !target.url.is_empty()
            && !target.bucket.is_empty()
            && !target.anon_key.is_empty()
            && !target.url.contains(PLACEHOLDER_MARKER)
            && !target.anon_key.contains(PLACEHOLDER_MARKER)
    }

    /// `should_upload(shot_type)`: only real (not emulated) brews, only when
    /// opted in.
    pub fn should_upload(&self, app: &App, shot_type: &str) -> bool {
        if !self.is_enabled(app) || shot_type != SHARED_SHOT_TYPE {
            return false;
        }
        if app.machine.emulated {
            tracing::info!("Not sharing emulated debug shots");
            return false;
        }
        true
    }

    /// `generate_sharing_id()`.
    pub fn generate_sharing_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// `get_sharing_id()`: the id, generated and saved when missing.
    pub fn get_sharing_id(&self, app: &App) -> String {
        if let Value::String(id) = app.config.get(keys::SYSTEM, keys::SHOT_DATA_SHARING_ID)
            && !id.is_empty()
        {
            return id;
        }
        let id = Self::generate_sharing_id();
        app.config
            .set(keys::SYSTEM, keys::SHOT_DATA_SHARING_ID, json!(id));
        app.config.save();
        tracing::info!("Generated a new shot data sharing id");
        id
    }

    /// `on_setting_changed(enabled)`: a new id on opt-in, none on opt-out.
    /// Called by the settings API before the new value is stored; the caller
    /// saves.
    pub fn on_setting_changed(&self, app: &App, value: &Value) {
        let enabled = crate::config::python_truthy(value);
        let previously_enabled = self.is_enabled(app);
        if enabled && !previously_enabled {
            app.config.set(
                keys::SYSTEM,
                keys::SHOT_DATA_SHARING_ID,
                json!(Self::generate_sharing_id()),
            );
            tracing::info!("User opted in to shot data sharing");
        } else if !enabled && previously_enabled {
            app.config
                .set(keys::SYSTEM, keys::SHOT_DATA_SHARING_ID, Value::Null);
            tracing::info!("User opted out of shot data sharing");
        }
    }

    /// `anonymize(debug_shot)`.
    pub fn anonymize(debug_shot: &Value) -> Value {
        select(debug_shot, shared_shot_schema()).unwrap_or_else(|| Value::Object(Map::new()))
    }

    /// `build_object_path(sharing_id, start, file_path)`.
    pub fn build_object_path(
        sharing_id: &str,
        start: chrono::DateTime<chrono::Local>,
        file_path: &Path,
    ) -> String {
        let name = file_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("{sharing_id}/{}/{name}", start.format("%Y-%m-%d"))
    }

    /// `build_upload_url(object_path)`: bucket names may contain spaces, so
    /// every segment is encoded.
    pub fn build_upload_url(&self, object_path: &str) -> String {
        let target = self.target.read();
        format!(
            "{}/storage/v1/object/{}/{}",
            target.url.trim_end_matches('/'),
            quote(&target.bucket, ""),
            quote(object_path, "/")
        )
    }

    /// `compress(payload)`: one zstd frame at level 10 with the content
    /// size, like `ZstdCompressor(level=10).compress`.
    pub fn compress(payload: &str) -> std::io::Result<Vec<u8>> {
        zstd::bulk::compress(payload.as_bytes(), COMPRESSION_LEVEL)
    }

    /// `build_headers()`. Legacy anon keys are JWTs and go in both headers;
    /// `sb_publishable_*` keys are not, and Storage rejects them as a bearer
    /// token, so they only go in `apikey`.
    pub fn build_headers(&self) -> Vec<(&'static str, String)> {
        let key = self.target.read().anon_key.clone();
        let mut headers = vec![
            ("apikey", key.clone()),
            ("Content-Type", "application/zstd".to_string()),
            ("x-upsert", "false".to_string()),
        ];
        if key.starts_with("eyJ") {
            headers.push(("Authorization", format!("Bearer {key}")));
        }
        headers
    }

    async fn post(&self, url: &str, body: Vec<u8>) -> Result<(), UploadError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|e| UploadError {
                type_name: "ClientError".into(),
                message: e.to_string(),
            })?;
        let mut request = client
            .post(url)
            .timeout(Duration::from_secs(UPLOAD_TIMEOUT_SECONDS))
            .body(body);
        for (name, value) in self.build_headers() {
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(|e| classify(&e, url))?;
        let status = response.status().as_u16();
        if status >= 400 {
            let text = response.text().await.unwrap_or_default();
            let details: String = text.chars().take(200).collect();
            return Err(UploadError {
                type_name: "ShotDataUploadError".into(),
                message: format!("Storage service answered HTTP {status}: {details}"),
            });
        }
        Ok(())
    }

    async fn try_upload(
        &self,
        app: &App,
        data_json: &str,
        start: chrono::DateTime<chrono::Local>,
        file_path: &Path,
        object_path: &mut Option<String>,
    ) -> Result<(), UploadError> {
        let debug_shot =
            crate::bug_report::py::json_loads(data_json).map_err(|message| UploadError {
                type_name: "JSONDecodeError".into(),
                message,
            })?;
        let anonymized = Self::anonymize(&debug_shot);
        let body = Self::compress(&pyfmt::dumps(&anonymized, &pyfmt::DumpOpts::unicode()))
            .map_err(|e| UploadError {
                type_name: "ZstdError".into(),
                message: e.to_string(),
            })?;
        let path = Self::build_object_path(&self.get_sharing_id(app), start, file_path);
        *object_path = Some(path.clone());
        let url = self.build_upload_url(&path);
        tracing::info!(
            "Uploading shared shot data ({} bytes) to {path}",
            body.len()
        );
        self.post(&url, body).await
    }

    /// `upload_debug_shot(data_json, start, file_path)`: upload an
    /// anonymized copy. Failures are logged and reported to Sentry, never
    /// raised, so the local debug file handling is unaffected.
    pub async fn upload(
        &self,
        app: &App,
        data_json: &str,
        start: chrono::DateTime<chrono::Local>,
        file_path: &Path,
    ) -> bool {
        if !self.is_configured() {
            tracing::warn!("Shot data sharing is enabled but the upload target is not configured");
            return false;
        }
        let mut object_path = None;
        match self
            .try_upload(app, data_json, start, file_path, &mut object_path)
            .await
        {
            Ok(()) => {
                let path = object_path.unwrap_or_default();
                tracing::info!("Shared shot data uploaded as {path}");
                true
            }
            Err(error) => {
                let path = object_path.clone().unwrap_or_else(|| "None".into());
                tracing::error!(
                    "Failed to upload shared shot data {path}: {}: {}",
                    error.type_name,
                    error.message
                );
                let context = json!({
                    "object_path": object_path,
                    "bucket": self.target.read().bucket,
                });
                app.reporting.capture_exception_with(
                    &error.type_name,
                    &error.message,
                    ("shot_data_sharing", "upload_failed"),
                    ("shot_data_sharing", &context),
                );
                #[cfg(test)]
                self.reported.lock().push(error);
                false
            }
        }
    }

    /// [`ShotDataSharing::upload`] from a plain thread, on its own event
    /// loop, like the Python's debug shot compression thread.
    pub fn upload_debug_shot(
        &self,
        app: &App,
        data_json: &str,
        start: chrono::DateTime<chrono::Local>,
        file_path: &Path,
    ) -> bool {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(self.upload(app, data_json, start, file_path)),
            Err(error) => {
                tracing::error!("Failed to upload shared shot data None: OSError: {error}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::bug_report::test_support::{TestServer, capture_logs};
    use crate::testing;

    fn shot_start() -> chrono::DateTime<chrono::Local> {
        use chrono::TimeZone;
        chrono::Local
            .with_ymd_and_hms(2026, 9, 29, 8, 12, 33)
            .single()
            .unwrap()
    }

    fn shot_file() -> PathBuf {
        PathBuf::from("/meticulous-user/history/debug/2026-09-29/08:12:33.shot.json.zst")
    }

    fn debug_shot() -> Value {
        json!({
            "time": shot_start().timestamp() as f64,
            "type": "shot",
            "profile_name": "Espresso",
            "machine": {
                "name": "MeticulousRoastedRobusta",
                "hostname": "meticulousRoastedRobusta-123",
                "serial_number": "123",
                "batch_number": "B1",
                "build_date": "2025-01-01",
                "color": "black",
                "software_version": "2026-09-01 00:00:00",
                "image_build_channel": "beta",
                "firmware_version": "1.2.3",
            },
            "profile": {"name": "Espresso"},
            "nodeJSON": {},
            "config": {
                "hostname_override": "my-machine",
                "machine_name": ["Roasted", "Robusta"],
                "wifi": {},
                "heat_on_boot": true,
            },
            "data": [{"shot": {"pressure": 9.0}, "sensors": {"motor_temp": 40.0}}],
            "logs": [],
        })
    }

    fn configured(app: &App, url: &str) {
        app.shot_sharing.set_target(SharingTarget {
            url: url.into(),
            bucket: "brew shots".into(),
            anon_key: "sb_publishable_test".into(),
        });
    }

    fn opted_in(app: &App) {
        app.config
            .set(keys::USER, keys::SHOT_DATA_SHARING, json!(true));
        app.config
            .set(keys::SYSTEM, keys::SHOT_DATA_SHARING_ID, json!("share-id"));
    }

    /// tests/test_shot_data_sharing.py::test_default_config_has_sharing_unanswered_and_no_sharing_id
    #[test]
    fn default_config_has_sharing_unanswered_and_no_sharing_id() {
        let defaults = crate::config::defaults();
        assert_eq!(defaults[keys::USER][keys::SHOT_DATA_SHARING], Value::Null);
        assert_eq!(
            defaults[keys::SYSTEM][keys::SHOT_DATA_SHARING_ID],
            Value::Null
        );
        assert!(defaults[keys::USER].get(keys::SHOT_DATA_SHARING).is_some());
        assert!(
            defaults[keys::SYSTEM]
                .get(keys::SHOT_DATA_SHARING_ID)
                .is_some()
        );
    }

    /// tests/test_shot_data_sharing.py::test_anonymize_keeps_only_allowlisted_fields
    #[test]
    fn anonymize_keeps_only_allowlisted_fields() {
        let original = debug_shot();
        let snapshot = original.clone();
        let anonymized = ShotDataSharing::anonymize(&original);
        assert_eq!(original, snapshot);
        let keys: Vec<&String> = anonymized.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "time",
                "type",
                "profile_name",
                "machine",
                "profile",
                "nodeJSON",
                "config",
                "data",
                "logs"
            ]
        );
        for key in [
            "name",
            "hostname",
            "serial_number",
            "batch_number",
            "build_date",
            "color",
        ] {
            assert!(anonymized["machine"].get(key).is_none());
        }
        assert_eq!(
            anonymized["machine"],
            json!({
                "software_version": "2026-09-01 00:00:00",
                "image_build_channel": "beta",
                "firmware_version": "1.2.3",
            })
        );
        assert_eq!(anonymized["config"], json!({"heat_on_boot": true}));
        assert_eq!(anonymized["profile"], json!({"name": "Espresso"}));
        assert_eq!(anonymized["data"], original["data"]);
        assert_eq!(anonymized["logs"], json!([]));
    }

    /// tests/test_shot_data_sharing.py::test_anonymize_drops_unknown_fields_at_every_level
    #[test]
    fn anonymize_drops_unknown_fields_at_every_level() {
        let mut shot = debug_shot();
        shot["new_top_level"] = json!("x");
        shot["machine"]["new_machine_field"] = json!("x");
        shot["config"]["new_setting"] = json!("x");
        for (k, v) in [("author", "Someone"), ("author_id", "u1"), ("id", "p1")] {
            shot["profile"][k] = json!(v);
        }
        shot["data"][0]["new_sample_field"] = json!("x");
        shot["data"][0]["shot"]["new_shot_field"] = json!("x");
        shot["data"][0]["shot"]["setpoints"] =
            json!({"active": "pressure", "pressure": 9.0, "x": 1});
        shot["data"][0]["sensors"]["new_sensor"] = json!(1.0);
        shot["logs"] = json!([
            {"profile_ms": 1, "loglevel": "INFO", "caller": "m", "log_message": "hi", "extra": 1}
        ]);

        let anonymized = ShotDataSharing::anonymize(&shot);

        assert!(anonymized.get("new_top_level").is_none());
        assert!(anonymized["machine"].get("new_machine_field").is_none());
        assert!(anonymized["config"].get("new_setting").is_none());
        assert_eq!(anonymized["profile"], json!({"name": "Espresso"}));
        let sample = &anonymized["data"][0];
        assert_eq!(
            sample.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["shot", "sensors"]
        );
        assert_eq!(
            sample["shot"],
            json!({"pressure": 9.0, "setpoints": {"active": "pressure", "pressure": 9.0}})
        );
        assert_eq!(sample["sensors"], json!({"motor_temp": 40.0}));
        assert_eq!(
            anonymized["logs"],
            json!([{"profile_ms": 1, "loglevel": "INFO", "caller": "m", "log_message": "hi"}])
        );
    }

    /// tests/test_shot_data_sharing.py::test_anonymize_drops_values_with_an_unexpected_shape
    #[test]
    fn anonymize_drops_values_with_an_unexpected_shape() {
        let mut shot = debug_shot();
        shot["machine"] = json!("not a dict");
        shot["data"] = json!({"not": "a list"});
        shot["logs"] = json!(["not a dict", {"loglevel": "INFO"}]);
        let anonymized = ShotDataSharing::anonymize(&shot);
        assert!(anonymized.get("machine").is_none());
        assert!(anonymized.get("data").is_none());
        assert_eq!(anonymized["logs"], json!([{"loglevel": "INFO"}]));
        assert_eq!(ShotDataSharing::anonymize(&json!({})), json!({}));
    }

    /// tests/test_shot_data_sharing.py::test_shared_schema_names_only_real_fields
    #[test]
    fn shared_schema_names_only_real_fields() {
        let sensors = crate::shots::sensor_dict(&esp::SensorData::default());
        for field in SHARED_SENSOR_FIELDS {
            assert!(sensors.get(field).is_some(), "{field}");
        }
        let user = &crate::config::defaults()[keys::USER];
        for field in SHARED_CONFIG_FIELDS {
            assert!(user.get(field).is_some(), "{field}");
        }
    }

    /// tests/test_shot_data_sharing.py::test_object_path_groups_by_sharing_id_and_day
    #[test]
    fn object_path_groups_by_sharing_id_and_day() {
        assert_eq!(
            ShotDataSharing::build_object_path("abc", shot_start(), &shot_file()),
            "abc/2026-09-29/08:12:33.shot.json.zst"
        );
    }

    /// tests/test_shot_data_sharing.py::test_is_configured_rejects_placeholders
    #[test]
    fn is_configured_rejects_placeholders() {
        let sharing = ShotDataSharing::new(SharingTarget {
            url: "https://PLACEHOLDER.supabase.co".into(),
            bucket: "brew shots".into(),
            anon_key: "anon-key".into(),
        });
        assert!(!sharing.is_configured());
        sharing.set_target(SharingTarget {
            url: "https://project.supabase.co".into(),
            bucket: "brew shots".into(),
            anon_key: "PLACEHOLDER_ANON_KEY".into(),
        });
        assert!(!sharing.is_configured());
        sharing.set_target(SharingTarget {
            url: "https://project.supabase.co".into(),
            bucket: "brew shots".into(),
            anon_key: "anon-key".into(),
        });
        assert!(sharing.is_configured());
    }

    /// tests/test_shot_data_sharing.py::test_should_upload_requires_opt_in_real_machine_and_shot_type
    #[test]
    fn should_upload_requires_opt_in_real_machine_and_shot_type() {
        let (_dir, app) = testing::app_with(crate::env::BackendMode::Fika, |_| {});
        assert!(!app.shot_sharing.should_upload(&app, "shot"));
        app.config
            .set(keys::USER, keys::SHOT_DATA_SHARING, json!(false));
        assert!(!app.shot_sharing.should_upload(&app, "shot"));
        app.config
            .set(keys::USER, keys::SHOT_DATA_SHARING, json!(true));
        assert!(app.shot_sharing.should_upload(&app, "shot"));
        assert!(!app.shot_sharing.should_upload(&app, "purge"));
        assert!(!app.shot_sharing.should_upload(&app, "boot"));

        let (_dir, emulated) = testing::app();
        emulated
            .config
            .set(keys::USER, keys::SHOT_DATA_SHARING, json!(true));
        assert!(!emulated.shot_sharing.should_upload(&emulated, "shot"));
    }

    /// tests/test_shot_data_sharing.py::test_opt_in_rotates_sharing_id_and_opt_out_forgets_it
    #[test]
    fn opt_in_rotates_sharing_id_and_opt_out_forgets_it() {
        let (_dir, app) = testing::app();
        let id = || app.config.get(keys::SYSTEM, keys::SHOT_DATA_SHARING_ID);
        assert_eq!(id(), Value::Null);

        app.shot_sharing.on_setting_changed(&app, &json!(true));
        let first = id();
        assert_eq!(first.as_str().map(str::len), Some(36));

        // Re-sending True while already enabled keeps the id stable.
        app.config
            .set(keys::USER, keys::SHOT_DATA_SHARING, json!(true));
        app.shot_sharing.on_setting_changed(&app, &json!(true));
        assert_eq!(id(), first);

        app.shot_sharing.on_setting_changed(&app, &json!(false));
        assert_eq!(id(), Value::Null);

        app.config
            .set(keys::USER, keys::SHOT_DATA_SHARING, json!(false));
        app.shot_sharing.on_setting_changed(&app, &json!(true));
        assert_ne!(id(), first);
    }

    /// tests/test_shot_data_sharing.py::test_get_sharing_id_generates_and_persists_when_missing
    #[test]
    fn get_sharing_id_generates_and_persists_when_missing() {
        let (_dir, app) = testing::app();
        let saves = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = saves.clone();
        app.config.on_save(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });

        let sharing_id = app.shot_sharing.get_sharing_id(&app);

        assert_eq!(
            app.config.get(keys::SYSTEM, keys::SHOT_DATA_SHARING_ID),
            json!(sharing_id)
        );
        assert_eq!(saves.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(app.shot_sharing.get_sharing_id(&app), sharing_id);
        assert_eq!(saves.load(std::sync::atomic::Ordering::SeqCst), 1);
        let on_disk = std::fs::read_to_string(app.config.path()).unwrap();
        assert!(on_disk.contains(&sharing_id));
    }

    /// tests/test_shot_data_sharing.py::test_upload_sends_anonymized_zstd_payload
    #[tokio::test]
    async fn upload_sends_anonymized_zstd_payload() {
        let (_dir, app) = testing::app();
        let server = TestServer::start(|_| (200, "{}".to_string()));
        configured(&app, &server.url(""));
        opted_in(&app);

        let ok = app
            .shot_sharing
            .upload(
                &app,
                &pyfmt::dumps_default(&debug_shot()),
                shot_start(),
                &shot_file(),
            )
            .await;

        assert!(ok);
        assert!(app.shot_sharing.reported.lock().is_empty());
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.target,
            "/storage/v1/object/brew%20shots/share-id/2026-09-29/08%3A12%3A33.shot.json.zst"
        );
        assert_eq!(request.header("apikey"), Some("sb_publishable_test"));
        assert_eq!(request.header("authorization"), None);
        assert_eq!(request.header("content-type"), Some("application/zstd"));
        assert_eq!(request.header("x-upsert"), Some("false"));
        assert_eq!(
            app.shot_sharing
                .build_upload_url("share-id/2026-09-29/08:12:33.shot.json.zst"),
            format!(
                "{}/storage/v1/object/brew%20shots/share-id/2026-09-29/08%3A12%3A33.shot.json.zst",
                server.url("")
            )
        );
        assert_eq!(UPLOAD_TIMEOUT_SECONDS, 60);

        let uploaded: Value =
            serde_json::from_slice(&zstd::stream::decode_all(request.body.as_slice()).unwrap())
                .unwrap();
        assert_eq!(uploaded, ShotDataSharing::anonymize(&debug_shot()));
        assert!(uploaded["machine"].get("serial_number").is_none());
    }

    /// The headers of a legacy JWT anon key (`build_headers`).
    #[test]
    fn legacy_jwt_keys_also_go_in_authorization() {
        let sharing = ShotDataSharing::new(SharingTarget {
            url: "https://project.supabase.co".into(),
            bucket: "brew shots".into(),
            anon_key: "eyJhbGciOi".into(),
        });
        assert!(
            sharing
                .build_headers()
                .contains(&("Authorization", "Bearer eyJhbGciOi".into()))
        );
    }

    /// tests/test_shot_data_sharing.py::test_upload_failure_is_logged_and_reported_to_sentry
    #[tokio::test]
    async fn upload_failure_is_logged_and_reported_to_sentry() {
        let (_dir, app) = testing::app();
        let server =
            TestServer::start(|_| (403, "new row violates row-level security".to_string()));
        configured(&app, &server.url(""));
        opted_in(&app);
        let logs = capture_logs();

        let ok = app
            .shot_sharing
            .upload(
                &app,
                &pyfmt::dumps_default(&debug_shot()),
                shot_start(),
                &shot_file(),
            )
            .await;

        assert!(!ok);
        let reported = app.shot_sharing.reported.lock().clone();
        assert_eq!(reported.len(), 1);
        assert_eq!(reported[0].type_name, "ShotDataUploadError");
        assert!(reported[0].message.contains("HTTP 403"));
        let errors = logs.messages(tracing::Level::ERROR, "shot_data_sharing");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Failed to upload shared shot data"));
        assert!(errors[0].contains("share-id/2026-09-29/08:12:33.shot.json.zst"));
    }

    /// tests/test_shot_data_sharing.py::test_network_error_is_reported_and_does_not_raise
    #[tokio::test]
    async fn network_error_is_reported_and_does_not_raise() {
        let (_dir, app) = testing::app();
        configured(
            &app,
            &format!("http://127.0.0.1:{}", TestServer::closed_port()),
        );
        opted_in(&app);

        let ok = app
            .shot_sharing
            .upload(
                &app,
                &pyfmt::dumps_default(&debug_shot()),
                shot_start(),
                &shot_file(),
            )
            .await;

        assert!(!ok);
        let reported = app.shot_sharing.reported.lock().clone();
        assert_eq!(reported.len(), 1);
        assert_eq!(reported[0].type_name, "ClientConnectorError");
    }

    /// tests/test_shot_data_sharing.py::test_upload_is_skipped_when_target_is_not_configured
    #[tokio::test]
    async fn upload_is_skipped_when_target_is_not_configured() {
        let (_dir, app) = testing::app();
        let server = TestServer::start(|_| (200, String::new()));
        app.shot_sharing.set_target(SharingTarget {
            url: "https://PLACEHOLDER.supabase.co".into(),
            bucket: "brew shots".into(),
            anon_key: "sb_publishable_test".into(),
        });
        opted_in(&app);

        let ok = app
            .shot_sharing
            .upload(
                &app,
                &pyfmt::dumps_default(&debug_shot()),
                shot_start(),
                &shot_file(),
            )
            .await;

        assert!(!ok);
        assert!(app.shot_sharing.reported.lock().is_empty());
        assert!(server.requests().is_empty());
    }

    /// The debug shot thread's entry point runs the upload on its own loop.
    #[test]
    fn upload_debug_shot_runs_from_a_plain_thread() {
        let (_dir, app) = testing::app();
        let server = TestServer::start(|_| (200, String::new()));
        configured(&app, &server.url(""));
        opted_in(&app);
        assert!(app.shot_sharing.upload_debug_shot(
            &app,
            &pyfmt::dumps_default(&debug_shot()),
            shot_start(),
            &shot_file()
        ));
        assert_eq!(server.requests().len(), 1);
    }
}
