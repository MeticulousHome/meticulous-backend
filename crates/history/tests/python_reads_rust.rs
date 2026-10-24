//! A history written by Rust, read by the Python backend.
//!
//! Rust boots a machine, records shots, ratings, bug reports and Pour Over
//! sessions, then `tests/data/read_rust_written.py` reads it with the Python
//! code (ShotDataBase, PourOverHistoryManager, the bug report functions,
//! `alembic current`) and answers the same queries. Then the Python rolls the
//! database back a revision as an older image does, finding the downgrade
//! script only in the backup directory Rust filled; Rust then updates it again.
//!
//! The test needs the repository's virtualenv (`uv sync --group dev`) and an
//! importable `log_redactor` (the submodule, or `PYTHONPATH`). Without them it
//! prints why and passes, unless `HISTORY_REQUIRE_PYTHON=1`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::Env;
use history::bug_reports::QueuedReport;
use history::files::{read_patched, shot_file_paths, write_shot_file};
use history::pour_over::{PourOverLimits, PourOverSession};
use history::{HistoryDb, LocalZone, SearchParams};
use serde_json::{Value, json};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The command that runs a Python file of tests/data with the repo's venv.
fn python(work: &Path) -> Option<Command> {
    let interpreter = repo_root().join(".venv/bin/python");
    let skip = |reason: String| -> Option<Command> {
        if std::env::var("HISTORY_REQUIRE_PYTHON").as_deref() == Ok("1") {
            panic!("the Python cross-check cannot run: {reason}");
        }
        eprintln!("SKIPPED python cross-check: {reason}");
        None
    };
    if !interpreter.exists() {
        return skip(format!(
            "{} is missing (uv sync --group dev)",
            interpreter.display()
        ));
    }
    let make = || {
        let mut command = Command::new(&interpreter);
        command
            .current_dir(repo_root())
            .env("TZ", "UTC")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("CONFIG_PATH", work.join("config"))
            .env("LOG_PATH", work.join("logs"))
            .env("HISTORY_PATH", work.join("default-history"))
            .env("DEBUG_HISTORY_PATH", work.join("debug"))
            .env("USER_DB_MIGRATION_DIR", work.join("default-dbmigrations"))
            .env("REDACTION_KEY_PATH", work.join(".redaction_key"));
        command
    };
    let probe = make()
        .args([
            "-c",
            "import log_redactor, shot_database, pour_over_history, api.bug_report",
        ])
        .output()
        .ok()?;
    if !probe.status.success() {
        return skip(format!(
            "the backend does not import: {}",
            String::from_utf8_lossy(&probe.stderr)
                .lines()
                .last()
                .unwrap_or_default()
        ));
    }
    Some(make())
}

fn profile(id: &str, name: &str, stages: &[&str], temperature: f64) -> Value {
    json!({
        "id": id,
        "author": "Meticulous",
        "author_id": "author-0001",
        "display": {"accentColor": "#FF7A00"},
        "final_weight": 36.0,
        "last_changed": 1760000000.0,
        "name": name,
        "temperature": temperature,
        "stages": stages.iter().enumerate().map(|(i, s)| json!({"key": format!("{id}-{i}"), "name": s, "type": "pressure"})).collect::<Vec<_>>(),
        "variables": [],
        "previous_authors": [],
    })
}

fn session(id: &str, started_at: &str) -> Value {
    json!({
        "schemaVersion": 4,
        "id": id,
        "brewType": "pour_over",
        "mode": "free_pour",
        "name": "Free Pour",
        "source": "dial",
        "startedAt": started_at,
        "completedAt": started_at.replace(":00.000Z", ":45.120Z"),
        "recipe": {"profileId": null, "profileName": "Free Pour", "doseG": 15, "temperatureC": 92,
                    "targetWaterG": null, "targetDurationMs": null, "pourTargets": []},
        "measurements": {"serverBaselineG": -0.4, "setupG": 149.2, "waterPouredG": 225.3,
                          "beverageG": 196.8, "retainedG": 28.5, "durationMs": 135000, "status": "measured"},
        "pours": [],
        "samples": [{"t": 0, "w": 0, "f": 0.8, "p": 1}],
        "completion": "brewer_removed",
        "sync": {"status": "pending", "attempts": 0},
    })
}

