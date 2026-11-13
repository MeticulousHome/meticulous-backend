//! The storage parts of `tests/test_pour_over_history.py`.
//!
//! The handler parts (status codes, the local-only write check, CORS
//! preflight) belong to the HTTP layer; here the same scenarios go through
//! the functions the handlers call.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use common::Env;
use history::pour_over::{
    MAX_POUR_OVER_RECORD_BYTES, PourOverFile, PourOverLimits, PourOverSession,
    read_compressed_record, resolve_pour_over_file,
};
use history::{Error, HistoryDb};
use serde_json::{Value, json};

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
            "durationMs": 135000,
            "status": "measured",
        },
        "pours": [{
            "number": 1, "startTimeMs": 0, "endTimeMs": 10000, "startWeightG": 0,
            "endWeightG": 40.2, "waterG": 40.2, "averageFlowGps": 4.02, "peakFlowGps": 4.8,
        }],
        "samples": [{"t": 0, "w": 0, "f": 0.8, "p": 1}, {"t": 10000, "w": 40.2, "f": 0.1, "p": 0}],
        "completion": "brewer_removed",
        "sync": {"status": "pending", "attempts": 0},
    })
}

fn default_session() -> Value {
    free_pour_session(
        "11111111-2222-4333-8444-555555555555",
        "2026-08-16T08:00:00.000Z",
    )
}

/// The test case's setUp: a database with the head schema.
fn store() -> (Env, HistoryDb) {
    let env = Env::new();
    let db = env.open();
    db.migrate().unwrap();
    (env, db)
}

fn save(db: &HistoryDb, payload: &Value) -> Result<history::pour_over::SavedPourOver, Error> {
    let session = PourOverSession::validate(payload).expect("valid session");
    db.save_pour_over(&session)
}

fn zst_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.extend(zst_files(&path));
            } else if path.to_string_lossy().ends_with(".zst") {
                found.push(path);
            }
        }
    }
    found
}

