//! `tests/test_shot_database.py`, case by case, against the Rust store.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use common::Env;
use history::search::SearchId;
use history::{HistoryDb, SearchOrder, SearchParams};
use serde_json::{Value, json};

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// The `shot_db` fixture: a fresh database with the head schema.
fn shot_db() -> (Env, HistoryDb) {
    let env = Env::new();
    let db = env.open();
    db.migrate().unwrap();
    (env, db)
}

fn make_profile(overrides: Value) -> Value {
    let mut base = json!({
        "id": "profile-001",
        "author": "Test Author",
        "author_id": "author-001",
        "display": {},
        "final_weight": 36,
        "last_changed": 0,
        "name": "Test Espresso",
        "temperature": 93,
        "stages": [
            {"key": "stage-1", "name": "Preinfusion", "type": "pressure"},
            {"key": "stage-2", "name": "Extraction", "type": "pressure"},
        ],
        "variables": [],
        "previous_authors": [],
    });
    for (key, value) in overrides.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

fn make_history_entry(profile: Option<Value>, overrides: Value) -> Value {
    let profile = profile.unwrap_or_else(|| make_profile(json!({})));
    let overrides = overrides.as_object().unwrap();
    let mut base = json!({
        "id": overrides.get("id").cloned().unwrap_or(json!("hist-001")),
        "file": overrides.get("file").cloned().unwrap_or(json!("shot_001.json.zst")),
        "time": overrides.get("time").cloned().unwrap_or(json!(now())),
        "profile_name": profile["name"],
        "profile": profile,
    });
    for (key, value) in overrides {
        if !["id", "file", "time"].contains(&key.as_str()) {
            base[key] = value.clone();
        }
    }
    base
}

fn search(db: &HistoryDb, params: SearchParams) -> Vec<Value> {
    let result = db.search_history(&params).unwrap();
    common::assert_search_json_agrees(db, &params, &result);
    result
}

fn no_data() -> SearchParams {
    SearchParams {
        dump_data: false,
        ..SearchParams::default()
    }
}

fn write_compressed_shot(env: &Env, filename: &str, shot: &Value) {
    let path = env.paths().shot_dir().join(filename);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let raw = pyfmt::dumps_default(shot);
    std::fs::write(&path, zstd::encode_all(raw.as_bytes(), 3).unwrap()).unwrap();
}

/// TestInsertProfile.test_insert_returns_key
#[test]
fn insert_profile_returns_key() {
    let (_env, db) = shot_db();
    let key = db.insert_profile(Some(&make_profile(json!({})))).unwrap();
    assert!(key > 0);
}

/// TestInsertProfile.test_insert_same_profile_returns_same_key
#[test]
fn insert_same_profile_returns_same_key() {
    let (_env, db) = shot_db();
    let profile = make_profile(json!({}));
    assert_eq!(
        db.insert_profile(Some(&profile)).unwrap(),
        db.insert_profile(Some(&profile)).unwrap()
    );
}

/// TestInsertProfile.test_insert_different_profiles_return_different_keys
#[test]
fn insert_different_profiles_return_different_keys() {
    let (_env, db) = shot_db();
    let p1 = make_profile(json!({"id": "p1", "name": "Profile A"}));
    let p2 = make_profile(json!({"id": "p2", "name": "Profile B"}));
    assert_ne!(
        db.insert_profile(Some(&p1)).unwrap(),
        db.insert_profile(Some(&p2)).unwrap()
    );
}

/// TestInsertProfile.test_insert_none_returns_negative
#[test]
fn insert_none_profile_returns_negative() {
    let (_env, db) = shot_db();
    assert_eq!(db.insert_profile(None).unwrap(), -1);
}

/// TestInsertHistory.test_insert_returns_id
#[test]
fn insert_history_returns_id() {
    let (_env, db) = shot_db();
    assert!(
        db.insert_history(&make_history_entry(None, json!({})))
            .unwrap()
            > 0
    );
}

/// TestInsertHistory.test_insert_same_file_returns_same_id
#[test]
fn insert_same_file_returns_same_id() {
    let (_env, db) = shot_db();
    let entry = make_history_entry(None, json!({"file": "same.json.zst"}));
    assert_eq!(
        db.insert_history(&entry).unwrap(),
        db.insert_history(&entry).unwrap()
    );
}

/// TestInsertHistory.test_insert_different_files_return_different_ids
#[test]
fn insert_different_files_return_different_ids() {
    let (_env, db) = shot_db();
    let e1 = make_history_entry(None, json!({"id": "h1", "file": "shot_a.json.zst"}));
    let e2 = make_history_entry(None, json!({"id": "h2", "file": "shot_b.json.zst"}));
    assert_ne!(
        db.insert_history(&e1).unwrap(),
        db.insert_history(&e2).unwrap()
    );
}

/// TestSearchHistory.test_search_returns_inserted_entry
#[test]
fn search_returns_inserted_entry() {
    let (_env, db) = shot_db();
    db.insert_history(&make_history_entry(None, json!({})))
        .unwrap();
    let results = search(&db, no_data());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["name"], "Test Espresso");
    assert_eq!(results[0]["profile"]["name"], "Test Espresso");
}

