//! tests/test_bug_report_api.py: the report handlers.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use history::bug_reports::BugReportValues;
use http::Method;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::bug_report::test_support::*;
use crate::bug_report::{
    self as reports, LOCAL_ID_RE, MACHINE_STATUS_NAME, REPORT_LOG_NAME, UPLOAD_REPORT_EVENT,
};
use crate::config::keys;
use crate::testing::{call, json as body_json};

const MACHINE_SERIAL: &str = "machine-test-id";
const QUEUED_ID: &str = "018f0a2b-1234-7abc-8def-0123456789ab";
const SECOND_ID: &str = "018f0a2b-1234-7abc-8def-0123456789ac";
const THIRD_ID: &str = "018f0a2b-1234-7abc-8def-0123456789ad";
const DISPATCHED_AT: i64 = 1_700_000_000;
const HOUR: i64 = 60 * 60;
const AUTOMATIC_DEBUG_FILE: &str = "2026-05-18/10:00:00.shot.json.zst";

fn with_serial(app: &App) {
    app.config.set(
        keys::SYSTEM,
        keys::MACHINE_SERIAL_NUMBER,
        json!(MACHINE_SERIAL),
    );
}

/// The `report_module`, `machine_serial` and `clock` fixtures.
fn dispatch_app() -> (tempfile::TempDir, Arc<App>) {
    let (dir, app) = emulated_app();
    with_serial(&app);
    app.reports.set_now(Some(DISPATCHED_AT));
    (dir, app)
}

fn request_with(body: &[u8]) -> Request {
    Request::synthetic(Method::POST, "/api/v1/reports/create", body.to_vec())
}

/// `_post_create(report_module, body)` with its own cancellation token.
async fn post_create(app: &Arc<App>, body: Value) -> (Option<Reply>, CancellationToken) {
    let cancellation = CancellationToken::new();
    let reply = create_report(
        app,
        &request_with(pyfmt::dumps_default(&body).as_bytes()),
        cancellation.clone(),
    )
    .await;
    (reply, cancellation)
}

async fn post_dispatch(app: &Arc<App>, body: Value) -> Reply {
    call(
        app,
        Method::POST,
        "/api/v1/reports/dispatch",
        pyfmt::dumps_default(&body),
    )
    .await
}

fn dispatch_body(fields: Value) -> Value {
    let mut body = json!({"localID": QUEUED_ID, "ticket": 4242});
    for (k, v) in fields.as_object().unwrap() {
        body[k] = v.clone();
    }
    body
}

fn assert_api_error(reply: &Reply, status: u16, code: &str) {
    assert_eq!(reply.status_code(), status);
    assert_eq!(body_json(reply)["data"]["code"], code);
}

fn emitted(app: &App) -> Vec<Value> {
    app.reports.hooks.emitted.lock().clone()
}

fn draft_dir_entries(app: &App) -> usize {
    std::fs::read_dir(app.reports.cfg().draft_dir)
        .unwrap()
        .count()
}

/// test_create_report_returns_machine_id_matching_report_info
#[tokio::test]
async fn create_report_returns_machine_id_matching_report_info() {
    let (_dir, app) = emulated_app();
    with_serial(&app);
    app.reports.set_now(Some(1));
    let calls = install_collector(&app, &[]);
    let cancellation = CancellationToken::new();

    let reply = create_report(&app, &request_with(b""), cancellation.clone())
        .await
        .unwrap();

    let body = body_json(&reply);
    let local_id = body["localID"].as_str().unwrap().to_string();
    assert!(LOCAL_ID_RE.is_match(&local_id));
    assert_eq!(
        body,
        json!({"localID": local_id, "machineID": MACHINE_SERIAL})
    );
    let report_info = reports::read_draft_report_info(
        &reports::draft_path(&app.reports.cfg(), &local_id).unwrap(),
    )
    .unwrap();
    assert_eq!(report_info["machineID"], body["machineID"]);
    assert_eq!(report_info["dateAndTime"], 1);
    assert_eq!(report_info["issueTime"], 1);
    let calls = calls.lock().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].range, None);
    assert!(calls[0].capture_active_debug_shot);
    cancellation.cancel();
    assert!(calls[0].cancellation.as_ref().unwrap().is_cancelled());
    let row = report_row(&app, &local_id).unwrap();
    assert_eq!(row.machine_id, MACHINE_SERIAL);
    assert_eq!(row.machine_status, true);
    assert_eq!(row.creation_time, 1);
    assert_eq!(row.issue_time, 1);
    assert_eq!(row.status, "draft");
}

/// test_create_report_with_historical_issue_time_persists_metadata_and_range
#[tokio::test]
async fn create_report_with_historical_issue_time_persists_metadata_and_range() {
    let (_dir, app) = emulated_app();
    with_serial(&app);
    let now = 200_000;
    let issue_time = now - 25 * HOUR;
    app.reports.set_now(Some(now));
    let calls = install_collector(&app, &[]);

    let (reply, _) = post_create(&app, json!({"issueTime": issue_time})).await;

    let local_id = body_json(&reply.unwrap())["localID"]
        .as_str()
        .unwrap()
        .to_string();
    let report_info = reports::read_draft_report_info(
        &reports::draft_path(&app.reports.cfg(), &local_id).unwrap(),
    )
    .unwrap();
    let listed = reports::list_report_page(&app, 0, 1, None).unwrap()["content"][0].clone();
    let row = report_row(&app, &local_id).unwrap();
    let calls = calls.lock().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].range,
        Some((issue_time - 12 * HOUR, issue_time + 12 * HOUR))
    );
    assert!(!calls[0].capture_active_debug_shot);
    assert_eq!(report_info["dateAndTime"], now);
    assert_eq!(report_info["issueTime"], issue_time);
    assert_eq!(row.creation_time, now);
    assert_eq!(row.issue_time, issue_time);
    assert_eq!(listed["dateAndTime"], now);
    assert_eq!(listed["issueTime"], issue_time);
}

/// test_create_report_with_recent_issue_time_keeps_active_shot_eligible
#[tokio::test]
async fn create_report_with_recent_issue_time_keeps_active_shot_eligible() {
    let (_dir, app) = emulated_app();
    with_serial(&app);
    let now = 200_000;
    app.reports.set_now(Some(now));
    let calls = install_collector(&app, &[]);

    post_create(&app, json!({"issueTime": now - 1})).await;

    let calls = calls.lock().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].range, Some((now - 24 * HOUR, now)));
    assert!(calls[0].capture_active_debug_shot);
}

/// test_create_report_cancelled_mid_collection_leaves_no_trace
#[tokio::test]
async fn create_report_cancelled_mid_collection_leaves_no_trace() {
    let (_dir, app) = emulated_app();
    with_serial(&app);
    install_cancelling_collector(&app);
    let logs = capture_logs();

    let reply = create_report(&app, &request_with(b""), CancellationToken::new()).await;

    assert!(reply.is_none());
    assert_eq!(draft_dir_entries(&app), 0);
    assert!(all_rows(&app).is_empty());
    assert!(
        logs.messages(tracing::Level::ERROR, "api::bug_report")
            .is_empty()
    );
    assert!(
        logs.messages(tracing::Level::ERROR, "bug_report")
            .is_empty()
    );
}

/// test_create_report_cancelled_during_debug_file_copy_never_inserts_row:
/// the collection stops before the next copy, not during this one.
#[tokio::test]
async fn create_report_cancelled_during_debug_file_copy_never_inserts_row() {
    let (_dir, app) = emulated_app();
    with_serial(&app);
    for index in 0..3 {
        debug_file(
            &app.reports.cfg().debug_history,
            "2026-05-18",
            &format!("10:00:0{index}.shot.json.zst"),
        );
    }
    let cancellation = CancellationToken::new();
    let copies = Arc::new(AtomicUsize::new(0));
    let (token, counter) = (cancellation.clone(), copies.clone());
    *app.reports.hooks.after_debug_copy.lock() = Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        token.cancel();
    }));

    let reply = create_report(&app, &request_with(b""), cancellation).await;

    assert!(reply.is_none());
    assert_eq!(draft_dir_entries(&app), 0);
    assert_eq!(copies.load(Ordering::SeqCst), 1);
    assert!(all_rows(&app).is_empty());
}

