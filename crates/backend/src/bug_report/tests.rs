//! tests/test_bug_report_api.py: the report storage, collection, archives
//! and sweep (the handler tests are in `api/bug_report/tests.rs`).

use std::path::Path;
use std::time::Duration;

use chrono::TimeZone;
use history::bug_reports::BugReportValues;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::test_support::*;
use super::*;

const QUEUED_ID: &str = "018f0a2b-1234-7abc-8def-0123456789ab";
const SECOND_ID: &str = "018f0a2b-1234-7abc-8def-0123456789ac";
const THIRD_ID: &str = "018f0a2b-1234-7abc-8def-0123456789ad";
const DISPATCHED_AT: i64 = 1_700_000_000;

fn names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn local_timestamp(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    chrono::Local
        .with_ymd_and_hms(y, mo, d, h, mi, s)
        .single()
        .unwrap()
        .timestamp()
}

/// A draft directory with a report info like the Python tests write.
fn write_draft(app: &App, local_id: &str, report_info: Value) -> PathBuf {
    let draft_dir = draft_path(&app.reports.cfg(), local_id).unwrap();
    std::fs::create_dir_all(&draft_dir).unwrap();
    write_draft_report_info(&draft_dir, &report_info).unwrap();
    draft_dir
}

fn report_info(
    local_id: &str,
    attachments: Value,
    multimedia: Value,
    ticket: Value,
    event_id: Value,
) -> Value {
    json!({
        "description": null,
        "dateAndTime": 1,
        "attachments": attachments,
        "multimedia": multimedia,
        "machineID": "machine",
        "eventID": event_id,
        "baseEventID": null,
        "ticket": ticket,
        "localID": local_id,
    })
}

fn empty_attachments() -> Value {
    json!({
        "debugFiles": {"automatic": [], "user": []},
        "machineInfo": false,
        "machineLogs": false,
        "machineStatus": false,
    })
}

fn draft_row(app: &App, local_id: &str, machine: bool, ticket: Value) {
    insert_row(
        app,
        local_id,
        "draft",
        1,
        BugReportValues {
            log_files: Some(Value::Null),
            machine_info: Some(json!(machine)),
            machine_logs: Some(json!(machine)),
            machine_status: Some(json!(machine)),
            ticket_number: Some(ticket),
            ..Default::default()
        },
    );
}

/// test_select_debug_files_descending
#[test]
fn select_debug_files_descending() {
    let (_dir, app) = emulated_app();
    let root = app.reports.cfg().debug_history;
    debug_file(&root, "2026-05-17", "08:00:00.shot.json.zst");
    debug_file(&root, "2026-05-18", "09:00:00.shot.json.zst");
    debug_file(&root, "2026-05-18", "10:00:00.shot.json.zst");

    let (selected, errors) = select_debug_files(&app.reports.cfg(), 2, None).unwrap();

    assert_eq!(
        names(&selected),
        ["10:00:00.shot.json.zst", "09:00:00.shot.json.zst"]
    );
    assert!(errors.is_empty());
}

/// test_fetch_report_files_uses_parent_debug_file_names
#[tokio::test]
async fn fetch_report_files_uses_parent_debug_file_names() {
    let server = watcher("logs", "{\"ok\": true}");
    let (_dir, app) = machine_app(
        &server.url("/health/logs?filter=*"),
        &server.url("/health/status"),
    );
    let debug_name = "2026-05-18/10:00:00.shot.json.zst";
    debug_file(
        &app.reports.cfg().debug_history,
        "2026-05-18",
        "10:00:00.shot.json.zst",
    );

    let draft_dir = draft_path(&app.reports.cfg(), "local-test-id").unwrap();
    let fetched = fetch_report_files(&app, &draft_dir, None, true, None, None)
        .await
        .unwrap();

    assert_eq!(fetched.automatic_debug_files, [debug_name]);
    let archive_name = debug_archive_name(debug_name);
    assert!(fetched.files.contains_key(&archive_name));
    assert!(fetched.machine_status);
    assert_eq!(
        std::fs::read_to_string(&fetched.files[&archive_name]).unwrap(),
        debug_name
    );
    assert_eq!(
        std::fs::read_to_string(draft_dir.join(MACHINE_STATUS_NAME)).unwrap(),
        "{\"ok\": true}"
    );
}

/// test_fetch_machine_logs_uses_emulated_response_without_watcher
#[tokio::test]
async fn fetch_machine_logs_uses_emulated_response_without_watcher() {
    let server = watcher("logs", "{}");
    let (_dir, app) = emulated_app();
    app.reports
        .configure(|cfg| cfg.watcher_logs_url = server.url("/health/logs"));

    let logs = fetch_machine_logs(&app, Some((123, 456)), None)
        .await
        .unwrap();

    assert!(logs.contains("Emulated machine logs generated for bug report"));
    assert!(logs.contains("start_time=123, end_time=456"));
    assert!(server.requests().is_empty());
}