/// TestSearchHistory.test_search_returns_push_to_brew_time_from_shot_file
#[test]
fn search_returns_push_to_brew_time_from_shot_file() {
    let (env, db) = shot_db();
    let entry = make_history_entry(None, json!({"file": "2026-09-25/shot.json.zst"}));
    write_compressed_shot(
        &env,
        "2026-09-25/shot.json.zst",
        &json!({"data": [], "push_to_brew_time": 1234}),
    );
    db.insert_history(&entry).unwrap();
    let results = search(&db, SearchParams::default());
    assert_eq!(results[0]["push_to_brew_time"], 1234);
}

/// TestSearchHistory.test_search_with_max_results
#[test]
fn search_with_max_results() {
    let (_env, db) = shot_db();
    for i in 0..5 {
        db.insert_history(&make_history_entry(
            None,
            json!({"id": format!("h{i}"), "file": format!("shot_{i}.json.zst"), "time": now() + f64::from(i)}),
        ))
        .unwrap();
    }
    let results = search(
        &db,
        SearchParams {
            max_results: 3,
            ..no_data()
        },
    );
    assert_eq!(results.len(), 3);
}

/// TestSearchHistory.test_upload_index_is_cursor_ordered_and_bounded
#[test]
fn upload_index_is_cursor_ordered_and_bounded() {
    let (_env, db) = shot_db();
    for index in 0..5 {
        db.insert_history(&make_history_entry(
            None,
            json!({"id": format!("upload-{index}"), "file": format!("2026-08-16/shot_{index}.shot.json.zst"), "time": now() + f64::from(index)}),
        ))
        .unwrap();
    }
    let results = db
        .list_history_files(Some("2026-08-16/shot_1.shot.json.zst"), 2)
        .unwrap();
    assert_eq!(
        results,
        vec![
            json!({"file": "2026-08-16/shot_2.shot.json.zst"}),
            json!({"file": "2026-08-16/shot_3.shot.json.zst"}),
        ]
    );
}

/// TestSearchHistory.test_search_by_profile_id
#[test]
fn search_by_profile_id() {
    let (_env, db) = shot_db();
    let p1 = make_profile(json!({"id": "p-aaa", "name": "Ristretto"}));
    let p2 = make_profile(json!({"id": "p-bbb", "name": "Lungo"}));
    db.insert_history(&make_history_entry(
        Some(p1),
        json!({"id": "h1", "file": "s1.zst"}),
    ))
    .unwrap();
    db.insert_history(&make_history_entry(
        Some(p2),
        json!({"id": "h2", "file": "s2.zst"}),
    ))
    .unwrap();
    let results = search(
        &db,
        SearchParams {
            ids: vec![SearchId::from("p-aaa")],
            ..no_data()
        },
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["profile"]["id"], "p-aaa");
}

fn three_spaced_shots(db: &HistoryDb) {
    for i in 0..3 {
        db.insert_history(&make_history_entry(
            None,
            json!({"id": format!("h{i}"), "file": format!("s{i}.zst"), "time": 1000000 + i * 100}),
        ))
        .unwrap();
    }
}

fn times(results: &[Value]) -> Vec<f64> {
    results
        .iter()
        .map(|r| r["time"].as_f64().unwrap())
        .collect()
}

/// TestSearchHistory.test_search_ascending_order
#[test]
fn search_ascending_order() {
    let (_env, db) = shot_db();
    three_spaced_shots(&db);
    let results = search(
        &db,
        SearchParams {
            sort: SearchOrder::Ascending,
            ..no_data()
        },
    );
    let t = times(&results);
    let mut sorted = t.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(t, sorted);
    assert_eq!(t, vec![1000000.0, 1000100.0, 1000200.0]);
}

