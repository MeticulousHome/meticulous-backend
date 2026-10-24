//! The database parts of `tests/test_bug_report_api.py`: the `bug_reports`
//! rows the report module writes and the lists it answers with.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Env, insert_row};
use history::bug_reports::{
    BugReportValues, FilterOp, FilterValue, QueuedReport, collected_db_values, draft_db_values,
    parse_fiql,
};
use history::{Error, HistoryDb};
use serde_json::{Map, Value, json};

const DISPATCHED_AT: i64 = 1_779_000_000;
const QUEUED_ID: &str = "018f0a2b-1234-7abc-8def-0123456789ab";
const SECOND_ID: &str = "018f0a2b-1234-7abc-8def-0123456789ac";
const THIRD_ID: &str = "018f0a2b-1234-7abc-8def-0123456789ad";
const SERIAL: &str = "MACHINE-SERIAL-1";

fn reports() -> (Env, HistoryDb) {
    let env = Env::new();
    let db = env.open();
    db.migrate().unwrap();
    (env, db)
}

/// `_insert_report_row` of the test module (and the inline inserts).
fn insert_report_row(env: &Env, local_id: &str, status: &str, creation_time: i64, values: Value) {
    let mut row: Map<String, Value> = Map::new();
    row.insert("localID".into(), json!(local_id));
    row.insert("issueTime".into(), json!(creation_time));
    row.insert("creationTime".into(), json!(creation_time));
    row.insert("status".into(), json!(status));
    for (key, value) in values.as_object().unwrap() {
        row.insert(key.clone(), value.clone());
    }
    insert_row(&env.connect(), "bug_reports", &row);
}

fn dispatch(local_id: &str) -> QueuedReport {
    QueuedReport {
        local_id: local_id.into(),
        ticket: json!(4242),
        issue_time: None,
        description: None,
        name: None,
        email: None,
    }
}

fn all_local_ids(db: &HistoryDb) -> Vec<String> {
    db.sweep_rows()
        .unwrap()
        .iter()
        .map(|r| r.local_id.as_str().unwrap().to_string())
        .collect()
}

/// test_fiql_filter_ignores_invalid_fields_and_rejects_empty
#[test]
fn fiql_filter_ignores_invalid_fields_and_rejects_empty() {
    let (valid, invalid) = parse_fiql(Some("unknown==x;status==draft,creationTime=gt=10")).unwrap();
    assert!(valid.is_some());
    assert!(!invalid);
    let groups = valid.unwrap().groups;
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0][0].column, "status");
    assert_eq!(groups[1][0].op, FilterOp::Gt);
    assert_eq!(groups[1][0].value, FilterValue::Int(10));
    let (empty, empty_invalid) = parse_fiql(Some("unknown==x")).unwrap();
    assert!(empty.is_none());
    assert!(empty_invalid);
    assert_eq!(parse_fiql(None).unwrap(), (None, false));
    assert_eq!(parse_fiql(Some("")).unwrap(), (None, false));
    // int("abc") raises ValueError in Python (the handler answers 500).
    assert!(parse_fiql(Some("creationTime=gt=abc")).is_err());
}

