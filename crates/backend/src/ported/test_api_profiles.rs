//! Port of `tests/test_api_profiles.py`, with the `SimpleProfile` cases of
//! `tests/test_simple_profile.py` and the `send_profile_to_esp32` case of
//! `tests/test_profile_uart_payload.py`.
//!
//! The handlers run against the real `ProfileManager` on temporary
//! directories, like the Python fixture. The Socket.IO emits and the bytes
//! written to the ESP32 are what the test app records; notifications are
//! read from the notification manager (the Python test records
//! `AlarmManager._notify_user` apart from the emits).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use http::Method;
use md5::{Digest, Md5};
use serde_json::{Map, Value, json};

use crate::alarms::AlarmType;
use crate::app::App;
use crate::config::keys;
use crate::env::BackendMode;
use crate::http::Reply;
use crate::notifications::now_seconds;
use crate::profiles::Hover;
use crate::simple_profile::SimpleProfile;
use crate::testing::{app_custom, call, call_with_headers, json as body_json};

const UNKNOWN_ID: &str = "00000000-1111-4222-8333-444444444444";
const MALFORMED_JSON_ERROR: &str =
    "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)";
const MOTOR_STRESSED_MESSAGE: &str = "Brewing has been disabled because of a recent high strain on the motor, let it rest for 10 more minutes";
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n-test-image-payload";
const ESP32_PROFILE_FIELDS: [&str; 5] =
    ["name", "temperature", "final_weight", "variables", "stages"];
const MANUAL_MODE_PROFILE_ID: &str = "4d616e75-616c-4d6f-8465-000000000001";
const SIMPLE_PROFILE_ID: &str = "f5db85cc-ab0d-4c81-aee8-bcac7da77141";

fn machine_busy() -> Value {
    json!({"status": "error", "error": "machine is busy"})
}

fn high_strain() -> Value {
    json!({"status": "error", "error": "high strain on motor"})
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn repo_json(relative: &str) -> Value {
    let path = repo().join(relative);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e} (is the submodule checked out?)",
            path.display()
        )
    });
    serde_json::from_str(&text).expect("JSON fixture")
}

fn example_profile() -> Value {
    repo_json("profile_schema/example_profile.json")
}

fn simple_profile() -> Value {
    repo_json("simple_profile.json")
}

fn md5_hex(bytes: &[u8]) -> String {
    hex::encode(Md5::digest(bytes))
}

fn default_image_source() -> PathBuf {
    repo().join("images/default/profile_default_1.png")
}

fn default_image_name() -> String {
    let bytes = std::fs::read(default_image_source()).expect("default image");
    format!("{}.png", md5_hex(&bytes))
}

fn default_image_accent() -> Value {
    repo_json("images/default/accent_colors.json")[default_image_name()].clone()
}

fn png_name() -> String {
    format!("{}.png", md5_hex(PNG_BYTES))
}

fn png_data_uri() -> String {
    use base64::Engine;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(PNG_BYTES)
    )
}

fn without(profile: &Value, key: &str) -> Value {
    let mut copy = profile.clone();
    copy.as_object_mut()
        .expect("profile object")
        .shift_remove(key);
    copy
}

fn with(profile: &Value, key: &str, value: Value) -> Value {
    let mut copy = profile.clone();
    copy[key] = value;
    copy
}

fn is_uuid4(value: &Value) -> bool {
    value
        .as_str()
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .is_some_and(|u| u.get_version_num() == 4)
}

fn substitute_variables(node: &Value, values: &Map<String, Value>) -> Value {
    match node {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), substitute_variables(v, values)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| substitute_variables(v, values))
                .collect(),
        ),
        Value::String(s) if s.starts_with('$') => values[&s[1..]].clone(),
        other => other.clone(),
    }
}