/// test_fetch_machine_logs_converts_range_to_watcher_hours
#[tokio::test]
async fn fetch_machine_logs_converts_range_to_watcher_hours() {
    let server = watcher("logs", "{}");
    let (_dir, app) = machine_app(
        &server.url("/health/logs?filter=*"),
        &server.url("/health/status"),
    );
    app.reports.set_now(Some(100_000));
    let range = Some((100_000 - 24 * 60 * 60 - 1, 100_000 - 60 * 60 - 1));

    let (url, timeout, _) = machine_logs_request(&app.reports.cfg(), range, 100_000);
    let logs = fetch_machine_logs(&app, range, None).await.unwrap();

    assert_eq!(logs, "logs");
    assert!(url.ends_with("&since=25&until=1"));
    assert_eq!(timeout, Duration::from_secs(600));
    assert!(server.requests()[0].target.ends_with("&since=25&until=1"));
}

/// test_fetch_machine_status_uses_emulated_response_without_watcher
#[tokio::test]
async fn fetch_machine_status_uses_emulated_response_without_watcher() {
    let server = watcher("logs", "{}");
    let (_dir, app) = emulated_app();
    app.reports
        .configure(|cfg| cfg.watcher_status_url = server.url("/health/status"));

    let status: Value =
        serde_json::from_str(&fetch_machine_status(&app, None).await.unwrap()).unwrap();

    assert_eq!(status["emulated"], true);
    assert_eq!(status["status"], "ok");
    assert_eq!(status["source"], "meticulous-backend");
    assert!(server.requests().is_empty());
}

/// test_fetch_watcher_text_uses_aiohttp_and_preserves_timeout_and_decoding
#[tokio::test]
async fn fetch_watcher_text_preserves_timeout_and_decoding() {
    let mut body = "caf\u{e9}".as_bytes().to_vec();
    body.push(0xff);
    let server = TestServer::start(move |_| (200, body.clone()));
    let client = watcher::build_client(true).unwrap();

    let text = watcher::fetch_text(
        &client,
        &server.url("/health/status"),
        Duration::from_secs(120),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(server.requests()[0].target, "/health/status");
    // An invalid UTF-8 tail is replaced (U+FFFD), not raised.
    assert_eq!(text, "caf\u{e9}\u{fffd}");
    let (_dir, app) = emulated_app();
    assert_eq!(
        machine_status_request(&app.reports.cfg()).1,
        Duration::from_secs(120)
    );

    // raise_for_status=True: a non-2xx answer is an error.
    let failing = TestServer::start(|_| (500, "boom".to_string()));
    let error = watcher::fetch_text(
        &client,
        &failing.url("/health/status"),
        Duration::from_secs(120),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        watcher::FetchError::Failed(watcher::HttpError::new(
            "ClientResponseError",
            format!(
                "500, message='Internal Server Error', url='{}'",
                failing.url("/health/status")
            )
        ))
    );
}

/// test_fetch_watcher_text_cancels_active_task_on_disconnect: the watcher
/// request is dropped, so its connection closes.
#[tokio::test]
async fn fetch_watcher_text_cancels_active_request_on_disconnect() {
    let server = TestServer::start(|_| ServerReply::Hang);
    let client = watcher::build_client(true).unwrap();
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let url = server.url("/health/logs");
    let fetch = tokio::spawn(async move {
        watcher::fetch_text(&client, &url, Duration::from_secs(600), Some(&token), None).await
    });
    eventually(|| !server.requests().is_empty()).await;

    cancellation.cancel();

    assert_eq!(fetch.await.unwrap(), Err(watcher::FetchError::Cancelled));
    eventually(|| server.closed_while_hanging() == 1).await;
}

/// test_fetch_report_files_raises_cancelled_at_next_boundary_after_disconnect
#[tokio::test]
async fn fetch_report_files_raises_cancelled_at_next_boundary_after_disconnect() {
    let server = watcher("logs", "{}");
    let (_dir, app) = machine_app(&server.url("/health/logs"), &server.url("/health/status"));
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let draft_dir = draft_path(&app.reports.cfg(), "boundary-test-id").unwrap();
    let fetched = fetch_report_files(&app, &draft_dir, None, true, Some(&cancellation), None).await;

    assert_eq!(fetched, Err(FetchFailure::Cancelled));
    assert!(
        server.requests().is_empty(),
        "machine logs must not be fetched after disconnect"
    );
}

/// test_fetch_report_files_logs_stage_error_as_it_happens
#[tokio::test]
async fn fetch_report_files_logs_stage_error_as_it_happens() {
    let server = watcher("logs", "{\"ok\": true}");
    let unreachable = format!("http://127.0.0.1:{}/health/logs", TestServer::closed_port());
    let (_dir, app) = machine_app(&unreachable, &server.url("/health/status"));
    let logs = capture_logs();

    let draft_dir = draft_path(&app.reports.cfg(), "logged-error-id").unwrap();
    let fetched = fetch_report_files(&app, &draft_dir, None, true, None, None)
        .await
        .unwrap();

    // Logged at the point of failure, tagged with the localID, and the
    // journal says exactly what the bundle says.
    let warnings = logs.messages(tracing::Level::WARN, "bug_report");
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0],
        format!("[logged-error-id] {}", fetched.errors[0])
    );
    assert!(
        fetched.errors[0]
            .starts_with("Failed to fetch machine logs: Cannot connect to host 127.0.0.1:")
    );
    assert!(!fetched.machine_logs);
    assert!(fetched.machine_status);
}

