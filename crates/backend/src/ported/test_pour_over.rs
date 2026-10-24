//! Port of `tests/test_pour_over_profiles.py` and
//! `tests/test_pour_over_history.py`: the backend storage and routes around
//! the validation and the records the `profile` and `history` crates test.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use http::Method;
use serde_json::{Value, json};

use history::pour_over::PourOverLimits;

use crate::app::App;
use crate::env::{BackendMode, Env};
use crate::http::Reply;
use crate::testing::{app, app_with, call, call_with_headers, json as body_json};

// --- tests/test_pour_over_profiles.py --------------------------------------

const PROFILE_ID: &str = "11111111-2222-4333-8444-555555555555";

fn valid_pour_over_profile() -> Value {
    json!({
        "version": 1,
        "brew_type": "pour_over",
        "id": PROFILE_ID,
        "name": "Lance 2 pour",
        "author": "Recipe author",
        "author_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "display": {
            "accentColor": "#23383F",
            "shortDescription": "A bright three-stage recipe",
            "description": "Two smaller pours followed by the main pour.",
        },
        "recipe": {
            "coffee_dose_g": 15,
            "total_water_g": 225,
            "water_temperature_c": 92,
            "target_total_time_s": 135,
            "target_total_time_max_s": 180,
        },
        "stages": [
            {
                "key": "pour-1",
                "name": "Pour 1",
                "starts_at_s": 0,
                "pour": {
                    "water_g": 35,
                    "duration_s": 10,
                    "target_cumulative_water_g": 35,
                    "flow_rate_g_s": 3.5,
                    "pattern": "spiral_out",
                },
            },
            {
                "key": "pour-2",
                "name": "Pour 2",
                "starts_at_s": 30,
                "pour": {
                    "water_g": 35,
                    "duration_s": 10,
                    "target_cumulative_water_g": 70,
                    "flow_rate_g_s": 3.5,
                    "flow_range_g_s": [3, 4],
                },
            },
            {
                "key": "pour-3",
                "name": "Pour 3",
                "starts_at_s": 60,
                "pour": {
                    "water_g": 155,
                    "duration_s": 31,
                    "target_cumulative_water_g": 225,
                    "flow_rate_g_s": 5,
                },
            },
        ],
    })
}