/// TestSearchHistory.test_search_descending_order
#[test]
fn search_descending_order() {
    let (_env, db) = shot_db();
    three_spaced_shots(&db);
    let t = times(&search(&db, no_data()));
    assert_eq!(t, vec![1000200.0, 1000100.0, 1000000.0]);
}

/// TestSearchHistory.test_search_by_query_matches_profile_name
#[test]
fn search_by_query_matches_profile_name() {
    let (_env, db) = shot_db();
    let p1 = make_profile(json!({"id": "p1", "name": "Morning Espresso"}));
    let p2 = make_profile(json!({"id": "p2", "name": "Evening Lungo"}));
    db.insert_history(&make_history_entry(
        Some(p1),
        json!({"id": "h1", "file": "s1.zst"}),
    ))
    .unwrap();
    db.insert_history(&make_history_entry(
        Some(p2),
        json!({"id": "h2", "file": "s2.zst"}),
    ))
    .unwrap();
    let results = search(
        &db,
        SearchParams {
            query: Some("Morning".into()),
            ..no_data()
        },
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["profile"]["name"], "Morning Espresso");
}

/// TestSearchHistory.test_search_by_query_matches_stage_name
#[test]
fn search_by_query_matches_stage_name() {
    let (_env, db) = shot_db();
    let p = make_profile(json!({
        "id": "p1",
        "name": "Custom Profile",
        "stages": [{"key": "s1", "name": "Bloom Phase", "type": "pressure"}],
    }));
    db.insert_history(&make_history_entry(
        Some(p),
        json!({"id": "h1", "file": "s1.zst"}),
    ))
    .unwrap();
    let results = search(
        &db,
        SearchParams {
            query: Some("Bloom".into()),
            ..no_data()
        },
    );
    assert_eq!(results.len(), 1);
}

/// TestSearchHistory.test_search_empty_database
#[test]
fn search_empty_database() {
    let (_env, db) = shot_db();
    assert!(search(&db, no_data()).is_empty());
}

fn insert_shot(db: &HistoryDb) -> String {
    let entry = make_history_entry(None, json!({}));
    db.insert_history(&entry).unwrap();
    entry["id"].as_str().unwrap().to_string()
}

/// TestRateShot.test_rate_like
#[test]
fn rate_like() {
    let (_env, db) = shot_db();
    let uuid = insert_shot(&db);
    assert!(db.rate_shot(&uuid, Some("like")));
    assert_eq!(db.get_shot_rating(&uuid).as_deref(), Some("like"));
}

/// TestRateShot.test_rate_dislike
#[test]
fn rate_dislike() {
    let (_env, db) = shot_db();
    let uuid = insert_shot(&db);
    assert!(db.rate_shot(&uuid, Some("dislike")));
    assert_eq!(db.get_shot_rating(&uuid).as_deref(), Some("dislike"));
}

/// TestRateShot.test_change_rating
#[test]
fn change_rating() {
    let (_env, db) = shot_db();
    let uuid = insert_shot(&db);
    db.rate_shot(&uuid, Some("like"));
    db.rate_shot(&uuid, Some("dislike"));
    assert_eq!(db.get_shot_rating(&uuid).as_deref(), Some("dislike"));
}

/// TestRateShot.test_remove_rating
#[test]
fn remove_rating() {
    let (_env, db) = shot_db();
    let uuid = insert_shot(&db);
    db.rate_shot(&uuid, Some("like"));
    db.rate_shot(&uuid, None);
    assert_eq!(db.get_shot_rating(&uuid), None);
}

/// TestRateShot.test_rate_nonexistent_shot
#[test]
fn rate_nonexistent_shot() {
    let (_env, db) = shot_db();
    assert!(!db.rate_shot("nonexistent-uuid", Some("like")));
}

/// TestRateShot.test_invalid_rating_rejected
#[test]
fn invalid_rating_rejected() {
    let (_env, db) = shot_db();
    let uuid = insert_shot(&db);
    assert!(!db.rate_shot(&uuid, Some("stars")));
}

/// TestRateShot.test_get_rating_unrated_shot
#[test]
fn get_rating_unrated_shot() {
    let (_env, db) = shot_db();
    let uuid = insert_shot(&db);
    assert_eq!(db.get_shot_rating(&uuid), None);
}