/// test_create_report_records_watcher_failure_but_still_returns_draft_info
#[tokio::test]
async fn create_report_records_watcher_failure_but_still_returns_draft_info() {
    let server = watcher("logs", "{\"ok\": true}");
    let unreachable = format!("http://127.0.0.1:{}/health/logs", TestServer::closed_port());
    let (_dir, app) = machine_app(&unreachable, &server.url("/health/status"));
    with_serial(&app);
    app.reports.set_now(Some(1));

    let reply = call(&app, Method::POST, "/api/v1/reports/create", "").await;

    assert_eq!(reply.status_code(), 200);
    let body = body_json(&reply);
    let local_id = body["localID"].as_str().unwrap();
    assert_eq!(body["machineID"], MACHINE_SERIAL);
    let draft_dir = reports::draft_path(&app.reports.cfg(), local_id).unwrap();
    let report_log = std::fs::read_to_string(draft_dir.join(REPORT_LOG_NAME)).unwrap();
    assert!(report_log.contains("Failed to fetch machine logs: Cannot connect to host"));
    let row = report_row(&app, local_id).unwrap();
    assert_eq!(row.machine_logs, false);
    assert_eq!(row.machine_status, true);
    assert_eq!(
        std::fs::read_to_string(draft_dir.join(MACHINE_STATUS_NAME)).unwrap(),
        "{\"ok\": true}"
    );
    let stats: Value = serde_json::from_str(
        &std::fs::read_to_string(draft_dir.join(reports::STATS_INFO_NAME)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        stats,
        json!({"totalSavedShots": 0, "byProfile": [], "mileage": null})
    );
}

/// test_create_report_request_requires_only_integer_issue_time
#[test]
fn create_report_request_requires_only_integer_issue_time() {
    assert_eq!(create_report_issue_time(b""), Ok(None));
    assert_eq!(
        create_report_issue_time(b"{\"issueTime\": 123}"),
        Ok(Some(123))
    );
    for body in [
        &b"{}"[..],
        b"{\"issueTime\": true}",
        b"{\"issueTime\": 1.5}",
        b"[]",
    ] {
        assert!(matches!(
            create_report_issue_time(body),
            Err(CreateBodyError::Value(_))
        ));
    }
}

/// test_get_draft_returns_finalized_archive_without_recompressing
#[tokio::test]
async fn get_draft_returns_finalized_archive_without_recompressing() {
    let (_dir, app) = emulated_app();
    let cfg = app.reports.cfg();
    let draft_dir = reports::draft_path(&cfg, QUEUED_ID).unwrap();
    std::fs::create_dir_all(&draft_dir).unwrap();
    std::fs::write(draft_dir.join(MACHINE_STATUS_NAME), "{\"ok\": true}").unwrap();
    reports::write_draft_report_info(
        &draft_dir,
        &json!({"localID": QUEUED_ID, "eventID": "event-1", "ticket": 42}),
    )
    .unwrap();
    reports::finalize_draft_archive(&cfg, QUEUED_ID).unwrap();
    let finalized = reports::finalized_draft_path(&cfg, QUEUED_ID).unwrap();
    let bytes = std::fs::read(&finalized).unwrap();
    // A different file at the same path proves nothing was recompressed.
    std::fs::write(&finalized, b"finalized archive bytes").unwrap();

    let reply = call(
        &app,
        Method::GET,
        &format!("/api/v1/reports/draft/{QUEUED_ID}"),
        "",
    )
    .await;

    assert!(!bytes.is_empty());
    assert!(!draft_dir.exists());
    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        reply.header("Content-Type"),
        Some("application/octet-stream")
    );
    assert_eq!(
        reply.header("Content-Disposition"),
        Some(format!("attachment; filename=\"{QUEUED_ID}.zstd\"").as_str())
    );
    assert_eq!(reply.header("Content-Length"), Some("23"));
    assert_eq!(reply.header("Etag"), None);
    assert_eq!(reply.body(), b"finalized archive bytes");
}

/// GET of a draft still being edited archives it on the fly; an unknown
/// localID is a 404 and a malformed one a 400.
#[tokio::test]
async fn get_draft_archives_an_open_draft() {
    let (_dir, app) = emulated_app();
    let draft_dir = reports::draft_path(&app.reports.cfg(), QUEUED_ID).unwrap();
    std::fs::create_dir_all(&draft_dir).unwrap();
    reports::write_draft_report_info(&draft_dir, &json!({"localID": QUEUED_ID})).unwrap();

    let reply = call(
        &app,
        Method::GET,
        &format!("/api/v1/reports/draft/{QUEUED_ID}"),
        "",
    )
    .await;

    assert_eq!(reply.status_code(), 200);
    let archive = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(archive.path(), reply.body()).unwrap();
    let (info, _, _extracted) = reports::archive::read_tar_zstd(archive.path());
    assert_eq!(info["localID"], QUEUED_ID);
    assert!(draft_dir.is_dir());
    assert_eq!(draft_dir_entries(&app), 1);

    let missing = call(
        &app,
        Method::GET,
        &format!("/api/v1/reports/draft/{SECOND_ID}"),
        "",
    )
    .await;
    assert_eq!(missing.status_code(), 404);
    assert_eq!(
        body_json(&missing),
        json!({"error": "Unknown localID", "description": "", "data": {"code": "UNKNOWN_LOCAL_ID"}})
    );
    let malformed = call(&app, Method::GET, "/api/v1/reports/draft/nope", "").await;
    assert_eq!(
        body_json(&malformed),
        json!({"error": "localID must be a UUID", "description": "", "data": {"code": "INVALID_LOCAL_ID"}})
    );
    assert_eq!(malformed.status_code(), 400);
}

fn deletable_row(app: &App, local_id: &str, status: &str) {
    insert_row(
        app,
        local_id,
        status,
        1,
        BugReportValues {
            machine_info: Some(json!(false)),
            machine_logs: Some(json!(false)),
            machine_status: Some(json!(false)),
            ..Default::default()
        },
    );
}

/// test_delete_draft_removes_all_report_representations_and_db_row
/// (directory, archive, both).
#[tokio::test]
async fn delete_draft_removes_all_report_representations_and_db_row() {
    for (representation, local_id) in [
        ("directory", QUEUED_ID),
        ("archive", SECOND_ID),
        ("both", THIRD_ID),
    ] {
        let (_dir, app) = emulated_app();
        let cfg = app.reports.cfg();
        let draft_dir = reports::draft_path(&cfg, local_id).unwrap();
        let archive_path = reports::finalized_draft_path(&cfg, local_id).unwrap();
        if representation != "archive" {
            std::fs::create_dir_all(&draft_dir).unwrap();
            std::fs::write(draft_dir.join("report.txt"), "draft").unwrap();
        }
        if representation != "directory" {
            std::fs::write(&archive_path, b"archive").unwrap();
        }
        deletable_row(&app, local_id, "draft");

        let reply = call(
            &app,
            Method::DELETE,
            &format!("/api/v1/reports/draft/{local_id}"),
            "",
        )
        .await;

        assert_eq!(reply.status_code(), 204, "{representation}");
        assert!(reply.body().is_empty());
        assert_eq!(reply.header("Content-Type"), None);
        assert!(!draft_dir.exists());
        assert!(!archive_path.exists());
        assert!(report_row(&app, local_id).is_none());
    }
}

/// test_delete_draft_allows_submitted_report
#[tokio::test]
async fn delete_draft_allows_submitted_report() {
    let (_dir, app) = emulated_app();
    let archive_path = reports::finalized_draft_path(&app.reports.cfg(), QUEUED_ID).unwrap();
    std::fs::write(&archive_path, b"archive").unwrap();
    deletable_row(&app, QUEUED_ID, "submitted");

    let reply = call(
        &app,
        Method::DELETE,
        &format!("/api/v1/reports/draft/{QUEUED_ID}"),
        "",
    )
    .await;

    assert_eq!(reply.status_code(), 204);
    assert!(!archive_path.exists());
    assert!(report_row(&app, QUEUED_ID).is_none());
}

/// test_delete_draft_returns_not_found_for_unknown_local_id
#[tokio::test]
async fn delete_draft_returns_not_found_for_unknown_local_id() {
    let (_dir, app) = emulated_app();
    let reply = call(
        &app,
        Method::DELETE,
        &format!("/api/v1/reports/draft/{QUEUED_ID}"),
        "",
    )
    .await;
    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        body_json(&reply),
        json!({"error": "Unknown localID", "description": "", "data": {"code": "UNKNOWN_LOCAL_ID"}})
    );
}

