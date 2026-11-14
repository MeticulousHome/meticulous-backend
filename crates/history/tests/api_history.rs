//! The storage parts of `tests/test_api_history.py`: what the history routes
//! read and write through the store, shots recorded the way `ShotManager`
//! records them (the shot file, then `insert_history`).
//!
//! The HTTP parts (status codes, headers, redirects, the local-only check,
//! the 400/422/500 answers) belong to the HTTP layer.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use common::Env;
use history::files;
use history::files::{
    HistoryFile, compressed_download_name, decompressed_content_type, latest_debug_file,
    list_directory, read_patched, resolve_history_file, shot_file_paths, write_shot_file,
};
use history::{HistoryDb, LocalZone, SearchParams};
use serde_json::{Map, Value, json};

const DAY: &str = "2026-10-09";
// datetime(2026, 10, 9, 8, 30).timestamp() in UTC (the tests run in a fixed zone).
const FIRST_SHOT_TIME: f64 = 1791534600.0;
const SECOND_SHOT_TIME: f64 = 1791534660.0;
const THIRD_SHOT_TIME: f64 = 1791534720.0;
const FIRST_SHOT_FILE: &str = "2026-10-09/08:30:00.shot.json.zst";
const SECOND_SHOT_FILE: &str = "2026-10-09/08:31:00.shot.json.zst";
const THIRD_SHOT_FILE: &str = "2026-10-09/08:32:00.shot.json.zst";

fn make_profile(id: &str, name: &str, stages: &[&str], temperature: f64) -> Value {
    json!({
        "id": id,
        "author": "Meticulous",
        "author_id": "author-0001",
        "display": {"accentColor": "#FF7A00"},
        "final_weight": 36.0,
        "last_changed": 1760000000.0,
        "name": name,
        "temperature": temperature,
        "stages": stages.iter().enumerate().map(|(i, s)| json!({"key": format!("{id}-stage-{i}"), "name": s, "type": "pressure"})).collect::<Vec<_>>(),
        "variables": [],
        "previous_authors": [],
    })
}

fn classic() -> Value {
    make_profile("p-classic", "Classic", &["Preinfusion", "Extraction"], 93.0)
}
fn classic_v2() -> Value {
    make_profile(
        "p-classic-v2",
        "Classic",
        &["Preinfusion", "Extraction"],
        94.0,
    )
}
fn bloom() -> Value {
    make_profile("p-bloom", "Bloom", &["Bloom", "Pour"], 93.0)
}

/// The data point layout Shot.addShotData writes.
fn stored_point(time_ms: i64, pressure: Value, weight: Value, gravimetric_flow: Value) -> Value {
    json!({
        "shot": {
            "pressure": pressure,
            "flow": 2.0,
            "weight": weight,
            "gravimetric_flow": gravimetric_flow,
            "setpoints": {"active": "pressure", "pressure": 9.0},
        },
        "time": time_ms,
        "profile_time": time_ms - 200,
        "status": "brewing",
    })
}

fn point(time_ms: i64, pressure: f64, weight: f64) -> Value {
    stored_point(time_ms, json!(pressure), json!(weight), json!(1.5))
}

struct Machine {
    _env: Env,
    db: HistoryDb,
    next_id: u32,
}

impl Machine {
    /// The fresh_machine fixture.
    fn fresh() -> Self {
        let env = Env::new();
        let db = env.open_in(LocalZone::Fixed(0));
        db.migrate().unwrap();
        Self {
            _env: env,
            db,
            next_id: 0,
        }
    }

    /// record_shot: ShotManager.stop() writes the file, then the database row.
    fn record_shot(
        &mut self,
        profile: Value,
        start: f64,
        points: Vec<Value>,
        push_to_brew_time: i64,
    ) -> String {
        self.next_id += 1;
        let id = format!("shot-{}", self.next_id);
        let (_, file) = shot_file_paths(self.db.zone(), start).unwrap();
        let mut shot = json!({
            "time": start,
            "profile_name": profile["name"],
            "data": points,
            "id": id,
            "push_to_brew_time": push_to_brew_time,
            "profile": profile,
        });
        write_shot_file(&self.db.paths().shot_dir().join(&file), &shot).unwrap();
        shot["file"] = json!(file);
        self.db.insert_history(&shot).unwrap();
        id
    }

