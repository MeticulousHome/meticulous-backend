//! A history the Python backend wrote, read by Rust.
//!
//! `tests/data/python_written/` was written by `generate_python_written.py`
//! through the Python code (ShotDataBase, ShotManager's zstd CLI call,
//! PourOverHistoryManager, the bug report functions); `expected.json` holds
//! what the Python read paths answered. Rust opens a copy and must answer the
//! same, value for value and type for type.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use common::{Env, data_dir, load_json};
use history::bug_reports::parse_fiql;
use history::files::{decompressed_content_type, read_patched};
use history::pour_over::PourOverSession;
use history::{HistoryDb, LocalZone, SearchParams};
use serde_json::{Value, json};

fn copy_dir(from: &Path, to: &Path, rename: &dyn Fn(&str) -> String) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to.join(&name), rename);
        } else {
            std::fs::create_dir_all(to).unwrap();
            std::fs::copy(entry.path(), to.join(rename(&name))).unwrap();
        }
    }
}

/// The fixture copied into a machine layout, opened like the backend does.
fn python_history() -> (Env, HistoryDb, Value) {
    let source = data_dir().join("python_written");
    let env = Env::new();
    let history = env.paths().history_dir;
    std::fs::create_dir_all(&history).unwrap();
    std::fs::copy(
        source.join("history.sqlite"),
        history.join("history.sqlite"),
    )
    .unwrap();
    copy_dir(
        &source.join("shot_files"),
        &history.join("shots"),
        &|name| name.replace('_', ":"),
    );
    copy_dir(
        &source.join("pour_over_files"),
        &history.join("pour-over"),
        &|name| name.to_string(),
    );
    let db = env.open_in(LocalZone::Fixed(0));
    assert_eq!(
        db.migrate().unwrap(),
        history::MigrationOutcome::AlreadyCurrent
    );
    let expected = load_json("python_written/expected.json");
    (env, db, expected)
}

/// json.dumps equality: keys in order, ints and floats apart.
fn assert_same(context: &str, actual: &Value, expected: &Value) {
    let actual = pyfmt::dumps_default(actual);
    let expected = pyfmt::dumps_default(expected);
    assert_eq!(actual, expected, "{context}");
}

/// search_history with every parameter, as GET/POST /history answer it.
#[test]
fn search_history_reads_python_rows_and_files() {
    let (_env, db, expected) = python_history();
    for case in expected["search"].as_array().unwrap() {
        let params = SearchParams::from_json(case["params"].as_object().unwrap()).unwrap();
        let result = db.search_history(&params).unwrap();
        assert_same(
            &format!("search {}", case["params"]),
            &Value::Array(result),
            &case["result"],
        );
    }
}

/// statistics, autocomplete, ratings and the upload index.
#[test]
fn other_shot_queries_read_python_rows() {
    let (_env, db, expected) = python_history();
    assert_same(
        "statistics",
        &db.statistics().unwrap(),
        &expected["statistics"],
    );
    for (prefix, result) in expected["autocomplete"].as_object().unwrap() {
        assert_same(
            &format!("autocomplete {prefix:?}"),
            &Value::Array(db.autocomplete_profile_name(prefix).unwrap()),
            result,
        );
    }
    for (uuid, rating) in expected["ratings"].as_object().unwrap() {
        assert_eq!(
            db.get_shot_rating(uuid)
                .map(Value::String)
                .unwrap_or(Value::Null),
            *rating,
            "rating of {uuid}"
        );
    }
    let entries = expected["entries"].as_array().unwrap();
    let first_file = entries[0]["file"].as_str().unwrap();
    assert_same(
        "upload index",
        &json!([
            db.list_history_files(None, 200).unwrap(),
            db.list_history_files(Some(first_file), 1).unwrap(),
        ]),
        &expected["upload_index"],
    );
}

/// The shot files ShotManager wrote with the zstd CLI, served decompressed
/// with the non-finite tokens patched (ZstdHistoryHandler.serve_decompressed_file).
#[test]
fn shot_files_written_by_python_are_served_like_python() {
    let (env, _db, expected) = python_history();
    for (file, served) in expected["served"].as_object().unwrap() {
        let path = env.paths().shot_dir().join(file);
        let body = read_patched(&path).unwrap();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            served["body"].as_str().unwrap(),
            "{file}"
        );
        assert_eq!(decompressed_content_type(&path), served["content_type"]);
    }
}

/// PourOverHistoryManager.search/latest on rows and records Python saved, and
/// a re-post of a saved session is "existing".
#[test]
fn pour_over_history_reads_python_rows_and_records() {
    let (_env, db, expected) = python_history();
    let pour_over = &expected["pour_over"];
    assert_same(
        "search",
        &Value::Array(db.search_pour_over(50, true, None, None).unwrap()),
        &pour_over["search"],
    );
    assert_same(
        "search asc profile",
        &Value::Array(
            db.search_pour_over(50, false, None, Some("profile"))
                .unwrap(),
        ),
        &pour_over["search_asc_profile"],
    );
    let after = pour_over["saved"][0][0]["file"].as_str().unwrap();
    assert_same(
        "search after",
        &Value::Array(db.search_pour_over(50, true, Some(after), None).unwrap()),
        &pour_over["search_after"],
    );
    assert_same(
        "latest",
        &db.latest_pour_over().unwrap().unwrap(),
        &pour_over["latest"],
    );
    // The record file holds what the session validates to: posting it again
    // is idempotent, and the answer is the stored row.
    let records = data_dir().join("python_written/pour_over_files");
    let mut checked = 0;
    for day in std::fs::read_dir(records).unwrap() {
        for record in std::fs::read_dir(day.unwrap().path()).unwrap() {
            let raw = history::pour_over::read_compressed_record(&record.unwrap().path()).unwrap();
            let payload: Value = pyfmt::loads_lenient(std::str::from_utf8(&raw).unwrap()).unwrap();
            let session = PourOverSession::validate(&payload).unwrap();
            let saved = db.save_pour_over(&session).unwrap();
            assert!(!saved.created);
            assert_same(
                "existing",
                &saved.history,
                &db.find_pour_over(&session.id).unwrap().unwrap(),
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 2);
}

/// The bug report list, the dispatch list and a report's info.
#[test]
fn bug_reports_read_python_rows() {
    let (_env, db, expected) = python_history();
    let reports = &expected["bug_reports"];
    assert_same(
        "page",
        &db.list_report_page(0, 10, None).unwrap(),
        &reports["page"],
    );
    assert_same(
        "page small",
        &db.list_report_page(1, 1, None).unwrap(),
        &reports["page_small"],
    );
    let (filter, invalid) = parse_fiql(Some("status==queued")).unwrap();
    assert!(!invalid);
    assert_same(
        "queued",
        &db.list_report_page(0, 10, filter.as_ref()).unwrap(),
        &reports["queued"],
    );
    let dispatch: Vec<Value> = db
        .queued_report_rows()
        .unwrap()
        .iter()
        .map(|r| r.dispatch_payload())
        .collect();
    assert_same("dispatch", &Value::Array(dispatch), &reports["dispatch"]);
    let row = db
        .get_report_row("018f0a2b-1234-7abc-8def-0123456789ab")
        .unwrap()
        .unwrap();
    assert_same(
        "report info",
        &row.to_report_info(),
        &reports["report_info"],
    );
}