/// Only the runtime fields reach the ESP32, with `$variables` resolved.
fn expected_esp32_payload(profile: &Value) -> Value {
    let values: Map<String, Value> = profile
        .get("variables")
        .and_then(Value::as_array)
        .map(|variables| {
            variables
                .iter()
                .map(|v| {
                    (
                        v["key"].as_str().expect("key").to_string(),
                        v["value"].clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut payload = Map::new();
    for field in ESP32_PROFILE_FIELDS {
        if let Some(value) = profile.get(field) {
            payload.insert(field.to_string(), value.clone());
        }
    }
    payload.insert(
        "stages".into(),
        substitute_variables(&profile["stages"], &values),
    );
    Value::Object(payload)
}

/// The `hash,<md5>\x03` frame followed by the `json\n<payload>\x03` frame.
fn expected_serial_bytes(payload: &Value) -> Vec<u8> {
    let body = pyfmt::dumps_default(payload);
    let digest = md5_hex(body.as_bytes());
    [
        b"hash,".as_slice(),
        digest.as_bytes(),
        b"\x03",
        format!("json\n{body}\x03").as_bytes(),
    ]
    .concat()
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The Python fixture: profiles and images under a temporary directory, the
/// bundled default image as the only default, the machine idle, nothing
/// loaded yet.
struct Fx {
    _dir: tempfile::TempDir,
    app: Arc<App>,
    config_at_start: Vec<u8>,
}

impl Fx {
    fn new() -> Fx {
        Fx::with_config(|_| {})
    }

    /// The fixture with the config adjusted before it is saved.
    fn with_config(adjust: impl FnOnce(&App)) -> Fx {
        let (dir, app) = app_custom(BackendMode::Emulator, |env| {
            let user = env.paths.profiles.parent().expect("user dir").to_path_buf();
            let defaults = user.join("default-images");
            std::fs::create_dir_all(&defaults).expect("mkdir default images");
            std::fs::copy(
                default_image_source(),
                defaults.join("profile_default_1.png"),
            )
            .expect("copy default image");
            std::fs::copy(
                repo().join("images/default/accent_colors.json"),
                defaults.join("accent_colors.json"),
            )
            .expect("copy accent colors");
            env.paths.default_images = defaults;
        });
        std::fs::create_dir_all(&app.env.paths.profiles).expect("mkdir profiles");
        app.config
            .set(keys::USER, keys::UPDATE_CHANNEL, json!("stable"));
        app.config.set(keys::USER, keys::PROFILE_ORDER, json!([]));
        app.config
            .set(keys::USER, keys::ALLOW_LEGACY_JSON, json!(false));
        app.config
            .set(keys::PROFILES, keys::PROFILE_LAST, Value::Null);
        adjust(&app);
        app.config.save();
        app.profiles.refresh_image_list();
        app.sio.clear_emitted();
        let config_at_start = std::fs::read(app.env.paths.config_file()).expect("config file");
        Fx {
            _dir: dir,
            app,
            config_at_start,
        }
    }

    fn profiles_dir(&self) -> &Path {
        &self.app.env.paths.profiles
    }

    fn images_dir(&self) -> &Path {
        &self.app.env.paths.images
    }

    async fn get(&self, path: &str) -> Reply {
        call(&self.app, Method::GET, &format!("/api/v1{path}"), "").await
    }

    async fn get_json(&self, path: &str) -> Value {
        let reply = self.get(path).await;
        assert_eq!(
            reply.status_code(),
            200,
            "{path}: {}",
            String::from_utf8_lossy(reply.body())
        );
        body_json(&reply)
    }

    async fn post(&self, path: &str, body: impl Into<Bytes>) -> Reply {
        self.post_with(path, body, &[]).await
    }

    async fn post_with(
        &self,
        path: &str,
        body: impl Into<Bytes>,
        headers: &[(&str, &str)],
    ) -> Reply {
        call_with_headers(
            &self.app,
            Method::POST,
            &format!("/api/v1{path}"),
            body,
            headers,
        )
        .await
    }

    async fn post_json(&self, path: &str, payload: &Value) -> Reply {
        self.post(path, payload.to_string()).await
    }

    async fn delete(&self, path: &str, headers: &[(&str, &str)]) -> Reply {
        call_with_headers(
            &self.app,
            Method::DELETE,
            &format!("/api/v1{path}"),
            "",
            headers,
        )
        .await
    }

    async fn save(&self, profile: &Value, change_id: Option<&str>) -> Value {
        let headers: Vec<(&str, &str)> =
            change_id.map(|c| ("X-Change-Id", c)).into_iter().collect();
        let reply = self
            .post_with("/profile/save", profile.to_string(), &headers)
            .await;
        assert_eq!(
            reply.status_code(),
            200,
            "{}",
            String::from_utf8_lossy(reply.body())
        );
        body_json(&reply)
    }

    /// The emits clients received, without the notifications.
    fn emitted(&self) -> Vec<(String, Value)> {
        self.app
            .sio
            .emitted()
            .into_iter()
            .filter(|e| e.event != "notification")
            .map(|e| {
                assert_eq!((&e.to, &e.skip_sid), (&None, &None), "{e:?}");
                (e.event, e.data)
            })
            .collect()
    }

    fn forget_emits(&self) {
        self.app.sio.clear_emitted();
    }

    fn notifications(&self) -> Vec<(String, Option<String>)> {
        self.app
            .notifications
            .all()
            .into_iter()
            .map(|n| (n.message, n.image))
            .collect()
    }

    fn stored_files(&self) -> Vec<String> {
        names(self.profiles_dir())
    }

    fn stored_images(&self) -> Vec<String> {
        names(self.images_dir())
    }

    fn stored_profile(&self, id: &str) -> Value {
        let text = std::fs::read_to_string(self.profiles_dir().join(format!("{id}.json")))
            .expect("stored profile");
        serde_json::from_str(&text).expect("stored profile JSON")
    }

    fn uart(&self) -> Vec<u8> {
        self.app.machine.uart_writes.lock().concat()
    }

    fn order(&self) -> Value {
        self.app.config.get(keys::USER, keys::PROFILE_ORDER)
    }

    fn set_order(&self, order: Value) {
        self.app.config.set(keys::USER, keys::PROFILE_ORDER, order);
    }

    fn last(&self) -> Value {
        self.app.config.get(keys::PROFILES, keys::PROFILE_LAST)
    }

    fn persisted_config(&self) -> Value {
        let text = std::fs::read_to_string(self.app.env.paths.config_file()).expect("config");
        serde_norway::from_str(&text).expect("config YAML")
    }

    fn set_idle(&self, idle: bool) {
        self.app.machine.state.lock().is_idle = idle;
    }

    fn stress_motor(&self) {
        // `Alarm(AlarmType.MOTOR_STRESSED, math.inf)`: no end, so the message
        // says 10 minutes.
        self.app
            .alarms
            .set(AlarmType::MotorStressed, None, true, true);
    }

    fn assert_nothing_persisted(&self) {
        assert_eq!(self.stored_files(), Vec::<String>::new());
        assert_eq!(self.stored_images(), vec![default_image_name()]);
        assert_eq!(self.app.profiles.known_count(), 0);
        assert_eq!(self.app.profiles.changes(), Vec::<Value>::new());
        assert_eq!(self.order(), json!([]));
        assert_eq!(
            std::fs::read(self.app.env.paths.config_file()).expect("config"),
            self.config_at_start,
            "the config was saved"
        );
        assert_eq!(self.emitted(), vec![]);
    }

    fn assert_nothing_sent(&self) {
        assert_eq!(self.uart(), Vec::<u8>::new());
        assert_eq!(self.app.debug_shots.node_json(), None);
        assert_eq!(self.last(), Value::Null);
        assert_eq!(self.emitted(), vec![]);
    }
}

fn event(name: &str, data: Value) -> (String, Value) {
    (name.to_string(), data)
}

// --- save ------------------------------------------------------------------

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_saving_a_valid_profile_persists_lists_and_announces_it
#[tokio::test]
async fn saving_a_valid_profile_persists_lists_and_announces_it() {
    let fx = Fx::new();
    let profile = example_profile();
    let id = profile["id"].as_str().expect("id").to_string();
    let before = now_seconds();

    let result = fx.save(&profile, Some("client-change-1")).await;

    let after = now_seconds();
    assert_eq!(
        result,
        json!({"profile": profile, "change_id": "client-change-1"})
    );
    assert_eq!(fx.stored_files(), vec![format!("{id}.json")]);
    assert_eq!(fx.stored_profile(&id), profile);

    let summary = without(&profile, "stages");
    assert_eq!(fx.get_json("/profile/list").await, json!([summary]));
    assert_eq!(
        fx.get_json("/profile/list?full=true").await,
        json!([profile])
    );
    assert_eq!(fx.get_json(&format!("/profile/get/{id}")).await, profile);

    assert_eq!(
        fx.emitted(),
        vec![
            event(
                "profile",
                json!({"change": "create", "profile_id": id, "change_id": "client-change-1"})
            ),
            event(
                "profileHover",
                json!({"id": id, "type": "focus", "from": "dial"})
            ),
        ]
    );
    let mut changes = fx.get_json("/profile/changes").await;
    let changes = changes.as_array_mut().expect("list");
    assert_eq!(changes.len(), 1);
    let timestamp = changes[0]
        .as_object_mut()
        .expect("change")
        .shift_remove("timestamp")
        .and_then(|t| t.as_f64())
        .expect("timestamp");
    assert!(before <= timestamp && timestamp <= after, "{timestamp}");
    assert_eq!(
        *changes,
        vec![json!({"type": "create", "profile_id": id, "change_id": "client-change-1"})]
    );
    assert_eq!(
        fx.get_json("/profile/selected").await,
        json!({"id": id, "type": "focus", "from": "dial"})
    );
    assert_eq!(fx.order(), json!([id]));
    assert_eq!(fx.persisted_config()["user"]["profile_order"], json!([id]));
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_saved_profile_survives_a_profile_manager_refresh
#[tokio::test]
async fn saved_profile_survives_a_profile_manager_refresh() {
    let fx = Fx::new();
    let profile = simple_profile();
    fx.save(&profile, None).await;
    fx.forget_emits();

    fx.app.profiles.refresh_profile_list(&fx.app);

    assert_eq!(
        fx.get_json(&format!("/profile/get/{SIMPLE_PROFILE_ID}"))
            .await,
        profile
    );
    assert_eq!(
        fx.get_json("/profile/list?full=true").await,
        json!([profile])
    );
    assert_eq!(
        fx.emitted(),
        vec![event("profile", json!({"change": "full_reload"}))]
    );
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_refresh_stamps_last_changed_on_a_profile_saved_without_one
#[tokio::test]
async fn refresh_stamps_last_changed_on_a_profile_saved_without_one() {
    let fx = Fx::new();
    let profile = example_profile();
    let id = profile["id"].as_str().expect("id").to_string();
    assert!(profile.get("last_changed").is_none());
    fx.save(&profile, None).await;

    let before = now_seconds();
    fx.app.profiles.refresh_profile_list(&fx.app);
    let after = now_seconds();

    let mut reloaded = fx.get_json(&format!("/profile/get/{id}")).await;
    let last_changed = reloaded
        .as_object_mut()
        .expect("profile")
        .shift_remove("last_changed")
        .expect("last_changed");
    let stamp = last_changed.as_f64().expect("float");
    assert!(before <= stamp && stamp <= after, "{stamp}");
    assert_eq!(reloaded, profile);
    assert_eq!(
        fx.stored_profile(&id),
        with(&profile, "last_changed", last_changed)
    );
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_saving_an_existing_id_records_an_update_without_moving_the_selection
#[tokio::test]
async fn saving_an_existing_id_records_an_update_without_moving_the_selection() {
    let fx = Fx::new();
    let profile = example_profile();
    let id = profile["id"].as_str().expect("id").to_string();
    fx.save(&profile, Some("first")).await;
    fx.forget_emits();
    let renamed = with(&profile, "name", json!("Renamed E61"));

    let result = fx.save(&renamed, Some("second")).await;

    assert_eq!(result, json!({"profile": renamed, "change_id": "second"}));
    assert_eq!(
        fx.get_json(&format!("/profile/get/{id}")).await["name"],
        "Renamed E61"
    );
    assert_eq!(fx.stored_files(), vec![format!("{id}.json")]);
    assert_eq!(
        fx.emitted(),
        vec![event(
            "profile",
            json!({"change": "update", "profile_id": id, "change_id": "second"})
        )]
    );
    let types: Vec<Value> = fx
        .get_json("/profile/changes")
        .await
        .as_array()
        .expect("list")
        .iter()
        .map(|c| c["type"].clone())
        .collect();
    assert_eq!(types, vec![json!("create"), json!("update")]);
    assert_eq!(fx.order(), json!([id]));
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_save_without_id_or_image_assigns_a_uuid_and_the_default_image
#[tokio::test]
async fn save_without_id_or_image_assigns_a_uuid_and_the_default_image() {
    let fx = Fx::new();
    let profile = without(&without(&example_profile(), "id"), "display");

    let result = fx.save(&profile, None).await;

    let saved = &result["profile"];
    assert!(is_uuid4(&saved["id"]), "{}", saved["id"]);
    assert!(is_uuid4(&result["change_id"]), "{}", result["change_id"]);
    assert_eq!(
        saved["display"],
        json!({
            "image": format!("/api/v1/profile/image/{}", default_image_name()),
            "accentColor": default_image_accent(),
        })
    );
    let expected = with(
        &with(&profile, "id", saved["id"].clone()),
        "display",
        saved["display"].clone(),
    );
    assert_eq!(*saved, expected);
    assert_eq!(
        fx.stored_files(),
        vec![format!("{}.json", saved["id"].as_str().expect("id"))]
    );
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_save_stores_a_data_uri_image_under_its_md5_and_serves_it
#[tokio::test]
async fn save_stores_a_data_uri_image_under_its_md5_and_serves_it() {
    let fx = Fx::new();
    let profile = with(
        &example_profile(),
        "display",
        json!({"image": png_data_uri(), "accentColor": "#123456"}),
    );
    let image_name = png_name();

    let saved = fx.save(&profile, None).await["profile"].clone();

    assert_eq!(
        saved["display"],
        json!({"image": format!("/api/v1/profile/image/{image_name}"), "accentColor": "#123456"})
    );
    let mut expected_images = vec![default_image_name(), image_name.clone()];
    expected_images.sort();
    assert_eq!(fx.stored_images(), expected_images);
    assert_eq!(
        std::fs::read(fx.images_dir().join(&image_name)).expect("image"),
        PNG_BYTES
    );
    let image = fx.get(&format!("/profile/image/{image_name}")).await;
    assert_eq!(image.status_code(), 200);
    assert_eq!(image.header("Content-Type"), Some("image/png"));
    assert_eq!(image.body(), PNG_BYTES);
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_save_rejects_a_data_uri_that_is_not_an_image
#[tokio::test]
async fn save_rejects_a_data_uri_that_is_not_an_image() {
    let fx = Fx::new();
    let mut profile = example_profile();
    profile["display"]["image"] = json!("data:text/plain;base64,eA==");

    let reply = fx.post_json("/profile/save", &profile).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "failed to save profile", "cause": "Invalid image MIME type"})
    );
    fx.assert_nothing_persisted();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_save_rejects_a_profile_missing_a_required_field
#[tokio::test]
async fn save_rejects_a_profile_missing_a_required_field() {
    let fx = Fx::new();
    let profile = without(&example_profile(), "stages");

    let reply = fx.post_json("/profile/save", &profile).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "JSON validation error: 'stages' is a required property"})
    );
    fx.assert_nothing_persisted();
    let get = fx
        .get(&format!(
            "/profile/get/{}",
            profile["id"].as_str().expect("id")
        ))
        .await;
    assert_eq!(get.status_code(), 404);
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_save_rejects_a_temperature_above_the_schema_maximum
#[tokio::test]
async fn save_rejects_a_temperature_above_the_schema_maximum() {
    let fx = Fx::new();
    let profile = with(&example_profile(), "temperature", json!(150));

    let reply = fx.post_json("/profile/save", &profile).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "JSON validation error: 150 is greater than the maximum of 100"})
    );
    fx.assert_nothing_persisted();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_save_rejects_a_profile_with_an_undefined_variable
#[tokio::test]
async fn save_rejects_a_profile_with_an_undefined_variable() {
    let fx = Fx::new();
    let mut profile = example_profile();
    profile["stages"][0]["dynamics"]["points"][0][1] = json!("$missing_var");

    let reply = fx.post_json("/profile/save", &profile).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "failed to save profile", "cause": "Variable missing_var is not defined"})
    );
    fx.assert_nothing_persisted();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_save_rejects_malformed_json
#[tokio::test]
async fn save_rejects_malformed_json() {
    let fx = Fx::new();

    let reply = fx.post("/profile/save", "{").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "failed to save profile", "cause": MALFORMED_JSON_ERROR})
    );
    fx.assert_nothing_persisted();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_save_rejects_non_finite_numbers
#[tokio::test]
async fn save_rejects_non_finite_numbers() {
    let fx = Fx::new();
    // `json.dumps` writes a NaN temperature as the bare token.
    let body = with(&example_profile(), "temperature", json!("@nan@"))
        .to_string()
        .replace("\"@nan@\"", "NaN");

    let reply = fx.post("/profile/save", body).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "failed to save profile", "cause": "Invalid number NaN"})
    );
    fx.assert_nothing_persisted();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_rejected_save_does_not_leave_its_uploaded_image_on_disk