struct Written {
    uuids: Vec<String>,
    files: Vec<String>,
}

/// Record a history through the Rust store, as the backend will.
fn write_history(db: &mut HistoryDb) -> Written {
    let classic = profile("p-classic", "Classic", &["Preinfusion", "Extraction"], 93.0);
    let bloom = profile("p-bloom", "Bloom caf\u{e9}", &["Bloom"], 92.5);
    let shots = [
        (1760000000.0, classic.clone(), json!(1500)),
        (1760000060.5, bloom, json!(0)),
        (1760000120.25, classic, json!(0)),
    ];
    let zone = LocalZone::Fixed(0);
    let mut written = Written {
        uuids: Vec::new(),
        files: Vec::new(),
    };
    for (index, (time, profile, ptb)) in shots.into_iter().enumerate() {
        let (_, file) = shot_file_paths(&zone, time).unwrap();
        let uuid = format!("00000000-0000-4000-8000-00000000000{index}");
        let mut shot = json!({
            "time": time,
            "profile_name": profile["name"],
            "data": [{"shot": {"pressure": 9.0, "flow": 2.0, "weight": index as f64, "gravimetric_flow": 1.5,
                               "setpoints": {"active": "pressure", "pressure": 9.0}},
                      "time": 1000, "profile_time": 800, "status": "brewing"}],
            "id": uuid,
            "push_to_brew_time": ptb,
            "profile": profile,
        });
        write_shot_file(&db.paths().shot_dir().join(&file), &shot).unwrap();
        shot["file"] = json!(file);
        let id = db.insert_history(&shot).unwrap();
        if index == 0 {
            db.link_debug_file(id, "2025-10-09/08:53:20.shot.json.zst")
                .unwrap();
        }
        written.uuids.push(uuid);
        written.files.push(file);
    }
    assert!(db.rate_shot(&written.uuids[0], Some("like")));
    assert!(db.rate_shot(&written.uuids[1], Some("dislike")));

    db.insert_report(&json!({
        "localID": "018f0a2b-1234-7abc-8def-0123456789ab",
        "dateAndTime": 1760000300,
        "issueTime": 1760000000,
        "description": "Grinder jammed",
        "multimedia": 2,
        "machineID": "MACHINE-001",
        "attachments": {"debugFiles": {"user": ["a.zst"], "automatic": ["b.zst"]},
                        "machineInfo": true, "machineLogs": false, "machineStatus": true},
        "ticket": null,
    }))
    .unwrap();
    db.insert_queued_report(
        &QueuedReport {
            local_id: "018f0a2b-1234-7abc-8def-0123456789ac".into(),
            ticket: json!(77),
            issue_time: None,
            description: Some("Steam wand".into()),
            name: Some("Ada".into()),
            email: Some("ada@example.com".into()),
        },
        1760000400,
        &json!("MACHINE-001"),
    )
    .unwrap();

    db.set_pour_over_limits(PourOverLimits {
        min_free_bytes: 0,
        ..PourOverLimits::default()
    });
    for (id, started) in [
        (
            "11111111-2222-4333-8444-555555555555",
            "2026-08-16T08:00:00.000Z",
        ),
        (
            "33333333-4444-4555-8666-777777777777",
            "2026-08-16T09:30:00.000Z",
        ),
    ] {
        let session = PourOverSession::validate(&session(id, started)).unwrap();
        assert!(db.save_pour_over(&session).unwrap().created);
    }
    written
}

fn same(context: &str, rust: &Value, python: &Value) {
    assert_eq!(
        pyfmt::dumps_default(rust),
        pyfmt::dumps_default(python),
        "{context}: Rust vs Python"
    );
}