/// TestStatistics.test_empty_database
#[test]
fn statistics_of_an_empty_database() {
    let (_env, db) = shot_db();
    assert_eq!(
        db.statistics().unwrap(),
        json!({"totalSavedShots": 0, "byProfile": []})
    );
}

/// TestStatistics.test_counts_shots_per_profile
#[test]
fn statistics_count_shots_per_profile() {
    let (_env, db) = shot_db();
    let p1 = make_profile(json!({"id": "p1", "name": "Espresso"}));
    let p2 = make_profile(json!({"id": "p2", "name": "Lungo"}));
    for i in 0..3 {
        db.insert_history(&make_history_entry(
            Some(p1.clone()),
            json!({"id": format!("h-esp-{i}"), "file": format!("esp_{i}.zst")}),
        ))
        .unwrap();
    }
    for i in 0..2 {
        db.insert_history(&make_history_entry(
            Some(p2.clone()),
            json!({"id": format!("h-lng-{i}"), "file": format!("lng_{i}.zst")}),
        ))
        .unwrap();
    }
    let stats = db.statistics().unwrap();
    assert_eq!(stats["totalSavedShots"], 5);
    assert_eq!(
        stats["byProfile"],
        json!([
            {"name": "Espresso", "count": 3, "profileVersions": 1},
            {"name": "Lungo", "count": 2, "profileVersions": 1},
        ])
    );
}

fn two_profiles(db: &HistoryDb) {
    let p1 = make_profile(json!({"id": "p1", "name": "Espresso"}));
    let p2 = make_profile(json!({"id": "p2", "name": "Lungo"}));
    db.insert_history(&make_history_entry(
        Some(p1),
        json!({"id": "h1", "file": "s1.zst"}),
    ))
    .unwrap();
    db.insert_history(&make_history_entry(
        Some(p2),
        json!({"id": "h2", "file": "s2.zst"}),
    ))
    .unwrap();
}

fn profiles(results: &[Value]) -> Vec<String> {
    results
        .iter()
        .map(|r| r["profile"].as_str().unwrap().to_string())
        .collect()
}

/// TestAutocompleteProfileName.test_no_prefix_returns_all
#[test]
fn autocomplete_without_prefix_returns_all() {
    let (_env, db) = shot_db();
    two_profiles(&db);
    let names = profiles(&db.autocomplete_profile_name("").unwrap());
    assert!(names.contains(&"Espresso".to_string()));
    assert!(names.contains(&"Lungo".to_string()));
}

/// TestAutocompleteProfileName.test_prefix_filters
#[test]
fn autocomplete_prefix_filters() {
    let (_env, db) = shot_db();
    two_profiles(&db);
    let names = profiles(&db.autocomplete_profile_name("Esp").unwrap());
    assert!(names.contains(&"Espresso".to_string()));
    assert!(!names.contains(&"Lungo".to_string()));
}

/// TestAutocompleteProfileName.test_stage_name_match
#[test]
fn autocomplete_matches_stage_names() {
    let (_env, db) = shot_db();
    let p = make_profile(json!({
        "id": "p1",
        "name": "Custom",
        "stages": [{"key": "s1", "name": "Bloom", "type": "pressure"}],
    }));
    db.insert_history(&make_history_entry(
        Some(p),
        json!({"id": "h1", "file": "s1.zst"}),
    ))
    .unwrap();
    let results = db.autocomplete_profile_name("Bloom").unwrap();
    assert!(
        results
            .iter()
            .any(|r| r.get("type") == Some(&json!("stage")))
    );
}

/// TestLinkDebugFile.test_link_and_search
#[test]
fn link_debug_file_and_search() {
    let (_env, db) = shot_db();
    let id = db
        .insert_history(&make_history_entry(None, json!({})))
        .unwrap();
    db.link_debug_file(id, "2024-01-01/12:00:00.shot.json.zst")
        .unwrap();
    let results = search(&db, no_data());
    assert_eq!(
        results[0]["debug_file"],
        "2024-01-01/12:00:00.shot.json.zst"
    );
}

/// TestLinkDebugFile.test_unlink_debug_file
#[test]
fn unlink_debug_file() {
    let (_env, db) = shot_db();
    let id = db
        .insert_history(&make_history_entry(None, json!({})))
        .unwrap();
    let debug_path = "2024-01-01/12:00:00.shot.json.zst";
    db.link_debug_file(id, debug_path).unwrap();
    assert_eq!(db.unlink_debug_file(debug_path).unwrap(), 1);
    assert_eq!(search(&db, no_data())[0]["debug_file"], Value::Null);
    assert_eq!(db.unlink_debug_file(debug_path).unwrap(), 0);
}