/// test_delete_draft_rejects_request_body
#[tokio::test]
async fn delete_draft_rejects_request_body() {
    let (_dir, app) = emulated_app();
    let reply = call(
        &app,
        Method::DELETE,
        &format!("/api/v1/reports/draft/{QUEUED_ID}"),
        "{}",
    )
    .await;
    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        body_json(&reply),
        json!({
            "error": "Delete report draft request must not contain a body",
            "description": "",
            "data": {"code": "INVALID_BODY"},
        })
    );
}

/// test_submit_body_contract_validation
#[test]
fn submit_body_contract_validation() {
    let body =
        json!({"localID": QUEUED_ID, "eventID": "event", "submissionTime": 3, "ticket": null});
    assert_eq!(
        parse_submit_body(pyfmt::dumps_default(&body).as_bytes(), 9).unwrap(),
        SubmitBody {
            local_id: QUEUED_ID.into(),
            event_id: "event".into(),
            submission_time: 3,
            ticket: Some(Value::Null),
        }
    );
    let defaulted = parse_submit_body(
        b"{\"localID\": \"018f0a2b-1234-7abc-8def-0123456789ab\", \"eventID\": \"e\"}",
        9,
    )
    .unwrap();
    assert_eq!(defaulted.submission_time, 9);
    assert_eq!(defaulted.ticket, None);
    for body in [
        json!({"localID": QUEUED_ID, "eventID": ""}),
        json!({"localID": QUEUED_ID, "eventID": "event", "submissionTime": true}),
        json!({"localID": QUEUED_ID, "eventID": "event", "ticket": "12"}),
        json!({"localID": QUEUED_ID, "eventID": "event", "extra": 1}),
    ] {
        let error = parse_submit_body(pyfmt::dumps_default(&body).as_bytes(), 9).unwrap_err();
        assert_eq!(error.code, "INVALID_BODY", "{body}");
    }
}

/// POST /reports/submit: 200 with an empty body, 404 for an unknown
/// localID, 400 for a bad body.
#[tokio::test]
async fn submit_handler_records_the_event() {
    let (_dir, app) = emulated_app();
    let draft_dir = reports::draft_path(&app.reports.cfg(), QUEUED_ID).unwrap();
    std::fs::create_dir_all(&draft_dir).unwrap();
    reports::write_draft_report_info(&draft_dir, &json!({"localID": QUEUED_ID, "ticket": null}))
        .unwrap();
    deletable_row(&app, QUEUED_ID, "draft");

    let body = json!({"localID": QUEUED_ID, "eventID": "event-1", "submissionTime": 5});
    let reply = call(
        &app,
        Method::POST,
        "/api/v1/reports/submit",
        pyfmt::dumps_default(&body),
    )
    .await;

    assert_eq!(reply.status_code(), 200);
    assert!(reply.body().is_empty());
    assert_eq!(reply.header("Content-Type"), Some("application/json"));
    let row = report_row(&app, QUEUED_ID).unwrap();
    assert_eq!(row.status, "submitted");
    assert_eq!(row.submission_time, 5);
    assert!(!draft_dir.exists());

    let unknown = json!({"localID": SECOND_ID, "eventID": "event-1"});
    let reply = call(
        &app,
        Method::POST,
        "/api/v1/reports/submit",
        pyfmt::dumps_default(&unknown),
    )
    .await;
    assert_api_error(&reply, 404, "UNKNOWN_LOCAL_ID");
    let reply = call(
        &app,
        Method::POST,
        "/api/v1/reports/submit",
        "{\"localID\": 1}",
    )
    .await;
    assert_api_error(&reply, 400, "INVALID_LOCAL_ID");
}

/// test_preflight_has_ordered_blockers
#[tokio::test]
async fn preflight_has_ordered_blockers() {
    let (_dir, app) = emulated_app();
    app.reports.configure(|cfg| cfg.min_free_bytes = i64::MAX);
    let unreachable = format!("https://127.0.0.1:{}", TestServer::closed_port());

    let reply = call(
        &app,
        Method::GET,
        &format!("/api/v1/reports/preflight?probe=http://invalid&probe={unreachable}&probe=http://invalid"),
        "",
    )
    .await;

    let body = body_json(&reply);
    assert_eq!(
        body["blockers"],
        json!([
            "NO_SERIAL_NUMBER",
            "INSUFFICIENT_DISK_SPACE",
            "NETWORK_UNREACHABLE"
        ])
    );
    assert_eq!(
        body["network"]["http://invalid"],
        json!({"reachable": false, "status": null, "latencyMs": null, "error": "INVALID_PROBE"})
    );
    assert_eq!(
        body["network"][unreachable.as_str()]["error"],
        "ClientConnectorError"
    );
    assert_eq!(body["network"].as_object().unwrap().len(), 2);
    assert_eq!(body["ok"], false);
    assert_eq!(body["machineID"], Value::Null);
    assert_eq!(body["disk"]["requiredBytes"], i64::MAX);
    assert_eq!(body["disk"]["ok"], false);
    assert_eq!(body["collectionInProgress"], false);
}

/// A preflight with nothing in the way is `ok`, in the Python's key order.
#[tokio::test]
async fn preflight_is_ok_when_nothing_blocks() {
    let (_dir, app) = emulated_app();
    with_serial(&app);
    app.reports.configure(|cfg| cfg.min_free_bytes = 0);

    let reply = call(&app, Method::GET, "/api/v1/reports/preflight", "").await;

    let body = body_json(&reply);
    assert_eq!(
        body.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "ok",
            "blockers",
            "machineID",
            "disk",
            "collectionInProgress",
            "network"
        ]
    );
    assert_eq!(body["ok"], true);
    assert_eq!(body["machineID"], MACHINE_SERIAL);
    assert_eq!(body["network"], json!({}));
    let guard = app.reports.try_begin_collection().unwrap();
    let busy = body_json(&call(&app, Method::GET, "/api/v1/reports/preflight", "").await);
    assert_eq!(busy["blockers"], json!(["COLLECTION_IN_PROGRESS"]));
    drop(guard);
}

/// A create while another collection runs is refused with 409; a disk
/// below the reserve with 507.
#[tokio::test]
async fn create_refuses_concurrent_collections_and_full_disks() {
    let (_dir, app) = emulated_app();
    install_collector(&app, &[]);
    let guard = app.reports.try_begin_collection().unwrap();
    let busy = call(&app, Method::POST, "/api/v1/reports/create", "").await;
    assert_eq!(busy.status_code(), 409);
    assert_eq!(
        body_json(&busy),
        json!({"error": "A report is already being collected", "description": "", "data": {"code": "COLLECTION_IN_PROGRESS"}})
    );
    drop(guard);

    app.reports.configure(|cfg| cfg.min_free_bytes = i64::MAX);
    let full = call(&app, Method::POST, "/api/v1/reports/create", "").await;
    assert_eq!(full.status_code(), 507);
    let body = body_json(&full);
    assert_eq!(
        body["error"],
        "Not enough free disk space to collect a report"
    );
    assert_eq!(body["data"]["code"], "INSUFFICIENT_DISK_SPACE");
    assert_eq!(body["data"]["requiredBytes"], i64::MAX);
    assert!(body["data"]["freeBytes"].is_i64());
    assert!(app.reports.try_begin_collection().is_some());
}

/// test_dispatch_routes_are_registered
#[test]
fn dispatch_routes_are_registered() {
    let (_dir, app) = emulated_app();
    let router = crate::api::router(&app);
    let find = |pattern: &str| {
        router
            .routes()
            .iter()
            .find(|r| r.pattern == pattern)
            .map(|r| (r.handler, r.module, r.method_names()))
    };
    assert_eq!(
        find("/api/v1/reports/request"),
        Some(("ReportsRequestHandler", "api.bug_report", vec!["POST"]))
    );
    assert_eq!(
        find("/api/v1/reports/dispatch"),
        Some((
            "ReportsDispatchHandler",
            "api.bug_report",
            vec!["GET", "POST"]
        ))
    );
    assert_eq!(
        find("/api/v1/reports/draft/([^/]+)"),
        Some((
            "ReportDraftHandler",
            "api.bug_report",
            vec!["GET", "PUT", "DELETE"]
        ))
    );
    for (pattern, handler) in [
        ("/api/v1/reports/create", "ReportsCreateHandler"),
        ("/api/v1/reports/preflight", "ReportsPreflightHandler"),
        ("/api/v1/reports/list", "ReportsListHandler"),
        ("/api/v1/reports/submit", "ReportsSubmitHandler"),
    ] {
        assert_eq!(find(pattern).map(|r| r.0), Some(handler));
    }
}