    fn record_three_shots(&mut self) -> (String, String, String) {
        let first = self.record_shot(
            classic(),
            FIRST_SHOT_TIME,
            vec![point(1000, 9.0, 0.0), point(1100, 8.5, 0.4)],
            1500,
        );
        let second = self.record_shot(bloom(), SECOND_SHOT_TIME, vec![point(1000, 2.0, 1.0)], 0);
        let third = self.record_shot(
            classic_v2(),
            THIRD_SHOT_TIME,
            vec![point(1000, 9.1, 0.0)],
            0,
        );
        (first, second, third)
    }

    fn search(&self, query: Value) -> Vec<Value> {
        let params = SearchParams::from_json(query.as_object().unwrap()).unwrap();
        let result = self.db.search_history(&params).unwrap();
        common::assert_search_json_agrees(&self.db, &params, &result);
        result
    }

    /// Rewrite a recorded shot file's JSON text.
    fn rewrite_shot(&self, file: &str, edit: impl Fn(&str) -> String) {
        let path = self.db.paths().shot_dir().join(file);
        let raw = String::from_utf8(files::decompress_file(&path, None).unwrap()).unwrap();
        files::write_compressed(&path, edit(&raw).as_bytes()).unwrap();
    }
}

#[allow(clippy::too_many_arguments)]
fn expected_entry(
    id: &str,
    db_key: i64,
    profile: &Value,
    profile_key: i64,
    time: f64,
    file: &str,
    data: Value,
    ptb: Value,
) -> Value {
    let mut profile_json: Map<String, Value> = profile.as_object().unwrap().clone();
    profile_json.insert("db_key".into(), json!(profile_key));
    // {**profile, "db_key": key}: db_key goes last in Python's literal but the
    // comparison there is a dict comparison; the store answers in its order.
    let ordered = pyfmt::object([
        ("id", profile_json["id"].clone()),
        ("db_key", json!(profile_key)),
        ("author", profile_json["author"].clone()),
        ("author_id", profile_json["author_id"].clone()),
        ("display", profile_json["display"].clone()),
        ("final_weight", json!(36)),
        ("last_changed", profile_json["last_changed"].clone()),
        ("name", profile_json["name"].clone()),
        (
            "temperature",
            json!(profile_json["temperature"].as_f64().unwrap() as i64),
        ),
        ("stages", profile_json["stages"].clone()),
        ("variables", profile_json["variables"].clone()),
        ("previous_authors", profile_json["previous_authors"].clone()),
    ]);
    pyfmt::object([
        ("id", json!(id)),
        ("db_key", json!(db_key)),
        ("time", json!(time)),
        ("file", json!(file)),
        ("debug_file", Value::Null),
        ("name", profile["name"].clone()),
        ("data", data),
        ("push_to_brew_time", ptb),
        ("profile", ordered),
    ])
}

fn assert_same(actual: &Value, expected: &Value) {
    assert_eq!(pyfmt::dumps_default(actual), pyfmt::dumps_default(expected));
}

/// TestHistoryList.test_fresh_machine_lists_no_shots
#[test]
fn fresh_machine_lists_no_shots() {
    let machine = Machine::fresh();
    assert!(machine.search(json!({})).is_empty());
}

