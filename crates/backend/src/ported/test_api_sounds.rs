//! Port of `tests/test_api_sounds.py`.
//!
//! As in the Python fixture, the system sounds are a temporary copy of the
//! repository's `sounds/default` theme and the user sounds an empty
//! temporary folder. The hardware hooks are off so a sound really plays: the
//! host commands go to a recording runner, which stands in for the Python's
//! mocked `playsound` (`gst-play-1.0`, what playsound3 runs for
//! `playsound(path, block=False)`, started without waiting and stopped with
//! the handle it returns) and `subprocess` (`pactl`).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use http::Method;
use serde_json::{Map, Value, json};

use crate::app::App;
use crate::config::keys;
use crate::env::BackendMode;
use crate::host::fake::FakeRunner;
use crate::testing::{self, call, call_with_headers};

const BOUNDARY: &str = "gate-boundary-7f3a";

struct Sounds {
    _dir: tempfile::TempDir,
    app: Arc<App>,
    runner: Arc<FakeRunner>,
    system: PathBuf,
    user: PathBuf,
    /// `MeticulousConfig.save` calls since the setup (the Python's
    /// `config_save` mock, reset after `SoundPlayer.init`).
    saves: Arc<AtomicUsize>,
}

/// `SoundsApiTestCase.setUp`.
fn sounds() -> Sounds {
    setup(true)
}