/// TestSearchHistoryDumpData.test_dump_data_reads_compressed_file
#[test]
fn dump_data_reads_compressed_file() {
    let (env, db) = shot_db();
    let shot = json!({"data": [{"time": 0.1, "pressure": 9.0, "flow": 2.5}]});
    write_compressed_shot(&env, "shot_001.json.zst", &shot);
    db.insert_history(&make_history_entry(
        None,
        json!({"file": "shot_001.json.zst"}),
    ))
    .unwrap();
    let results = search(&db, SearchParams::default());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["data"], shot["data"]);
}

/// TestSearchHistoryDumpData.test_dump_data_false_returns_none
#[test]
fn dump_data_false_returns_none() {
    let (_env, db) = shot_db();
    db.insert_history(&make_history_entry(None, json!({})))
        .unwrap();
    let results = search(&db, no_data());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["data"], Value::Null);
}

fn old_and_new(db: &HistoryDb) {
    db.insert_history(&make_history_entry(
        None,
        json!({"id": "h-old", "file": "old.zst", "time": 1000000.0}),
    ))
    .unwrap();
    db.insert_history(&make_history_entry(
        None,
        json!({"id": "h-new", "file": "new.zst", "time": 2000000.0}),
    ))
    .unwrap();
}

/// TestSearchHistoryDateFilters.test_start_date_filter
#[test]
fn start_date_filter() {
    let (_env, db) = shot_db();
    old_and_new(&db);
    let results = search(
        &db,
        SearchParams {
            start_date: Some(1500000.0),
            ..no_data()
        },
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], "h-new");
}

/// TestSearchHistoryDateFilters.test_end_date_filter
#[test]
fn end_date_filter() {
    let (_env, db) = shot_db();
    old_and_new(&db);
    let results = search(
        &db,
        SearchParams {
            end_date: Some(1500000.0),
            ..no_data()
        },
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], "h-old");
}

/// TestSearchHistoryDateFilters.test_date_range_filter
#[test]
fn date_range_filter() {
    let (_env, db) = shot_db();
    for (i, t) in [1000000.0, 1500000.0, 2000000.0].iter().enumerate() {
        db.insert_history(&make_history_entry(
            None,
            json!({"id": format!("h{i}"), "file": format!("s{i}.zst"), "time": t}),
        ))
        .unwrap();
    }
    let results = search(
        &db,
        SearchParams {
            start_date: Some(1200000.0),
            end_date: Some(1800000.0),
            ..no_data()
        },
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], "h1");
}

/// TestSearchHistoryByIds.test_search_by_history_uuid
#[test]
fn search_by_history_uuid() {
    let (_env, db) = shot_db();
    db.insert_history(&make_history_entry(
        None,
        json!({"id": "uuid-aaa", "file": "s1.zst"}),
    ))
    .unwrap();
    db.insert_history(&make_history_entry(
        None,
        json!({"id": "uuid-bbb", "file": "s2.zst"}),
    ))
    .unwrap();
    let results = search(
        &db,
        SearchParams {
            ids: vec![SearchId::from("uuid-aaa")],
            ..no_data()
        },
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], "uuid-aaa");
}

/// TestSearchHistoryByIds.test_search_by_history_db_id
#[test]
fn search_by_history_db_id() {
    let (_env, db) = shot_db();
    let id = db
        .insert_history(&make_history_entry(
            None,
            json!({"id": "h1", "file": "s1.zst"}),
        ))
        .unwrap();
    let results = search(
        &db,
        SearchParams {
            ids: vec![SearchId::Int(id)],
            ..no_data()
        },
    );
    assert_eq!(results.len(), 1);
    // A numeric string matches the integer id too (INTEGER affinity), as the
    // GET handler passes ids as strings.
    let results = search(
        &db,
        SearchParams {
            ids: vec![SearchId::Str(id.to_string())],
            ..no_data()
        },
    );
    assert_eq!(results.len(), 1);
}

/// TestSearchHistoryByIds.test_search_by_multiple_ids
#[test]
fn search_by_multiple_ids() {
    let (_env, db) = shot_db();
    for i in 0..3 {
        db.insert_history(&make_history_entry(
            None,
            json!({"id": format!("uuid-{i}"), "file": format!("s{i}.zst")}),
        ))
        .unwrap();
    }
    let results = search(
        &db,
        SearchParams {
            ids: vec![SearchId::from("uuid-0"), SearchId::from("uuid-2")],
            ..no_data()
        },
    );
    let mut ids: Vec<String> = results
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["uuid-0", "uuid-2"]);
}