/// TestHistoryList.test_lists_recorded_shots_newest_first_with_shot_data
/// (the Python asserts `temperature: 93.0`; SQLite's INTEGER affinity stores
/// a whole float as an integer, so both implementations answer 93).
#[test]
fn lists_recorded_shots_newest_first_with_shot_data() {
    let mut machine = Machine::fresh();
    let (first, second, third) = machine.record_three_shots();
    let result = Value::Array(machine.search(json!({})));
    assert_same(
        &result,
        &json!([
            expected_entry(
                &third,
                3,
                &classic_v2(),
                3,
                THIRD_SHOT_TIME,
                THIRD_SHOT_FILE,
                json!([point(1000, 9.1, 0.0)]),
                json!(0)
            ),
            expected_entry(
                &second,
                2,
                &bloom(),
                2,
                SECOND_SHOT_TIME,
                SECOND_SHOT_FILE,
                json!([point(1000, 2.0, 1.0)]),
                json!(0)
            ),
            expected_entry(
                &first,
                1,
                &classic(),
                1,
                FIRST_SHOT_TIME,
                FIRST_SHOT_FILE,
                json!([point(1000, 9.0, 0.0), point(1100, 8.5, 0.4)]),
                json!(1500)
            ),
        ]),
    );
}

/// TestHistoryList.test_dump_data_false_omits_shot_file_contents
#[test]
fn dump_data_false_omits_shot_file_contents() {
    let mut machine = Machine::fresh();
    let (first, _, _) = machine.record_three_shots();
    let result = machine.search(json!({"dump_data": "false", "sort": "asc", "max_results": "1"}));
    assert_same(
        &Value::Array(result),
        &json!([expected_entry(
            &first,
            1,
            &classic(),
            1,
            FIRST_SHOT_TIME,
            FIRST_SHOT_FILE,
            Value::Null,
            Value::Null
        )]),
    );
}

/// TestHistoryList.test_query_parameters_filter_sort_and_limit, with the
/// query arguments as the GET handler passes them (strings, lists).
#[test]
fn query_parameters_filter_sort_and_limit() {
    let mut machine = Machine::fresh();
    let (first, second, third) = machine.record_three_shots();
    let ids = |query: Value| -> Vec<String> {
        machine
            .search(query)
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect()
    };
    let (first, second, third) = (first.as_str(), second.as_str(), third.as_str());
    assert_eq!(ids(json!({"sort": "asc"})), vec![first, second, third]);
    assert_eq!(ids(json!({"max_results": "2"})), vec![third, second]);
    // max_results <= 0 disables the limit.
    assert_eq!(ids(json!({"max_results": "0"})), vec![third, second, first]);
    assert_eq!(ids(json!({"query": "bloom"})), vec![second]);
    assert_eq!(ids(json!({"query": "Preinfusion"})), vec![third, first]);
    assert_eq!(ids(json!({"ids": [first, third]})), vec![third, first]);
    assert_eq!(ids(json!({"ids": ["2"]})), vec![second]);
    assert_eq!(ids(json!({"ids": ["p-classic-v2"]})), vec![third]);
    assert_eq!(
        ids(json!({"order_by": ["profile"], "sort": "asc"})),
        vec![second, first, third]
    );
    assert_eq!(
        ids(json!({"start_date": SECOND_SHOT_TIME.to_string()})),
        vec![third, second]
    );
    assert_eq!(
        ids(json!({"end_date": SECOND_SHOT_TIME.to_string()})),
        vec![second, first]
    );
    assert!(ids(json!({"ids": ["no-such-shot"]})).is_empty());
}

/// TestHistoryList.test_shot_with_unreadable_file_is_left_out_of_the_listing
#[test]
fn shot_with_unreadable_file_is_left_out_of_the_listing() {
    let mut machine = Machine::fresh();
    let (first, second, _) = machine.record_three_shots();
    std::fs::remove_file(machine.db.paths().shot_dir().join(THIRD_SHOT_FILE)).unwrap();
    let ids: Vec<Value> = machine
        .search(json!({}))
        .iter()
        .map(|e| e["id"].clone())
        .collect();
    assert_eq!(ids, vec![json!(second), json!(first)]);
}

/// A shot file as the Python wrote it before the ESP32 stopped sending
/// non-finite values: json.dumps writes the bare token.
fn write_python_shot_with(machine: &mut Machine, token: &str) -> String {
    let id = machine.record_shot(classic(), FIRST_SHOT_TIME, vec![point(1000, 9.0, 0.0)], 0);
    let path = machine.db.paths().shot_dir().join(FIRST_SHOT_FILE);
    let raw = String::from_utf8(read_patched(&path).unwrap()).unwrap();
    let raw = raw.replace(
        "\"gravimetric_flow\": 1.5",
        &format!("\"gravimetric_flow\": {token}"),
    );
    std::fs::write(&path, zstd::encode_all(raw.as_bytes(), 10).unwrap()).unwrap();
    id
}