/// test_cancelled_stage_is_never_logged_as_a_collection_error: a disconnect
/// leaves no trace, the journal included.
#[tokio::test]
async fn cancelled_stage_is_never_logged_as_a_collection_error() {
    let server = TestServer::start(|_| ServerReply::Hang);
    let (_dir, app) = machine_app(&server.url("/health/logs"), &server.url("/health/status"));
    let logs = capture_logs();
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let draft_dir = draft_path(&app.reports.cfg(), "cancelled-stage-id").unwrap();
    let (a, dir) = (app.clone(), draft_dir.clone());
    let fetch =
        tokio::spawn(
            async move { fetch_report_files(&a, &dir, None, true, Some(&token), None).await },
        );
    eventually(|| !server.requests().is_empty()).await;

    cancellation.cancel();

    assert_eq!(fetch.await.unwrap(), Err(FetchFailure::Cancelled));
    assert!(logs.messages(tracing::Level::WARN, "bug_report").is_empty());
    assert!(logs.messages(tracing::Level::INFO, "bug_report").is_empty());
}

/// test_short_debug_file_count_logs_at_info_not_warning: a machine that has
/// not brewed ten shots reports a short count on every report.
#[tokio::test]
async fn short_debug_file_count_logs_at_info_not_warning() {
    let server = watcher("logs", "{\"ok\": true}");
    let (_dir, app) = machine_app(&server.url("/health/logs"), &server.url("/health/status"));
    let logs = capture_logs();

    let draft_dir = draft_path(&app.reports.cfg(), "few-shots-id").unwrap();
    let fetched = fetch_report_files(&app, &draft_dir, None, true, None, None)
        .await
        .unwrap();

    assert!(logs.messages(tracing::Level::WARN, "bug_report").is_empty());
    assert_eq!(
        logs.messages(tracing::Level::INFO, "bug_report"),
        ["[few-shots-id] Only found 0 debug files while reporting; requested 10."]
    );
    assert!(
        fetched
            .errors
            .contains(&"Only found 0 debug files while reporting; requested 10.".to_string())
    );
}

/// test_fetch_report_files_includes_active_incomplete_debug_shot_first
#[tokio::test]
async fn fetch_report_files_includes_active_incomplete_debug_shot_first() {
    let (_dir, app) = emulated_app();
    let root = app.reports.cfg().debug_history;
    for index in 0..10 {
        debug_file(
            &root,
            "2026-05-18",
            &format!("10:00:0{index}.shot.json.zst"),
        );
    }
    app.debug_shots.start(&app);

    let draft_dir = draft_path(&app.reports.cfg(), "local-test-id").unwrap();
    let fetched = fetch_report_files(&app, &draft_dir, None, true, None, None)
        .await
        .unwrap();

    assert_eq!(fetched.automatic_debug_files.len(), MAX_DEBUG_SHOTS);
    let incomplete = &fetched.automatic_debug_files[0];
    assert!(
        incomplete.ends_with(".shot_incomplete.json.zst"),
        "{incomplete}"
    );
    assert!(fetched.files.contains_key(&debug_archive_name(incomplete)));
    assert!(draft_dir.join(debug_archive_name(incomplete)).is_file());
    assert!(
        !fetched
            .files
            .contains_key(&debug_archive_name("2026-05-18/10:00:00.shot.json.zst"))
    );
}