/// test_parse_create_body_accepts_each_documented_shape
#[test]
fn parse_create_body_accepts_each_documented_shape() {
    let local_id_body = pyfmt::dumps_default(&json!({"localID": QUEUED_ID}));
    for (body, expected) in [
        (&b""[..], (None, None)),
        (b"{\"issueTime\": 123}", (Some(123), None)),
        (b"{\"issueTime\": 0}", (Some(0), None)),
        (
            local_id_body.as_bytes(),
            (None, Some(QUEUED_ID.to_string())),
        ),
    ] {
        assert_eq!(parse_create_body(body), Ok(expected.clone()));
        assert_eq!(create_report_issue_time(body), Ok(expected.0));
    }
}

/// test_parse_create_body_rejects_everything_else
#[test]
fn parse_create_body_rejects_everything_else() {
    let bodies = [
        "{}".to_string(),
        pyfmt::dumps_default(&json!({"issueTime": 1, "localID": QUEUED_ID})),
        pyfmt::dumps_default(&json!({"localID": QUEUED_ID, "extra": 1})),
        "{\"issueTime\": 1, \"extra\": 1}".into(),
        "{\"extra\": 1}".into(),
        "[]".into(),
        "\"text\"".into(),
        "{".into(),
        "{\"issueTime\": true}".into(),
        "{\"issueTime\": 1.5}".into(),
        "{\"issueTime\": \"1\"}".into(),
        "{\"issueTime\": null}".into(),
    ];
    for body in bodies {
        assert!(
            matches!(
                parse_create_body(body.as_bytes()),
                Err(CreateBodyError::Value(_))
            ),
            "{body}"
        );
        assert!(matches!(
            create_report_issue_time(body.as_bytes()),
            Err(CreateBodyError::Value(_))
        ));
    }
}

/// test_parse_create_body_rejects_malformed_local_id
#[test]
fn parse_create_body_rejects_malformed_local_id() {
    for local_id in [
        json!("not-a-uuid"),
        json!("../history"),
        json!(QUEUED_ID.to_uppercase()),
        json!(123),
        Value::Null,
    ] {
        let body = pyfmt::dumps_default(&json!({"localID": local_id}));
        match parse_create_body(body.as_bytes()) {
            Err(CreateBodyError::Request(error)) => {
                assert_eq!(error.status, 400);
                assert_eq!(error.code, "INVALID_LOCAL_ID");
            }
            other => panic!("{body}: {other:?}"),
        }
    }
}

/// test_request_handler_mints_local_id_and_stores_nothing
#[tokio::test]
async fn request_handler_mints_local_id_and_stores_nothing() {
    let (_dir, app) = dispatch_app();
    let first = call(&app, Method::POST, "/api/v1/reports/request", "").await;
    let second = call(&app, Method::POST, "/api/v1/reports/request", "").await;

    for reply in [&first, &second] {
        assert_eq!(reply.status_code(), 200);
        let body = body_json(reply);
        assert_eq!(
            body.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["localID", "machineID"]
        );
        assert!(LOCAL_ID_RE.is_match(body["localID"].as_str().unwrap()));
        assert_eq!(body["machineID"], MACHINE_SERIAL);
    }
    assert_ne!(body_json(&first)["localID"], body_json(&second)["localID"]);
    assert!(all_rows(&app).is_empty());
    assert_eq!(draft_dir_entries(&app), 0);
}

/// test_request_handler_rejects_any_body
#[tokio::test]
async fn request_handler_rejects_any_body() {
    let (_dir, app) = dispatch_app();
    for body in [
        "{}".to_string(),
        "garbage".into(),
        pyfmt::dumps_default(&json!({"localID": QUEUED_ID})),
    ] {
        let reply = call(&app, Method::POST, "/api/v1/reports/request", body).await;
        assert_api_error(&reply, 400, "INVALID_BODY");
        assert_eq!(body_json(&reply)["error"], "Request report takes no body");
    }
    assert!(all_rows(&app).is_empty());
}

fn parse_dispatch(body: Value) -> Result<QueuedReport, RequestError> {
    parse_dispatch_body(pyfmt::dumps_default(&body).as_bytes())
}

/// test_parse_dispatch_body_accepts_a_full_body
#[test]
fn parse_dispatch_body_accepts_a_full_body() {
    let parsed = parse_dispatch(json!({
        "localID": QUEUED_ID,
        "ticket": 42,
        "issueTime": 1_700_000_000,
        "description": "Espresso is slow",
        "name": "Ada",
        "email": "ada@example.com",
    }))
    .unwrap();
    assert_eq!(
        parsed,
        QueuedReport {
            local_id: QUEUED_ID.into(),
            ticket: json!(42),
            issue_time: Some(1_700_000_000),
            description: Some("Espresso is slow".into()),
            name: Some("Ada".into()),
            email: Some("ada@example.com".into()),
        }
    );
}

/// test_parse_dispatch_body_fills_missing_optionals_with_none
#[test]
fn parse_dispatch_body_fills_missing_optionals_with_none() {
    assert_eq!(
        parse_dispatch(json!({"localID": QUEUED_ID, "ticket": 42})).unwrap(),
        QueuedReport {
            local_id: QUEUED_ID.into(),
            ticket: json!(42),
            issue_time: None,
            description: None,
            name: None,
            email: None,
        }
    );
}

/// test_parse_dispatch_body_trims_text_fields
#[test]
fn parse_dispatch_body_trims_text_fields() {
    let parsed = parse_dispatch(json!({
        "localID": QUEUED_ID,
        "ticket": 42,
        "description": "  Espresso is slow \n",
        "name": "  Ada ",
        "email": " ada@example.com ",
    }))
    .unwrap();
    assert_eq!(parsed.description.as_deref(), Some("Espresso is slow"));
    assert_eq!(parsed.name.as_deref(), Some("Ada"));
    assert_eq!(parsed.email.as_deref(), Some("ada@example.com"));
}

/// test_parse_dispatch_body_maps_empty_text_fields_to_none
#[test]
fn parse_dispatch_body_maps_empty_text_fields_to_none() {
    let parsed = parse_dispatch(json!({
        "localID": QUEUED_ID,
        "ticket": 42,
        "issueTime": null,
        "description": "   ",
        "name": "",
        "email": null,
    }))
    .unwrap();
    assert_eq!(parsed.issue_time, None);
    assert_eq!(parsed.description, None);
    assert_eq!(parsed.name, None);
    assert_eq!(parsed.email, None);
}

/// test_parse_dispatch_body_accepts_text_at_the_limits
#[test]
fn parse_dispatch_body_accepts_text_at_the_limits() {
    let parsed = parse_dispatch(json!({
        "localID": QUEUED_ID,
        "ticket": 42,
        "description": "d".repeat(10_000),
        "name": "n".repeat(200),
        "email": format!("{}@b.cd", "e".repeat(249)),
    }))
    .unwrap();
    assert_eq!(parsed.description.unwrap().len(), 10_000);
    assert_eq!(parsed.name.unwrap().len(), 200);
    assert_eq!(parsed.email.unwrap().len(), 254);
}