/// TestHistoryList.test_non_finite_values_in_shot_file_are_served_as_zero
#[test]
fn non_finite_values_in_shot_file_are_listed_as_zero() {
    let mut machine = Machine::fresh();
    let id = write_python_shot_with(&mut machine, "Infinity");
    let result = machine.search(json!({}));
    assert_eq!(result[0]["id"], json!(id));
    assert_same(
        &result[0]["data"],
        &json!([stored_point(1000, json!(9.0), json!(0.0), json!(0.0))]),
    );
}

/// Tokens the `: Infinity`/`: NaN` patch does not reach: Python reads them
/// as floats and json.dumps writes them back as bare tokens, which the
/// handler's path does; the values of the internal path hold `null`.
#[test]
fn unpatched_non_finite_tokens_are_listed_like_python() {
    let mut machine = Machine::fresh();
    machine.record_shot(classic(), FIRST_SHOT_TIME, vec![point(1000, 9.0, 0.0)], 0);
    machine.rewrite_shot(FIRST_SHOT_FILE, |raw| {
        raw.replace(
            "\"gravimetric_flow\": 1.5",
            "\"gravimetric_flow\": -Infinity, \"extra\": [NaN, Infinity]",
        )
    });
    let params = SearchParams::default();
    let values = machine.db.search_history(&params).unwrap();
    assert_eq!(values[0]["data"][0]["shot"]["extra"], json!([null, null]));
    let expected = pyfmt::tornado_json_encode(&Value::Array(values)).replace(
        "\"gravimetric_flow\": null, \"extra\": [null, null]",
        "\"gravimetric_flow\": -Infinity, \"extra\": [NaN, Infinity]",
    );
    assert!(expected.contains("-Infinity"));
    assert_eq!(
        common::search_json(&machine.db, &params, pyfmt::JsonStyle::TORNADO),
        expected
    );
}

/// What else Python reads that JSON values cannot hold: integers past 64
/// bits (kept exact), `-0` (the int 0), floats past the double range
/// (`Infinity`), lone surrogates (kept escaped). The internal path cannot
/// read such a file and leaves the shot out.
#[test]
fn shot_data_keeps_what_python_reads() {
    let mut machine = Machine::fresh();
    machine.record_shot(classic(), FIRST_SHOT_TIME, vec![point(1000, 9.0, 0.0)], 0);
    machine.rewrite_shot(FIRST_SHOT_FILE, |raw| {
        raw.replace(
            "\"time\": 1000",
            "\"time\": 123456789012345678901234567890, \"zero\": -0, \"big\": 1e400, \"s\": \"\\ud800</\"",
        )
    });
    let params = SearchParams::default();
    assert!(machine.db.search_history(&params).unwrap().is_empty());
    let body = common::search_json(&machine.db, &params, pyfmt::JsonStyle::TORNADO);
    assert!(
        body.contains(
            "\"time\": 123456789012345678901234567890, \"zero\": 0, \"big\": Infinity, \"s\": \"\\ud800<\\/\""
        ),
        "{body}"
    );
}

/// A repeated key in a shot file: `dict` keeps the last value, in the place
/// of the first. Both paths agree (checked by `search`).
#[test]
fn repeated_keys_in_a_shot_file_keep_the_last_value() {
    let mut machine = Machine::fresh();
    machine.record_shot(classic(), FIRST_SHOT_TIME, vec![point(1000, 9.0, 0.0)], 7);
    machine.rewrite_shot(FIRST_SHOT_FILE, |raw| {
        let raw = raw.replace("\"flow\": 2.0", "\"flow\": 2.0, \"weight\": 5, \"flow\": 3.0");
        format!(
            "{}, \"data\": [], \"d\\u0061ta\": [{{\"x\": 1, \"y\": 2, \"x\": 3}}], \"push_to_brew_time\": 9}}",
            raw.strip_suffix('}').unwrap()
        )
    });
    let result = machine.search(json!({}));
    assert_eq!(
        pyfmt::dumps_default(&result[0]["data"]),
        r#"[{"x": 3, "y": 2}]"#
    );
    assert_eq!(result[0]["push_to_brew_time"], json!(9));
    let result = machine.search(json!({"dump_data": false}));
    assert!(result[0]["data"].is_null());
}