fn setup(init: bool) -> Sounds {
    let (dir, app) = testing::app_custom(BackendMode::Emulator, |env| {
        env.hooks.fake_hardware = false;
        let repo_theme = env.paths.system_sounds.join("default");
        env.paths.system_sounds = env.paths.user_sounds.with_file_name("system-sounds");
        copy_dir(&repo_theme, &env.paths.system_sounds.join("default"));
        std::fs::create_dir_all(&env.paths.user_sounds).expect("user sounds");
    });
    let runner = Arc::new(FakeRunner::default());
    app.host.replace(runner.clone());
    app.config
        .set(keys::SYSTEM, keys::SOUNDS_THEME, json!("default"));
    app.config
        .set(keys::USER, keys::SOUNDS_ENABLED, json!(true));
    if init {
        app.sounds.init(true, false);
    }
    let saves = Arc::new(AtomicUsize::new(0));
    let counter = saves.clone();
    app.config.on_save(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    Sounds {
        system: app.env.paths.system_sounds.clone(),
        user: app.env.paths.user_sounds.clone(),
        _dir: dir,
        app,
        runner,
        saves,
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("theme folder");
    for entry in std::fs::read_dir(from).expect("repository theme") {
        let entry = entry.expect("entry");
        std::fs::copy(entry.path(), to.join(entry.file_name())).expect("copy");
    }
}

impl Sounds {
    /// `add_user_theme`: a theme folder in the user sounds, then
    /// `SoundPlayer.init` again.
    fn add_user_theme(&self, name: &str, config: Value, files: &[&str]) -> PathBuf {
        let folder = self.user.join(name);
        std::fs::create_dir(&folder).expect("theme folder");
        std::fs::write(folder.join("config.json"), config.to_string()).expect("config");
        for file in files {
            std::fs::write(folder.join(file), b"ID3").expect("sound");
        }
        self.app.sounds.init(true, false);
        self.saves.store(0, Ordering::SeqCst);
        folder
    }

    async fn get(&self, uri: &str) -> crate::http::Reply {
        call(&self.app, Method::GET, uri, "").await
    }

    fn started(&self) -> Vec<Vec<String>> {
        self.runner.started.lock().clone()
    }

    fn stopped(&self) -> Vec<Vec<String>> {
        self.runner.stopped.lock().clone()
    }

    fn saves(&self) -> usize {
        self.saves.load(Ordering::SeqCst)
    }

    /// The theme as saved in config.yml.
    fn saved_theme(&self) -> Value {
        let text = std::fs::read_to_string(self.app.config.path()).expect("config.yml");
        let saved: Value = serde_norway::from_str(&text).expect("yaml");
        saved[keys::SYSTEM][keys::SOUNDS_THEME].clone()
    }

    async fn upload(&self, content: &[u8], field: &str) -> crate::http::Reply {
        let (body, content_type) = multipart_body(field, "theme.zip", content);
        call_with_headers(
            &self.app,
            Method::POST,
            "/api/v1/sounds/theme/upload",
            body,
            &[("Content-Type", &content_type)],
        )
        .await
    }

    fn assert_nothing_extracted(&self) {
        let entries: Vec<_> = std::fs::read_dir(&self.user)
            .expect("user sounds")
            .collect();
        assert!(entries.is_empty(), "{entries:?}");
    }
}

/// What playing `path` runs.
fn played(path: &Path) -> Vec<String> {
    ["gst-play-1.0", "--no-interactive", "--quiet"]
        .into_iter()
        .map(str::to_string)
        .chain([path.to_string_lossy().into_owned()])
        .collect()
}

/// `DEFAULT_THEME_SOUNDS`: the repository's default theme config.
fn default_theme_sounds() -> Map<String, Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sounds/default/config.json");
    let text = std::fs::read_to_string(path).expect("default theme config");
    match serde_json::from_str(&text).expect("json") {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

fn default_theme_keys() -> Vec<Value> {
    default_theme_sounds()
        .keys()
        .map(|k| Value::String(k.clone()))
        .collect()
}

fn multipart_body(field: &str, filename: &str, content: &[u8]) -> (Vec<u8>, String) {
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\nContent-Type: application/zip\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    (body, format!("multipart/form-data; boundary={BOUNDARY}"))
}

/// `zip_bytes`: Python's `ZipFile.writestr` stores entries uncompressed.
fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in entries {
        writer.start_file(*name, options).expect("zip entry");
        writer.write_all(content).expect("zip content");
    }
    writer.finish().expect("zip").into_inner()
}

fn body_text(reply: &crate::http::Reply) -> &str {
    std::str::from_utf8(reply.body()).expect("utf-8 body")
}

// TestPlaySoundRoute

/// tests/test_api_sounds.py::TestPlaySoundRoute::test_theme_sound_is_played_non_blocking_from_the_theme_folder
#[tokio::test]
async fn theme_sound_is_played_non_blocking_from_the_theme_folder() {
    let s = sounds();

    let reply = s.get("/api/v1/sounds/play/notification").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(testing::json(&reply), json!({"status": "okay"}));
    assert_eq!(
        s.started(),
        vec![played(&s.system.join("default/notification.mp3"))]
    );
}

/// tests/test_api_sounds.py::TestPlaySoundRoute::test_sound_disabled_by_the_theme_reports_okay_without_playing
#[tokio::test]
async fn sound_disabled_by_the_theme_reports_okay_without_playing() {
    let s = sounds();

    // sounds/default/config.json maps "startup" to {}.
    let reply = s.get("/api/v1/sounds/play/startup").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(testing::json(&reply), json!({"status": "okay"}));
    assert!(s.started().is_empty());
}

/// tests/test_api_sounds.py::TestPlaySoundRoute::test_unknown_sound_is_not_found
#[tokio::test]
async fn unknown_sound_is_not_found() {
    let s = sounds();

    let reply = s.get("/api/v1/sounds/play/grinder").await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        testing::json(&reply),
        json!({"error": "sound not found", "details": "grinder"})
    );
    assert!(s.started().is_empty());
}

/// tests/test_api_sounds.py::TestPlaySoundRoute::test_globally_disabled_sounds_report_okay_for_any_name_without_playing
#[tokio::test]
async fn globally_disabled_sounds_report_okay_for_any_name_without_playing() {
    let s = sounds();
    s.app
        .config
        .set(keys::USER, keys::SOUNDS_ENABLED, json!(false));

    let reply = s.get("/api/v1/sounds/play/grinder").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(testing::json(&reply), json!({"status": "okay"}));
    assert!(s.started().is_empty());
}

/// tests/test_api_sounds.py::TestPlaySoundRoute::test_sound_whose_file_is_missing_is_not_found
#[tokio::test]
async fn sound_whose_file_is_missing_is_not_found() {
    let s = sounds();
    std::fs::remove_file(s.system.join("default/notification.mp3")).expect("remove");

    let reply = s.get("/api/v1/sounds/play/notification").await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        testing::json(&reply),
        json!({"error": "sound not found", "details": "notification"})
    );
    assert!(s.started().is_empty());
}