/// test_parse_dispatch_body_rejects_invalid_bodies
#[test]
fn parse_dispatch_body_rejects_invalid_bodies() {
    let cases = [
        (
            json!({"localID": QUEUED_ID, "ticket": 42, "email": "not-an-email"}),
            "INVALID_BODY",
        ),
        (
            json!({"localID": QUEUED_ID, "ticket": 42, "email": format!("{}@b.cd", "e".repeat(250))}),
            "INVALID_BODY",
        ),
        (
            json!({"localID": QUEUED_ID, "ticket": 42, "email": 7}),
            "INVALID_BODY",
        ),
        (
            json!({"localID": QUEUED_ID, "ticket": true}),
            "INVALID_BODY",
        ),
        (
            json!({"localID": QUEUED_ID, "ticket": "42"}),
            "INVALID_BODY",
        ),
        (json!({"localID": QUEUED_ID, "ticket": 4.5}), "INVALID_BODY"),
        (
            json!({"localID": QUEUED_ID, "ticket": null}),
            "INVALID_BODY",
        ),
        (json!({"localID": QUEUED_ID}), "INVALID_BODY"),
        (
            json!({"localID": QUEUED_ID, "ticket": 42, "issueTime": true}),
            "INVALID_BODY",
        ),
        (
            json!({"localID": QUEUED_ID, "ticket": 42, "issueTime": "1"}),
            "INVALID_BODY",
        ),
        (
            json!({"localID": QUEUED_ID, "ticket": 42, "issueTime": 1.5}),
            "INVALID_BODY",
        ),
        (
            json!({"localID": QUEUED_ID, "ticket": 42, "unknown": 1}),
            "INVALID_BODY",
        ),
        (
            json!({"localID": QUEUED_ID, "ticket": 42, "description": "d".repeat(10_001)}),
            "INVALID_BODY",
        ),
        (
            json!({"localID": QUEUED_ID, "ticket": 42, "description": 5}),
            "INVALID_BODY",
        ),
        (
            json!({"localID": QUEUED_ID, "ticket": 42, "name": "n".repeat(201)}),
            "INVALID_BODY",
        ),
        (
            json!({"localID": "not-a-uuid", "ticket": 42}),
            "INVALID_LOCAL_ID",
        ),
        (
            json!({"localID": QUEUED_ID.to_uppercase(), "ticket": 42}),
            "INVALID_LOCAL_ID",
        ),
        (json!({"localID": null, "ticket": 42}), "INVALID_LOCAL_ID"),
        (json!({"ticket": 42}), "INVALID_LOCAL_ID"),
    ];
    for (body, code) in cases {
        let error = parse_dispatch(body.clone()).unwrap_err();
        assert_eq!(error.status, 400, "{body}");
        assert_eq!(error.code, code, "{body}");
    }
    assert_eq!(
        parse_dispatch(json!({"localID": QUEUED_ID, "ticket": 42, "b": 1, "a": 2}))
            .unwrap_err()
            .message,
        "Unknown keys: a, b"
    );
}

/// test_parse_dispatch_body_rejects_non_object_json
#[test]
fn parse_dispatch_body_rejects_non_object_json() {
    for body in [&b"[]"[..], b"{", b"\"text\""] {
        let error = parse_dispatch_body(body).unwrap_err();
        assert_eq!(error.status, 400);
        assert_eq!(error.code, "INVALID_BODY");
    }
}

/// test_dispatch_post_queues_the_report_and_emits_upload_report
#[tokio::test]
async fn dispatch_post_queues_the_report_and_emits_upload_report() {
    let (_dir, app) = dispatch_app();
    let issue_time = DISPATCHED_AT - 600;

    let reply = post_dispatch(
        &app,
        dispatch_body(json!({
            "issueTime": issue_time,
            "description": "Grinder jammed",
            "name": "Ada",
            "email": "ada@example.com",
        })),
    )
    .await;

    let expected = json!({
        "localID": QUEUED_ID,
        "machineID": MACHINE_SERIAL,
        "ticket": 4242,
        "issueTime": issue_time,
        "requestTime": DISPATCHED_AT,
        "description": "Grinder jammed",
        "name": "Ada",
        "email": "ada@example.com",
    });
    assert_eq!(reply.status_code(), 202);
    assert_eq!(body_json(&reply), expected);
    assert_eq!(
        body_json(&reply)
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        expected.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    assert_eq!(emitted(&app), [expected]);
    assert_eq!(UPLOAD_REPORT_EVENT, "upload_report");
    let rows = all_rows(&app);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.local_id, QUEUED_ID);
    assert_eq!(row.status, "queued");
    assert_eq!(row.ticket_number, 4242);
    assert_eq!(row.issue_time, issue_time);
    assert_eq!(row.creation_time, DISPATCHED_AT);
    assert_eq!(row.submission_time, Value::Null);
    assert_eq!(row.description, "Grinder jammed");
    assert_eq!(row.machine_id, MACHINE_SERIAL);
    assert_eq!(row.contact_name, "Ada");
    assert_eq!(row.contact_email, "ada@example.com");
    assert_eq!(row.event_id, Value::Null);
    assert_eq!(row.log_files, Value::Null);
    assert_eq!(row.machine_status, Value::Null);
}

/// test_dispatch_post_defaults_issue_time_to_now_and_contact_to_none
#[tokio::test]
async fn dispatch_post_defaults_issue_time_to_now_and_contact_to_none() {
    let (_dir, app) = dispatch_app();

    let reply = post_dispatch(&app, dispatch_body(json!({}))).await;

    assert_eq!(reply.status_code(), 202);
    let expected = json!({
        "localID": QUEUED_ID,
        "machineID": MACHINE_SERIAL,
        "ticket": 4242,
        "issueTime": DISPATCHED_AT,
        "requestTime": DISPATCHED_AT,
        "description": null,
        "name": null,
        "email": null,
    });
    assert_eq!(body_json(&reply), expected);
    assert_eq!(emitted(&app), [expected]);
    let row = report_row(&app, QUEUED_ID).unwrap();
    assert_eq!(row.issue_time, DISPATCHED_AT);
    assert_eq!(row.creation_time, DISPATCHED_AT);
    assert_eq!(row.contact_name, Value::Null);
    assert_eq!(row.contact_email, Value::Null);
}

/// test_dispatch_post_with_the_same_local_id_is_a_conflict
#[tokio::test]
async fn dispatch_post_with_the_same_local_id_is_a_conflict() {
    let (_dir, app) = dispatch_app();
    let first = post_dispatch(&app, dispatch_body(json!({"description": "first"}))).await;
    app.reports.set_now(Some(DISPATCHED_AT + 60));

    let second = post_dispatch(
        &app,
        dispatch_body(json!({"ticket": 9999, "description": "second", "name": "Eve"})),
    )
    .await;

    assert_eq!(first.status_code(), 202);
    assert_api_error(&second, 409, "DUPLICATE_LOCAL_ID");
    assert_eq!(
        body_json(&second)["error"],
        "A report with this localID already exists"
    );
    assert_eq!(emitted(&app).len(), 1);
    let rows = all_rows(&app);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].ticket_number, 4242);
    assert_eq!(rows[0].description, "first");
    assert_eq!(rows[0].contact_name, Value::Null);
    assert_eq!(rows[0].creation_time, DISPATCHED_AT);
}

/// test_dispatch_post_conflicts_with_a_report_in_any_status
#[tokio::test]
async fn dispatch_post_conflicts_with_a_report_in_any_status() {
    for status in ["draft", "submitted"] {
        let (_dir, app) = dispatch_app();
        insert_row(
            &app,
            QUEUED_ID,
            status,
            DISPATCHED_AT,
            BugReportValues {
                machine_id: Some(json!(MACHINE_SERIAL)),
                ..Default::default()
            },
        );

        let reply = post_dispatch(&app, dispatch_body(json!({}))).await;

        assert_api_error(&reply, 409, "DUPLICATE_LOCAL_ID");
        assert!(emitted(&app).is_empty());
        let rows = all_rows(&app);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, status);
    }
}

/// test_dispatch_post_without_a_socket_server_still_queues_the_report and
/// test_dispatch_post_survives_a_failing_socket_emit: the emit cannot fail
/// the request (here no Socket.IO server is attached at all).
#[tokio::test]
async fn dispatch_post_without_a_socket_server_still_queues_the_report() {
    let (_dir, app) = dispatch_app();
    assert!(!app.sio.attached());

    let reply = post_dispatch(&app, dispatch_body(json!({}))).await;

    assert_eq!(reply.status_code(), 202);
    assert_eq!(body_json(&reply)["localID"], QUEUED_ID);
    assert_eq!(report_row(&app, QUEUED_ID).unwrap().status, "queued");
}

/// test_init_socket_registers_the_server_used_for_emits: the dispatch emits
/// `upload_report` once through the app's Socket.IO server.
#[tokio::test]
async fn dispatch_emits_upload_report_once() {
    let (_dir, app) = dispatch_app();
    post_dispatch(&app, dispatch_body(json!({}))).await;
    let events = emitted(&app);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["localID"], QUEUED_ID);
}