/// TestHistoryList.test_shot_with_unreadable_file_is_left_out_of_the_listing,
/// for every way a file can be unreadable; both paths leave the same rows out.
#[test]
fn shots_with_bad_files_are_left_out_of_both_listings() {
    let mut machine = Machine::fresh();
    let mut ids = Vec::new();
    for minute in 0..6 {
        ids.push(machine.record_shot(
            classic(),
            FIRST_SHOT_TIME + 60.0 * f64::from(minute),
            vec![point(1000, 9.0, 0.0)],
            0,
        ));
    }
    let file = |minute: u32| format!("{DAY}/08:{:02}:00.shot.json.zst", 30 + minute);
    let shots = machine.db.paths().shot_dir();
    std::fs::write(shots.join(file(1)), b"not zstd").unwrap();
    machine.rewrite_shot(&file(2), |raw| raw[..raw.len() - 1].to_string());
    machine.rewrite_shot(&file(3), |raw| format!("[{raw}]"));
    let path = shots.join(file(4));
    let mut raw = files::decompress_file(&path, None).unwrap();
    raw.extend_from_slice(b" \xff");
    files::write_compressed(&path, &raw).unwrap();
    machine.rewrite_shot(&file(5), |raw| {
        raw.replace("\"brewing\"", "\"brew\u{1}ing\"")
    });
    let listed: Vec<Value> = machine
        .search(json!({}))
        .iter()
        .map(|e| e["id"].clone())
        .collect();
    assert_eq!(listed, vec![json!(ids[0])]);
}

/// TestHistoryList.test_linked_debug_file_is_reported
#[test]
fn linked_debug_file_is_reported() {
    let mut machine = Machine::fresh();
    machine.record_shot(classic(), FIRST_SHOT_TIME, vec![point(1000, 9.0, 0.0)], 0);
    machine.db.link_debug_file(1, FIRST_SHOT_FILE).unwrap();
    assert_eq!(machine.search(json!({}))[0]["debug_file"], FIRST_SHOT_FILE);
}

/// TestHistorySearchPost.test_search_body_filters_and_limits
#[test]
fn search_body_filters_and_limits() {
    let mut machine = Machine::fresh();
    let (first, second, third) = machine.record_three_shots();
    let result = machine.search(json!({"ids": [first, 2], "sort": "asc", "dump_data": false}));
    let ids: Vec<Value> = result.iter().map(|e| e["id"].clone()).collect();
    assert_eq!(ids, vec![json!(first), json!(second)]);
    assert!(result.iter().all(|e| e["data"].is_null()));
    let result = machine.search(json!({"query": "classic", "max_results": 1}));
    assert_eq!(result[0]["id"], json!(third));
    assert_same(&result[0]["data"], &json!([point(1000, 9.1, 0.0)]));
}

/// TestLastShot: ShotManager.getLastShot() is search_history(desc, 1); a
/// shot of a profile already stored reuses its key.
#[test]
fn last_shot_is_the_newest_recorded_shot() {
    let mut machine = Machine::fresh();
    assert!(
        machine
            .search(json!({"sort": "desc", "max_results": 1}))
            .is_empty()
    );
    machine.record_three_shots();
    let fourth_time = 1791536400.0; // 09:00
    let fourth = machine.record_shot(
        classic(),
        fourth_time,
        vec![stored_point(1, json!(1), json!(0), json!(1.5))],
        0,
    );
    let last = machine.search(json!({"sort": "desc", "max_results": 1}));
    assert_same(
        &last[0],
        &expected_entry(
            &fourth,
            4,
            &classic(),
            1,
            fourth_time,
            &format!("{DAY}/09:00:00.shot.json.zst"),
            json!([stored_point(1, json!(1), json!(0), json!(1.5))]),
            json!(0),
        ),
    );
}

