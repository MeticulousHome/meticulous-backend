//! Port of `tests/test_manual_mode.py`: the backend side of Manual mode,
//! where the Python test mocks its neighbours (`save_profile`,
//! `search_history`, `list_profiles`). Here the profile manager, the history
//! database and the routes are the real ones on temporary directories, and
//! each case checks what its Python counterpart asserts through them.

use std::path::PathBuf;
use std::sync::Arc;

use http::Method;
use serde_json::{Value, json};

use crate::app::App;
use crate::env::BackendMode;
use crate::http::Reply;
use crate::testing::{app, app_with, call, call_with_headers, json as body_json};

const MANUAL_MODE_PROFILE_ID: &str = "4d616e75-616c-4d6f-8465-000000000001";
const SHOT_TIME: f64 = 1757193600.0;
const OLDER_SHOT_ID: &str = "8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6";
const NEWER_SHOT_ID: &str = "9b14e45f-ceea-467a-9a3c-a1b2c3d4e5f7";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn example_profile() -> Value {
    let path = repo().join("profile_schema/example_profile.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e} (is the profile_schema submodule checked out?)",
            path.display()
        )
    });
    serde_json::from_str(&text).expect("example profile")
}

fn pressure_sample(profile_time: i64, pressure: f64, weight: f64) -> Value {
    json!({
        "shot": {
            "pressure": 6.0,
            "flow": 1.2,
            "weight": weight,
            "gravimetric_flow": 1.1,
            "setpoints": {"active": "pressure", "pressure": pressure},
        },
        "time": profile_time,
        "profile_time": profile_time,
        "status": "Manual",
    })
}

/// The Manual mode profile a shot was brewed with, as the history stores it.
fn manual_profile(temperature: Value, last_changed: f64) -> Value {
    json!({
        "id": MANUAL_MODE_PROFILE_ID,
        "author": "Barista",
        "author_id": "11111111-2222-4333-8444-555555555555",
        "display": {},
        "final_weight": 1000.0,
        "last_changed": last_changed,
        "name": "Manual mode",
        "temperature": temperature,
        "stages": [],
        "variables": [],
        "previous_authors": [],
    })
}

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

/// Two manual brews and a later brew of another profile: the older manual
/// brew at 93.5 °C targeting 6 bar, the newer one at 92 °C targeting 7 bar.
fn record_manual_brews(app: &App) {
    record_shot(
        app,
        OLDER_SHOT_ID,
        SHOT_TIME,
        manual_profile(json!(93.5), SHOT_TIME - 600.0),
        json!([pressure_sample(1000, 6.0, 0.0)]),
    );
    record_shot(
        app,
        NEWER_SHOT_ID,
        SHOT_TIME + 3600.0,
        manual_profile(json!(92.0), SHOT_TIME + 3000.0),
        json!([pressure_sample(1000, 7.0, 0.0)]),
    );
    let mut other = example_profile();
    other["last_changed"] = json!(SHOT_TIME);
    record_shot(
        app,
        "00000000-ceea-467a-9a3c-a1b2c3d4e5f8",
        SHOT_TIME + 7200.0,
        other,
        json!([pressure_sample(1000, 9.0, 0.0)]),
    );
}

fn stored_files(app: &App) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(&app.env.paths.profiles)
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

fn profiles_app() -> (tempfile::TempDir, Arc<App>) {
    let (dir, app) = app();
    std::fs::create_dir_all(&app.env.paths.profiles).expect("mkdir profiles");
    app.profiles.refresh_image_list();
    (dir, app)
}

// --- seeding ---------------------------------------------------------------

/// tests/test_manual_mode.py::test_ensure_manual_mode_profile_survives_a_failing_save
#[test]
fn ensure_manual_mode_profile_survives_a_failing_save() {
    // A file where the profiles directory should be: the save cannot write.
    let (_dir, app) = app_with(BackendMode::Emulator, |env| {
        let user = env.paths.profiles.parent().expect("user dir");
        std::fs::create_dir_all(user).expect("mkdir user");
        std::fs::write(&env.paths.profiles, "read-only filesystem").expect("write");
    });
    // With the default images known, only the write can fail.
    app.profiles.refresh_image_list();

    assert!(!app.profiles.ensure_manual_mode_profile(&app));
    assert_eq!(app.profiles.known_count(), 0);
}