/// The Python backend reads what Rust wrote, alembic reports the head, and
/// an older image rolls the database back with the script Rust backed up.
#[test]
fn python_reads_and_rolls_back_a_rust_history() {
    let env = Env::new();
    let work = env.dir.path().join("python");
    std::fs::create_dir_all(&work).unwrap();
    let Some(mut command) = python(&work) else {
        return;
    };
    let mut db = env.open_in(LocalZone::Fixed(0));
    db.migrate().unwrap();
    let written = write_history(&mut db);

    let searches = vec![
        json!({}),
        json!({"dump_data": false, "sort": "asc"}),
        json!({"query": "bloom"}),
        json!({"ids": [written.uuids[2], 1]}),
        json!({"order_by": ["profile"], "dump_data": false}),
        json!({"start_date": 1760000060.5, "dump_data": false}),
    ];
    let prefixes = ["", "Pre", "caf"];
    let queries = json!({
        "search": searches,
        "autocomplete": prefixes,
        "ratings": written.uuids,
        "served": written.files,
    });
    let rust = json!({
        "search": searches.iter().map(|p| Value::Array(db.search_history(&SearchParams::from_json(p.as_object().unwrap()).unwrap()).unwrap())).collect::<Vec<_>>(),
        "statistics": db.statistics().unwrap(),
        "autocomplete": prefixes.iter().map(|p| Value::Array(db.autocomplete_profile_name(p).unwrap())).collect::<Vec<_>>(),
        "ratings": written.uuids.iter().map(|u| db.get_shot_rating(u).map(Value::String).unwrap_or(Value::Null)).collect::<Vec<_>>(),
        "upload_index": db.list_history_files(None, 200).unwrap(),
        "served": written.files.iter().map(|f| Value::String(String::from_utf8(read_patched(&db.paths().shot_dir().join(f)).unwrap()).unwrap())).collect::<Vec<_>>(),
        "pour_over_search": db.search_pour_over(50, true, None, None).unwrap(),
        "pour_over_latest": db.latest_pour_over().unwrap(),
        "bug_report_page": db.list_report_page(0, 10, None).unwrap(),
        "dispatch": db.queued_report_rows().unwrap().iter().map(|r| r.dispatch_payload()).collect::<Vec<_>>(),
    });
    let paths = db.paths().clone();
    drop(db);

    let queries_path = work.join("queries.json");
    let answers_path = work.join("answers.json");
    std::fs::write(&queries_path, queries.to_string()).unwrap();
    let output = command
        .arg(repo_root().join("crates/history/tests/data/read_rust_written.py"))
        .arg(&paths.history_dir)
        .arg(&paths.migration_backup_dir)
        .arg(&queries_path)
        .arg(&answers_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the Python reader failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python: Value =
        pyfmt::loads_lenient(&std::fs::read_to_string(&answers_path).unwrap()).unwrap();

    assert_eq!(python["revision_before"], "c4d2e9f1a7b3");
    assert_eq!(python["alembic_current"], "c4d2e9f1a7b3 (head)");
    assert_eq!(python["boot_error"], Value::Null);
    assert_eq!(python["revision_after_boot"], "c4d2e9f1a7b3");
    for key in [
        "search",
        "statistics",
        "autocomplete",
        "ratings",
        "upload_index",
        "served",
        "pour_over_search",
        "pour_over_latest",
        "bug_report_page",
        "dispatch",
    ] {
        same(key, &rust[key], &python[key]);
    }
    // The records Rust wrote are what the sessions validate to in Python.
    for (record, id) in python["pour_over_records"].as_array().unwrap().iter().zip([
        "33333333-4444-4555-8666-777777777777",
        "11111111-2222-4333-8444-555555555555",
    ]) {
        assert_eq!(record["id"], id);
        assert_eq!(record["recipe"]["doseG"], json!(15.0));
    }

    // The rollback ran the downgrade from the script Rust backed up.
    assert_eq!(python["rollback_error"], Value::Null);
    assert_eq!(python["revision_after_rollback"], "b71d84a4c2ef");
    let columns: Vec<&str> = python["bug_report_columns_after_rollback"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(!columns.contains(&"contactName") && !columns.contains(&"contactEmail"));

    // Back on the Rust image: the update runs again and the history is intact.
    let db = HistoryDb::open(paths, LocalZone::Fixed(0)).unwrap();
    assert_eq!(
        db.migrate().unwrap(),
        history::MigrationOutcome::Upgraded {
            from: "b71d84a4c2ef".into()
        }
    );
    assert_eq!(db.statistics().unwrap(), rust["statistics"]);
    let page = db.list_report_page(0, 10, None).unwrap();
    assert_eq!(page["content"].as_array().unwrap().len(), 2);
    let queued = db.queued_report_rows().unwrap();
    assert_eq!(queued[0].contact_name, Value::Null);
}