fn ids(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

/// test_saves_compressed_record_and_returns_it_from_history
#[test]
fn saves_compressed_record_and_returns_it_from_history() {
    let (env, db) = store();
    let saved = save(&db, &default_session()).unwrap();
    assert!(saved.created);
    let relative = saved.history["file"].as_str().unwrap();
    assert_eq!(
        relative,
        "2026-08-16/08-00-00-000-cf4c4732fd3b.pour-over.json.zst"
    );
    let stored = env.paths().pour_over_dir().join(relative);
    assert!(stored.is_file());
    let raw = zstd::decode_all(std::fs::read(&stored).unwrap().as_slice()).unwrap();
    let record: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(record["brewType"], "pour_over");
    assert_eq!(record["measurements"]["serverBaselineG"], json!(-0.4));
    // json.dumps(..., separators=(",", ":"), ensure_ascii=False)
    assert!(
        std::str::from_utf8(&raw)
            .unwrap()
            .starts_with("{\"schemaVersion\":4,\"id\":")
    );

    let latest = db.latest_pour_over().unwrap().unwrap();
    assert_eq!(latest["id"], default_session()["id"]);

    let PourOverFile::Record(path) =
        resolve_pour_over_file(&env.paths().pour_over_dir(), relative).unwrap()
    else {
        panic!("the record is served");
    };
    let served: Value = serde_json::from_slice(&read_compressed_record(&path).unwrap()).unwrap();
    assert_eq!(served["id"], default_session()["id"]);
    // The created answer: isoformat of the posted times, the stored row with Z.
    assert_eq!(
        pyfmt::dumps_default(&saved.history),
        pyfmt::dumps_default(&json!({
            "db_key": 1,
            "id": "11111111-2222-4333-8444-555555555555",
            "brewType": "pour_over",
            "mode": "free_pour",
            "file": relative,
            "time": 1786867200.0,
            "startedAt": "2026-08-16T08:00:00+00:00",
            "completedAt": "2026-08-16T08:02:15+00:00",
            "name": "Free Pour",
            "schemaVersion": 4,
        }))
    );
    assert_eq!(latest["startedAt"], "2026-08-16T08:00:00Z");
}

/// test_duplicate_session_is_idempotent
#[test]
fn duplicate_session_is_idempotent() {
    let (env, db) = store();
    assert!(save(&db, &default_session()).unwrap().created);
    let second = save(&db, &default_session()).unwrap();
    assert!(!second.created);
    assert_eq!(db.search_pour_over(50, true, None, None).unwrap().len(), 1);
    assert_eq!(zst_files(&env.paths().pour_over_dir()).len(), 1);
}

/// test_repost_with_changed_sync_state_is_idempotent
#[test]
fn repost_with_changed_sync_state_is_idempotent() {
    let (_env, db) = store();
    let mut after_upload = default_session();
    after_upload["sync"] =
        json!({"status": "uploaded", "attempts": 3, "uploadedAt": "2026-08-16T08:05:00.000Z"});
    assert!(save(&db, &default_session()).unwrap().created);
    assert!(!save(&db, &after_upload).unwrap().created);
    assert_eq!(db.search_pour_over(50, true, None, None).unwrap().len(), 1);
}

/// test_rejects_reused_session_id_with_different_measurements
#[test]
fn rejects_reused_session_id_with_different_measurements() {
    let (_env, db) = store();
    let mut changed = default_session();
    changed["measurements"]["waterPouredG"] = json!(250);
    assert!(save(&db, &default_session()).unwrap().created);
    assert!(matches!(save(&db, &changed), Err(Error::PourOverConflict)));
    assert_eq!(db.search_pour_over(50, true, None, None).unwrap().len(), 1);
}

/// test_rejects_invalid_contract_without_writing_history
#[test]
fn rejects_invalid_contract_without_writing_history() {
    let (env, db) = store();
    let mut invalid = default_session();
    invalid["samples"] = json!([]);
    assert!(PourOverSession::validate(&invalid).is_err());
    assert!(
        db.search_pour_over(50, true, None, None)
            .unwrap()
            .is_empty()
    );
    assert!(zst_files(&env.paths().pour_over_dir()).is_empty());
}

/// test_lan_client_can_read_and_paginate_history_without_writing (the
/// storage part: ascending pages, the record file, the cursor).
#[test]
fn history_paginates_by_file_cursor() {
    let (env, db) = store();
    let first = default_session();
    let mut second = free_pour_session(
        "33333333-4444-4555-8666-777777777777",
        "2026-08-16T09:00:00.000Z",
    );
    second["completedAt"] = json!("2026-08-16T09:02:15.000Z");
    save(&db, &first).unwrap();
    save(&db, &second).unwrap();

    let page = db.search_pour_over(1, false, None, None).unwrap();
    assert_eq!(ids(&page), vec![first["id"].as_str().unwrap()]);
    let file = page[0]["file"].as_str().unwrap();
    let PourOverFile::Record(path) =
        resolve_pour_over_file(&env.paths().pour_over_dir(), file).unwrap()
    else {
        panic!("record");
    };
    let record: Value = serde_json::from_slice(&read_compressed_record(&path).unwrap()).unwrap();
    assert_eq!(record["id"], first["id"]);
    let next = db.search_pour_over(1, false, Some(file), None).unwrap();
    assert_eq!(ids(&next), vec![second["id"].as_str().unwrap()]);
    assert_eq!(zst_files(&env.paths().pour_over_dir()).len(), 2);
}

/// test_profile_mode_and_legacy_contract_are_accepted
#[test]
fn profile_mode_and_legacy_contract_are_accepted() {
    let (_env, db) = store();
    let mut profile = default_session();
    profile["mode"] = json!("profile");
    profile["name"] = json!("Three Pour 1:15");
    profile["recipe"]["profileId"] = json!("profile-1");
    profile["recipe"]["targetWaterG"] = json!(225);
    profile["recipe"]["targetDurationMs"] = json!(135000);
    profile["recipe"]["pourTargets"] =
        json!([{"number": 1, "startTimeMs": 0, "stopWeightG": 40, "flowGps": 4}]);
    assert!(save(&db, &profile).unwrap().created);

    let mut legacy = free_pour_session(
        "33333333-4444-4555-8666-777777777777",
        "2026-08-16T07:00:00.000Z",
    );
    legacy["schemaVersion"] = json!(2);
    legacy["measurements"]["emptyServerG"] = json!(182.4);
    for (block, key) in [
        ("measurements", "serverBaselineG"),
        ("recipe", "temperatureC"),
        ("recipe", "targetDurationMs"),
        ("measurements", "doseG"),
        ("measurements", "waterTemperatureC"),
    ] {
        legacy[block].as_object_mut().unwrap().remove(key);
    }
    assert!(save(&db, &legacy).unwrap().created);
    let records = db.search_pour_over(50, false, None, None).unwrap();
    let versions: Vec<i64> = records
        .iter()
        .map(|r| r["schemaVersion"].as_i64().unwrap())
        .collect();
    assert_eq!(versions, vec![2, 4]);
    let free_pour = db
        .search_pour_over(1, true, None, Some("free_pour"))
        .unwrap();
    assert_eq!(free_pour[0]["id"], legacy["id"]);
}

/// test_prunes_oldest_records_at_the_history_limit
#[test]
fn prunes_oldest_records_at_the_history_limit() {
    let (env, mut db) = store();
    db.set_pour_over_limits(PourOverLimits {
        max_history_records: 2,
        min_free_bytes: 0,
        ..PourOverLimits::default()
    });
    let mut sessions = Vec::new();
    for index in 1..=3 {
        let started = format!("2026-08-16T0{index}:00:00.000Z");
        let mut session =
            free_pour_session(&format!("00000000-0000-4000-8000-{index:012}"), &started);
        session["completedAt"] = json!(started);
        assert!(save(&db, &session).unwrap().created);
        sessions.push(session);
    }
    let records = db.search_pour_over(50, false, None, None).unwrap();
    assert_eq!(
        ids(&records),
        vec![
            sessions[1]["id"].as_str().unwrap(),
            sessions[2]["id"].as_str().unwrap()
        ]
    );
    assert_eq!(zst_files(&env.paths().pour_over_dir()).len(), 2);
}

/// test_low_disk_refusal_preserves_existing_records
#[test]
fn low_disk_refusal_preserves_existing_records() {
    let (env, mut db) = store();
    let first = default_session();
    assert!(save(&db, &first).unwrap().created);
    let stored = zst_files(&env.paths().pour_over_dir());
    assert_eq!(stored.len(), 1);
    db.set_pour_over_limits(PourOverLimits {
        min_free_bytes: 1 << 60,
        ..PourOverLimits::default()
    });
    let mut second = free_pour_session(
        "99999999-8888-4777-8666-555555555555",
        "2026-08-16T09:00:00.000Z",
    );
    second["completedAt"] = second["startedAt"].clone();
    assert!(matches!(save(&db, &second), Err(Error::Storage(_))));
    assert_eq!(
        ids(&db.search_pour_over(50, true, None, None).unwrap()),
        vec![first["id"].as_str().unwrap()]
    );
    assert!(stored[0].is_file());
}

/// test_prune_never_removes_the_most_recent_record
#[test]
fn prune_never_removes_the_most_recent_record() {
    let (env, mut db) = store();
    let first = default_session();
    assert!(save(&db, &first).unwrap().created);
    let stored = zst_files(&env.paths().pour_over_dir());
    db.set_pour_over_limits(PourOverLimits {
        max_history_bytes: std::fs::metadata(&stored[0]).unwrap().len() + 10,
        min_free_bytes: 0,
        ..PourOverLimits::default()
    });
    let mut second = free_pour_session(
        "99999999-8888-4777-8666-555555555555",
        "2026-08-16T09:00:00.000Z",
    );
    second["completedAt"] = second["startedAt"].clone();
    assert!(matches!(save(&db, &second), Err(Error::Storage(_))));
    assert_eq!(
        ids(&db.search_pour_over(50, true, None, None).unwrap()),
        vec![first["id"].as_str().unwrap()]
    );
    assert!(stored[0].is_file());
}

/// test_compressed_record_reader_bounds_expansion (the reader part).
#[test]
fn compressed_record_reader_bounds_expansion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oversized.pour-over.json.zst");
    let raw = vec![b'0'; MAX_POUR_OVER_RECORD_BYTES + 1];
    std::fs::write(&path, zstd::encode_all(raw.as_slice(), 10).unwrap()).unwrap();
    assert!(matches!(
        read_compressed_record(&path),
        Err(Error::PourOverTooLarge(_))
    ));
    let fits = dir.path().join("fits.zst");
    std::fs::write(&fits, zstd::encode_all(&raw[1..], 10).unwrap()).unwrap();
    assert_eq!(
        read_compressed_record(&fits).unwrap().len(),
        MAX_POUR_OVER_RECORD_BYTES
    );
    let garbage = dir.path().join("garbage.zst");
    std::fs::write(&garbage, b"not zstd").unwrap();
    assert!(matches!(
        read_compressed_record(&garbage),
        Err(Error::Zstd { .. })
    ));
}