///
/// A strict xfail: `save_profile` writes a data-URI image before validating.
/// Not a crash, so the port keeps it.
#[tokio::test]
async fn rejected_save_does_not_leave_its_uploaded_image_on_disk() {
    let fx = Fx::new();
    let mut profile = without(&example_profile(), "stages");
    profile["display"]["image"] = json!(png_data_uri());

    let reply = fx.post_json("/profile/save", &profile).await;

    assert_eq!(reply.status_code(), 400);
    // PARITY: the rejected profile's image stays on disk.
    let mut expected = vec![default_image_name(), png_name()];
    expected.sort();
    assert_eq!(fx.stored_images(), expected);
}

// --- get / list ------------------------------------------------------------

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_get_of_an_unknown_profile_is_404
#[tokio::test]
async fn get_of_an_unknown_profile_is_404() {
    let fx = Fx::new();

    let reply = fx.get(&format!("/profile/get/{UNKNOWN_ID}")).await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "profile not found", "id": UNKNOWN_ID})
    );
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_list_is_empty_without_profiles
#[tokio::test]
async fn list_is_empty_without_profiles() {
    let fx = Fx::new();
    assert_eq!(fx.get_json("/profile/list").await, json!([]));
    assert_eq!(fx.get_json("/profile/list?full=true").await, json!([]));
}