/// TestProfileSearch
#[test]
fn profile_search_suggestions() {
    let mut machine = Machine::fresh();
    assert!(machine.db.autocomplete_profile_name("").unwrap().is_empty());
    assert!(
        machine
            .db
            .autocomplete_profile_name("classic")
            .unwrap()
            .is_empty()
    );
    machine.record_three_shots();
    assert_eq!(
        machine.db.autocomplete_profile_name("").unwrap(),
        vec![
            json!({"profile": "Classic", "type": "profile"}),
            json!({"profile": "Bloom", "type": "profile"})
        ]
    );
    assert_eq!(
        machine.db.autocomplete_profile_name("bloom").unwrap(),
        vec![
            json!({"profile": "Bloom", "type": "profile"}),
            json!({"profile": "Bloom", "type": "stage", "name": "Bloom"}),
        ]
    );
    assert_eq!(
        machine.db.autocomplete_profile_name("infus").unwrap(),
        vec![json!({"profile": "Classic", "type": "stage", "name": "Preinfusion"})]
    );
    assert!(
        machine
            .db
            .autocomplete_profile_name("nothing-like-this")
            .unwrap()
            .is_empty()
    );
}

/// TestStatistics (without the ESP mileage, which the handler adds).
#[test]
fn statistics_count_shots_and_profile_versions() {
    let mut machine = Machine::fresh();
    assert_eq!(
        machine.db.statistics().unwrap(),
        json!({"totalSavedShots": 0, "byProfile": []})
    );
    machine.record_three_shots();
    machine.record_shot(classic(), 1791536400.0, vec![point(1, 1.0, 0.0)], 0);
    assert_eq!(
        machine.db.statistics().unwrap(),
        json!({
            "totalSavedShots": 4,
            "byProfile": [
                {"name": "Classic", "count": 3, "profileVersions": 2},
                {"name": "Bloom", "count": 1, "profileVersions": 1},
            ],
        })
    );
}

/// TestUploadIndex (the handler clamps max_results the same way).
#[test]
fn upload_index_is_ordered_by_file_and_paged_by_cursor() {
    let mut machine = Machine::fresh();
    assert!(machine.db.list_history_files(None, 200).unwrap().is_empty());
    machine.record_three_shots();
    assert_eq!(
        machine.db.list_history_files(None, 200).unwrap(),
        vec![
            json!({"file": FIRST_SHOT_FILE}),
            json!({"file": SECOND_SHOT_FILE}),
            json!({"file": THIRD_SHOT_FILE})
        ]
    );
    assert_eq!(
        machine
            .db
            .list_history_files(Some(FIRST_SHOT_FILE), 1)
            .unwrap(),
        vec![json!({"file": SECOND_SHOT_FILE})]
    );
    assert!(
        machine
            .db
            .list_history_files(Some(THIRD_SHOT_FILE), 200)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        machine.db.list_history_files(None, -5).unwrap(),
        vec![json!({"file": FIRST_SHOT_FILE})]
    );
}

fn ratings_in_db(db: &HistoryDb) -> Vec<(String, String)> {
    db.with_connection(|conn| {
        let mut statement = conn.prepare(
            "SELECT shot_annotation.history_uuid, shot_rating.basic FROM shot_annotation JOIN shot_rating ON shot_annotation.id = shot_rating.annotation_id",
        )?;
        let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    })
    .unwrap()
}