/// tests/test_api_sounds.py::TestPlaySoundRoute::test_playing_a_sound_stops_the_one_still_playing
///
/// The Python plants a playing sound; here an earlier request plays it.
#[tokio::test]
async fn playing_a_sound_stops_the_one_still_playing() {
    let s = sounds();
    let notification = played(&s.system.join("default/notification.mp3"));
    let finished = played(&s.system.join("default/event_finished.mp3"));
    s.get("/api/v1/sounds/play/notification").await;
    assert!(s.stopped().is_empty());

    let reply = s.get("/api/v1/sounds/play/brewing_end").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(s.stopped(), vec![notification.clone()]);
    assert_eq!(s.started(), vec![notification.clone(), finished.clone()]);
    // The new sound is now the current one: the next play stops it.
    s.get("/api/v1/sounds/play/notification").await;
    assert_eq!(s.stopped(), vec![notification, finished]);
}

/// tests/test_api_sounds.py::TestPlaySoundRoute::test_playback_failure_is_reported_as_not_found
#[tokio::test]
async fn playback_failure_is_reported_as_not_found() {
    let s = sounds();
    s.runner.missing("gst-play-1.0");

    let reply = s.get("/api/v1/sounds/play/notification").await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        testing::json(&reply),
        json!({"error": "sound not found", "details": "notification"})
    );
}

/// tests/test_api_sounds.py::TestPlaySoundRoute::test_sound_requested_before_player_init_is_not_found
#[tokio::test]
async fn sound_requested_before_player_init_is_not_found() {
    let s = setup(false);

    let reply = s.get("/api/v1/sounds/play/notification").await;

    assert_eq!(reply.status_code(), 404);
    assert!(s.started().is_empty());
}

/// tests/test_api_sounds.py::TestPlaySoundRoute::test_sounds_of_a_user_theme_are_played_from_the_user_folder
///
/// The configured theme changes without `set_theme`; playing notices the
/// stale mapping and loads the user theme (the Python's
/// `CURRENT_THEME_NAME == "retro"`, seen here in the folder played from).
#[tokio::test]
async fn sounds_of_a_user_theme_are_played_from_the_user_folder() {
    let s = sounds();
    let folder = s.add_user_theme("retro", json!({"notification": "ding.mp3"}), &["ding.mp3"]);
    s.app
        .config
        .set(keys::SYSTEM, keys::SOUNDS_THEME, json!("retro"));

    let reply = s.get("/api/v1/sounds/play/notification").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(s.started(), vec![played(&folder.join("ding.mp3"))]);
}

// TestSoundListRoutes

/// tests/test_api_sounds.py::TestSoundListRoutes::test_sound_list_has_every_event_of_the_default_theme
#[tokio::test]
async fn sound_list_has_every_event_of_the_default_theme() {
    let s = sounds();

    let reply = s.get("/api/v1/sounds/list").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(testing::json(&reply), Value::Array(default_theme_keys()));
    assert_eq!(
        testing::json(&reply),
        json!([
            "startup",
            "heating_start",
            "heating_end",
            "brewing_start",
            "brewing_end",
            "abort",
            "idle",
            "notification",
        ])
    );
}

/// tests/test_api_sounds.py::TestSoundListRoutes::test_theme_list_has_only_the_system_default_theme
#[tokio::test]
async fn theme_list_has_only_the_system_default_theme() {
    let s = sounds();

    let reply = s.get("/api/v1/sounds/theme/list").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(testing::json(&reply), json!(["default"]));
}

/// tests/test_api_sounds.py::TestSoundListRoutes::test_theme_list_puts_user_themes_before_system_themes
#[tokio::test]
async fn theme_list_puts_user_themes_before_system_themes() {
    let s = sounds();
    s.add_user_theme("retro", json!({"notification": "ding.mp3"}), &[]);

    let reply = s.get("/api/v1/sounds/theme/list").await;

    assert_eq!(testing::json(&reply), json!(["retro", "default"]));
}

/// tests/test_api_sounds.py::TestSoundListRoutes::test_user_folder_without_valid_config_is_not_a_theme
#[tokio::test]
async fn user_folder_without_valid_config_is_not_a_theme() {
    let s = sounds();
    let broken = s.user.join("broken");
    std::fs::create_dir(&broken).expect("folder");
    std::fs::write(broken.join("config.json"), "{not json").expect("config");
    s.app.sounds.init(true, false);

    let reply = s.get("/api/v1/sounds/theme/list").await;

    assert_eq!(testing::json(&reply), json!(["default"]));
}