fn ids(listed: &Value) -> Vec<Value> {
    listed
        .as_array()
        .expect("list")
        .iter()
        .map(|p| p["id"].clone())
        .collect()
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_list_follows_the_configured_profile_order
#[tokio::test]
async fn list_follows_the_configured_profile_order() {
    let fx = Fx::new();
    let first = example_profile();
    let second = simple_profile();
    fx.save(&first, None).await;
    fx.save(&second, None).await;
    assert_eq!(fx.order(), json!([first["id"], second["id"]]));

    fx.set_order(json!([second["id"], first["id"]]));

    assert_eq!(
        ids(&fx.get_json("/profile/list").await),
        vec![second["id"].clone(), first["id"].clone()]
    );
    assert_eq!(
        fx.get_json("/profile/list?full=true").await,
        json!([second, first])
    );
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_list_appends_a_profile_missing_from_the_order_to_the_order
///
/// A strict xfail: Python appends the stale loop variable (the last id of the
/// order) instead of the missing profile's id. The port appends the profile's
/// own id (`Deliberate fix:` in `ProfileManager::list_profiles`), so the
/// Python test's expectation holds.
#[tokio::test]
async fn list_appends_a_profile_missing_from_the_order_to_the_order() {
    let fx = Fx::new();
    let first = example_profile();
    let second = simple_profile();
    fx.save(&first, None).await;
    fx.save(&second, None).await;
    // profile_order is client-writable through the settings API.
    fx.set_order(json!([first["id"]]));

    assert_eq!(
        ids(&fx.get_json("/profile/list").await),
        vec![first["id"].clone(), second["id"].clone()]
    );
    assert_eq!(fx.order(), json!([first["id"], second["id"]]));
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_list_with_an_empty_order_still_lists_the_profiles
///
/// A strict xfail: with an empty order Python reads the never-bound loop
/// variable and answers 500. The port fixes the crash.
#[tokio::test]
async fn list_with_an_empty_order_still_lists_the_profiles() {
    let fx = Fx::new();
    let profile = example_profile();
    fx.save(&profile, None).await;
    fx.set_order(json!([]));

    let reply = fx.get("/profile/list").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(ids(&body_json(&reply)), vec![profile["id"].clone()]);
    assert_eq!(fx.order(), json!([profile["id"]]));
}

// --- load ------------------------------------------------------------------

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_loading_a_saved_profile_streams_only_its_runtime_fields_to_the_esp32
#[tokio::test]
async fn loading_a_saved_profile_streams_only_its_runtime_fields_to_the_esp32() {
    let fx = Fx::new();
    let profile = example_profile();
    let id = profile["id"].as_str().expect("id").to_string();
    fx.save(&profile, None).await;
    fx.forget_emits();
    let before = now_seconds();

    let reply = fx.get(&format!("/profile/load/{id}")).await;

    let after = now_seconds();
    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body_json(&reply),
        json!({"name": profile["name"], "id": id})
    );
    let payload = expected_esp32_payload(&profile);
    assert_eq!(payload["stages"][1]["dynamics"]["points"], json!([[0, 8]]));
    assert_eq!(fx.uart(), expected_serial_bytes(&payload));
    assert_eq!(fx.app.debug_shots.node_json(), Some(payload));
    assert!(fx.app.machine.profile_ready());
    assert_eq!(
        fx.emitted(),
        vec![
            event("profile", json!({"change": "load", "profile_id": id})),
            event(
                "profileHover",
                json!({"id": id, "type": "focus", "from": "dial"})
            ),
        ]
    );

    let last = fx.last();
    assert_eq!(last["profile"], profile);
    let load_time = last["load_time"].as_f64().expect("load_time");
    assert!(before <= load_time && load_time <= after, "{load_time}");
    assert_eq!(fx.persisted_config()["profiles"]["LastProfile"], last);
    assert_eq!(fx.get_json("/profile/last").await, last);
    assert_eq!(fx.stored_files(), vec![format!("{id}.json")]);
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_loading_a_profile_body_streams_it_without_saving_it
#[tokio::test]
async fn loading_a_profile_body_streams_it_without_saving_it() {
    let fx = Fx::new();
    let profile = simple_profile();

    let reply = fx.post_json("/profile/load", &profile).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body_json(&reply),
        json!({"name": "Simple", "id": SIMPLE_PROFILE_ID})
    );
    let payload = expected_esp32_payload(&profile);
    assert_eq!(
        payload["stages"][0]["dynamics"]["points"],
        json!([[0, 10], [3, 10]])
    );
    assert_eq!(fx.uart(), expected_serial_bytes(&payload));
    assert_eq!(
        fx.emitted(),
        vec![
            event(
                "profile",
                json!({"change": "load", "profile_id": SIMPLE_PROFILE_ID})
            ),
            event(
                "profileHover",
                json!({"id": SIMPLE_PROFILE_ID, "type": "focus", "from": "dial"})
            ),
        ]
    );
    assert_eq!(fx.last()["profile"], profile);
    assert_eq!(fx.stored_files(), Vec::<String>::new());
    assert_eq!(fx.get_json("/profile/list").await, json!([]));
    assert_eq!(fx.app.profiles.changes(), Vec::<Value>::new());
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_loading_a_profile_body_without_an_id_assigns_one
#[tokio::test]
async fn loading_a_profile_body_without_an_id_assigns_one() {
    let fx = Fx::new();
    let profile = without(&example_profile(), "id");

    let reply = fx.post_json("/profile/load", &profile).await;

    assert_eq!(reply.status_code(), 200);
    let body = body_json(&reply);
    assert!(is_uuid4(&body["id"]), "{}", body["id"]);
    assert_eq!(body, json!({"name": profile["name"], "id": body["id"]}));
    assert_eq!(
        fx.uart(),
        expected_serial_bytes(&expected_esp32_payload(&profile))
    );
    assert_eq!(
        fx.emitted()[0],
        event(
            "profile",
            json!({"change": "load", "profile_id": body["id"]})
        )
    );
    assert_eq!(fx.last()["profile"]["id"], body["id"]);
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_loading_while_the_machine_is_busy_is_refused
#[tokio::test]
async fn loading_while_the_machine_is_busy_is_refused() {
    let fx = Fx::new();
    let profile = example_profile();
    fx.save(&profile, None).await;
    fx.forget_emits();
    fx.set_idle(false);

    let by_id = fx
        .get(&format!(
            "/profile/load/{}",
            profile["id"].as_str().expect("id")
        ))
        .await;
    let by_body = fx.post_json("/profile/load", &profile).await;

    assert_eq!(
        (by_id.status_code(), body_json(&by_id)),
        (409, machine_busy())
    );
    assert_eq!(
        (by_body.status_code(), body_json(&by_body)),
        (409, machine_busy())
    );
    fx.assert_nothing_sent();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_loading_an_unknown_profile_id_is_404
#[tokio::test]
async fn loading_an_unknown_profile_id_is_404() {
    let fx = Fx::new();

    let reply = fx.get(&format!("/profile/load/{UNKNOWN_ID}")).await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "profile not found", "id": UNKNOWN_ID})
    );
    fx.assert_nothing_sent();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_loading_a_schema_invalid_profile_body_is_rejected_before_the_esp32
#[tokio::test]
async fn loading_a_schema_invalid_profile_body_is_rejected_before_the_esp32() {
    let fx = Fx::new();
    let profile = with(&example_profile(), "temperature", json!(150));

    let reply = fx.post_json("/profile/load", &profile).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "JSON validation error: 150 is greater than the maximum of 100"})
    );
    fx.assert_nothing_sent();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_loading_a_profile_body_with_an_undefined_variable_is_rejected
#[tokio::test]
async fn loading_a_profile_body_with_an_undefined_variable_is_rejected() {
    let fx = Fx::new();
    let mut profile = example_profile();
    profile["stages"][0]["dynamics"]["points"][0][1] = json!("$missing_var");

    let reply = fx.post_json("/profile/load", &profile).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "variable error: UndefinedVariableException:Variable missing_var is not defined"})
    );
    fx.assert_nothing_sent();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_loading_malformed_json_is_rejected