/// test_select_debug_files_prioritizes_range_then_older_history
#[test]
fn select_debug_files_prioritizes_range_then_older_history() {
    let (_dir, app) = emulated_app();
    let root = app.reports.cfg().debug_history;
    for name in [
        "09:00:00.shot.json.zst",
        "10:00:00.shot.json.zst",
        "11:00:00.shot.json.zst",
        "12:00:00.shot.json.zst",
        "13:00:00.shot.json.zst",
    ] {
        debug_file(&root, "2026-05-18", name);
    }
    let start = local_timestamp(2026, 5, 18, 10, 0, 0);
    let end = local_timestamp(2026, 5, 18, 12, 0, 0);

    let (selected, errors) = select_debug_files(&app.reports.cfg(), 4, Some((start, end))).unwrap();

    assert_eq!(
        names(&selected),
        [
            "12:00:00.shot.json.zst",
            "11:00:00.shot.json.zst",
            "10:00:00.shot.json.zst",
            "09:00:00.shot.json.zst",
        ]
    );
    assert!(errors.is_empty());
}

/// test_fetch_report_files_skips_active_debug_shot_for_historical_range
#[tokio::test]
async fn fetch_report_files_skips_active_debug_shot_for_historical_range() {
    let (_dir, app) = emulated_app();
    app.debug_shots.start(&app);

    let draft_dir = draft_path(&app.reports.cfg(), "historic").unwrap();
    let fetched = fetch_report_files(&app, &draft_dir, Some((1, 2)), false, None, None)
        .await
        .unwrap();

    assert!(fetched.automatic_debug_files.is_empty());
    assert!(!draft_dir.join(DEBUG_ARCHIVE_DIR).exists());
}

/// test_incomplete_debug_shot_snapshot_keeps_active_state: the snapshot is
/// `<day>/<time>.shot_incomplete.json.zst` and the shot keeps running.
#[test]
fn incomplete_debug_shot_snapshot_keeps_active_state() {
    let (dir, app) = emulated_app();
    app.debug_shots.start(&app);
    let root = dir.path().join("snapshot");

    let name = app
        .debug_shots
        .write_current_incomplete(&app, &root)
        .unwrap();

    let pattern =
        regex::Regex::new(r"^\d{4}-\d{2}-\d{2}/\d{2}:\d{2}:\d{2}\.shot_incomplete\.json\.zst$")
            .unwrap();
    assert!(pattern.is_match(&name), "{name}");
    assert!(
        app.debug_shots
            .write_current_incomplete(&app, &root)
            .is_some()
    );
    let payload: Value = serde_json::from_slice(
        &zstd::stream::decode_all(std::fs::read(root.join(&name)).unwrap().as_slice()).unwrap(),
    )
    .unwrap();
    assert_eq!(payload["type"], "shot");
    assert_eq!(payload["data"], json!([]));
}

/// test_fiql_filter_ignores_invalid_fields_and_rejects_empty
#[test]
fn fiql_filter_ignores_invalid_fields_and_rejects_empty() {
    let (valid, invalid) =
        history::bug_reports::parse_fiql(Some("unknown==x;status==draft,creationTime=gt=10"))
            .unwrap();
    let (empty, empty_invalid) = history::bug_reports::parse_fiql(Some("unknown==x")).unwrap();
    assert!(valid.is_some());
    assert!(!invalid);
    assert!(empty.is_none());
    assert!(empty_invalid);
}

/// test_draft_patch_adds_user_debug_file_to_draft_directory
#[test]
fn draft_patch_adds_user_debug_file_to_draft_directory() {
    let (_dir, app) = emulated_app();
    let user_file = debug_file(
        &app.reports.cfg().debug_history,
        "2026-05-18",
        "14:00:00.user.json.zst",
    );
    let user_file_name = safe_archive_name(&user_file);
    let local_id = "local-test-id";
    let attachments = json!({
        "debugFiles": {"automatic": [], "user": []},
        "machineInfo": true,
        "machineLogs": true,
        "machineStatus": true,
    });
    let draft_dir = write_draft(
        &app,
        local_id,
        report_info(local_id, attachments, Value::Null, Value::Null, Value::Null),
    );
    draft_row(&app, local_id, true, Value::Null);

    let patch = json!({"attachments": {"debugFiles": {"user": [user_file_name]}}});
    let updated = apply_draft_patch(&app, local_id, patch.as_object().unwrap()).unwrap();

    assert_eq!(
        updated["attachments"]["debugFiles"]["user"],
        json!([user_file_name])
    );
    assert!(draft_files(&draft_dir).contains_key(&debug_archive_name(&user_file_name)));
    assert_eq!(
        std::fs::read_to_string(draft_dir.join(debug_archive_name(&user_file_name))).unwrap(),
        "2026-05-18/14:00:00.user.json.zst"
    );
    assert_eq!(
        report_row(&app, local_id).unwrap().log_files,
        json!(user_file_name)
    );
}