/// tests/test_manual_mode.py::test_init_seeds_the_manual_mode_profile_after_refreshing
///
/// The Python test reads the order of the calls in `ProfileManager.init`'s
/// source. Here the order shows in what init leaves behind: a Manual mode
/// profile the user already set up (renamed, its stage pair reordered, an
/// uploaded image) survives because the refresh read it before the seeding
/// looked for it, and its image survives because the image collection ran
/// after the refresh. A fresh machine gets the profile seeded.
#[tokio::test]
async fn init_seeds_the_manual_mode_profile_after_refreshing() {
    let image = "0123456789abcdef0123456789abcdef.png";
    let mut users = profile::build_manual_mode_profile();
    users["name"] = json!("My manual");
    users["last_changed"] = json!(SHOT_TIME);
    users["display"] = json!({
        "image": format!("/api/v1/profile/image/{image}"),
        "accentColor": "#123456",
    });
    let stages = users["stages"].as_array_mut().expect("stages");
    stages.reverse();
    let on_disk = users.clone();
    let (_dir, app) = app_with(BackendMode::Emulator, |env| {
        std::fs::create_dir_all(&env.paths.profiles).expect("mkdir profiles");
        std::fs::write(
            env.paths
                .profiles
                .join(format!("{MANUAL_MODE_PROFILE_ID}.json")),
            on_disk.to_string(),
        )
        .expect("write profile");
        std::fs::create_dir_all(&env.paths.images).expect("mkdir images");
        std::fs::write(env.paths.images.join(image), b"uploaded").expect("write image");
    });

    app.profiles.init(&app);

    let reply = call(
        &app,
        Method::GET,
        &format!("/api/v1/profile/get/{MANUAL_MODE_PROFILE_ID}"),
        "",
    )
    .await;
    assert_eq!(body_json(&reply), users);
    let stored: Value = serde_json::from_str(
        &std::fs::read_to_string(
            app.env
                .paths
                .profiles
                .join(format!("{MANUAL_MODE_PROFILE_ID}.json")),
        )
        .expect("stored profile"),
    )
    .expect("JSON");
    assert_eq!(stored, users);
    assert!(app.env.paths.images.join(image).is_file());

    let (_fresh_dir, fresh) = crate::testing::app();
    fresh.profiles.init(&fresh);
    let seeded = fresh
        .profiles
        .get_profile(MANUAL_MODE_PROFILE_ID)
        .expect("seeded profile");
    assert_eq!(seeded["name"], "Manual mode");
    assert_eq!(seeded["manual"], true);
    assert_eq!(
        stored_files(&fresh),
        vec![format!("{MANUAL_MODE_PROFILE_ID}.json")]
    );
}

// --- locating the manual shot ------------------------------------------------

/// tests/test_manual_mode.py::test_find_manual_shot_queries_the_latest_manual_profile_shot
///
/// The query sorts by time descending, keeps one result and loads its data:
/// with two manual brews and a later brew of another profile, the newer
/// manual brew comes back with its samples.
#[test]
fn find_manual_shot_queries_the_latest_manual_profile_shot() {
    let (_dir, app) = app();
    record_manual_brews(&app);

    let shot = crate::manual_mode::find_manual_shot(&app, None).expect("manual shot");

    assert_eq!(shot["id"], NEWER_SHOT_ID);
    assert_eq!(shot["profile"]["id"], MANUAL_MODE_PROFILE_ID);
    assert_eq!(shot["data"], json!([pressure_sample(1000, 7.0, 0.0)]));
}

// --- GET /api/v1/profile/list ----------------------------------------------

/// The Python test's three listed profiles, schema-valid so the real save
/// takes them: a regular one (`manual: false`), a manual one, a plain one.
fn listed_profiles() -> Vec<Value> {
    let base = example_profile();
    let make = |id: &str, name: &str, manual: Option<Value>| {
        let mut profile = base.clone();
        profile["id"] = json!(id);
        profile["name"] = json!(name);
        if let Some(manual) = manual {
            profile["manual"] = manual;
        }
        profile
    };
    vec![
        make(
            "11111111-0000-4000-8000-000000000001",
            "Regular",
            Some(json!(false)),
        ),
        make(
            "11111111-0000-4000-8000-000000000002",
            "Manual mode",
            Some(json!(true)),
        ),
        make("11111111-0000-4000-8000-000000000003", "Plain", None),
    ]
}