#[tokio::test]
async fn loading_malformed_json_is_rejected() {
    let fx = Fx::new();

    let reply = fx.post("/profile/load", "{").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": format!("Invalid JSON: {MALFORMED_JSON_ERROR}")})
    );
    fx.assert_nothing_sent();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_loading_a_profile_body_while_the_motor_is_stressed_is_refused
#[tokio::test]
async fn loading_a_profile_body_while_the_motor_is_stressed_is_refused() {
    let fx = Fx::new();
    fx.stress_motor();

    let reply = fx.post_json("/profile/load", &example_profile()).await;

    assert_eq!(reply.status_code(), 403);
    assert_eq!(body_json(&reply), high_strain());
    assert_eq!(
        fx.notifications(),
        vec![(
            MOTOR_STRESSED_MESSAGE.to_string(),
            Some(crate::images::WARNING_TRIANGLE_IMAGE.to_string())
        )]
    );
    fx.assert_nothing_sent();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_loading_a_saved_profile_while_the_motor_is_stressed_is_refused
///
/// A strict xfail: `load_profile_and_send` drops the refusal, so the load by
/// id answers 200 although nothing was sent. Not a crash, so the port keeps it.
#[tokio::test]
async fn loading_a_saved_profile_while_the_motor_is_stressed_is_refused() {
    let fx = Fx::new();
    let profile = example_profile();
    let id = profile["id"].as_str().expect("id").to_string();
    fx.save(&profile, None).await;
    fx.stress_motor();

    let reply = fx.get(&format!("/profile/load/{id}")).await;

    assert_eq!(fx.uart(), Vec::<u8>::new());
    // PARITY: 200 with the profile, where the Python test expects 403.
    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body_json(&reply),
        json!({"name": profile["name"], "id": id})
    );
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_get_on_the_load_route_without_an_id_is_method_not_allowed
///
/// A strict xfail: Python routes GET /profile/load to `get(self, profile_id)`
/// without an id and answers 500. The port serves only POST there.
#[tokio::test]
async fn get_on_the_load_route_without_an_id_is_method_not_allowed() {
    let fx = Fx::new();

    let reply = fx.get("/profile/load").await;

    assert_eq!(reply.status_code(), 405);
    assert_eq!(fx.uart(), Vec::<u8>::new());
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_loading_a_schema_valid_profile_body_without_display
///
/// A strict xfail: `handle_image` indexes `data["display"]`, which only
/// `save_profile` adds. The KeyError is caught and answered as a 400, not a
/// crash, so the port keeps it.
#[tokio::test]
async fn loading_a_schema_valid_profile_body_without_display() {
    let fx = Fx::new();
    let profile = without(&example_profile(), "display");

    let reply = fx.post_json("/profile/load", &profile).await;

    // PARITY: refused, where the Python test expects the profile loaded.
    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "failed to load profile 'display'"})
    );
    fx.assert_nothing_sent();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_limited_access_loads_the_bundled_simple_profile_by_id
#[tokio::test]
async fn limited_access_loads_the_bundled_simple_profile_by_id() {
    let fx = Fx::with_config(|app| {
        app.config.set(
            keys::USER,
            keys::UPDATE_CHANNEL,
            json!(crate::limited_access::LIMITED_ACCESS_CHANNEL),
        );
    });
    let profile = simple_profile();

    let reply = fx.get(&format!("/profile/load/{SIMPLE_PROFILE_ID}")).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body_json(&reply),
        json!({"name": "Simple", "id": SIMPLE_PROFILE_ID})
    );
    assert_eq!(
        fx.uart(),
        expected_serial_bytes(&expected_esp32_payload(&profile))
    );
    assert_eq!(
        fx.emitted()[0],
        event(
            "profile",
            json!({"change": "load", "profile_id": SIMPLE_PROFILE_ID})
        )
    );
    assert_eq!(fx.app.profiles.known_count(), 0);
}

// --- legacy ----------------------------------------------------------------

fn legacy_document() -> Value {
    json!({
        "id": "legacy-1",
        "name": "Legacy JSON",
        "author": "someone",
        "stages": [{"name": "initialize", "nodes": []}],
    })
}

/// `LEGACY_DUMMY_PROFILE` (`api/emulation.py`).
fn legacy_dummy_profile() -> Value {
    json!({
        "id": "0eddc1d9-aa2e-404b-a209-e7e76c9552bc",
        "name": "LEGACY JSON PLACEHOLDER",
        "author": "",
        "author_id": "00000000-0000-0000-0000-000000000000",
        "previous_authors": [{"name": "", "author_id": "", "profile_id": ""}],
        "temperature": 88,
        "final_weight": 36,
        "display": {
            "image": "/api/v1/profile/image/f98ff6e029e70ee9a99565796615d55c.png",
            "accentColor": "#42a59d",
        },
        "variables": [],
        "last_changed": 1723127046.5605135,
        "stages": [
            {
                "key": "206c654b-5766-4ebb-a4bc-dcafa5aacd8b",
                "name": "EMPTY FLOW STAGE",
                "type": "flow",
                "dynamics": {"points": [[0, 0]], "over": "time", "interpolation": "linear"},
                "exit_triggers": [],
                "limits": [],
            }
        ],
    })
}