/// tests/test_api_sounds.py::TestSoundListRoutes::test_current_theme_is_returned_as_a_bare_string
#[tokio::test]
async fn current_theme_is_returned_as_a_bare_string() {
    let s = sounds();

    let reply = s.get("/api/v1/sounds/theme/get").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.body(), b"default");
}

// TestSetThemeRoute

/// tests/test_api_sounds.py::TestSetThemeRoute::test_get_selects_a_known_theme_and_persists_it
#[tokio::test]
async fn get_selects_a_known_theme_and_persists_it() {
    let s = sounds();
    s.add_user_theme("retro", json!({"notification": "ding.mp3"}), &[]);

    let reply = s.get("/api/v1/sounds/theme/set/retro").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(testing::json(&reply), json!({"status": "okay"}));
    assert_eq!(
        s.app.config.get(keys::SYSTEM, keys::SOUNDS_THEME),
        json!("retro")
    );
    assert_eq!(s.saves(), 1);
    assert_eq!(s.saved_theme(), json!("retro"));
    assert_eq!(s.get("/api/v1/sounds/theme/get").await.body(), b"retro");
}

/// tests/test_api_sounds.py::TestSetThemeRoute::test_post_selects_a_known_theme_like_get
#[tokio::test]
async fn post_selects_a_known_theme_like_get() {
    let s = sounds();
    s.add_user_theme("retro", json!({"notification": "ding.mp3"}), &[]);

    let reply = call(&s.app, Method::POST, "/api/v1/sounds/theme/set/retro", "").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(testing::json(&reply), json!({"status": "okay"}));
    assert_eq!(
        s.app.config.get(keys::SYSTEM, keys::SOUNDS_THEME),
        json!("retro")
    );
}

/// tests/test_api_sounds.py::TestSetThemeRoute::test_theme_missing_events_falls_back_to_the_default_mapping
///
/// The Python reads `SoundPlayer.get_theme()` directly.
#[tokio::test]
async fn theme_missing_events_falls_back_to_the_default_mapping() {
    let s = sounds();
    s.add_user_theme("retro", json!({"notification": "ding.mp3"}), &[]);

    s.get("/api/v1/sounds/theme/set/retro").await;

    let mut expected = default_theme_sounds();
    expected.insert("notification".into(), json!("ding.mp3"));
    assert_eq!(s.app.sounds.theme_config(), expected);
}

/// tests/test_api_sounds.py::TestSetThemeRoute::test_unknown_theme_is_not_found_and_reverts_to_default
#[tokio::test]
async fn unknown_theme_is_not_found_and_reverts_to_default() {
    let s = sounds();
    s.add_user_theme("retro", json!({"notification": "ding.mp3"}), &[]);
    s.get("/api/v1/sounds/theme/set/retro").await;
    s.saves.store(0, Ordering::SeqCst);

    let reply = s.get("/api/v1/sounds/theme/set/vaporwave").await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        testing::json(&reply),
        json!({"error": "theme not found", "details": "vaporwave"})
    );
    assert_eq!(
        s.app.config.get(keys::SYSTEM, keys::SOUNDS_THEME),
        json!("default")
    );
    assert_eq!(s.saves(), 1);
    assert_eq!(s.saved_theme(), json!("default"));
}

/// tests/test_api_sounds.py::TestSetThemeRoute::test_switching_back_to_default_drops_sounds_only_the_previous_theme_had
///
/// PARITY: a strict xfail in the Python (sounds.py:126-128 aliases
/// DEFAULT_THEME_CONFIG as CURRENT_THEME_CONFIG and updates it in place),
/// not a crash, so the port keeps the leak: the sound only the user theme
/// had is still listed after switching back. Flip this when sounds.py is
/// fixed.
#[tokio::test]
async fn switching_back_to_default_drops_sounds_only_the_previous_theme_had() {
    let s = sounds();
    s.add_user_theme(
        "retro",
        json!({"notification": "ding.mp3", "grinder": "grind.mp3"}),
        &[],
    );
    s.get("/api/v1/sounds/theme/set/retro").await;

    s.get("/api/v1/sounds/theme/set/default").await;

    let mut listed = default_theme_keys();
    listed.push(json!("grinder"));
    assert_eq!(
        testing::json(&s.get("/api/v1/sounds/list").await),
        Value::Array(listed)
    );
}

// TestUploadThemeRoute