async fn save_all(app: &Arc<App>, profiles: &[Value]) {
    for profile in profiles {
        let reply = call(
            app,
            Method::POST,
            "/api/v1/profile/save",
            profile.to_string(),
        )
        .await;
        assert_eq!(
            reply.status_code(),
            200,
            "{}",
            String::from_utf8_lossy(reply.body())
        );
    }
}

async fn listed(app: &Arc<App>, query: &str) -> Vec<Value> {
    let reply = call(
        app,
        Method::GET,
        &format!("/api/v1/profile/list{query}"),
        "",
    )
    .await;
    assert_eq!(reply.status_code(), 200);
    body_json(&reply).as_array().expect("list").clone()
}

fn listed_ids(profiles: &[Value]) -> Vec<Value> {
    profiles.iter().map(|p| p["id"].clone()).collect()
}

/// tests/test_manual_mode.py::TestListHandlerHidesManualProfiles::test_the_summary_list_omits_the_manual_profile
#[tokio::test]
async fn the_summary_list_omits_the_manual_profile() {
    let (_dir, app) = profiles_app();
    let profiles = listed_profiles();
    save_all(&app, &profiles).await;

    let listed = listed(&app, "").await;

    assert_eq!(
        listed_ids(&listed),
        vec![profiles[0]["id"].clone(), profiles[2]["id"].clone()]
    );
    assert!(listed.iter().all(|p| p.get("stages").is_none()));
}

/// tests/test_manual_mode.py::TestListHandlerHidesManualProfiles::test_the_full_list_omits_the_manual_profile
#[tokio::test]
async fn the_full_list_omits_the_manual_profile() {
    let (_dir, app) = profiles_app();
    let profiles = listed_profiles();
    save_all(&app, &profiles).await;

    let listed = listed(&app, "?full=true").await;

    assert_eq!(
        listed_ids(&listed),
        vec![profiles[0]["id"].clone(), profiles[2]["id"].clone()]
    );
    assert!(listed.iter().all(|p| p.get("stages").is_some()));
}

/// tests/test_manual_mode.py::TestListHandlerHidesManualProfiles::test_a_non_boolean_manual_flag_does_not_hide_a_profile
#[tokio::test]
async fn a_non_boolean_manual_flag_does_not_hide_a_profile() {
    let (_dir, app) = profiles_app();
    let mut odd = example_profile();
    odd["manual"] = json!("true");
    save_all(&app, std::slice::from_ref(&odd)).await;

    assert_eq!(listed_ids(&listed(&app, "").await), vec![odd["id"].clone()]);
}

/// tests/test_manual_mode.py::TestListHandlerHidesManualProfiles::test_the_list_does_not_mutate_the_stored_profiles
#[tokio::test]
async fn the_list_does_not_mutate_the_stored_profiles() {
    let (_dir, app) = profiles_app();
    let profiles = listed_profiles();
    save_all(&app, &profiles).await;

    listed(&app, "").await;

    for profile in &profiles {
        let stored = app
            .profiles
            .get_profile(profile["id"].as_str().expect("id"))
            .expect("stored profile");
        assert!(stored.get("stages").is_some());
    }
    assert!(
        listed(&app, "?full=true")
            .await
            .iter()
            .all(|p| p.get("stages").is_some())
    );
}

// --- POST /api/v1/profile/from_manual --------------------------------------

async fn from_manual(app: &Arc<App>, body: &str, headers: &[(&str, &str)]) -> Reply {
    call_with_headers(
        app,
        Method::POST,
        "/api/v1/profile/from_manual",
        body.to_string(),
        headers,
    )
    .await
}

fn assert_nothing_saved(app: &App) {
    assert_eq!(stored_files(app), Vec::<String>::new());
    assert_eq!(app.profiles.changes(), Vec::<Value>::new());
}

/// tests/test_manual_mode.py::TestCreateProfileFromManualHandler::test_an_empty_body_saves_the_latest_manual_brew
#[tokio::test]
async fn an_empty_body_saves_the_latest_manual_brew() {
    let (_dir, app) = profiles_app();
    record_manual_brews(&app);

    let reply = from_manual(&app, "", &[]).await;

    assert_eq!(
        reply.status_code(),
        200,
        "{}",
        String::from_utf8_lossy(reply.body())
    );
    let result = body_json(&reply);
    let profile = &result["profile"];
    // Built from the newer manual brew.
    assert_eq!(profile["temperature"], 92.0);
    assert_eq!(
        profile["stages"][0]["dynamics"]["points"],
        json!([[0.0, 7.0]])
    );
    let change_id = result["change_id"].as_str().expect("generated change id");
    assert!(uuid::Uuid::parse_str(change_id).is_ok());
    let id = profile["id"].as_str().expect("id");
    assert_eq!(stored_files(&app), vec![format!("{id}.json")]);
    assert_eq!(app.profiles.get_profile(id).as_ref(), Some(profile));
    let changes = app.profiles.changes();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["change_id"], change_id);
}