/// test_draft_directory_can_be_compressed
#[test]
fn draft_directory_can_be_compressed() {
    let (_dir, app) = emulated_app();
    let local_id = "local-test-id";
    let draft_dir = write_draft(
        &app,
        local_id,
        report_info(
            local_id,
            json!({"machineStatus": true}),
            Value::Null,
            Value::Null,
            Value::Null,
        ),
    );
    std::fs::write(draft_dir.join(MACHINE_STATUS_NAME), "{\"ok\": true}").unwrap();
    let archive_path = app.reports.cfg().draft_dir.join("out.zstd");

    archive::write_tar_zstd_from_draft(&archive_path, &draft_dir).unwrap();
    let (info, files, _extracted) = archive::read_tar_zstd(&archive_path);

    assert_eq!(info["localID"], local_id);
    assert!(files.contains_key(MACHINE_STATUS_NAME));
}

/// test_create_report_uses_trailing_range_for_recent_and_future_issue_times
#[test]
fn collection_range_is_trailing_for_recent_and_future_issue_times() {
    let now = 200_000;
    assert_eq!(collection_range(now - 1, now), (now - 24 * HOUR, now));
    assert_eq!(collection_range(now + 1, now), (now - 24 * HOUR, now));
    assert_eq!(
        collection_range(now - 12 * HOUR, now),
        (now - 24 * HOUR, now)
    );
    assert_eq!(
        collection_range(now - 25 * HOUR, now),
        (now - 37 * HOUR, now - 13 * HOUR)
    );
}

/// test_draft_patch_persists_ticket_and_multimedia_in_db_and_report_info
#[test]
fn draft_patch_persists_ticket_and_multimedia_in_db_and_report_info() {
    let (_dir, app) = emulated_app();
    let local_id = "local-test-id";
    let attachments = json!({
        "debugFiles": {"automatic": [], "user": []},
        "machineInfo": true,
        "machineLogs": true,
        "machineStatus": true,
    });
    let draft_dir = write_draft(
        &app,
        local_id,
        report_info(local_id, attachments, Value::Null, Value::Null, Value::Null),
    );
    draft_row(&app, local_id, true, Value::Null);

    let patch = json!({"ticket": 1234, "multimedia": 2});
    crate::api::bug_report::validate_draft_patch(patch.as_object().unwrap()).unwrap();
    let updated = apply_draft_patch(&app, local_id, patch.as_object().unwrap()).unwrap();

    assert_eq!(updated["ticket"], 1234);
    assert_eq!(updated["multimedia"], 2);
    let stored = read_draft_report_info(&draft_dir).unwrap();
    assert_eq!(stored["ticket"], 1234);
    assert_eq!(stored["multimedia"], 2);
    let row = report_row(&app, local_id).unwrap();
    assert_eq!(row.ticket_number, 1234);
    assert_eq!(row.multimedia, 2);
}

/// test_list_report_page_returns_newest_first_with_machine_id
#[test]
fn list_report_page_returns_newest_first_with_machine_id() {
    let (_dir, app) = emulated_app();
    for (id, time, machine, flag) in [
        ("older-id", 1, "older-machine", false),
        ("newer-id", 2, "newer-machine", true),
    ] {
        insert_row(
            &app,
            id,
            "draft",
            time,
            BugReportValues {
                machine_id: Some(json!(machine)),
                machine_info: Some(json!(flag)),
                machine_logs: Some(json!(flag)),
                machine_status: Some(json!(flag)),
                ..Default::default()
            },
        );
    }

    let response = list_report_page(&app, 0, 1, None).unwrap();

    let first = &response["content"][0];
    assert_eq!(first["localID"], "newer-id");
    assert_eq!(first["machineID"], "newer-machine");
    assert_eq!(first["dateAndTime"], 2);
    assert_eq!(first["issueTime"], 2);
    assert_eq!(first["status"], "draft");
    assert_eq!(response["hasMore"], true);
}