/// TestDeleteShot.test_delete_removes_history
#[test]
fn delete_removes_history() {
    let (_env, db) = shot_db();
    let id = db
        .insert_history(&make_history_entry(None, json!({})))
        .unwrap();
    db.delete_shot(id).unwrap();
    assert!(search(&db, no_data()).is_empty());
}

/// TestDeleteShot.test_delete_cleans_up_orphaned_profile
#[test]
fn delete_cleans_up_orphaned_profile() {
    let (_env, db) = shot_db();
    let id = db
        .insert_history(&make_history_entry(None, json!({})))
        .unwrap();
    db.delete_shot(id).unwrap();
    let p2 = make_profile(json!({"id": "p-new", "name": "New Profile"}));
    db.insert_history(&make_history_entry(
        Some(p2),
        json!({"id": "h2", "file": "s2.zst"}),
    ))
    .unwrap();
    let results = search(&db, no_data());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["profile"]["name"], "New Profile");
    // The orphan's FTS rows went with it.
    assert!(
        db.autocomplete_profile_name("Test Espresso")
            .unwrap()
            .is_empty()
    );
}

/// TestDeleteShot.test_delete_preserves_shared_profile
#[test]
fn delete_preserves_shared_profile() {
    let (_env, db) = shot_db();
    let profile = make_profile(json!({}));
    let id = db
        .insert_history(&make_history_entry(
            Some(profile.clone()),
            json!({"id": "h1", "file": "s1.zst"}),
        ))
        .unwrap();
    db.insert_history(&make_history_entry(
        Some(profile),
        json!({"id": "h2", "file": "s2.zst"}),
    ))
    .unwrap();
    db.delete_shot(id).unwrap();
    let results = search(&db, no_data());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["profile"]["name"], "Test Espresso");
}

/// TestDeleteShot.test_delete_nonexistent_shot
#[test]
fn delete_nonexistent_shot() {
    let (_env, db) = shot_db();
    db.delete_shot(99999).unwrap();
}

/// The shape and number types of a search result: the profile JSON columns
/// parsed, `time` a float, `null`s present.
#[test]
fn search_result_shape() {
    let (_env, db) = shot_db();
    let profile = make_profile(
        json!({"display": {"accentColor": "#FF7A00"}, "temperature": 93.5, "final_weight": 36.0}),
    );
    let id = db
        .insert_history(&make_history_entry(
            Some(profile),
            json!({"id": "h1", "file": "s1.zst", "time": 1760000000.25}),
        ))
        .unwrap();
    let results = search(&db, no_data());
    assert_eq!(
        pyfmt::dumps_default(&results[0]),
        pyfmt::dumps_default(&json!({
            "id": "h1",
            "db_key": id,
            "time": 1760000000.25,
            "file": "s1.zst",
            "debug_file": null,
            "name": "Test Espresso",
            "data": null,
            "push_to_brew_time": null,
            "profile": {
                "id": "profile-001",
                "db_key": 1,
                "author": "Test Author",
                "author_id": "author-001",
                "display": {"accentColor": "#FF7A00"},
                "final_weight": 36,
                "last_changed": 0.0,
                "name": "Test Espresso",
                "temperature": 93.5,
                "stages": [
                    {"key": "stage-1", "name": "Preinfusion", "type": "pressure"},
                    {"key": "stage-2", "name": "Extraction", "type": "pressure"},
                ],
                "variables": [],
                "previous_authors": [],
            },
        }))
    );
}

/// A history entry without a profile, or without the keys Python indexes,
/// fails like Python's KeyError/TypeError and stores no history row.
#[test]
fn malformed_entries_are_rejected() {
    let (_env, db) = shot_db();
    let mut entry = make_history_entry(None, json!({"file": "x.zst"}));
    entry.as_object_mut().unwrap().remove("profile");
    assert!(db.insert_history(&entry).is_err());
    let mut entry = make_history_entry(None, json!({"file": "y.zst"}));
    entry.as_object_mut().unwrap().remove("profile_name");
    assert!(db.insert_history(&entry).is_err());
    assert!(search(&db, no_data()).is_empty());
}