/// TestShotRating
#[test]
fn shot_ratings() {
    let mut machine = Machine::fresh();
    let (first, second, _) = machine.record_three_shots();
    let db = &machine.db;
    assert_eq!(db.get_shot_rating(&first), None);
    assert_eq!(db.get_shot_rating("no-such-shot"), None);
    assert_eq!(db.get_shot_rating(""), None);

    assert!(db.rate_shot(&first, Some("like")));
    assert_eq!(ratings_in_db(db), vec![(first.clone(), "like".to_string())]);
    assert_eq!(db.get_shot_rating(&first).as_deref(), Some("like"));
    assert!(db.rate_shot(&first, Some("dislike")));
    assert_eq!(
        ratings_in_db(db),
        vec![(first.clone(), "dislike".to_string())]
    );
    assert!(db.rate_shot(&second, Some("like")));
    // A body without "rating" clears it too (data.get("rating", None)).
    assert!(db.rate_shot(&first, None));
    assert_eq!(
        ratings_in_db(db),
        vec![(second.clone(), "like".to_string())]
    );
    assert_eq!(db.get_shot_rating(&first), None);

    assert!(!db.rate_shot("no-such-shot", Some("like")));
    // Ratings are keyed by the shot uuid, not the integer db_key.
    assert!(!db.rate_shot("1", Some("like")));
    assert!(!db.rate_shot(&first, Some("LIKE")));
    assert_eq!(ratings_in_db(db), vec![(second, "like".to_string())]);
}

fn set_mtime(path: &Path, seconds: f64) {
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(seconds);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

/// TestShotFiles: listings, decompressed and raw files, 404s.
#[test]
fn shot_files() {
    let mut machine = Machine::fresh();
    let root = machine.db.paths().shot_dir();
    // SHOT_PATH only exists once the first shot is written.
    assert_eq!(
        resolve_history_file(&root, "", false),
        HistoryFile::NotFound
    );
    let (first, _, _) = machine.record_three_shots();
    for (index, name) in [FIRST_SHOT_FILE, SECOND_SHOT_FILE, THIRD_SHOT_FILE]
        .iter()
        .enumerate()
    {
        set_mtime(&root.join(name), FIRST_SHOT_TIME + index as f64);
    }

    let HistoryFile::Directory(dir) = resolve_history_file(&root, "", false) else {
        panic!("root listing");
    };
    let listing: Vec<Value> = list_directory(&dir)
        .unwrap()
        .iter()
        .map(|e| e.to_json())
        .collect();
    assert_eq!(listing, vec![json!({"name": DAY, "url": DAY})]);
    let HistoryFile::Directory(dir) = resolve_history_file(&root, DAY, false) else {
        panic!("day listing");
    };
    let listing: Vec<Value> = list_directory(&dir)
        .unwrap()
        .iter()
        .map(|e| e.to_json())
        .collect();
    assert_eq!(
        listing,
        vec![
            json!({"name": "08:32:00.shot.json", "url": "08:32:00.shot.json.zst"}),
            json!({"name": "08:31:00.shot.json", "url": "08:31:00.shot.json.zst"}),
            json!({"name": "08:30:00.shot.json", "url": "08:30:00.shot.json.zst"}),
        ]
    );

    // Served decompressed as written, with or without the .zst suffix.
    let expected = json!({
        "time": FIRST_SHOT_TIME,
        "profile_name": "Classic",
        "data": [point(1000, 9.0, 0.0), point(1100, 8.5, 0.4)],
        "id": first,
        "push_to_brew_time": 1500,
        "profile": classic(),
    });
    for url in [
        FIRST_SHOT_FILE,
        FIRST_SHOT_FILE.strip_suffix(".zst").unwrap(),
    ] {
        let HistoryFile::Decompressed(path) = resolve_history_file(&root, url, false) else {
            panic!("{url} is served decompressed");
        };
        assert_eq!(decompressed_content_type(&path), "application/json");
        let body: Value =
            pyfmt::loads_lenient(&String::from_utf8(read_patched(&path).unwrap()).unwrap())
                .unwrap();
        assert_same(&body, &expected);
    }

    // ?compressed serves the file on disk, named for download.
    let HistoryFile::Raw(path) = resolve_history_file(&root, FIRST_SHOT_FILE, true) else {
        panic!("raw file");
    };
    assert_eq!(path, root.join(FIRST_SHOT_FILE));
    assert_eq!(
        compressed_download_name("Meticulous42", FIRST_SHOT_FILE),
        "Meticulous42_2026_10_09_08:30:00.shot.json.zst"
    );

    for url in [
        format!("{DAY}/23:59:59.shot.json.zst"),
        "1999-01-01/00:00:00.shot.json".to_string(),
    ] {
        assert_eq!(
            resolve_history_file(&root, &url, false),
            HistoryFile::NotFound
        );
    }
    assert_eq!(
        resolve_history_file(&root, &format!("{DAY}/23:59:59.shot.json.zst"), true),
        HistoryFile::NotFound
    );
    assert_eq!(
        resolve_history_file(&root, "../history.sqlite", false),
        HistoryFile::Forbidden
    );
}

/// TestShotFiles.test_non_finite_values_are_served_as_zero
#[test]
fn non_finite_values_are_served_as_zero() {
    let mut machine = Machine::fresh();
    write_python_shot_with(&mut machine, "NaN");
    let root = machine.db.paths().shot_dir();
    let HistoryFile::Decompressed(path) = resolve_history_file(&root, FIRST_SHOT_FILE, false)
    else {
        panic!("decompressed");
    };
    let body: Value = serde_json::from_slice(&read_patched(&path).unwrap()).unwrap();
    assert_same(
        &body["data"],
        &json!([stored_point(1000, json!(9.0), json!(0.0), json!(0.0))]),
    );
}

fn write_debug_shot(debug: &Path, relative: &str, payload: &Value, mtime: f64) {
    let path = debug.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        zstd::encode_all(pyfmt::dumps_default(payload).as_bytes(), 3).unwrap(),
    )
    .unwrap();
    set_mtime(&path, mtime);
}