/// test_submit_update_persists_db_and_report_info
#[test]
fn submit_update_persists_db_and_report_info() {
    let (_dir, app) = emulated_app();
    let local_id = "submit-id";
    let draft_dir = write_draft(
        &app,
        local_id,
        report_info(
            local_id,
            empty_attachments(),
            json!(1),
            Value::Null,
            Value::Null,
        ),
    );
    draft_row(&app, local_id, false, Value::Null);

    let updated = mark_report_submitted(&app, local_id, "event-1", 3, Some(json!(42))).unwrap();

    let row = report_row(&app, local_id).unwrap();
    let finalized = finalized_draft_path(&app.reports.cfg(), local_id).unwrap();
    let (archived, _files, _extracted) = archive::read_tar_zstd(&finalized);
    assert!(updated);
    assert!(!draft_dir.exists());
    assert!(finalized.exists());
    assert_eq!(row.event_id, "event-1");
    assert_eq!(row.ticket_number, 42);
    assert_eq!(row.submission_time, 3);
    assert_eq!(row.status, "submitted");
    assert_eq!(archived["eventID"], "event-1");
    assert_eq!(archived["ticket"], 42);
    assert_eq!(archived["multimedia"], 1);
}

/// test_compressed_draft_contains_latest_report_info_after_updates
#[test]
fn compressed_draft_contains_latest_report_info_after_updates() {
    let (_dir, app) = emulated_app();
    let local_id = "submit-id";
    let draft_dir = write_draft(
        &app,
        local_id,
        report_info(
            local_id,
            empty_attachments(),
            Value::Null,
            Value::Null,
            Value::Null,
        ),
    );
    draft_row(&app, local_id, false, Value::Null);

    let patch = json!({"ticket": 42, "multimedia": 2});
    apply_draft_patch(&app, local_id, patch.as_object().unwrap()).unwrap();
    mark_report_submitted(&app, local_id, "event-1", 3, Some(json!(42))).unwrap();

    let finalized = finalized_draft_path(&app.reports.cfg(), local_id).unwrap();
    let (archived, files, _extracted) = archive::read_tar_zstd(&finalized);
    assert!(!files.contains_key(REPORT_INFO_NAME));
    assert!(!draft_dir.exists());
    assert_eq!(archived["eventID"], "event-1");
    assert_eq!(archived["ticket"], 42);
    assert_eq!(archived["multimedia"], 2);
}

/// test_submit_without_ticket_preserves_existing_ticket
#[test]
fn submit_without_ticket_preserves_existing_ticket() {
    let (_dir, app) = emulated_app();
    let local_id = "submit-id";
    let draft_dir = write_draft(
        &app,
        local_id,
        report_info(
            local_id,
            empty_attachments(),
            Value::Null,
            json!(42),
            Value::Null,
        ),
    );
    draft_row(&app, local_id, false, json!(42));

    let updated = mark_report_submitted(&app, local_id, "event-1", 3, None).unwrap();

    let row = report_row(&app, local_id).unwrap();
    let finalized = finalized_draft_path(&app.reports.cfg(), local_id).unwrap();
    let (archived, _files, _extracted) = archive::read_tar_zstd(&finalized);
    assert!(updated);
    assert!(!draft_dir.exists());
    assert_eq!(row.event_id, "event-1");
    assert_eq!(row.ticket_number, 42);
    assert_eq!(archived["eventID"], "event-1");
    assert_eq!(archived["ticket"], 42);
}

/// test_repeat_submit_after_finalization_is_a_successful_noop
#[test]
fn repeat_submit_after_finalization_is_a_successful_noop() {
    let (_dir, app) = emulated_app();
    insert_row(
        &app,
        QUEUED_ID,
        "submitted",
        1,
        BugReportValues {
            machine_info: Some(json!(false)),
            machine_logs: Some(json!(false)),
            machine_status: Some(json!(false)),
            event_id: Some(json!("first-event")),
            submission_time: Some(json!(2)),
            ..Default::default()
        },
    );

    assert!(mark_report_submitted(&app, QUEUED_ID, "retry-event", 3, None).unwrap());

    let row = report_row(&app, QUEUED_ID).unwrap();
    assert_eq!(row.status, "submitted");
    assert_eq!(row.event_id, "first-event");
    assert_eq!(row.submission_time, 2);
}

/// test_local_id_validation_and_paths_reject_traversal
#[test]
fn local_id_validation_and_paths_reject_traversal() {
    let valid = "018f0a2b-1234-7abc-8def-0123456789ab";
    assert_eq!(validate_local_id(&json!(valid)).unwrap(), valid);
    for value in [
        "../history",
        "..",
        "A18f0a2b-1234-7abc-8def-0123456789ab",
        "short",
    ] {
        let error = validate_local_id(&json!(value)).unwrap_err();
        assert_eq!(error.code, "INVALID_LOCAL_ID");
    }
    let (_dir, app) = emulated_app();
    assert_eq!(
        draft_path(&app.reports.cfg(), "../history")
            .unwrap_err()
            .code,
        "INVALID_LOCAL_ID"
    );
}