/// A record whose file disappeared is rewritten when the same session is
/// posted again (`_validate_existing_record`).
#[test]
fn a_missing_record_file_is_repaired() {
    let (env, db) = store();
    let saved = save(&db, &default_session()).unwrap();
    let path = env
        .paths()
        .pour_over_dir()
        .join(saved.history["file"].as_str().unwrap());
    std::fs::remove_file(&path).unwrap();
    assert!(!save(&db, &default_session()).unwrap().created);
    assert!(path.is_file());
}

/// PourOverFileHandler: directory listings, `.zst` fallback, traversal.
#[test]
fn record_files_resolve_like_the_file_handler() {
    let (env, db) = store();
    let saved = save(&db, &default_session()).unwrap();
    let root = env.paths().pour_over_dir();
    let file = saved.history["file"].as_str().unwrap();
    let PourOverFile::Directory(days) = resolve_pour_over_file(&root, "").unwrap() else {
        panic!("root listing");
    };
    assert_eq!(days.len(), 1);
    assert_eq!(days[0].name, "2026-08-16");
    let PourOverFile::Directory(records) = resolve_pour_over_file(&root, "2026-08-16").unwrap()
    else {
        panic!("day listing");
    };
    assert_eq!(
        records[0].to_json(),
        json!({"name": "08-00-00-000-cf4c4732fd3b.pour-over.json", "url": "08-00-00-000-cf4c4732fd3b.pour-over.json.zst"})
    );
    assert!(matches!(
        resolve_pour_over_file(&root, file).unwrap(),
        PourOverFile::Record(_)
    ));
    let without_suffix = file.strip_suffix(".zst").unwrap();
    assert!(matches!(
        resolve_pour_over_file(&root, without_suffix).unwrap(),
        PourOverFile::Record(_)
    ));
    assert_eq!(
        resolve_pour_over_file(&root, "2026-08-16/missing.json").unwrap(),
        PourOverFile::NotFound
    );
    assert_eq!(
        resolve_pour_over_file(&root, "../history.sqlite").unwrap(),
        PourOverFile::Outside
    );
}