fn allow_legacy_json(app: &App) {
    app.config
        .set(keys::USER, keys::ALLOW_LEGACY_JSON, json!(true));
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_legacy_route_is_404_unless_legacy_json_is_allowed
#[tokio::test]
async fn legacy_route_is_404_unless_legacy_json_is_allowed() {
    let fx = Fx::new();

    let reply = fx.post_json("/profile/legacy", &legacy_document()).await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(reply.body(), b"");
    fx.assert_nothing_sent();
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_legacy_route_streams_the_raw_document_and_records_the_placeholder
#[tokio::test]
async fn legacy_route_streams_the_raw_document_and_records_the_placeholder() {
    let fx = Fx::with_config(allow_legacy_json);
    let document = legacy_document();

    let reply = fx.post_json("/profile/legacy", &document).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body_json(&reply),
        json!({"name": "Legacy JSON", "id": "legacy-1"})
    );
    // The legacy route sends the whole document, metadata included.
    assert_eq!(fx.uart(), expected_serial_bytes(&document));
    assert_eq!(fx.app.debug_shots.node_json(), Some(document));
    assert_eq!(fx.last()["profile"], legacy_dummy_profile());
    assert_eq!(fx.emitted(), vec![]);
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_legacy_route_while_the_machine_is_busy_is_refused
#[tokio::test]
async fn legacy_route_while_the_machine_is_busy_is_refused() {
    let fx = Fx::with_config(allow_legacy_json);
    fx.set_idle(false);

    let reply = fx.post_json("/profile/legacy", &legacy_document()).await;

    assert_eq!(
        (reply.status_code(), body_json(&reply)),
        (409, machine_busy())
    );
    fx.assert_nothing_sent();
}

// --- delete ----------------------------------------------------------------

fn change_triples(changes: &Value) -> Vec<(Value, Value, Value)> {
    changes
        .as_array()
        .expect("list")
        .iter()
        .map(|c| {
            (
                c["type"].clone(),
                c["profile_id"].clone(),
                c["change_id"].clone(),
            )
        })
        .collect()
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_deleting_a_profile_removes_it_from_disk_list_order_and_selection
#[tokio::test]
async fn deleting_a_profile_removes_it_from_disk_list_order_and_selection() {
    let fx = Fx::new();
    let profile = example_profile();
    let id = profile["id"].as_str().expect("id").to_string();
    fx.save(&profile, Some("create-1")).await;
    fx.forget_emits();

    let reply = fx.delete(&format!("/profile/delete/{id}"), &[]).await;

    assert_eq!(reply.status_code(), 200);
    let result = body_json(&reply);
    assert_eq!(result["profile"], profile);
    assert!(is_uuid4(&result["change_id"]), "{}", result["change_id"]);
    assert_eq!(fx.stored_files(), Vec::<String>::new());
    assert_eq!(fx.get_json("/profile/list").await, json!([]));
    assert_eq!(
        fx.get(&format!("/profile/get/{id}")).await.status_code(),
        404
    );
    assert_eq!(fx.order(), json!([]));
    assert_eq!(fx.persisted_config()["user"]["profile_order"], json!([]));

    let emitted = fx.emitted();
    assert_eq!(emitted.len(), 1);
    let (name, payload) = &emitted[0];
    assert_eq!(name, "profile");
    assert_eq!(
        (&payload["change"], &payload["profile_id"]),
        (&json!("delete"), &json!(id))
    );

    let changes = fx.get_json("/profile/changes").await;
    assert_eq!(
        change_triples(&changes),
        vec![
            (json!("create"), json!(id), json!("create-1")),
            (json!("delete"), json!(id), result["change_id"].clone()),
        ]
    );
    // The hover is cleared silently: the dial moves its own carousel on delete.
    assert_eq!(fx.get("/profile/selected").await.status_code(), 204);
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_get_on_the_delete_route_also_deletes
#[tokio::test]
async fn get_on_the_delete_route_also_deletes() {
    let fx = Fx::new();
    let profile = example_profile();
    fx.save(&profile, None).await;

    let reply = fx
        .get(&format!(
            "/profile/delete/{}",
            profile["id"].as_str().expect("id")
        ))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(body_json(&reply)["profile"], profile);
    assert_eq!(fx.stored_files(), Vec::<String>::new());
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_deleting_an_unknown_profile_is_404
#[tokio::test]
async fn deleting_an_unknown_profile_is_404() {
    let fx = Fx::new();

    let reply = fx
        .delete(&format!("/profile/delete/{UNKNOWN_ID}"), &[])
        .await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "profile not found", "id": UNKNOWN_ID})
    );
    assert_eq!(fx.emitted(), vec![]);
    assert_eq!(fx.app.profiles.changes(), Vec::<Value>::new());
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_deleting_a_profile_removes_an_image_it_uploaded_before_boot
#[tokio::test]
async fn deleting_a_profile_removes_an_image_it_uploaded_before_boot() {
    let fx = Fx::new();
    let image_name = png_name();
    std::fs::write(fx.images_dir().join(&image_name), PNG_BYTES).expect("write image");
    let mut profile = simple_profile();
    profile["display"]["image"] = json!(format!("/api/v1/profile/image/{image_name}"));
    std::fs::write(
        fx.profiles_dir().join(format!("{SIMPLE_PROFILE_ID}.json")),
        profile.to_string(),
    )
    .expect("write profile");
    fx.app.profiles.refresh_image_list();
    fx.app.profiles.refresh_profile_list(&fx.app);
    assert_eq!(
        fx.get_json(&format!("/profile/get/{SIMPLE_PROFILE_ID}"))
            .await,
        profile
    );

    let reply = fx
        .delete(&format!("/profile/delete/{SIMPLE_PROFILE_ID}"), &[])
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(fx.stored_images(), vec![default_image_name()]);
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_deleting_a_profile_removes_an_image_it_uploaded_in_this_session
///
/// A strict xfail: the image collection only knows the images of the last
/// image refresh, which a new upload does not trigger. Not a crash, so the
/// port keeps it.
#[tokio::test]
async fn deleting_a_profile_removes_an_image_it_uploaded_in_this_session() {
    let fx = Fx::new();
    let mut profile = example_profile();
    profile["display"]["image"] = json!(png_data_uri());
    fx.save(&profile, None).await;

    let reply = fx
        .delete(
            &format!("/profile/delete/{}", profile["id"].as_str().expect("id")),
            &[],
        )
        .await;

    assert_eq!(reply.status_code(), 200);
    // PARITY: the uploaded image outlives its profile.
    let mut expected = vec![default_image_name(), png_name()];
    expected.sort();
    assert_eq!(fx.stored_images(), expected);
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_delete_records_the_client_change_id
///
/// A strict xfail: `delete_profile` passes the client's change id as the
/// timestamp of the change. Not a crash, so the port keeps it.
#[tokio::test]
async fn delete_records_the_client_change_id() {
    let fx = Fx::new();
    let profile = example_profile();
    let id = profile["id"].as_str().expect("id").to_string();
    fx.save(&profile, None).await;

    let reply = fx
        .delete(
            &format!("/profile/delete/{id}"),
            &[("X-Change-Id", "client-delete-1")],
        )
        .await;

    let change_id = body_json(&reply)["change_id"].clone();
    // PARITY: a fresh change id is returned and recorded, and the client's
    // id becomes the change's timestamp.
    assert!(is_uuid4(&change_id), "{change_id}");
    let changes = fx.get_json("/profile/changes").await;
    let delete_change = changes.as_array().expect("list").last().expect("change");
    assert_eq!(
        *delete_change,
        json!({
            "type": "delete",
            "profile_id": id,
            "change_id": change_id,
            "timestamp": "client-delete-1",
        })
    );
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_delete_event_carries_the_change_id
///
/// A strict xfail: the delete event is emitted without its change id. Not a
/// crash, so the port keeps it.
#[tokio::test]
async fn delete_event_carries_the_change_id() {
    let fx = Fx::new();
    let profile = example_profile();
    let id = profile["id"].as_str().expect("id").to_string();
    fx.save(&profile, None).await;
    fx.forget_emits();

    let reply = fx.delete(&format!("/profile/delete/{id}"), &[]).await;

    assert_eq!(reply.status_code(), 200);
    // PARITY: no change_id in the event.
    assert_eq!(
        fx.emitted(),
        vec![event(
            "profile",
            json!({"change": "delete", "profile_id": id})
        )]
    );
}

// --- changes ---------------------------------------------------------------

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_changes_is_empty_before_any_change
#[tokio::test]
async fn changes_is_empty_before_any_change() {
    let fx = Fx::new();
    assert_eq!(fx.get_json("/profile/changes").await, json!([]));
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_changes_lists_every_change_in_order_with_the_client_change_ids
#[tokio::test]
async fn changes_lists_every_change_in_order_with_the_client_change_ids() {
    let fx = Fx::new();
    let first = example_profile();
    let second = simple_profile();
    fx.save(&first, Some("c1")).await;
    fx.save(&second, Some("c2")).await;
    fx.save(&with(&first, "name", json!("Edited")), Some("c3"))
        .await;
    let delete = body_json(
        &fx.delete(&format!("/profile/delete/{SIMPLE_PROFILE_ID}"), &[])
            .await,
    );

    let changes = fx.get_json("/profile/changes").await;

    assert_eq!(
        change_triples(&changes),
        vec![
            (json!("create"), first["id"].clone(), json!("c1")),
            (json!("create"), second["id"].clone(), json!("c2")),
            (json!("update"), first["id"].clone(), json!("c3")),
            (
                json!("delete"),
                second["id"].clone(),
                delete["change_id"].clone()
            ),
        ]
    );
    let timestamps: Vec<f64> = changes
        .as_array()
        .expect("list")
        .iter()
        .map(|c| c["timestamp"].as_f64().expect("timestamp"))
        .collect();
    let mut sorted = timestamps.clone();
    sorted.sort_by(f64::total_cmp);
    assert_eq!(timestamps, sorted);
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_changes_keeps_only_the_latest_hundred
#[tokio::test]
async fn changes_keeps_only_the_latest_hundred() {
    let fx = Fx::new();
    let profile = example_profile();
    for index in 0..101 {
        fx.save(&profile, Some(&format!("change-{index}"))).await;
    }

    let changes = fx.get_json("/profile/changes").await;

    let changes = changes.as_array().expect("list");
    let change_ids: Vec<Value> = changes.iter().map(|c| c["change_id"].clone()).collect();
    let expected: Vec<Value> = (1..101).map(|i| json!(format!("change-{i}"))).collect();
    assert_eq!(change_ids, expected);
    assert!(changes.iter().all(|c| c["type"] == "update"));
}

// --- last / selected -------------------------------------------------------

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_last_is_204_before_any_load
#[tokio::test]
async fn last_is_204_before_any_load() {
    let fx = Fx::new();

    let reply = fx.get("/profile/last").await;

    assert_eq!(reply.status_code(), 204);
    assert_eq!(reply.body(), b"");
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_last_returns_the_recorded_profile_with_its_load_time_as_last_modified
#[tokio::test]
async fn last_returns_the_recorded_profile_with_its_load_time_as_last_modified() {
    let fx = Fx::new();
    let last = json!({"load_time": 1700000000.0, "profile": example_profile()});
    fx.app
        .config
        .set(keys::PROFILES, keys::PROFILE_LAST, last.clone());

    let reply = fx.get("/profile/last").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        reply.header("Last-Modified"),
        Some("Tue, 14 Nov 2023 22:13:20 GMT")
    );
    assert_eq!(body_json(&reply), last);
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_last_without_a_load_time_has_no_last_modified_header
#[tokio::test]
async fn last_without_a_load_time_has_no_last_modified_header() {
    let fx = Fx::new();
    let last = json!({"load_time": null, "profile": example_profile()});
    fx.app
        .config
        .set(keys::PROFILES, keys::PROFILE_LAST, last.clone());

    let reply = fx.get("/profile/last").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.header("Last-Modified"), None);
    assert_eq!(body_json(&reply), last);
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_selected_is_204_without_a_hovered_profile
#[tokio::test]
async fn selected_is_204_without_a_hovered_profile() {
    let fx = Fx::new();

    let reply = fx.get("/profile/selected").await;

    assert_eq!(reply.status_code(), 204);
    assert_eq!(reply.body(), b"");
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_selected_returns_the_hovered_profile
#[tokio::test]
async fn selected_returns_the_hovered_profile() {
    let fx = Fx::new();
    fx.app.profiles.set_hover(Hover {
        id: UNKNOWN_ID.into(),
        kind: "hover".into(),
        from: "app".into(),
    });

    assert_eq!(
        fx.get_json("/profile/selected").await,
        json!({"id": UNKNOWN_ID, "type": "hover", "from": "app"})
    );
}

// --- images / defaults -----------------------------------------------------

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_image_list_returns_the_default_images
#[tokio::test]
async fn image_list_returns_the_default_images() {
    let fx = Fx::new();
    assert_eq!(
        fx.get_json("/profile/image").await,
        json!([default_image_name()])
    );
    assert_eq!(
        fx.get_json("/profile/image/").await,
        json!([default_image_name()])
    );
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_image_route_serves_a_stored_image_and_404s_a_missing_one
#[tokio::test]
async fn image_route_serves_a_stored_image_and_404s_a_missing_one() {
    let fx = Fx::new();

    let image = fx
        .get(&format!("/profile/image/{}", default_image_name()))
        .await;

    assert_eq!(image.status_code(), 200);
    assert_eq!(image.header("Content-Type"), Some("image/png"));
    assert_eq!(
        image.body(),
        std::fs::read(default_image_source()).expect("default image")
    );
    assert_eq!(
        fx.get("/profile/image/missing.png").await.status_code(),
        404
    );
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_defaults_returns_the_bundled_simple_profile
#[tokio::test]
async fn defaults_returns_the_bundled_simple_profile() {
    let fx = Fx::new();
    assert_eq!(
        fx.get_json("/profile/defaults").await,
        json!({"default": [simple_profile()], "community": []})
    );
}

// --- from_manual -----------------------------------------------------------

/// A shot as `ShotManager.stop()` stores it: the file, then the history row.
fn record_shot(app: &App, id: &str, time: f64, profile: Value, data: Value) {
    let db = app.history.db().expect("history database");
    let (_, file) = history::files::shot_file_paths(db.zone(), time).expect("shot file paths");
    let mut shot = json!({
        "time": time,
        "profile_name": profile["name"],
        "data": data,
        "id": id,
        "push_to_brew_time": 0,
        "profile": profile,
    });
    history::files::write_shot_file(&db.paths().shot_dir().join(&file), &shot)
        .expect("write shot file");
    shot["file"] = json!(file);
    db.insert_history(&shot).expect("insert history");
}

/// tests/test_api_profiles.py::TestEspressoProfileRoutes::test_profile_built_from_a_manual_brew_is_saved_like_any_other_profile
#[tokio::test]
async fn profile_built_from_a_manual_brew_is_saved_like_any_other_profile() {
    let fx = Fx::new();
    let data: Vec<Value> = [(1000, 6.0, 0.0), (5000, 9.0, 36.0)]
        .into_iter()
        .map(|(profile_time, pressure, weight)| {
            json!({
                "shot": {
                    "pressure": pressure,
                    "flow": 1.2,
                    "weight": weight,
                    "setpoints": {"active": "pressure", "pressure": pressure},
                },
                "time": profile_time,
                "profile_time": profile_time,
                "status": "Manual",
            })
        })
        .collect();
    let mut manual = profile::build_manual_mode_profile();
    manual["temperature"] = json!(93.5);
    manual["last_changed"] = json!(1757193000.0);
    record_shot(
        &fx.app,
        "8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6",
        1757193600.0,
        manual,
        json!(data),
    );
    // A later brew of another profile: the query asks for the Manual mode
    // profile's shots only (`ids == [MANUAL_MODE_PROFILE_ID]`).
    let mut other = example_profile();
    other["last_changed"] = json!(1757193000.0);
    record_shot(
        &fx.app,
        "9a14e45f-ceea-467a-9a3c-a1b2c3d4e5f7",
        1757197200.0,
        other,
        json!([]),
    );
    assert_eq!(
        crate::manual_mode::find_manual_shot(&fx.app, None).expect("manual shot")["profile"]["id"],
        MANUAL_MODE_PROFILE_ID
    );

    let reply = fx
        .post_with(
            "/profile/from_manual",
            json!({"name": "Morning manual"}).to_string(),
            &[("X-Change-Id", "m-1")],
        )
        .await;

    assert_eq!(
        reply.status_code(),
        200,
        "{}",
        String::from_utf8_lossy(reply.body())
    );
    let result = body_json(&reply);
    let saved = &result["profile"];
    let id = saved["id"].as_str().expect("id").to_string();
    assert_eq!(result["change_id"], "m-1");
    assert_eq!(saved["name"], "Morning manual");
    assert_eq!(saved["temperature"], 93.5);
    assert!(saved.get("manual").is_none());
    assert_eq!(fx.stored_profile(&id), *saved);
    assert_eq!(fx.get_json(&format!("/profile/get/{id}")).await, *saved);
    assert_eq!(ids(&fx.get_json("/profile/list").await), vec![json!(id)]);
    assert_eq!(
        fx.emitted()[0],
        event(
            "profile",
            json!({"change": "create", "profile_id": id, "change_id": "m-1"})
        )
    );
}

// --- tests/test_simple_profile.py ------------------------------------------

/// tests/test_simple_profile.py::test_defaults_have_only_the_simple_profile
#[test]
fn defaults_have_only_the_simple_profile() {
    let simple = SimpleProfile::new(repo().join("simple_profile.json"));
    let defaults = simple.defaults();
    let default_ids: Vec<Value> = defaults["default"]
        .as_array()
        .expect("list")
        .iter()
        .map(|p| p["id"].clone())
        .collect();
    assert_eq!(default_ids, vec![json!(SIMPLE_PROFILE_ID)]);
    assert_eq!(defaults["community"], json!([]));
}

/// tests/test_simple_profile.py::test_missing_file_is_empty_not_fatal
#[test]
fn missing_file_is_empty_not_fatal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let simple = SimpleProfile::new(dir.path().join("missing.json"));
    assert_eq!(simple.get(), None);
    assert_eq!(simple.list(true), Vec::<Value>::new());
    assert_eq!(simple.defaults(), json!({"default": [], "community": []}));
}

// --- tests/test_profile_uart_payload.py ------------------------------------

/// `adaptive_profile()` of the Python test: a community profile with a
/// 296,904-character embedded image and 10,000 characters of metadata the
/// machine never needs.
fn adaptive_profile() -> Value {
    json!({
        "id": "a188c420-4952-4b33-8489-a082e9500ecb",
        "name": "Adaptive (Gagn\u{e9} Method)",
        "author": "Corb3t",
        "author_id": "550e8400-e29b-41d4-a716-446655449999",
        "version": 9,
        "last_changed": 1776608894.8666232,
        "image": format!("data:image/png;base64,{}", "A".repeat(296_904)),
        "display": {
            "image": "/api/v1/profile/image/ae3e6ef14875bad88cad00f09945e681.png",
            "graphs": [{"x": "time", "y": "flow", "label": "Flow"}],
            "description": "Display-only recipe guidance",
        },
        "temperature": 90,
        "final_weight": 36,
        "variables": [
            {"key": "pressure_PeakLimit", "name": "Peak Pressure Limit", "type": "pressure", "value": 8.6},
            {"key": "flow_TargetExtract", "name": "Target Extraction Flow", "type": "flow", "value": 4},
            {"key": "pressure_PILimit", "name": "Pre-infusion Pressure Limit", "type": "pressure", "value": 4},
            {"key": "time_FillPause", "name": "Fill Pause Duration", "type": "time", "value": 4},
        ],
        "stages": [
            {
                "key": "fast_fill_1",
                "name": "Fast Fill",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PILimit"}],
                "dynamics": {"over": "time", "points": [[0, 8]], "interpolation": "linear"},
                "exit_triggers": [
                    {"type": "pressure", "value": "$pressure_PILimit", "relative": false, "comparison": ">="}
                ],
            },
            {
                "key": "fill_pause_2",
                "name": "Fill Pause",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PILimit"}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, 0.5], ["$time_FillPause", 0.5]],
                    "interpolation": "linear",
                },
                "exit_triggers": [
                    {"type": "time", "value": "$time_FillPause", "relative": true, "comparison": ">="}
                ],
            },
            {
                "key": "pressure_ramp_3",
                "name": "Pressure Ramp",
                "type": "pressure",
                "limits": [{"type": "flow", "value": 6}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, "$pressure_PILimit"], [4, "$pressure_PeakLimit"]],
                    "interpolation": "linear",
                },
                "exit_triggers": [
                    {"type": "time", "value": 4, "relative": true, "comparison": ">="}
                ],
            },
            {
                "key": "adaptive_extraction_4",
                "name": "Adaptive Extraction",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PeakLimit"}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, "$flow_TargetExtract"], [35, 0.5]],
                    "interpolation": "linear",
                },
                "exit_triggers": [
                    {"type": "time", "value": 45, "relative": true, "comparison": ">="}
                ],
            },
        ],
        "community": {"downloads": 123, "featured": true},
        "unknown_future_metadata": {"large": "B".repeat(10_000)},
    })
}

/// The JSON frame of a profile upload (`json\n<payload>\x03`).
fn uploaded_payloads(bytes: &[u8]) -> Vec<Value> {
    let text = String::from_utf8(bytes.to_vec()).expect("UTF-8 frames");
    text.split('\x03')
        .filter_map(|frame| frame.strip_prefix("json\n"))
        .map(|json| serde_json::from_str(json).expect("payload JSON"))
        .collect()
}

/// tests/test_profile_uart_payload.py::test_send_preserves_alarm_guard_profile_state_and_load_event
#[tokio::test]
async fn send_preserves_alarm_guard_profile_state_and_load_event() {
    let fx = Fx::new();
    let profile = adaptive_profile();
    let app = fx.app.clone();

    let result = app
        .profiles
        .send_profile_to_esp32(&app, profile.clone())
        .expect("sent");

    assert_eq!(result, Some(profile.clone()));
    assert_eq!(fx.last()["profile"], profile);
    let profile_events: Vec<(String, Value)> = fx
        .emitted()
        .into_iter()
        .filter(|(name, _)| name == "profile")
        .collect();
    assert_eq!(
        profile_events,
        vec![event(
            "profile",
            json!({"change": "load", "profile_id": profile["id"]})
        )]
    );
    let sent = uploaded_payloads(&fx.uart());
    assert_eq!(sent.len(), 1);
    let fields: Vec<&str> = sent[0]
        .as_object()
        .expect("payload")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(fields, ESP32_PROFILE_FIELDS);
    assert!(pyfmt::dumps_default(&sent[0]).len() < 5_000);

    fx.stress_motor();

    assert_eq!(
        app.profiles
            .send_profile_to_esp32(&app, profile.clone())
            .expect("refused"),
        None
    );
    assert_eq!(uploaded_payloads(&fx.uart()).len(), 1);
}