/// test_dispatch_post_rejects_invalid_bodies_without_side_effects
#[tokio::test]
async fn dispatch_post_rejects_invalid_bodies_without_side_effects() {
    for body in [
        json!({"localID": QUEUED_ID}),
        json!({"localID": QUEUED_ID, "ticket": 42, "email": "not-an-email"}),
        json!({"localID": "nope", "ticket": 42}),
    ] {
        let (_dir, app) = dispatch_app();
        let reply = post_dispatch(&app, body).await;
        assert_eq!(reply.status_code(), 400);
        let code = body_json(&reply)["data"]["code"].clone();
        assert!(code == "INVALID_BODY" || code == "INVALID_LOCAL_ID");
        assert!(emitted(&app).is_empty());
        assert!(all_rows(&app).is_empty());
    }
}

/// test_dispatch_get_lists_only_queued_reports_oldest_first: ties on the
/// creation time fall back to the localID.
#[tokio::test]
async fn dispatch_get_lists_only_queued_reports_oldest_first() {
    let (_dir, app) = dispatch_app();
    let row = |ticket: i64| BugReportValues {
        ticket_number: Some(json!(ticket)),
        ..Default::default()
    };
    insert_row(
        &app,
        SECOND_ID,
        "queued",
        DISPATCHED_AT + 10,
        BugReportValues {
            issue_time: Some(json!(DISPATCHED_AT + 5)),
            machine_id: Some(json!(MACHINE_SERIAL)),
            description: Some(json!("later")),
            ..row(2)
        },
    );
    insert_row(
        &app,
        "018f0a2b-1234-7abc-8def-0123456789ae",
        "draft",
        DISPATCHED_AT - 5,
        BugReportValues {
            contact_name: Some(json!("Draft")),
            ..row(3)
        },
    );
    insert_row(
        &app,
        THIRD_ID,
        "queued",
        DISPATCHED_AT,
        BugReportValues {
            machine_id: Some(json!(MACHINE_SERIAL)),
            contact_name: Some(json!("Ada")),
            contact_email: Some(json!("ada@example.com")),
            ..row(4)
        },
    );
    insert_row(
        &app,
        "018f0a2b-1234-7abc-8def-0123456789af",
        "submitted",
        DISPATCHED_AT - 10,
        BugReportValues {
            contact_name: Some(json!("Done")),
            ..row(5)
        },
    );
    insert_row(
        &app,
        QUEUED_ID,
        "queued",
        DISPATCHED_AT,
        BugReportValues {
            issue_time: Some(json!(DISPATCHED_AT - 600)),
            machine_id: Some(json!(MACHINE_SERIAL)),
            ..row(1)
        },
    );

    let reply = call(&app, Method::GET, "/api/v1/reports/dispatch", "").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body_json(&reply),
        json!({"content": [
            {
                "localID": QUEUED_ID,
                "machineID": MACHINE_SERIAL,
                "ticket": 1,
                "issueTime": DISPATCHED_AT - 600,
                "requestTime": DISPATCHED_AT,
                "description": null,
                "name": null,
                "email": null,
            },
            {
                "localID": THIRD_ID,
                "machineID": MACHINE_SERIAL,
                "ticket": 4,
                "issueTime": DISPATCHED_AT,
                "requestTime": DISPATCHED_AT,
                "description": null,
                "name": "Ada",
                "email": "ada@example.com",
            },
            {
                "localID": SECOND_ID,
                "machineID": MACHINE_SERIAL,
                "ticket": 2,
                "issueTime": DISPATCHED_AT + 5,
                "requestTime": DISPATCHED_AT + 10,
                "description": "later",
                "name": null,
                "email": null,
            },
        ]})
    );
}

/// test_dispatch_get_is_empty_without_queued_reports
#[tokio::test]
async fn dispatch_get_is_empty_without_queued_reports() {
    let (_dir, app) = dispatch_app();
    insert_row(
        &app,
        QUEUED_ID,
        "draft",
        DISPATCHED_AT,
        BugReportValues::default(),
    );
    let reply = call(&app, Method::GET, "/api/v1/reports/dispatch", "").await;
    assert_eq!(body_json(&reply), json!({"content": []}));
}

async fn dispatch(app: &Arc<App>, fields: Value) {
    let reply = post_dispatch(app, dispatch_body(fields)).await;
    assert_eq!(reply.status_code(), 202);
}

/// test_create_by_local_id_collects_around_the_dispatch_and_promotes_the_row
#[tokio::test]
async fn create_by_local_id_collects_around_the_dispatch_and_promotes_the_row() {
    let (_dir, app) = dispatch_app();
    let calls = install_collector(&app, &[AUTOMATIC_DEBUG_FILE]);
    let issue_time = DISPATCHED_AT - 600;
    dispatch(
        &app,
        json!({"issueTime": issue_time, "description": "Grinder jammed", "name": "Ada", "email": "ada@example.com"}),
    )
    .await;
    let queued = report_row(&app, QUEUED_ID).unwrap();
    // More than 12 hours after the issue time: the historical range applies.
    let now = issue_time + 13 * HOUR;
    app.reports.set_now(Some(now));

    let (reply, _) = post_create(&app, json!({"localID": QUEUED_ID})).await;

    let reply = reply.unwrap();
    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body_json(&reply),
        json!({"localID": QUEUED_ID, "machineID": MACHINE_SERIAL})
    );
    let calls = calls.lock().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].range,
        Some(reports::collection_range(
            queued.issue_time.as_i64().unwrap(),
            now
        ))
    );
    assert_eq!(
        calls[0].range,
        Some((issue_time - 12 * HOUR, issue_time + 12 * HOUR))
    );
    assert!(!calls[0].capture_active_debug_shot);

    let report_info = reports::read_draft_report_info(
        &reports::draft_path(&app.reports.cfg(), QUEUED_ID).unwrap(),
    )
    .unwrap();
    assert_eq!(report_info["localID"], QUEUED_ID);
    assert_eq!(report_info["description"], "Grinder jammed");
    assert_eq!(report_info["ticket"], 4242);
    assert_eq!(report_info["dateAndTime"], queued.creation_time);
    assert_eq!(report_info["dateAndTime"], DISPATCHED_AT);
    assert_eq!(report_info["issueTime"], issue_time);
    assert_eq!(report_info["machineID"], MACHINE_SERIAL);

    let rows = all_rows(&app);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.local_id, QUEUED_ID);
    assert_eq!(row.status, "draft");
    assert_eq!(row.ticket_number, 4242);
    assert_eq!(row.contact_name, "Ada");
    assert_eq!(row.contact_email, "ada@example.com");
    assert_eq!(row.creation_time, DISPATCHED_AT);
    assert_eq!(row.issue_time, issue_time);
    assert_eq!(row.description, "Grinder jammed");
    assert_eq!(row.machine_id, MACHINE_SERIAL);
    assert_eq!(row.machine_status, true);
    assert_eq!(row.log_files, AUTOMATIC_DEBUG_FILE);
}

/// test_create_by_local_id_shortly_after_the_dispatch_uses_the_trailing_range
#[tokio::test]
async fn create_by_local_id_shortly_after_the_dispatch_uses_the_trailing_range() {
    let (_dir, app) = dispatch_app();
    let calls = install_collector(&app, &[AUTOMATIC_DEBUG_FILE]);
    dispatch(&app, json!({})).await;
    let now = DISPATCHED_AT + 60;
    app.reports.set_now(Some(now));

    let (reply, _) = post_create(&app, json!({"localID": QUEUED_ID})).await;

    assert_eq!(
        body_json(&reply.unwrap()),
        json!({"localID": QUEUED_ID, "machineID": MACHINE_SERIAL})
    );
    let calls = calls.lock().clone();
    assert_eq!(calls[0].range, Some((now - 24 * HOUR, now)));
    assert!(calls[0].capture_active_debug_shot);
    assert_eq!(report_row(&app, QUEUED_ID).unwrap().status, "draft");
}

/// test_create_by_local_id_picked_up_late_still_covers_the_dispatch_time:
/// without an issueTime the dispatch time is the issue time, even when the
/// dial collects hours later.
#[tokio::test]
async fn create_by_local_id_picked_up_late_still_covers_the_dispatch_time() {
    let (_dir, app) = dispatch_app();
    let calls = install_collector(&app, &[AUTOMATIC_DEBUG_FILE]);
    dispatch(&app, json!({})).await;
    app.reports.set_now(Some(DISPATCHED_AT + 20 * HOUR));

    post_create(&app, json!({"localID": QUEUED_ID})).await;

    let calls = calls.lock().clone();
    assert_eq!(
        calls[0].range,
        Some((DISPATCHED_AT - 12 * HOUR, DISPATCHED_AT + 12 * HOUR))
    );
    assert!(!calls[0].capture_active_debug_shot);
    let row = report_row(&app, QUEUED_ID).unwrap();
    assert_eq!(row.issue_time, DISPATCHED_AT);
    assert_eq!(row.creation_time, DISPATCHED_AT);
}