fn file_names(dir: &Path) -> Vec<String> {
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

/// tests/test_pour_over_profiles.py::test_profile_validation_and_atomic_persistence
#[test]
fn profile_validation_and_atomic_persistence() {
    let (_dir, app) = app();
    app.pour_over.init();

    let result = app
        .pour_over
        .save_profile(&app, valid_pour_over_profile(), None)
        .expect("saved");

    assert_eq!(result["profile"]["recipe"]["coffee_dose_g"], 15);
    let dir = &app.env.paths.pour_over_profiles;
    let stored = dir.join(format!("{PROFILE_ID}.json"));
    assert!(stored.is_file());
    let parsed: Value =
        serde_json::from_str(&std::fs::read_to_string(&stored).expect("read")).expect("JSON");
    assert_eq!(parsed["stages"][2]["starts_at_s"], 60);
    let temporaries: Vec<String> = file_names(dir)
        .into_iter()
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert_eq!(temporaries, Vec::<String>::new());
}

/// tests/test_pour_over_profiles.py::test_refresh_skips_corrupt_or_filename_mismatched_profiles
#[test]
fn refresh_skips_corrupt_or_filename_mismatched_profiles() {
    let (_dir, app) = app();
    app.pour_over.init();
    let dir = &app.env.paths.pour_over_profiles;
    std::fs::write(dir.join("broken.json"), "{not json").expect("write");
    std::fs::write(
        dir.join("wrong-id.json"),
        valid_pour_over_profile().to_string(),
    )
    .expect("write");

    // `refresh_profile_list` is `init`'s refresh.
    app.pour_over.init();

    assert!(app.pour_over.is_available());
    assert_eq!(
        app.pour_over.list_profiles().expect("available"),
        Vec::<Value>::new()
    );
}

/// tests/test_pour_over_profiles.py::test_optional_feature_init_failure_does_not_raise_or_advertise_available
///
/// The Python test makes the schema file disappear; the Rust build compiles
/// the schema in, so the failure left to provoke is the storage: a file
/// stands where the profile directory should be.
#[tokio::test]
async fn optional_feature_init_failure_does_not_raise_or_advertise_available() {
    let (_dir, app) = app_with(BackendMode::Emulator, |env| {
        let dir = &env.paths.pour_over_profiles;
        std::fs::create_dir_all(dir.parent().expect("parent")).expect("mkdir");
        std::fs::write(dir, "not a directory").expect("write");
    });

    app.pour_over.init();

    assert!(!app.pour_over.is_available());
    let reply = call(&app, Method::GET, "/api/v1/pour-over/profile/list", "").await;
    assert_eq!(reply.status_code(), 503);
    assert_eq!(
        body_json(&reply)["error"],
        "Pour Over profiles are unavailable"
    );
}

/// tests/test_pour_over_profiles.py::TestPourOverProfilesAPI::test_existing_profile_save_endpoint_dispatches_pour_over
#[tokio::test]
async fn existing_profile_save_endpoint_dispatches_pour_over() {
    let (_dir, app) = app();
    app.pour_over.init();
    let mut profile = valid_pour_over_profile();
    profile["display"]["image"] = json!("data:image/jpeg;base64,/9j/2Q==");
    let get = |uri: String| {
        let app = app.clone();
        async move { call(&app, Method::GET, &uri, "").await }
    };

    let saved = call(
        &app,
        Method::POST,
        "/api/v1/profile/save",
        profile.to_string(),
    )
    .await;
    assert_eq!(
        saved.status_code(),
        200,
        "{}",
        String::from_utf8_lossy(saved.body())
    );
    assert_eq!(body_json(&saved)["profile"]["brew_type"], "pour_over");
    assert_eq!(app.profiles.known_count(), 0);

    let listed = body_json(&get("/api/v1/pour-over/profile/list".into()).await);
    let listed = listed["profiles"].as_array().expect("profiles");
    assert_eq!(listed.len(), 1);
    assert!(listed[0]["display"].get("image").is_none());

    let fetched = get(format!("/api/v1/pour-over/profile/get/{PROFILE_ID}")).await;
    assert_eq!(fetched.status_code(), 200);
    let fetched = body_json(&fetched);
    assert_eq!(fetched["name"], profile["name"]);
    assert!(
        fetched["display"]["image"]
            .as_str()
            .expect("image")
            .starts_with("data:image/jpeg;base64,")
    );

    // The stored summary (`_known_profiles`) carries no image either.
    let without_image = body_json(
        &get(format!(
            "/api/v1/pour-over/profile/get/{PROFILE_ID}?include_image=false"
        ))
        .await,
    );
    assert!(without_image["display"].get("image").is_none());

    let image = get(format!("/api/v1/pour-over/profile/image/{PROFILE_ID}")).await;
    assert_eq!(image.status_code(), 200);
    assert!(
        image
            .header("Content-Type")
            .expect("content type")
            .starts_with("image/jpeg")
    );
    assert_eq!(image.body(), b"\xff\xd8\xff\xd9");

    let deleted = call(
        &app,
        Method::DELETE,
        &format!("/api/v1/pour-over/profile/delete/{PROFILE_ID}"),
        "",
    )
    .await;
    assert_eq!(deleted.status_code(), 200);
    assert_eq!(
        get(format!("/api/v1/pour-over/profile/get/{PROFILE_ID}"))
            .await
            .status_code(),
        404
    );
}

// --- tests/test_pour_over_history.py ---------------------------------------

const SESSION_ID: &str = "11111111-2222-4333-8444-555555555555";
const SECOND_ID: &str = "33333333-4444-4555-8666-777777777777";
const LAN: [(&str, &str); 2] = [("Host", "machine.local"), ("X-Real-IP", "192.168.10.20")];

fn free_pour_session(session_id: &str, started_at: &str) -> Value {
    json!({
        "schemaVersion": 4,
        "id": session_id,
        "brewType": "pour_over",
        "mode": "free_pour",
        "name": "Free Pour",
        "source": "dial",
        "startedAt": started_at,
        "completedAt": "2026-08-16T08:02:15.000Z",
        "recipe": {
            "profileId": null,
            "profileName": "Free Pour",
            "doseG": 15,
            "temperatureC": 92,
            "targetWaterG": null,
            "targetDurationMs": null,
            "pourTargets": [],
        },
        "measurements": {
            "serverBaselineG": -0.4,
            "brewerG": 134.2,
            "setupG": 149.2,
            "doseG": 15.1,
            "waterTemperatureC": 92,
            "waterPouredG": 225.3,
            "beverageG": 196.8,
            "retainedG": 28.5,
            "durationMs": 135_000,
            "status": "measured",
        },
        "pours": [
            {
                "number": 1,
                "startTimeMs": 0,
                "endTimeMs": 10_000,
                "startWeightG": 0,
                "endWeightG": 40.2,
                "waterG": 40.2,
                "averageFlowGps": 4.02,
                "peakFlowGps": 4.8,
            }
        ],
        "samples": [
            {"t": 0, "w": 0, "f": 0.8, "p": 1},
            {"t": 10_000, "w": 40.2, "f": 0.1, "p": 0},
        ],
        "completion": "brewer_removed",
        "sync": {"status": "pending", "attempts": 0},
    })
}

fn default_session() -> Value {
    free_pour_session(SESSION_ID, "2026-08-16T08:00:00.000Z")
}

async fn save_session(app: &Arc<App>, payload: &Value, headers: &[(&str, &str)]) -> Reply {
    call_with_headers(
        app,
        Method::POST,
        "/api/v1/history/pour-over",
        payload.to_string(),
        headers,
    )
    .await
}

async fn history_get(app: &Arc<App>, uri: &str, headers: &[(&str, &str)]) -> Reply {
    call_with_headers(app, Method::GET, uri, "", headers).await
}

/// `PourOverHistoryManager.search()`: the stored records, newest first.
async fn records(app: &Arc<App>, query: &str) -> Vec<Value> {
    let reply = history_get(app, &format!("/api/v1/history/pour-over{query}"), &[]).await;
    assert_eq!(reply.status_code(), 200);
    body_json(&reply)["history"]
        .as_array()
        .expect("history")
        .clone()
}

fn record_ids(records: &[Value]) -> Vec<Value> {
    records.iter().map(|r| r["id"].clone()).collect()
}

/// Every `*.zst` under the Pour Over history directory.
fn zst_files(app: &App) -> Vec<PathBuf> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|e| e == "zst") {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(&app.env.paths.pour_over_history(), &mut found);
    found.sort();
    found
}

