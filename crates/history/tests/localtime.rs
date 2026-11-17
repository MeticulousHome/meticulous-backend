//! The local wall clock times the history stores, against CPython.
//!
//! `insert_history` stores `datetime.fromtimestamp(time)` and `search_history`
//! answers `datetime.timestamp()` of it; `tests/data/localtime.json` holds
//! CPython's answers in zones with DST changes (`generate_localtime.py`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use chrono::{Offset, TimeZone};
use common::load_json;
use history::LocalZone;
use history::localtime::parse_stored;

fn zone(name: &str) -> LocalZone {
    let tz: chrono_tz::Tz = name.parse().unwrap();
    LocalZone::Custom(Arc::new(move |utc| {
        let instant = chrono::DateTime::from_timestamp(utc, 0)
            .unwrap()
            .naive_utc();
        tz.offset_from_utc_datetime(&instant)
            .fix()
            .local_minus_utc()
    }))
}

#[test]
fn from_timestamp_and_timestamp_match_cpython() {
    let reference = load_json("localtime.json");
    for (name, cases) in reference.as_object().unwrap() {
        let zone = zone(name);
        for case in cases["fromtimestamp"].as_array().unwrap() {
            let timestamp: f64 = case[0].as_str().unwrap().parse().unwrap();
            let wall = zone.from_timestamp(timestamp).unwrap().to_sqlite().unwrap();
            assert_eq!(
                wall,
                case[1].as_str().unwrap(),
                "{name} fromtimestamp({timestamp})"
            );
        }
        for case in cases["timestamp"].as_array().unwrap() {
            let stored = parse_stored(case[0].as_str().unwrap()).unwrap();
            let timestamp = zone.stored_timestamp(stored);
            assert_eq!(
                pyfmt::repr_f64(timestamp),
                case[1].as_str().unwrap(),
                "{name} {}.timestamp()",
                case[0]
            );
        }
    }
}

/// Shot file names come from the same local time (`_timestampToFilePaths`).
#[test]
fn shot_file_paths_use_local_time() {
    let zone = zone("America/New_York");
    let (folder, file) = history::files::shot_file_paths(&zone, 1793512800.0).unwrap();
    assert_eq!(folder, "2026-11-01");
    assert_eq!(file, "2026-11-01/01:00:00.shot.json.zst");
}