/// test_create_by_local_id_of_a_collected_draft_answers_without_collecting
#[tokio::test]
async fn create_by_local_id_of_a_collected_draft_answers_without_collecting() {
    let (_dir, app) = dispatch_app();
    let calls = install_collector(&app, &[AUTOMATIC_DEBUG_FILE]);
    insert_row(
        &app,
        QUEUED_ID,
        "draft",
        DISPATCHED_AT,
        BugReportValues {
            machine_id: Some(json!(MACHINE_SERIAL)),
            ticket_number: Some(json!(4242)),
            ..Default::default()
        },
    );
    let draft_dir = reports::draft_path(&app.reports.cfg(), QUEUED_ID).unwrap();
    std::fs::create_dir_all(&draft_dir).unwrap();
    std::fs::write(draft_dir.join("kept.txt"), "kept").unwrap();

    let (reply, _) = post_create(&app, json!({"localID": QUEUED_ID})).await;

    let reply = reply.unwrap();
    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body_json(&reply),
        json!({"localID": QUEUED_ID, "machineID": MACHINE_SERIAL})
    );
    assert!(calls.lock().is_empty());
    assert_eq!(
        std::fs::read_to_string(draft_dir.join("kept.txt")).unwrap(),
        "kept"
    );
    let rows = all_rows(&app);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "draft");
    assert_eq!(rows[0].ticket_number, 4242);
}

/// test_create_by_unknown_local_id_is_not_found
#[tokio::test]
async fn create_by_unknown_local_id_is_not_found() {
    let (_dir, app) = dispatch_app();
    let calls = install_collector(&app, &[]);
    let (reply, _) = post_create(&app, json!({"localID": QUEUED_ID})).await;
    let reply = reply.unwrap();
    assert_api_error(&reply, 404, "UNKNOWN_LOCAL_ID");
    assert!(calls.lock().is_empty());
    assert!(all_rows(&app).is_empty());
    assert_eq!(draft_dir_entries(&app), 0);
}

/// test_create_by_submitted_local_id_is_a_conflict
#[tokio::test]
async fn create_by_submitted_local_id_is_a_conflict() {
    let (_dir, app) = dispatch_app();
    let calls = install_collector(&app, &[]);
    insert_row(
        &app,
        QUEUED_ID,
        "submitted",
        DISPATCHED_AT,
        BugReportValues::default(),
    );
    let (reply, _) = post_create(&app, json!({"localID": QUEUED_ID})).await;
    let reply = reply.unwrap();
    assert_api_error(&reply, 409, "DUPLICATE_LOCAL_ID");
    assert_eq!(
        body_json(&reply)["error"],
        "This report was already submitted"
    );
    assert!(calls.lock().is_empty());
    assert_eq!(report_row(&app, QUEUED_ID).unwrap().status, "submitted");
}

/// test_create_by_draft_local_id_without_a_directory_is_not_found
#[tokio::test]
async fn create_by_draft_local_id_without_a_directory_is_not_found() {
    let (_dir, app) = dispatch_app();
    let calls = install_collector(&app, &[]);
    insert_row(
        &app,
        QUEUED_ID,
        "draft",
        DISPATCHED_AT,
        BugReportValues::default(),
    );
    let (reply, _) = post_create(&app, json!({"localID": QUEUED_ID})).await;
    assert_api_error(&reply.unwrap(), 404, "UNKNOWN_LOCAL_ID");
    assert!(calls.lock().is_empty());
    assert_eq!(report_row(&app, QUEUED_ID).unwrap().status, "draft");
    assert!(
        !reports::draft_path(&app.reports.cfg(), QUEUED_ID)
            .unwrap()
            .exists()
    );
}

/// test_create_rejects_issue_time_together_with_local_id
#[tokio::test]
async fn create_rejects_issue_time_together_with_local_id() {
    let (_dir, app) = dispatch_app();
    let calls = install_collector(&app, &[]);
    dispatch(&app, json!({})).await;
    let reply = call(
        &app,
        Method::POST,
        "/api/v1/reports/create",
        pyfmt::dumps_default(&json!({"localID": QUEUED_ID, "issueTime": 1})),
    )
    .await;
    assert_api_error(&reply, 400, "INVALID_BODY");
    assert_eq!(
        body_json(&reply)["error"],
        "issueTime is taken from the dispatch when localID is given"
    );
    assert!(calls.lock().is_empty());
    assert_eq!(report_row(&app, QUEUED_ID).unwrap().status, "queued");
}

/// test_create_rejects_malformed_local_id
#[tokio::test]
async fn create_rejects_malformed_local_id() {
    let (_dir, app) = dispatch_app();
    let calls = install_collector(&app, &[]);
    let reply = call(
        &app,
        Method::POST,
        "/api/v1/reports/create",
        "{\"localID\": \"../history\"}",
    )
    .await;
    assert_api_error(&reply, 400, "INVALID_LOCAL_ID");
    assert!(calls.lock().is_empty());
}

/// test_create_by_local_id_cancelled_mid_collection_keeps_the_row_queued:
/// the dial can simply try again.
#[tokio::test]
async fn create_by_local_id_cancelled_mid_collection_keeps_the_row_queued() {
    let (_dir, app) = dispatch_app();
    dispatch(&app, json!({"name": "Ada", "email": "ada@example.com"})).await;
    let queued = report_row(&app, QUEUED_ID).unwrap();
    install_cancelling_collector(&app);
    app.reports.set_now(Some(DISPATCHED_AT + HOUR));
    let logs = capture_logs();

    let (reply, _) = post_create(&app, json!({"localID": QUEUED_ID})).await;

    assert!(reply.is_none());
    assert!(
        !reports::draft_path(&app.reports.cfg(), QUEUED_ID)
            .unwrap()
            .exists()
    );
    assert!(
        logs.messages(tracing::Level::ERROR, "api::bug_report")
            .is_empty()
    );
    let row = report_row(&app, QUEUED_ID).unwrap();
    assert_eq!(row.status, "queued");
    assert_eq!(row, queued);

    install_collector(&app, &[AUTOMATIC_DEBUG_FILE]);
    let (retry, _) = post_create(&app, json!({"localID": QUEUED_ID})).await;
    assert_eq!(
        body_json(&retry.unwrap()),
        json!({"localID": QUEUED_ID, "machineID": MACHINE_SERIAL})
    );
    assert_eq!(report_row(&app, QUEUED_ID).unwrap().status, "draft");
}

/// test_list_report_page_includes_queued_reports_with_their_ticket
#[tokio::test]
async fn list_report_page_includes_queued_reports_with_their_ticket() {
    let (_dir, app) = dispatch_app();
    dispatch(&app, json!({})).await;

    let reply = call(&app, Method::GET, "/api/v1/reports/list?page=0&size=10", "").await;

    let content = body_json(&reply)["content"].clone();
    assert_eq!(content.as_array().unwrap().len(), 1);
    assert_eq!(content[0]["localID"], QUEUED_ID);
    assert_eq!(content[0]["ticket"], 4242);
    assert_eq!(content[0]["dateAndTime"], DISPATCHED_AT);
}