/// tests/test_api_sounds.py::TestUploadThemeRoute::test_valid_theme_zip_is_extracted_and_becomes_selectable
#[tokio::test]
async fn valid_theme_zip_is_extracted_and_becomes_selectable() {
    let s = sounds();
    let config = json!({"notification": "ding.mp3"}).to_string();
    let archive = zip_bytes(&[
        ("retro/config.json", config.as_bytes()),
        ("retro/ding.mp3", b"ID3"),
    ]);

    let reply = s.upload(&archive, "file").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body_text(&reply),
        "Zip file uploaded and unpacked successfully."
    );
    let stored = std::fs::read_to_string(s.user.join("retro/config.json")).expect("config");
    assert_eq!(
        serde_json::from_str::<Value>(&stored).expect("json"),
        json!({"notification": "ding.mp3"})
    );
    assert_eq!(
        std::fs::read(s.user.join("retro/ding.mp3")).expect("sound"),
        b"ID3"
    );
    assert_eq!(
        testing::json(&s.get("/api/v1/sounds/theme/list").await),
        json!(["retro", "default"])
    );
    // The reload keeps the selected theme and never plays the startup sound.
    assert_eq!(
        s.app.config.get(keys::SYSTEM, keys::SOUNDS_THEME),
        json!("default")
    );
    assert!(s.started().is_empty());
}

/// tests/test_api_sounds.py::TestUploadThemeRoute::test_request_without_file_field_is_rejected
#[tokio::test]
async fn request_without_file_field_is_rejected() {
    let s = sounds();

    let reply = s
        .upload(&zip_bytes(&[("retro/config.json", b"{}")]), "theme")
        .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        testing::json(&reply),
        json!({"error": "invalid zip", "details": "'file' not found in request"})
    );
    s.assert_nothing_extracted();
}

/// tests/test_api_sounds.py::TestUploadThemeRoute::test_zip_with_two_root_folders_is_rejected
#[tokio::test]
async fn zip_with_two_root_folders_is_rejected() {
    let s = sounds();
    let archive = zip_bytes(&[("retro/config.json", b"{}"), ("modern/config.json", b"{}")]);

    let reply = s.upload(&archive, "file").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        testing::json(&reply),
        json!({
            "error": "invalid zip",
            "details": "Zip must contain exactly one folder with the themes name at the root.",
        })
    );
    s.assert_nothing_extracted();
}

/// tests/test_api_sounds.py::TestUploadThemeRoute::test_zip_with_files_only_at_the_root_is_rejected
#[tokio::test]
async fn zip_with_files_only_at_the_root_is_rejected() {
    let s = sounds();

    let reply = s
        .upload(
            &zip_bytes(&[("config.json", b"{}"), ("ding.mp3", b"ID3")]),
            "file",
        )
        .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        testing::json(&reply)["details"],
        json!("Zip must contain exactly one folder with the themes name at the root.")
    );
    s.assert_nothing_extracted();
}

/// tests/test_api_sounds.py::TestUploadThemeRoute::test_zip_without_config_in_the_theme_folder_is_rejected
#[tokio::test]
async fn zip_without_config_in_the_theme_folder_is_rejected() {
    let s = sounds();
    let archive = zip_bytes(&[
        ("retro/sounds/config.json", b"{}"),
        ("retro/ding.mp3", b"ID3"),
    ]);

    let reply = s.upload(&archive, "file").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        testing::json(&reply),
        json!({"error": "invalid zip", "details": "No config.json found in the root folder."})
    );
    s.assert_nothing_extracted();
}

/// tests/test_api_sounds.py::TestUploadThemeRoute::test_zip_with_invalid_config_json_is_rejected
#[tokio::test]
async fn zip_with_invalid_config_json_is_rejected() {
    let s = sounds();

    let reply = s
        .upload(&zip_bytes(&[("retro/config.json", b"{not json")]), "file")
        .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        testing::json(&reply),
        json!({"error": "invalid zip", "details": "config.json is not valid JSON."})
    );
    s.assert_nothing_extracted();
}

/// tests/test_api_sounds.py::TestUploadThemeRoute::test_corrupted_zip_is_rejected
#[tokio::test]
async fn corrupted_zip_is_rejected() {
    let s = sounds();

    let reply = s.upload(b"PK\x03\x04 truncated", "file").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        testing::json(&reply),
        json!({"error": "invalid zip", "details": "zip file corrupted"})
    );
    s.assert_nothing_extracted();
}