/// A second app on the same data directory, its history opened with these
/// Pour Over limits (the Python test patches the module constants between
/// two saves).
fn reopened_with_limits(dir: &Path, limits: PourOverLimits) -> Arc<App> {
    let env = Env::for_tests(dir, BackendMode::Emulator);
    let log = Arc::new(crate::logging::LogHooks::default());
    let app = App::new(env, log, crate::sentry_reporting::Reporting::init(false));
    app.history.init_with(|db| db.set_pour_over_limits(limits));
    app
}

/// tests/test_pour_over_history.py::TestPourOverHistoryAPI::test_saves_compressed_record_and_returns_it_from_history
#[tokio::test]
async fn saves_compressed_record_and_returns_it_from_history() {
    let (_dir, app) = app();

    let reply = save_session(&app, &default_session(), &[]).await;

    assert_eq!(reply.status_code(), 201);
    let relative = body_json(&reply)["history"]["file"]
        .as_str()
        .expect("file")
        .to_string();
    let stored = app.env.paths.pour_over_history().join(&relative);
    assert!(stored.is_file());
    let raw = zstd::decode_all(std::fs::File::open(&stored).expect("open")).expect("zstd");
    let saved: Value = serde_json::from_slice(&raw).expect("JSON");
    assert_eq!(saved["brewType"], "pour_over");
    assert_eq!(saved["measurements"]["serverBaselineG"], -0.4);

    let last = history_get(&app, "/api/v1/history/pour-over/last", &[]).await;
    assert_eq!(last.status_code(), 200);
    assert_eq!(body_json(&last)["id"], SESSION_ID);

    let file = history_get(
        &app,
        &format!("/api/v1/history/pour-over/files/{relative}"),
        &[],
    )
    .await;
    assert_eq!(file.status_code(), 200);
    assert_eq!(body_json(&file)["id"], SESSION_ID);
}

/// tests/test_pour_over_history.py::TestPourOverHistoryAPI::test_duplicate_session_is_idempotent
#[tokio::test]
async fn duplicate_session_is_idempotent() {
    let (_dir, app) = app();

    let first = save_session(&app, &default_session(), &[]).await;
    let second = save_session(&app, &default_session(), &[]).await;

    assert_eq!(first.status_code(), 201);
    assert_eq!(second.status_code(), 200);
    assert_eq!(body_json(&second)["status"], "existing");
    assert_eq!(records(&app, "").await.len(), 1);
    assert_eq!(zst_files(&app).len(), 1);
}