/// tests/test_manual_mode.py::TestCreateProfileFromManualHandler::test_the_change_id_header_is_forwarded_to_save_profile
#[tokio::test]
async fn the_change_id_header_is_forwarded_to_save_profile() {
    let (_dir, app) = profiles_app();
    record_manual_brews(&app);

    let reply = from_manual(&app, "", &[("X-Change-Id", "change-42")]).await;

    assert_eq!(reply.status_code(), 200);
    let result = body_json(&reply);
    assert_eq!(result["change_id"], "change-42");
    let changes = app.profiles.changes();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        (&changes[0]["profile_id"], &changes[0]["change_id"]),
        (&result["profile"]["id"], &json!("change-42"))
    );
}

/// tests/test_manual_mode.py::TestCreateProfileFromManualHandler::test_the_requested_shot_id_is_forwarded
#[tokio::test]
async fn the_requested_shot_id_is_forwarded() {
    let (_dir, app) = profiles_app();
    record_manual_brews(&app);

    let reply = from_manual(
        &app,
        &json!({"shot_id": OLDER_SHOT_ID, "name": "Morning"}).to_string(),
        &[],
    )
    .await;

    assert_eq!(reply.status_code(), 200);
    let profile = &body_json(&reply)["profile"];
    // Built from the older brew the request named, not the latest one.
    assert_eq!(profile["temperature"], 93.5);
    assert_eq!(
        profile["stages"][0]["dynamics"]["points"],
        json!([[0.0, 6.0]])
    );
    assert_eq!(profile["name"], "Morning");
}

/// tests/test_manual_mode.py::TestCreateProfileFromManualHandler::test_a_non_object_body_is_rejected
#[tokio::test]
async fn a_non_object_body_is_rejected() {
    let (_dir, app) = profiles_app();
    record_manual_brews(&app);

    let reply = from_manual(&app, r#"["not", "an", "object"]"#, &[]).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "body must be a JSON object"})
    );
    assert_nothing_saved(&app);
}

/// tests/test_manual_mode.py::TestCreateProfileFromManualHandler::test_a_non_string_name_is_rejected
#[tokio::test]
async fn a_non_string_name_is_rejected() {
    let (_dir, app) = profiles_app();
    record_manual_brews(&app);

    let reply = from_manual(&app, r#"{"name": 7}"#, &[]).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "name must be a string"})
    );
    assert_nothing_saved(&app);
}

/// tests/test_manual_mode.py::TestCreateProfileFromManualHandler::test_a_non_string_shot_id_is_rejected
#[tokio::test]
async fn a_non_string_shot_id_is_rejected() {
    let (_dir, app) = profiles_app();
    record_manual_brews(&app);

    let reply = from_manual(&app, r#"{"shot_id": 7}"#, &[]).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "shot_id must be a string"})
    );
    assert_nothing_saved(&app);
}

/// tests/test_manual_mode.py::TestCreateProfileFromManualHandler::test_a_schema_violation_is_reported_like_profile_save
///
/// The Python test makes the save raise; here the brew's own profile carries
/// a temperature the schema refuses, which the built profile inherits.
#[tokio::test]
async fn a_schema_violation_is_reported_like_profile_save() {
    let (_dir, app) = profiles_app();
    record_shot(
        &app,
        OLDER_SHOT_ID,
        SHOT_TIME,
        manual_profile(json!(150), SHOT_TIME - 600.0),
        json!([pressure_sample(1000, 6.0, 0.0)]),
    );

    let reply = from_manual(&app, "", &[]).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({"status": "error", "error": "JSON validation error: 150 is greater than the maximum of 100"})
    );
    assert_nothing_saved(&app);
}

/// tests/test_manual_mode.py::TestCreateProfileFromManualHandler::test_a_malformed_body_is_rejected
#[tokio::test]
async fn a_malformed_body_is_rejected() {
    let (_dir, app) = profiles_app();
    record_manual_brews(&app);

    let reply = from_manual(&app, "{not json", &[]).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(body_json(&reply)["status"], "error");
    assert_nothing_saved(&app);
}