/// GET /reports/list: pagination errors read like the Python's, a filter
/// without a known field is refused, a filter narrows the page.
#[tokio::test]
async fn list_handler_validates_pagination_and_filters() {
    let (_dir, app) = dispatch_app();
    dispatch(&app, json!({})).await;
    for (query, error) in [
        (
            "",
            "Invalid pagination params: HTTP 400: Bad Request (Missing argument page)",
        ),
        (
            "?page=0",
            "Invalid pagination params: HTTP 400: Bad Request (Missing argument size)",
        ),
        (
            "?page=x&size=1",
            "Invalid pagination params: invalid literal for int() with base 10: 'x'",
        ),
        (
            "?page=-1&size=1",
            "Invalid pagination params: page must be >= 0 and size must be > 0",
        ),
        (
            "?page=0&size=0",
            "Invalid pagination params: page must be >= 0 and size must be > 0",
        ),
        (
            "?page=0&size=1&filter=unknown%3D%3Dx",
            "Filter contains no valid fields",
        ),
    ] {
        let reply = call(
            &app,
            Method::GET,
            &format!("/api/v1/reports/list{query}"),
            "",
        )
        .await;
        assert_eq!(reply.status_code(), 400, "{query}");
        assert_eq!(
            body_json(&reply),
            json!({"error": error, "description": ""}),
            "{query}"
        );
    }
    let queued = call(
        &app,
        Method::GET,
        "/api/v1/reports/list?page=0&size=5&filter=status%3D%3Dqueued",
        "",
    )
    .await;
    assert_eq!(body_json(&queued)["content"].as_array().unwrap().len(), 1);
    assert_eq!(body_json(&queued)["size"], 5);
    assert_eq!(body_json(&queued)["page"], 0);
    assert_eq!(body_json(&queued)["hasMore"], false);
    let drafts = call(
        &app,
        Method::GET,
        "/api/v1/reports/list?page=0&size=5&filter=status%3D%3Ddraft",
        "",
    )
    .await;
    assert_eq!(body_json(&drafts)["content"], json!([]));
}

/// test_draft_patch_rejects_date_and_issue_times
#[test]
fn draft_patch_rejects_date_and_issue_times() {
    for patch in [json!({"dateAndTime": 2}), json!({"issueTime": 2})] {
        assert!(matches!(
            validate_draft_patch(patch.as_object().unwrap()),
            Err(PatchError::Forbidden(_))
        ));
    }
}

/// `_validate_draft_patch`: only `attachments.debugFiles.user`, a list of
/// debug file names.
#[test]
fn draft_patch_validation_messages() {
    let check = |patch: Value| validate_draft_patch(patch.as_object().unwrap());
    assert_eq!(
        check(json!({"b": 1, "a": 1, "ticket": 1})),
        Err(PatchError::Forbidden("Forbidden update keys: a, b".into()))
    );
    let only_user = Err(PatchError::Forbidden(
        "Only attachments.debugFiles.user can be updated".into(),
    ));
    assert_eq!(check(json!({"attachments": []})), only_user);
    assert_eq!(check(json!({"attachments": {}})), only_user);
    assert_eq!(
        check(json!({"attachments": {"debugFiles": {"automatic": []}}})),
        only_user
    );
    assert_eq!(
        check(json!({"attachments": {"debugFiles": {}, "machineInfo": true}})),
        only_user
    );
    assert_eq!(
        check(json!({"attachments": {"debugFiles": {"user": "x"}}})),
        Err(PatchError::Invalid(
            "attachments.debugFiles.user must be a list".into()
        ))
    );
    assert_eq!(
        check(json!({"attachments": {"debugFiles": {"user": ["../x", 1]}}})),
        Err(PatchError::Invalid(
            "Invalid debug file name: '../x'".into()
        ))
    );
    assert_eq!(
        check(json!({"attachments": {"debugFiles": {"user": [{"a": 1}]}}})),
        Err(PatchError::Invalid(
            "Invalid debug file name: {'a': 1}".into()
        ))
    );
    assert_eq!(
        check(json!({"attachments": {"debugFiles": {"user": null}}})),
        Ok(())
    );
    assert_eq!(
        check(
            json!({"description": "x", "attachments": {"debugFiles": {"user": ["10:00:00.shot.json.zst"]}}})
        ),
        Ok(())
    );
}

/// PUT /reports/draft/<id>: 403 for forbidden keys, 400 for bad bodies, 404
/// without a row or draft, the report info on success, and the 500 of a
/// `"user": null` patch (PARITY).
#[tokio::test]
async fn put_draft_handler() {
    let (_dir, app) = dispatch_app();
    let uri = format!("/api/v1/reports/draft/{QUEUED_ID}");
    let forbidden = call(&app, Method::PUT, &uri, "{\"issueTime\": 1}").await;
    assert_api_error(&forbidden, 403, "FORBIDDEN_UPDATE");
    assert_eq!(
        body_json(&forbidden)["error"],
        "Forbidden update keys: issueTime"
    );
    let invalid = call(&app, Method::PUT, &uri, "[]").await;
    assert_api_error(&invalid, 400, "INVALID_BODY");
    assert_eq!(
        body_json(&invalid)["error"],
        "Request body must be a JSON object"
    );
    let unknown = call(&app, Method::PUT, &uri, "{\"ticket\": 1}").await;
    assert_api_error(&unknown, 404, "UNKNOWN_LOCAL_ID");

    deletable_row(&app, QUEUED_ID, "draft");
    let no_draft = call(&app, Method::PUT, &uri, "{\"ticket\": 1}").await;
    assert_api_error(&no_draft, 404, "UNKNOWN_LOCAL_ID");

    let draft_dir = reports::draft_path(&app.reports.cfg(), QUEUED_ID).unwrap();
    std::fs::create_dir_all(&draft_dir).unwrap();
    reports::write_draft_report_info(&draft_dir, &json!({"localID": QUEUED_ID, "ticket": null}))
        .unwrap();
    let updated = call(
        &app,
        Method::PUT,
        &uri,
        "{\"ticket\": 7, \"description\": \"d\"}",
    )
    .await;
    assert_eq!(updated.status_code(), 200);
    assert_eq!(
        body_json(&updated),
        json!({
            "localID": QUEUED_ID,
            "ticket": 7,
            "attachments": {"debugFiles": {"automatic": [], "user": []}},
            "description": "d",
        })
    );
    assert_eq!(report_row(&app, QUEUED_ID).unwrap().ticket_number, 7);

    let missing_file = call(
        &app,
        Method::PUT,
        &uri,
        "{\"attachments\": {\"debugFiles\": {\"user\": [\"2026-01-01/10:00:00.shot.json.zst\"]}}}",
    )
    .await;
    assert_eq!(missing_file.status_code(), 200);
    let log = std::fs::read_to_string(draft_dir.join(REPORT_LOG_NAME)).unwrap();
    assert!(log.contains("User selected debug file not found: 2026-01-01/10:00:00.shot.json.zst"));

    let null_user = call(
        &app,
        Method::PUT,
        &uri,
        "{\"attachments\": {\"debugFiles\": {\"user\": null}}}",
    )
    .await;
    assert_eq!(null_user.status_code(), 500);
    assert_eq!(
        body_json(&null_user),
        json!({
            "error": "Failed to update report draft",
            "description": "",
            "data": {"message": "'NoneType' object is not iterable", "code": "INTERNAL"},
        })
    );
}

/// `_api_error`: a 5xx gets `data.code` `INTERNAL` after what it has.
#[test]
fn api_errors_carry_internal_codes() {
    let reply = api_error(500, "boom", message("why"));
    assert_eq!(
        crate::testing::json(&reply),
        json!({"error": "boom", "description": "", "data": {"message": "why", "code": "INTERNAL"}})
    );
    let reply = api_error(503, "boom", None);
    assert_eq!(
        crate::testing::json(&reply),
        json!({"error": "boom", "description": "", "data": {"code": "INTERNAL"}})
    );
    assert_eq!(
        reply.header("Content-Type"),
        Some("application/json; charset=UTF-8")
    );
}

/// `ReportsCreateHandler.on_connection_close`: a client that disconnects
/// over real HTTP stops the collection (the watcher request's connection
/// closes) and nothing is left behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_disconnect_over_http_cancels_the_collection() {
    let watcher = TestServer::start(|_| ServerReply::Hang);
    let (_dir, app) = machine_app(&watcher.url("/health/logs"), &watcher.url("/health/status"));
    let router = Arc::new(crate::api::router(&app));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let served = app.clone();
    let service = axum::Router::new().fallback(move |request: axum::extract::Request| {
        let (router, app) = (router.clone(), served.clone());
        async move { crate::http::serve(router, app, None, request).await }
    });
    tokio::spawn(async move { axum::serve(listener, service).await });

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let sent = client
        .post(format!("http://{address}/api/v1/reports/create"))
        .timeout(std::time::Duration::from_millis(500))
        .send()
        .await;

    assert!(sent.is_err(), "the collection must still be waiting on the watcher");
    eventually(|| watcher.closed_while_hanging() == 1).await;
    eventually(|| !app.reports.collection_in_progress()).await;
    assert_eq!(draft_dir_entries(&app), 0);
    assert!(all_rows(&app).is_empty());
}