/// The filters a FIQL list query applies, row for row (the expected rows
/// were checked against `_parse_fiql` + `_list_report_page` on the same rows).
#[test]
fn fiql_filters_select_rows_like_sqlalchemy() {
    let (env, db) = reports();
    insert_report_row(
        &env,
        QUEUED_ID,
        "draft",
        100,
        json!({"machineInfo": true, "ticketNumber": 7, "description": "50%_off"}),
    );
    insert_report_row(
        &env,
        SECOND_ID,
        "submitted",
        200,
        json!({"machineInfo": false, "eventID": "e-1"}),
    );
    insert_report_row(&env, THIRD_ID, "queued", 300, json!({}));
    let ids = |filter: &str| -> Vec<String> {
        let (condition, invalid) = parse_fiql(Some(filter)).unwrap();
        assert!(!invalid, "{filter}");
        db.list_report_page(0, 10, condition.as_ref()).unwrap()["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["localID"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids("status==draft"), vec![QUEUED_ID]);
    assert_eq!(ids("status==dra*"), vec![QUEUED_ID]);
    assert_eq!(ids("status==?ueued"), vec![THIRD_ID]);
    assert_eq!(ids("creationTime=gt=150"), vec![THIRD_ID, SECOND_ID]);
    assert_eq!(ids("creationTime=le=200;status!=draft"), vec![SECOND_ID]);
    assert_eq!(
        ids("status==queued,creationTime=lt=150"),
        vec![THIRD_ID, QUEUED_ID]
    );
    assert_eq!(ids("machineInfo==true"), vec![QUEUED_ID]);
    assert_eq!(ids("machineInfo==no"), vec![SECOND_ID]);
    assert_eq!(ids("eventID==null"), vec![THIRD_ID, QUEUED_ID]);
    assert_eq!(ids("eventID!=NULL"), vec![SECOND_ID]);
    assert_eq!(ids("ticketNumber==%207"), vec![QUEUED_ID]);
    // % and _ in the value are literal, * and ? are the wildcards.
    assert_eq!(ids("description==50%25_*"), vec![QUEUED_ID]);
    assert!(ids("description==5_%25*").is_empty());
    // SQLAlchemy raises ArgumentError for an ordering against None/True/False.
    assert!(parse_fiql(Some("creationTime=gt=null")).is_err());
    assert!(parse_fiql(Some("machineInfo=ge=true")).is_err());
}

/// test_list_report_page_returns_newest_first_with_machine_id
#[test]
fn list_report_page_returns_newest_first_with_machine_id() {
    let (env, db) = reports();
    insert_report_row(
        &env,
        "older-id",
        "draft",
        1,
        json!({"machineID": "older-machine", "machineInfo": false, "machineLogs": false, "machineStatus": false}),
    );
    insert_report_row(
        &env,
        "newer-id",
        "draft",
        2,
        json!({"machineID": "newer-machine", "machineInfo": true, "machineLogs": true, "machineStatus": true}),
    );
    let response = db.list_report_page(0, 1, None).unwrap();
    let first = &response["content"][0];
    assert_eq!(first["localID"], "newer-id");
    assert_eq!(first["machineID"], "newer-machine");
    assert_eq!(first["dateAndTime"], 2);
    assert_eq!(first["issueTime"], 2);
    assert_eq!(first["status"], "draft");
    assert_eq!(response["hasMore"], true);
    assert_eq!(
        pyfmt::dumps_default(first),
        pyfmt::dumps_default(&json!({
            "description": null,
            "dateAndTime": 2,
            "issueTime": 2,
            "attachments": {"debugFiles": {"automatic": []}, "machineInfo": true, "machineLogs": true, "machineStatus": true},
            "multimedia": null,
            "machineID": "newer-machine",
            "eventID": null,
            "baseEventID": null,
            "ticket": null,
            "localID": "newer-id",
            "status": "draft",
        }))
    );
    let last = db.list_report_page(1, 1, None).unwrap();
    assert_eq!(last["hasMore"], false);
    assert_eq!(last["content"][0]["localID"], "older-id");
}

/// test_draft_patch_persists_ticket_and_multimedia_in_db_and_report_info and
/// test_draft_patch_adds_user_debug_file_to_draft_directory (the row part).
#[test]
fn draft_patch_updates_the_row() {
    let (env, db) = reports();
    insert_report_row(
        &env,
        "local-test-id",
        "draft",
        1,
        json!({"logFiles": null, "machineInfo": true, "machineLogs": true, "machineStatus": true}),
    );
    let patch = BugReportValues {
        ticket_number: Some(json!(1234)),
        multimedia: Some(json!(2)),
        ..BugReportValues::default()
    };
    assert!(db.update_report("local-test-id", &patch).unwrap());
    let attachments = json!({"debugFiles": {"automatic": [], "user": ["2026-05-18/14:00:00.user.json.zst"]},
                             "machineInfo": true, "machineLogs": true, "machineStatus": true});
    assert!(
        db.update_report("local-test-id", &draft_db_values(&attachments))
            .unwrap()
    );
    let row = db.get_report_row("local-test-id").unwrap().unwrap();
    assert_eq!(row.ticket_number, json!(1234));
    assert_eq!(row.multimedia, json!(2));
    assert_eq!(row.log_files, json!("2026-05-18/14:00:00.user.json.zst"));
    assert_eq!(row.machine_info, json!(true));
    assert!(!db.update_report("unknown-id", &patch).unwrap());
}

/// test_submit_update_persists_db_and_report_info and
/// test_submit_without_ticket_preserves_existing_ticket (the row part).
#[test]
fn submit_updates_the_row() {
    let (env, db) = reports();
    insert_report_row(
        &env,
        "submit-id",
        "draft",
        1,
        json!({"ticketNumber": 42, "machineInfo": false}),
    );
    let submitted = BugReportValues {
        event_id: Some(json!("event-1")),
        submission_time: Some(json!(3)),
        status: Some(json!("submitted")),
        ..BugReportValues::default()
    };
    assert!(db.update_report("submit-id", &submitted).unwrap());
    let row = db.get_report_row("submit-id").unwrap().unwrap();
    assert_eq!(row.event_id, json!("event-1"));
    assert_eq!(row.ticket_number, json!(42));
    assert_eq!(row.submission_time, json!(3));
    assert_eq!(row.status, json!("submitted"));
    assert_eq!(row.machine_info, json!(false));
    let with_ticket = BugReportValues {
        ticket_number: Some(json!(43)),
        ..submitted
    };
    assert!(db.update_report("submit-id", &with_ticket).unwrap());
    assert_eq!(
        db.get_report_row("submit-id")
            .unwrap()
            .unwrap()
            .ticket_number,
        json!(43)
    );
}

/// The `Boolean` columns refuse what SQLAlchemy refuses.
#[test]
fn boolean_columns_are_strict() {
    let (env, db) = reports();
    insert_report_row(&env, "id", "draft", 1, json!({}));
    let bad = BugReportValues {
        machine_info: Some(json!("yes")),
        ..BugReportValues::default()
    };
    assert!(db.update_report("id", &bad).is_err());
    let ok = BugReportValues {
        machine_info: Some(json!(1)),
        ..BugReportValues::default()
    };
    assert!(db.update_report("id", &ok).unwrap());
    assert_eq!(
        db.get_report_row("id").unwrap().unwrap().machine_info,
        json!(true)
    );
}

/// test_delete_draft_removes_all_report_representations_and_db_row,
/// test_delete_draft_allows_submitted_report and
/// test_delete_draft_returns_not_found_for_unknown_local_id (the row part).
#[test]
fn delete_removes_the_row() {
    let (env, db) = reports();
    insert_report_row(&env, QUEUED_ID, "draft", 1, json!({}));
    insert_report_row(&env, SECOND_ID, "submitted", 1, json!({}));
    assert!(db.delete_report_row(QUEUED_ID).unwrap());
    assert!(db.get_report_row(QUEUED_ID).unwrap().is_none());
    assert!(db.delete_report_row(SECOND_ID).unwrap());
    assert!(!db.delete_report_row(THIRD_ID).unwrap());
}

/// test_dispatch_post_queues_the_report_and_emits_upload_report (the row
/// and the payload).
#[test]
fn dispatch_queues_the_report() {
    let (_env, db) = reports();
    let issue_time = DISPATCHED_AT - 600;
    let row = db
        .insert_queued_report(
            &QueuedReport {
                issue_time: Some(issue_time),
                description: Some("Grinder jammed".into()),
                name: Some("Ada".into()),
                email: Some("ada@example.com".into()),
                ..dispatch(QUEUED_ID)
            },
            DISPATCHED_AT,
            &json!(SERIAL),
        )
        .unwrap();
    assert_eq!(
        row.dispatch_payload(),
        json!({
            "localID": QUEUED_ID,
            "machineID": SERIAL,
            "ticket": 4242,
            "issueTime": issue_time,
            "requestTime": DISPATCHED_AT,
            "description": "Grinder jammed",
            "name": "Ada",
            "email": "ada@example.com",
        })
    );
    assert_eq!(row.status, json!("queued"));
    assert_eq!(row.submission_time, Value::Null);
    assert_eq!(row.event_id, Value::Null);
    assert_eq!(row.log_files, Value::Null);
    assert_eq!(row.machine_status, Value::Null);
    assert_eq!(row.contact_email, json!("ada@example.com"));
}

/// test_dispatch_post_defaults_issue_time_to_now_and_contact_to_none
#[test]
fn dispatch_defaults_issue_time_to_now_and_contact_to_none() {
    let (_env, db) = reports();
    let row = db
        .insert_queued_report(&dispatch(QUEUED_ID), DISPATCHED_AT, &json!(SERIAL))
        .unwrap();
    assert_eq!(
        row.dispatch_payload(),
        json!({
            "localID": QUEUED_ID,
            "machineID": SERIAL,
            "ticket": 4242,
            "issueTime": DISPATCHED_AT,
            "requestTime": DISPATCHED_AT,
            "description": null,
            "name": null,
            "email": null,
        })
    );
}

/// test_dispatch_post_with_the_same_local_id_is_a_conflict and
/// test_dispatch_post_conflicts_with_a_report_in_any_status
#[test]
fn dispatch_of_an_existing_local_id_is_a_conflict() {
    let (env, db) = reports();
    db.insert_queued_report(
        &QueuedReport {
            description: Some("first".into()),
            ..dispatch(QUEUED_ID)
        },
        DISPATCHED_AT,
        &json!(SERIAL),
    )
    .unwrap();
    let second = QueuedReport {
        ticket: json!(9999),
        description: Some("second".into()),
        name: Some("Eve".into()),
        ..dispatch(QUEUED_ID)
    };
    assert!(matches!(
        db.insert_queued_report(&second, DISPATCHED_AT + 60, &json!(SERIAL)),
        Err(Error::DuplicateLocalId)
    ));
    let row = db.get_report_row(QUEUED_ID).unwrap().unwrap();
    assert_eq!(row.ticket_number, json!(4242));
    assert_eq!(row.description, json!("first"));
    assert_eq!(row.contact_name, Value::Null);
    assert_eq!(row.creation_time, json!(DISPATCHED_AT));
    for status in ["draft", "submitted"] {
        insert_report_row(
            &env,
            status,
            status,
            DISPATCHED_AT,
            json!({"machineID": SERIAL}),
        );
        assert!(matches!(
            db.insert_queued_report(&dispatch(status), DISPATCHED_AT, &json!(SERIAL)),
            Err(Error::DuplicateLocalId)
        ));
        assert_eq!(
            db.get_report_row(status).unwrap().unwrap().status,
            json!(status)
        );
    }
}

/// test_dispatch_get_lists_only_queued_reports_oldest_first
#[test]
fn dispatch_list_holds_only_queued_reports_oldest_first() {
    let (env, db) = reports();
    insert_report_row(
        &env,
        SECOND_ID,
        "queued",
        DISPATCHED_AT + 10,
        json!({"issueTime": DISPATCHED_AT + 5, "ticketNumber": 2, "machineID": SERIAL, "description": "later"}),
    );
    insert_report_row(
        &env,
        "018f0a2b-1234-7abc-8def-0123456789ae",
        "draft",
        DISPATCHED_AT - 5,
        json!({"ticketNumber": 3, "contactName": "Draft"}),
    );
    insert_report_row(
        &env,
        THIRD_ID,
        "queued",
        DISPATCHED_AT,
        json!({"ticketNumber": 4, "machineID": SERIAL, "contactName": "Ada", "contactEmail": "ada@example.com"}),
    );
    insert_report_row(
        &env,
        "018f0a2b-1234-7abc-8def-0123456789af",
        "submitted",
        DISPATCHED_AT - 10,
        json!({"ticketNumber": 5, "contactName": "Done"}),
    );
    insert_report_row(
        &env,
        QUEUED_ID,
        "queued",
        DISPATCHED_AT,
        json!({"issueTime": DISPATCHED_AT - 600, "ticketNumber": 1, "machineID": SERIAL}),
    );
    let content: Vec<Value> = db
        .queued_report_rows()
        .unwrap()
        .iter()
        .map(|r| r.dispatch_payload())
        .collect();
    assert_eq!(
        Value::Array(content),
        json!([
            {"localID": QUEUED_ID, "machineID": SERIAL, "ticket": 1, "issueTime": DISPATCHED_AT - 600,
             "requestTime": DISPATCHED_AT, "description": null, "name": null, "email": null},
            {"localID": THIRD_ID, "machineID": SERIAL, "ticket": 4, "issueTime": DISPATCHED_AT,
             "requestTime": DISPATCHED_AT, "description": null, "name": "Ada", "email": "ada@example.com"},
            {"localID": SECOND_ID, "machineID": SERIAL, "ticket": 2, "issueTime": DISPATCHED_AT + 5,
             "requestTime": DISPATCHED_AT + 10, "description": "later", "name": null, "email": null},
        ])
    );
}

/// test_dispatch_get_is_empty_without_queued_reports
#[test]
fn dispatch_list_is_empty_without_queued_reports() {
    let (env, db) = reports();
    insert_report_row(&env, QUEUED_ID, "draft", DISPATCHED_AT, json!({}));
    assert!(db.queued_report_rows().unwrap().is_empty());
}

/// test_create_by_local_id_collects_around_the_dispatch_and_promotes_the_row
/// (the row part: the collected values turn the queued row into a draft).
#[test]
fn collecting_a_dispatch_promotes_the_row_to_draft() {
    let (_env, db) = reports();
    db.insert_queued_report(&dispatch(QUEUED_ID), DISPATCHED_AT, &json!(SERIAL))
        .unwrap();
    let report_info = json!({
        "localID": QUEUED_ID,
        "dateAndTime": DISPATCHED_AT,
        "issueTime": DISPATCHED_AT - 600,
        "description": null,
        "multimedia": null,
        "machineID": SERIAL,
        "attachments": {"debugFiles": {"automatic": ["2026-05-18/10:00:00.shot.json.zst"], "user": []},
                        "machineInfo": true, "machineLogs": true, "machineStatus": true},
        "ticket": 4242,
    });
    assert!(
        db.update_report(QUEUED_ID, &collected_db_values(&report_info).unwrap())
            .unwrap()
    );
    let row = db.get_report_row(QUEUED_ID).unwrap().unwrap();
    assert_eq!(row.status, json!("draft"));
    assert_eq!(row.log_files, json!("2026-05-18/10:00:00.shot.json.zst"));
    assert_eq!(row.contact_name, Value::Null);
}

/// `_insert_report`: the row of a draft created without a dispatch.
#[test]
fn insert_report_stores_the_collected_draft() {
    let (_env, db) = reports();
    db.insert_report(&json!({
        "localID": "local-test-id",
        "dateAndTime": 10,
        "issueTime": 5,
        "description": "desc",
        "multimedia": 1,
        "machineID": "machine",
        "attachments": {"debugFiles": {"user": ["u.zst"], "automatic": ["a.zst", "u.zst"]},
                        "machineInfo": true, "machineLogs": false, "machineStatus": true},
        "ticket": 9,
    }))
    .unwrap();
    let info = db
        .get_report_row("local-test-id")
        .unwrap()
        .unwrap()
        .to_report_info();
    assert_eq!(
        pyfmt::dumps_default(&info),
        pyfmt::dumps_default(&json!({
            "description": "desc",
            "dateAndTime": 10,
            "issueTime": 5,
            "attachments": {"debugFiles": {"automatic": ["u.zst", "a.zst"]}, "machineInfo": true, "machineLogs": false, "machineStatus": true},
            "multimedia": 1,
            "machineID": "machine",
            "eventID": null,
            "baseEventID": null,
            "ticket": 9,
            "localID": "local-test-id",
            "status": "draft",
        }))
    );
    assert!(db.insert_report(&json!({"localID": "x"})).is_err());
}

/// test_sweep_keeps_fresh_queued_reports_and_expires_stale_ones and
/// test_sweep_still_deletes_draft_rows_without_files (the rows the sweep
/// reads and deletes).
#[test]
fn sweep_reads_every_row() {
    let (env, db) = reports();
    insert_report_row(&env, QUEUED_ID, "queued", DISPATCHED_AT - 60, json!({}));
    insert_report_row(
        &env,
        SECOND_ID,
        "draft",
        DISPATCHED_AT - 60,
        json!({"submissionTime": 5}),
    );
    let rows = db.sweep_rows().unwrap();
    assert_eq!(rows.len(), 2);
    let draft = rows
        .iter()
        .find(|r| r.local_id == json!(SECOND_ID))
        .unwrap();
    assert_eq!(draft.status, json!("draft"));
    assert_eq!(draft.creation_time, json!(DISPATCHED_AT - 60));
    assert_eq!(draft.submission_time, json!(5));
    assert!(db.delete_report_row(SECOND_ID).unwrap());
    assert_eq!(all_local_ids(&db), vec![QUEUED_ID]);
}

/// test_list_report_page_includes_queued_reports_with_their_ticket
#[test]
fn list_report_page_includes_queued_reports_with_their_ticket() {
    let (_env, db) = reports();
    db.insert_queued_report(&dispatch(QUEUED_ID), DISPATCHED_AT, &json!(SERIAL))
        .unwrap();
    let page = db.list_report_page(0, 10, None).unwrap();
    let content = page["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["localID"], QUEUED_ID);
    assert_eq!(content[0]["ticket"], 4242);
    assert_eq!(content[0]["dateAndTime"], DISPATCHED_AT);
}