/// test_write_tar_zstd_cleans_atomic_temp_on_failure (the failure here is
/// an unreadable draft file instead of a failing `zstd`).
#[test]
fn write_tar_zstd_cleans_atomic_temp_on_failure() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, app) = emulated_app();
    let draft_dir = draft_path(&app.reports.cfg(), "draft").unwrap();
    std::fs::create_dir_all(&draft_dir).unwrap();
    let file = draft_dir.join("file.txt");
    std::fs::write(&file, "data").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&file).is_ok() {
        eprintln!("running with permission to read anything; the failure cannot be provoked");
        return;
    }
    let output = app.reports.cfg().draft_dir.join("draft.zstd");

    let error = archive::write_tar_zstd_from_draft(&output, &draft_dir).unwrap_err();

    assert_eq!(error.type_name, "PermissionError");
    assert!(!output.exists());
    assert!(!app.reports.cfg().draft_dir.join("draft.zstd.tmp").exists());
}

/// test_find_debug_file_rejects_globs
#[test]
fn find_debug_file_rejects_globs() {
    let (_dir, app) = emulated_app();
    let cfg = app.reports.cfg();
    debug_file(&cfg.debug_history, "2026-05-18", "10:00:00.shot.json.zst");
    assert_eq!(find_debug_file(&cfg, "*").unwrap(), None);
    assert_eq!(find_debug_file(&cfg, "**/*").unwrap(), None);
    assert_eq!(find_debug_file(&cfg, "../x.json.zst").unwrap(), None);
    assert!(
        find_debug_file(&cfg, "10:00:00.shot.json.zst")
            .unwrap()
            .is_some()
    );
    assert!(
        find_debug_file(&cfg, "2026-05-18/10:00:00.shot.json.zst")
            .unwrap()
            .is_some()
    );
}

/// test_upload_diagnostics_survive_report_archive
#[tokio::test]
async fn upload_diagnostics_survive_report_archive() {
    let server = watcher("[CommunityUpload] upload_resumed", "{}");
    let (dir, app) = machine_app(&server.url("/health/logs"), &server.url("/health/status"));
    let source = app
        .env
        .paths
        .config_dir
        .join("community-upload")
        .join("diagnostics.json");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(
        &source,
        "{\"schemaVersion\": 1, \"pendingCount\": 7, \"privateSeed\": \"SECRET\"}",
    )
    .unwrap();

    let draft = draft_path(&app.reports.cfg(), "upload-diagnostics-test").unwrap();
    let fetched = fetch_report_files(&app, &draft, None, false, None, None)
        .await
        .unwrap();
    let name = crate::api::community_upload_diagnostics::NAME;
    assert!(fetched.files.contains_key(name));
    std::fs::write(draft.join(REPORT_INFO_NAME), "{}").unwrap();
    let archive_path = dir.path().join("report.tar.zst");
    archive::write_tar_zstd_from_draft(&archive_path, &draft).unwrap();

    let (_, files, _extracted) = archive::read_tar_zstd(&archive_path);
    let payload = std::fs::read_to_string(&files[name]).unwrap();
    assert!(!payload.contains("SECRET"));
    assert_eq!(
        serde_json::from_str::<Value>(&payload).unwrap()["pendingCount"],
        7
    );
    assert!(
        std::fs::read_to_string(&files[MACHINE_LOGS_NAME])
            .unwrap()
            .contains("upload_resumed")
    );
}

/// test_sweep_keeps_fresh_queued_reports_and_expires_stale_ones
#[test]
fn sweep_keeps_fresh_queued_reports_and_expires_stale_ones() {
    let (_dir, app) = emulated_app();
    let now = DISPATCHED_AT;
    let max_age = app.reports.cfg().draft_max_age;
    insert_row(
        &app,
        QUEUED_ID,
        "queued",
        now - 60,
        BugReportValues::default(),
    );
    insert_row(
        &app,
        SECOND_ID,
        "queued",
        now - max_age,
        BugReportValues::default(),
    );
    insert_row(
        &app,
        THIRD_ID,
        "queued",
        now - max_age - 1,
        BugReportValues::default(),
    );

    let stats = sweep_reports(&app, Some(now)).unwrap();

    assert_eq!(
        stats.to_json(),
        json!({"tmp": 0, "drafts": 0, "rows": 1, "archives": 0, "finalized": 0})
    );
    let mut ids: Vec<Value> = all_rows(&app).into_iter().map(|r| r.local_id).collect();
    ids.sort_by_key(|v| v.to_string());
    assert_eq!(ids, [json!(QUEUED_ID), json!(SECOND_ID)]);
}