/// TestDebugFiles: listings, files, CSV and the last debug file.
#[test]
fn debug_files() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path().join("debug");
    assert_eq!(
        resolve_history_file(&debug, "", false),
        HistoryFile::NotFound
    );
    assert_eq!(latest_debug_file(&debug), None);
    std::fs::create_dir_all(&debug).unwrap();
    assert_eq!(latest_debug_file(&debug), None);

    let payload = json!({"type": "shot", "profile_name": "Classic", "data": [], "logs": []});
    write_debug_shot(
        &debug,
        &format!("{DAY}/08:30:00.shot.json.zst"),
        &payload,
        FIRST_SHOT_TIME,
    );
    write_debug_shot(
        &debug,
        &format!("{DAY}/08:40:00.purge.json.zst"),
        &payload,
        SECOND_SHOT_TIME,
    );
    let HistoryFile::Directory(day) = resolve_history_file(&debug, DAY, false) else {
        panic!("listing");
    };
    let listing: Vec<Value> = list_directory(&day)
        .unwrap()
        .iter()
        .map(|e| e.to_json())
        .collect();
    assert_eq!(
        listing,
        vec![
            json!({"name": "08:40:00.purge.json", "url": "08:40:00.purge.json.zst"}),
            json!({"name": "08:30:00.shot.json", "url": "08:30:00.shot.json.zst"}),
        ]
    );
    let HistoryFile::Decompressed(path) =
        resolve_history_file(&debug, &format!("{DAY}/08:30:00.shot.json.zst"), false)
    else {
        panic!("file");
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&read_patched(&path).unwrap()).unwrap(),
        payload
    );

    let csv = debug.join(DAY).join("sensors.csv.zst");
    std::fs::write(&csv, zstd::encode_all(&b"t,p\n0,9.0\n"[..], 3).unwrap()).unwrap();
    set_mtime(&csv, FIRST_SHOT_TIME - 100.0);
    let HistoryFile::Decompressed(path) =
        resolve_history_file(&debug, &format!("{DAY}/sensors.csv.zst"), false)
    else {
        panic!("csv");
    };
    assert_eq!(decompressed_content_type(&path), "text/csv");
    assert_eq!(read_patched(&path).unwrap(), b"t,p\n0,9.0\n");

    write_debug_shot(
        &debug,
        "2026-10-08/23:00:00.shot.json.zst",
        &json!({}),
        THIRD_SHOT_TIME,
    );
    assert_eq!(
        latest_debug_file(&debug).unwrap(),
        debug.join("2026-10-08/23:00:00.shot.json.zst")
    );
}