/// tests/test_pour_over_history.py::TestPourOverHistoryAPI::test_repost_with_changed_sync_state_is_idempotent
#[tokio::test]
async fn repost_with_changed_sync_state_is_idempotent() {
    let (_dir, app) = app();
    let mut after_upload_attempt = default_session();
    after_upload_attempt["sync"] = json!({
        "status": "uploaded",
        "attempts": 3,
        "uploadedAt": "2026-08-16T08:05:00.000Z",
    });

    assert_eq!(
        save_session(&app, &default_session(), &[])
            .await
            .status_code(),
        201
    );
    let reply = save_session(&app, &after_upload_attempt, &[]).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(body_json(&reply)["status"], "existing");
    assert_eq!(records(&app, "").await.len(), 1);
}

/// tests/test_pour_over_history.py::TestPourOverHistoryAPI::test_rejects_reused_session_id_with_different_measurements
#[tokio::test]
async fn rejects_reused_session_id_with_different_measurements() {
    let (_dir, app) = app();
    let mut changed = default_session();
    changed["measurements"]["waterPouredG"] = json!(250);

    assert_eq!(
        save_session(&app, &default_session(), &[])
            .await
            .status_code(),
        201
    );
    let reply = save_session(&app, &changed, &[]).await;

    assert_eq!(reply.status_code(), 409);
    assert_eq!(records(&app, "").await.len(), 1);
}

/// tests/test_pour_over_history.py::TestPourOverHistoryAPI::test_lan_client_can_read_and_paginate_history_without_writing
#[tokio::test]
async fn lan_client_can_read_and_paginate_history_without_writing() {
    let (_dir, app) = app();
    let first = default_session();
    let mut second = free_pour_session(SECOND_ID, "2026-08-16T09:00:00.000Z");
    second["completedAt"] = json!("2026-08-16T09:02:15.000Z");
    assert_eq!(save_session(&app, &first, &[]).await.status_code(), 201);
    assert_eq!(save_session(&app, &second, &[]).await.status_code(), 201);

    let reply = history_get(
        &app,
        "/api/v1/history/pour-over?sort=asc&max_results=1",
        &LAN,
    )
    .await;
    assert_eq!(reply.status_code(), 200);
    let history = body_json(&reply)["history"]
        .as_array()
        .expect("history")
        .clone();
    assert_eq!(record_ids(&history), vec![first["id"].clone()]);
    let file = history[0]["file"].as_str().expect("file").to_string();

    let record = history_get(
        &app,
        &format!("/api/v1/history/pour-over/files/{file}"),
        &LAN,
    )
    .await;
    assert_eq!(record.status_code(), 200);
    assert_eq!(body_json(&record)["id"], first["id"]);

    let reply = history_get(
        &app,
        &format!("/api/v1/history/pour-over?sort=asc&max_results=1&after={file}"),
        &LAN,
    )
    .await;
    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        record_ids(body_json(&reply)["history"].as_array().expect("history")),
        vec![second["id"].clone()]
    );

    let rejected = free_pour_session(
        "99999999-8888-4777-8666-555555555555",
        "2026-08-16T10:00:00.000Z",
    );
    assert_eq!(save_session(&app, &rejected, &LAN).await.status_code(), 403);
    assert_eq!(records(&app, "").await.len(), 2);
    assert_eq!(zst_files(&app).len(), 2);
}

/// tests/test_pour_over_history.py::TestPourOverHistoryAPI::test_lan_read_preflight_is_allowed
#[tokio::test]
async fn lan_read_preflight_is_allowed() {
    let (_dir, app) = app();

    let reply = call_with_headers(
        &app,
        Method::OPTIONS,
        "/api/v1/history/pour-over",
        "",
        &[
            ("Host", "machine.local"),
            ("X-Real-IP", "192.168.10.20"),
            ("Origin", "https://community.meticuloushome.com"),
            ("Access-Control-Request-Method", "GET"),
        ],
    )
    .await;

    assert_eq!(reply.status_code(), 204);
    assert_eq!(reply.header("Access-Control-Allow-Origin"), Some("*"));
    assert_eq!(records(&app, "").await, Vec::<Value>::new());
}