/// test_sweep_still_deletes_draft_rows_without_files
#[test]
fn sweep_still_deletes_draft_rows_without_files() {
    let (_dir, app) = emulated_app();
    let now = DISPATCHED_AT;
    insert_row(
        &app,
        QUEUED_ID,
        "queued",
        now - 60,
        BugReportValues::default(),
    );
    insert_row(
        &app,
        SECOND_ID,
        "draft",
        now - 60,
        BugReportValues::default(),
    );

    let stats = sweep_reports(&app, Some(now)).unwrap();

    assert_eq!(stats.rows, 1);
    let ids: Vec<Value> = all_rows(&app).into_iter().map(|r| r.local_id).collect();
    assert_eq!(ids, [json!(QUEUED_ID)]);
}

/// `_sweep_entry`: old temporary files, expired archives (by submission
/// time, else mtime), orphan draft directories, and the retry of a
/// submitted draft's archive.
#[test]
fn sweep_removes_expired_artifacts_and_retries_finalization() {
    let (_dir, app) = emulated_app();
    let cfg = app.reports.cfg();
    let now = chrono::Utc::now().timestamp() + 2 * HOUR;
    std::fs::write(cfg.draft_dir.join("x.zstd.tmp"), "tmp").unwrap();
    std::fs::create_dir_all(cfg.draft_dir.join("orphan")).unwrap();
    insert_row(
        &app,
        SECOND_ID,
        "submitted",
        1,
        BugReportValues {
            submission_time: Some(json!(now - cfg.archive_max_age - 1)),
            ..Default::default()
        },
    );
    std::fs::write(cfg.draft_dir.join(format!("{SECOND_ID}.zstd")), "old").unwrap();
    insert_row(&app, THIRD_ID, "submitted", now, BugReportValues::default());
    write_draft(&app, THIRD_ID, json!({"localID": THIRD_ID}));

    let stats = sweep_reports(&app, Some(now)).unwrap();

    assert_eq!(
        stats,
        SweepStats {
            tmp: 1,
            drafts: 1,
            rows: 0,
            archives: 1,
            finalized: 1
        }
    );
    assert!(!cfg.draft_dir.join("x.zstd.tmp").exists());
    assert!(!cfg.draft_dir.join("orphan").exists());
    assert!(!cfg.draft_dir.join(format!("{SECOND_ID}.zstd")).exists());
    assert!(!cfg.draft_dir.join(THIRD_ID).exists());
    let (info, _, _extracted) =
        archive::read_tar_zstd(&cfg.draft_dir.join(format!("{THIRD_ID}.zstd")));
    assert_eq!(info["localID"], THIRD_ID);
}

/// An old draft (by its row's creation time) goes with its row.
#[test]
fn sweep_expires_old_drafts_with_their_rows() {
    let (_dir, app) = emulated_app();
    let now = DISPATCHED_AT;
    let max_age = app.reports.cfg().draft_max_age;
    insert_row(
        &app,
        QUEUED_ID,
        "draft",
        now - max_age - 1,
        BugReportValues::default(),
    );
    let draft_dir = write_draft(&app, QUEUED_ID, json!({"localID": QUEUED_ID}));

    let stats = sweep_reports(&app, Some(now)).unwrap();

    assert_eq!(stats.drafts, 1);
    assert_eq!(stats.rows, 1);
    assert!(!draft_dir.exists());
    assert!(all_rows(&app).is_empty());
}

/// `_delete_report` refuses names that are not a plain file name.
#[test]
fn delete_report_refuses_paths() {
    let (_dir, app) = emulated_app();
    for name in ["", ".", "..", "a/b", "a/"] {
        assert!(!delete_report(&app, name).unwrap(), "{name}");
    }
    assert!(!delete_report(&app, QUEUED_ID).unwrap());
}

/// The emulated status and the isoformat timestamps of the reporting log.
#[test]
fn emulated_status_and_reporting_log() {
    assert_eq!(
        emulated_machine_status(5),
        "{\"emulated\": true, \"status\": \"ok\", \"source\": \"meticulous-backend\", \"timestamp\": 5}"
    );
    let dir = tempfile::tempdir().unwrap();
    append_reporting_log(dir.path(), &["one".into(), "two".into()]).unwrap();
    let text = std::fs::read_to_string(dir.path().join(REPORT_LOG_NAME)).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with("+00:00 one"), "{}", lines[0]);
    assert!(lines[1].ends_with(" two"));
    append_reporting_log(dir.path(), &[]).unwrap();
    assert!(Path::new(&dir.path().join(REPORT_LOG_NAME)).exists());
}