/// tests/test_pour_over_history.py::TestPourOverHistoryAPI::test_profile_mode_and_legacy_contract_are_accepted
#[tokio::test]
async fn profile_mode_and_legacy_contract_are_accepted() {
    let (_dir, app) = app();
    let mut profile = default_session();
    profile["mode"] = json!("profile");
    profile["name"] = json!("Three Pour 1:15");
    profile["recipe"]["profileId"] = json!("profile-1");
    profile["recipe"]["targetWaterG"] = json!(225);
    profile["recipe"]["targetDurationMs"] = json!(135_000);
    profile["recipe"]["pourTargets"] =
        json!([{"number": 1, "startTimeMs": 0, "stopWeightG": 40, "flowGps": 4}]);
    assert_eq!(save_session(&app, &profile, &[]).await.status_code(), 201);

    let mut legacy = free_pour_session(SECOND_ID, "2026-08-16T07:00:00.000Z");
    legacy["schemaVersion"] = json!(2);
    legacy["measurements"]["emptyServerG"] = json!(182.4);
    for (section, key) in [
        ("measurements", "serverBaselineG"),
        ("recipe", "temperatureC"),
        ("recipe", "targetDurationMs"),
        ("measurements", "doseG"),
        ("measurements", "waterTemperatureC"),
    ] {
        legacy[section]
            .as_object_mut()
            .expect("section")
            .shift_remove(key);
    }
    assert_eq!(save_session(&app, &legacy, &[]).await.status_code(), 201);

    let versions: Vec<Value> = records(&app, "?sort=asc")
        .await
        .iter()
        .map(|r| r["schemaVersion"].clone())
        .collect();
    assert_eq!(versions, vec![json!(2), json!(4)]);
    let free_pour = records(&app, "?mode=free_pour&max_results=1").await;
    assert_eq!(free_pour[0]["id"], legacy["id"]);
}

/// tests/test_pour_over_history.py::TestPourOverHistoryAPI::test_low_disk_refusal_preserves_existing_records
#[tokio::test]
async fn low_disk_refusal_preserves_existing_records() {
    let (dir, app) = app();
    assert_eq!(
        save_session(&app, &default_session(), &[])
            .await
            .status_code(),
        201
    );
    let stored = zst_files(&app);
    assert_eq!(stored.len(), 1);

    // More free space than any disk provides: the save must be refused
    // without pruning history that cannot recover the shortfall.
    let short_of_space = reopened_with_limits(
        dir.path(),
        PourOverLimits {
            min_free_bytes: 1 << 60,
            ..PourOverLimits::default()
        },
    );
    let mut second = free_pour_session(
        "99999999-8888-4777-8666-555555555555",
        "2026-08-16T09:00:00.000Z",
    );
    second["completedAt"] = second["startedAt"].clone();
    let reply = save_session(&short_of_space, &second, &[]).await;

    assert_eq!(reply.status_code(), 500);
    assert_eq!(
        record_ids(&records(&short_of_space, "").await),
        vec![json!(SESSION_ID)]
    );
    assert!(stored[0].is_file());
}

/// tests/test_pour_over_history.py::TestPourOverHistoryAPI::test_prune_never_removes_the_most_recent_record
#[tokio::test]
async fn prune_never_removes_the_most_recent_record() {
    let (dir, app) = app();
    assert_eq!(
        save_session(&app, &default_session(), &[])
            .await
            .status_code(),
        201
    );
    let stored = zst_files(&app);
    assert_eq!(stored.len(), 1);

    // A quota the existing record plus the incoming one cannot fit in.
    // Pruning must stop at the newest stored record and refuse the save
    // instead of destroying the whole history.
    let size = std::fs::metadata(&stored[0]).expect("metadata").len();
    let tight = reopened_with_limits(
        dir.path(),
        PourOverLimits {
            max_history_bytes: size + 10,
            min_free_bytes: 0,
            ..PourOverLimits::default()
        },
    );
    let mut second = free_pour_session(
        "99999999-8888-4777-8666-555555555555",
        "2026-08-16T09:00:00.000Z",
    );
    second["completedAt"] = second["startedAt"].clone();
    let reply = save_session(&tight, &second, &[]).await;

    assert_eq!(reply.status_code(), 500);
    assert_eq!(
        record_ids(&records(&tight, "").await),
        vec![json!(SESSION_ID)]
    );
    assert!(stored[0].is_file());
}
