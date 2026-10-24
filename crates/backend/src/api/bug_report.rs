//! `api/bug_report.py`: the report routes. Drafts, archives, the collection
//! and the sweep live in `crate::bug_report`.

use std::sync::Arc;

use history::bug_reports::{self as rows, BugReportRow, BugReportValues, QueuedReport};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::bug_report::progress::{Failure, Origin, Stage, StageUpdate};
use crate::bug_report::{
    self as reports, CollectionCancellation, Fail, FetchFailure, REPORT_STATUS_CANCELLED,
    REPORT_STATUS_QUEUED, RequestError, StageReporter, blocking, py,
};
use crate::http::{Kind, Reply, Request, Route};

pub const CREATE_KEYS: [&str; 2] = ["issueTime", "localID"];
pub const DISPATCH_KEYS: [&str; 6] = [
    "localID",
    "ticket",
    "issueTime",
    "description",
    "name",
    "email",
];
pub const DISPATCH_TEXT_LIMITS: [(&str, usize); 3] =
    [("description", 10_000), ("name", 200), ("email", 254)];
pub const SUBMIT_KEYS: [&str; 4] = ["localID", "eventID", "submissionTime", "ticket"];
pub const ALLOWED_DRAFT_UPDATE_KEYS: [&str; 5] = [
    "description",
    "baseEventID",
    "ticket",
    "multimedia",
    "attachments",
];
pub const PREFLIGHT_MAX_PROBES: usize = 4;

pub fn routes() -> Vec<Route> {
    let m = "api.bug_report";
    vec![
        Route::new("/reports/create", Kind::Base, m, "ReportsCreateHandler").post(create),
        Route::new("/reports/request", Kind::Base, m, "ReportsRequestHandler").post(request),
        Route::new("/reports/dispatch", Kind::Base, m, "ReportsDispatchHandler")
            .get(dispatch_get)
            .post(dispatch_post),
        Route::new(
            "/reports/preflight",
            Kind::Base,
            m,
            "ReportsPreflightHandler",
        )
        .get(preflight),
        Route::new(
            "/reports/draft/([^/]+)",
            Kind::Base,
            m,
            "ReportDraftHandler",
        )
        .get(draft_get)
        .put(draft_put)
        .delete(draft_delete),
        Route::new("/reports/list", Kind::Base, m, "ReportsListHandler").get(list),
        Route::new("/reports/submit", Kind::Base, m, "ReportsSubmitHandler").post(submit),
        Route::new("/reports/progress", Kind::Base, m, "ReportsProgressHandler").get(progress),
        Route::new(
            "/reports/([^/]+)/progress",
            Kind::Base,
            m,
            "ReportProgressUpdateHandler",
        )
        .post(progress_update),
        Route::new(
            "/reports/([^/]+)/cancel",
            Kind::Base,
            m,
            "ReportCancelHandler",
        )
        .post(cancel),
    ]
}

/// `{"code": code}`.
fn code(code: &str) -> Option<Map<String, Value>> {
    let mut data = Map::new();
    data.insert("code".into(), json!(code));
    Some(data)
}

fn message(text: &str) -> Option<Map<String, Value>> {
    let mut data = Map::new();
    data.insert("message".into(), json!(text));
    Some(data)
}

/// `_api_error(handler, status_code, error, context)`: a 5xx always carries
/// a `data.code` (`INTERNAL` unless given).
pub fn api_error(status: u16, error: &str, data: Option<Map<String, Value>>) -> Reply {
    let mut body = Map::new();
    body.insert("error".into(), json!(error));
    body.insert("description".into(), json!(""));
    let mut data = data;
    if status >= 500 {
        data.get_or_insert_with(Map::new)
            .entry("code")
            .or_insert(json!("INTERNAL"));
    }
    if let Some(data) = data {
        body.insert("data".into(), Value::Object(data));
    }
    Reply::base().status(status).json(Value::Object(body))
}

/// `_reply_error(handler, exc)`.
pub fn request_error(error: &RequestError) -> Reply {
    api_error(error.status, &error.message, Some(error.data()))
}

/// What tornado answers for an exception the handler does not catch.
fn uncaught(req: &Request) -> Reply {
    Reply::error(Kind::Base, 500, req)
}

/// `get_query_arguments(name)`: control characters become spaces, then
/// the value is stripped.
fn query_arguments(req: &Request, name: &str) -> Vec<String> {
    req.query_arguments()
        .into_iter()
        .filter(|(k, _)| k == name)
        .map(|(_, v)| {
            let cleaned: String = v
                .chars()
                .map(|c| {
                    if matches!(c, '\u{0}'..='\u{8}' | '\u{e}'..='\u{1f}') {
                        ' '
                    } else {
                        c
                    }
                })
                .collect();
            py::strip(&cleaned).to_string()
        })
        .collect()
}

/// `get_query_argument(name)`: the last value.
fn query_argument(req: &Request, name: &str) -> Option<String> {
    query_arguments(req, name).pop()
}

/// `int(get_query_argument(name))`, with the exception message. A valid
/// literal beyond `i64` is `Err(negative)`: Python carries on with it.
fn int_argument(req: &Request, name: &str) -> Result<Result<i64, bool>, String> {
    let value = query_argument(req, name)
        .ok_or_else(|| format!("HTTP 400: Bad Request (Missing argument {name})"))?;
    if let Some(number) = pyfmt::py_int(&value) {
        return Ok(Ok(number));
    }
    let text = py::strip(&value);
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let literal = !digits.is_empty()
        && digits
            .split('_')
            .all(|group| !group.is_empty() && group.bytes().all(|b| b.is_ascii_digit()));
    if literal {
        return Ok(Err(negative));
    }
    Err(format!(
        "invalid literal for int() with base 10: {}",
        py::repr_str(&value)
    ))
}

fn validated_path_id(req: &Request) -> Result<String, Reply> {
    let local_id = Value::String(req.arg(0).unwrap_or("").to_string());
    reports::validate_local_id(&local_id).map_err(|e| request_error(&e))
}

/// Why `_parse_create_body` refused a body.
#[derive(Debug, Clone, PartialEq)]
pub enum CreateBodyError {
    /// `ValueError` (400 `INVALID_BODY`).
    Value(String),
    /// A malformed `localID`.
    Request(RequestError),
}

/// `_parse_create_body(body)`: `(issueTime, localID)`, at most one set. A
/// `localID` names a dispatched report whose issue time came with the
/// dispatch, so both together are refused.
pub fn parse_create_body(body: &[u8]) -> Result<(Option<i64>, Option<String>), CreateBodyError> {
    if body.is_empty() {
        return Ok((None, None));
    }
    let data = py::json_loads_body(body).map_err(CreateBodyError::Value)?;
    if data.is_empty() || data.keys().any(|k| !CREATE_KEYS.contains(&k.as_str())) {
        return Err(CreateBodyError::Value(
            "Create report body may only contain issueTime or localID".into(),
        ));
    }
    if let Some(local_id) = data.get("localID") {
        if data.contains_key("issueTime") {
            return Err(CreateBodyError::Value(
                "issueTime is taken from the dispatch when localID is given".into(),
            ));
        }
        return reports::validate_local_id(local_id)
            .map(|id| (None, Some(id)))
            .map_err(CreateBodyError::Request);
    }
    let issue_time = data.get("issueTime").unwrap_or(&Value::Null);
    py::as_int(issue_time)
        .map(|t| (Some(t), None))
        .ok_or_else(|| {
            CreateBodyError::Value("issueTime must be an integer epoch timestamp".into())
        })
}

/// `_create_report_issue_time(body)`.
#[cfg(test)]
pub fn create_report_issue_time(body: &[u8]) -> Result<Option<i64>, CreateBodyError> {
    parse_create_body(body).map(|(issue_time, _)| issue_time)
}

/// `_queued_report_for_create(local_id)`: the dispatched row, and whether
/// its draft was already collected (the dial then resumes from there).
pub fn queued_report_for_create(app: &App, local_id: &str) -> Result<(BugReportRow, bool), Fail> {
    let row = reports::get_report_row(app, local_id)?.ok_or(RequestError::unknown_local_id())?;
    match row.status_str() {
        Some(REPORT_STATUS_QUEUED) => Ok((row, false)),
        Some("draft") if reports::draft_path(&app.reports.cfg(), local_id)?.is_dir() => {
            Ok((row, true))
        }
        Some("submitted") => {
            Err(RequestError::duplicate_local_id("This report was already submitted").into())
        }
        Some(REPORT_STATUS_CANCELLED) => Err(RequestError::report_cancelled().into()),
        _ => Err(RequestError::unknown_local_id().into()),
    }
}

/// Why a collection did not produce a draft.
enum CollectError {
    Cancelled,
    Failed(String),
}

impl From<Fail> for CollectError {
    fn from(fail: Fail) -> CollectError {
        CollectError::Failed(fail.message())
    }
}

/// What a create collects, settled before the collection starts.
struct Collection {
    local_id: String,
    queued_row: Option<BugReportRow>,
    requested_issue_time: Option<i64>,
    now: i64,
    cancellation: CollectionCancellation,
}

/// `_collect_report`: the part of `ReportsCreateHandler.post` under the
/// collection lock, from the sweep on.
async fn collect(app: &Arc<App>, req: &Request, job: Collection) -> Option<Reply> {
    let Collection {
        local_id,
        queued_row,
        requested_issue_time,
        now,
        cancellation,
    } = job;
    let queued = queued_row.is_some();
    let a = app.clone();
    match tokio::task::spawn_blocking(move || reports::sweep_reports(&a, None)).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => tracing::warn!("Report sweep failed: {}", error.message()),
        Err(error) => tracing::warn!("Report sweep failed: {error}"),
    }
    let cfg = app.reports.cfg();
    let check_cfg = cfg.clone();
    match blocking(move || reports::check_disk_space(&check_cfg)).await {
        Ok(()) => {}
        Err(Fail::Request(error)) => {
            let failure = Failure {
                code: error.code.to_string(),
                message: error.message.clone(),
            };
            let _state = app.reports.hold_state().await;
            reports::collection_stopped(
                app,
                &local_id,
                queued,
                cancellation.requested(),
                Some(failure),
            );
            app.reports.end_collection(&local_id);
            return Some(request_error(&error));
        }
        // PARITY: a failure to read the disk usage escapes the handler (a
        // 500) and leaves the report at `preparing`.
        Err(_) => return Some(uncaught(req)),
    }
    let issue_time = requested_issue_time.unwrap_or(now);
    let range = requested_issue_time.map(|t| reports::collection_range(t, now));
    let Ok(draft_dir) = reports::draft_path(&cfg, &local_id) else {
        return Some(uncaught(req));
    };
    let (a, id) = (app.clone(), local_id.clone());
    let progress: StageReporter = Arc::new(move |stage, detail| {
        let update = StageUpdate {
            detail,
            ..StageUpdate::default()
        };
        reports::report_progress(&a, &id, stage, update);
    });
    let mut state = None;
    let outcome: Result<Reply, CollectError> = async {
        if cancellation.token.is_cancelled() {
            return Err(CollectError::Cancelled);
        }
        let a = app.clone();
        let mut statistics = blocking(move || reports::statistics(&a)).await?;
        if let Value::Object(map) = &mut statistics {
            map.insert(
                "mileage".into(),
                app.machine.mileage().map_or(Value::Null, |m| json!(m)),
            );
        }
        let capture_active = range.is_none_or(|(_, end)| end == now);
        let fetched = reports::fetch_report_files(
            app,
            &draft_dir,
            range,
            capture_active,
            Some(&cancellation.token),
            Some(progress),
        )
        .await
        .map_err(|failure| match failure {
            FetchFailure::Cancelled => CollectError::Cancelled,
            FetchFailure::Failed(message) => CollectError::Failed(message),
        })?;
        let field = |f: fn(&BugReportRow) -> &Value| {
            queued_row
                .as_ref()
                .map_or(Value::Null, |row| f(row).clone())
        };
        let attachments = pyfmt::object([
            (
                "debugFiles",
                pyfmt::object([("automatic", json!(fetched.automatic_debug_files))]),
            ),
            ("machineInfo", json!(fetched.machine_info)),
            ("machineLogs", json!(fetched.machine_logs)),
            ("machineStatus", json!(fetched.machine_status)),
            ("DBStatistics", statistics.clone()),
        ]);
        let report_info = pyfmt::object([
            ("description", field(|r| &r.description)),
            (
                "dateAndTime",
                queued_row
                    .as_ref()
                    .map_or(json!(now), |row| row.creation_time.clone()),
            ),
            ("issueTime", json!(issue_time)),
            ("attachments", attachments),
            ("multimedia", Value::Null),
            ("machineID", reports::machine_serial(app)),
            ("eventID", Value::Null),
            ("baseEventID", Value::Null),
            ("ticket", field(|r| &r.ticket_number)),
            ("localID", json!(local_id)),
        ]);
        // From here to the answer nothing awaits in the Python: a cancel that
        // lands after the last checkpoint still wins, and one that comes
        // after the draft is written finds no collection to stop.
        state = Some(app.reports.hold_state().await);
        if cancellation.token.is_cancelled() {
            return Err(CollectError::Cancelled);
        }
        let a = app.clone();
        let (dir, info, queued, id) = (
            draft_dir.clone(),
            report_info.clone(),
            queued_row.is_some(),
            local_id.clone(),
        );
        blocking(move || -> Result<(), Fail> {
            reports::write_draft_report_info(&dir, &info)?;
            reports::write_draft_machine_stats(&dir, &statistics)?;
            if queued {
                reports::update_report_db(&a, &id, &rows::collected_db_values(&info)?)?;
            } else {
                reports::insert_report(&a, &info)?;
            }
            Ok(())
        })
        .await?;
        app.reports.end_collection(&local_id);
        Ok(Reply::base().json(json!({"localID": local_id, "machineID": report_info["machineID"]})))
    }
    .await;
    let error = match outcome {
        Ok(reply) => return Some(reply),
        Err(error) => error,
    };
    let _state = match state {
        Some(state) => state,
        None => app.reports.hold_state().await,
    };
    let dir = draft_dir.clone();
    blocking(move || std::fs::remove_dir_all(dir)).await.ok();
    let requested = cancellation.requested();
    let reply = match error {
        CollectError::Cancelled => {
            reports::collection_stopped(app, &local_id, queued, requested, None);
            // Cancelled from either app while the Dial still waits on this
            // request: tell it, so it does not retry. Otherwise the client
            // left: not an error, no log, no response, nothing left behind.
            requested.then(|| request_error(&RequestError::report_cancelled()))
        }
        CollectError::Failed(error) => {
            tracing::error!("Failed to create bug report draft: {error}");
            let failure = Failure {
                code: "INTERNAL".into(),
                message: error.clone(),
            };
            reports::collection_stopped(app, &local_id, queued, requested, Some(failure));
            Some(api_error(
                500,
                "Failed to create report draft",
                message(&error),
            ))
        }
    };
    app.reports.end_collection(&local_id);
    reply
}

/// `ReportsCreateHandler.post`. `None` when the client disconnected
/// mid-collection, which `cancellation` signals: nothing is answered.
pub async fn create_report(
    app: &Arc<App>,
    req: &Request,
    cancellation: CancellationToken,
) -> Option<Reply> {
    let now = app.reports.now();
    let (mut requested_issue_time, queued_local_id) = match parse_create_body(&req.body) {
        Ok(parsed) => parsed,
        Err(CreateBodyError::Value(text)) => {
            return Some(api_error(400, &text, code("INVALID_BODY")));
        }
        Err(CreateBodyError::Request(error)) => return Some(request_error(&error)),
    };
    // From the lookup of a queued row to `preparing` nothing awaits in the
    // Python, so a cancel cannot slip in between.
    let state = app.reports.hold_state().await;
    let mut queued_row = None;
    if let Some(local_id) = &queued_local_id {
        let (a, id) = (app.clone(), local_id.clone());
        match blocking(move || queued_report_for_create(&a, &id)).await {
            Ok((row, true)) => {
                return Some(
                    Reply::base().json(json!({"localID": local_id, "machineID": row.machine_id})),
                );
            }
            // The window is anchored on the dispatch, not on when the dial
            // got to it, so a report picked up after a restart still covers
            // the time the user reported.
            Ok((row, false)) => {
                requested_issue_time = row.issue_time.as_i64();
                queued_row = Some(row);
            }
            Err(Fail::Request(error)) => return Some(request_error(&error)),
            Err(_) => return Some(uncaught(req)),
        }
    }
    let Some(guard) = app.reports.try_begin_collection() else {
        return Some(request_error(&RequestError::collection_in_progress()));
    };
    let local_id = queued_local_id.unwrap_or_else(reports::new_local_id);
    let cancellation = CollectionCancellation::new(cancellation);
    let registration = app
        .reports
        .register_collection(&local_id, cancellation.clone(), guard);
    let (origin, ticket) = match &queued_row {
        Some(row) => (Origin::Mobile, row.ticket_number.clone()),
        None => (Origin::Dial, Value::Null),
    };
    let preparing = StageUpdate {
        origin: Some(origin),
        ticket,
        ..StageUpdate::default()
    };
    reports::report_progress(app, &local_id, Stage::Preparing, preparing);
    drop(state);
    let job = Collection {
        local_id,
        queued_row,
        requested_issue_time,
        now,
        cancellation,
    };
    // The collection runs on its own task so that a disconnect stops it only
    // where the Python checks (stage boundaries and watcher requests), and
    // its cleanup always runs.
    let (a, r) = (app.clone(), req.clone());
    let task = tokio::spawn(async move {
        let _registration = registration;
        collect(&a, &r, job).await
    });
    match task.await {
        Ok(reply) => reply,
        Err(error) => {
            tracing::error!("Failed to create bug report draft: {error}");
            Some(api_error(
                500,
                "Failed to create report draft",
                message(&error.to_string()),
            ))
        }
    }
}

async fn create(app: Arc<App>, req: Request) -> Reply {
    // Dropping this future (the client disconnected) cancels the collection.
    let cancellation = CancellationToken::new();
    let _disconnect = cancellation.clone().drop_guard();
    create_report(&app, &req, cancellation)
        .await
        .unwrap_or_else(Reply::base)
}

/// `ReportDraftHandler.get`: the archive, built for a draft still open.
async fn draft_get(app: Arc<App>, req: Request) -> Reply {
    let local_id = match validated_path_id(&req) {
        Ok(id) => id,
        Err(reply) => return reply,
    };
    let cfg = app.reports.cfg();
    let (Ok(draft_dir), Ok(finalized)) = (
        reports::draft_path(&cfg, &local_id),
        reports::finalized_draft_path(&cfg, &local_id),
    ) else {
        return uncaught(&req);
    };
    let disposition = format!("attachment; filename=\"{local_id}.zstd\"");
    // The cancel check and `compressing` happen at once, as in the Python.
    let state = app.reports.hold_state().await;
    let (a, id, dir, archive) = (
        app.clone(),
        local_id.clone(),
        draft_dir.clone(),
        finalized.clone(),
    );
    let found = blocking(move || -> Result<Option<bool>, Fail> {
        if reports::is_cancelled(&a, &id)? {
            return Err(RequestError::report_cancelled().into());
        }
        Ok(if dir.is_dir() {
            Some(true)
        } else if archive.is_file() {
            Some(false)
        } else {
            None
        })
    })
    .await;
    let open = match found {
        Ok(Some(open)) => open,
        Ok(None) => return api_error(404, "Unknown localID", code("UNKNOWN_LOCAL_ID")),
        Err(Fail::Request(error)) => return request_error(&error),
        Err(_) => return uncaught(&req),
    };
    if open {
        reports::report_progress(&app, &local_id, Stage::Compressing, StageUpdate::default());
    }
    drop(state);
    let read = blocking(move || -> Result<Vec<u8>, String> {
        if open {
            reports::archive::archive_bytes(&draft_dir).map_err(|e| e.message)
        } else {
            std::fs::read(&finalized).map_err(|e| py::os_error(&e, &finalized))
        }
    })
    .await;
    match read {
        Ok(bytes) => {
            let mut reply = Reply::base()
                .with_header("Content-Type", "application/octet-stream")
                .with_header("Content-Disposition", disposition)
                .with_header("Content-Length", bytes.len().to_string());
            reply.write_bytes(&bytes);
            // The Python streams with flush(): the headers go out first, so
            // tornado adds no Etag.
            if !bytes.is_empty() {
                reply.disable_etag();
            }
            if open {
                // The uploader has the archive; next it goes to Sentry. The
                // Python moves on once the last chunk is flushed, this once
                // the archive is handed to the server.
                reports::report_progress(&app, &local_id, Stage::Uploading, StageUpdate::default());
            }
            reply
        }
        Err(error) => {
            tracing::error!("Failed to read bug report draft: {error}");
            // PARITY: the error body keeps the attachment header set before
            // the archive failed.
            api_error(500, "Failed to read report draft", message(&error))
                .with_header("Content-Disposition", disposition)
        }
    }
}

/// Why `_validate_draft_patch` refused a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    /// `PermissionError` (403 `FORBIDDEN_UPDATE`).
    Forbidden(String),
    /// `ValueError` (400 `INVALID_BODY`).
    Invalid(String),
}

/// `_validate_draft_patch(data)`: only the allowed keys, and of the
/// attachments only `debugFiles.user`, a list of debug file names.
pub fn validate_draft_patch(data: &Map<String, Value>) -> Result<(), PatchError> {
    let invalid: Vec<&String> = data
        .keys()
        .filter(|k| !ALLOWED_DRAFT_UPDATE_KEYS.contains(&k.as_str()))
        .collect();
    if !invalid.is_empty() {
        return Err(PatchError::Forbidden(format!(
            "Forbidden update keys: {}",
            py::sorted_join(invalid)
        )));
    }
    let Some(attachments) = data.get("attachments") else {
        return Ok(());
    };
    let only_user =
        || PatchError::Forbidden("Only attachments.debugFiles.user can be updated".into());
    let Some(attachments) = attachments.as_object() else {
        return Err(only_user());
    };
    let Some(debug_files) = attachments.get("debugFiles").and_then(Value::as_object) else {
        return Err(only_user());
    };
    if attachments.keys().any(|k| k != "debugFiles") || debug_files.keys().any(|k| k != "user") {
        return Err(only_user());
    }
    match debug_files.get("user") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Array(names)) => {
            for name in names {
                let valid = name
                    .as_str()
                    .is_some_and(|n| reports::DEBUG_FILE_NAME_RE.is_match(n));
                if !valid {
                    return Err(PatchError::Invalid(format!(
                        "Invalid debug file name: {}",
                        py::repr(name)
                    )));
                }
            }
            Ok(())
        }
        Some(_) => Err(PatchError::Invalid(
            "attachments.debugFiles.user must be a list".into(),
        )),
    }
}

/// `ReportDraftHandler.put`.
async fn draft_put(app: Arc<App>, req: Request) -> Reply {
    let local_id = match validated_path_id(&req) {
        Ok(id) => id,
        Err(reply) => return reply,
    };
    let data = match py::json_loads_body(&req.body) {
        Ok(data) => data,
        Err(text) => return api_error(400, &text, code("INVALID_BODY")),
    };
    match validate_draft_patch(&data) {
        Ok(()) => {}
        Err(PatchError::Forbidden(text)) => return api_error(403, &text, code("FORBIDDEN_UPDATE")),
        Err(PatchError::Invalid(text)) => return api_error(400, &text, code("INVALID_BODY")),
    }
    let (a, id) = (app.clone(), local_id.clone());
    match blocking(move || reports::get_report_row(&a, &id)).await {
        Ok(Some(row)) if row.status_str() == Some(REPORT_STATUS_CANCELLED) => {
            return request_error(&RequestError::report_cancelled());
        }
        Ok(Some(_)) => {}
        Ok(None) => return api_error(404, "Unknown localID", code("UNKNOWN_LOCAL_ID")),
        Err(_) => return uncaught(&req),
    }
    let (a, id) = (app.clone(), local_id.clone());
    match blocking(move || reports::apply_draft_patch(&a, &id, &data)).await {
        Ok(report_info) => {
            // `isinstance(ticket, int)`, which a bool passes.
            let ticket = &report_info["ticket"];
            if matches!(ticket, Value::Bool(_)) || matches!(ticket, Value::Number(n) if !n.is_f64())
            {
                reports::report_ticket(&app, &local_id, ticket.clone());
            }
            Reply::base().json(report_info)
        }
        Err(Fail::NotFound(_)) => api_error(404, "Unknown localID", code("UNKNOWN_LOCAL_ID")),
        Err(Fail::Value(text)) => api_error(400, &text, code("INVALID_BODY")),
        Err(other) => {
            let text = other.message();
            tracing::error!("Failed to update bug report draft: {text}");
            api_error(500, "Failed to update report draft", message(&text))
        }
    }
}

/// `ReportDraftHandler.delete`.
async fn draft_delete(app: Arc<App>, req: Request) -> Reply {
    let local_id = match validated_path_id(&req) {
        Ok(id) => id,
        Err(reply) => return reply,
    };
    if !req.body.is_empty() {
        return api_error(
            400,
            "Delete report draft request must not contain a body",
            code("INVALID_BODY"),
        );
    }
    let (a, id) = (app.clone(), local_id.clone());
    let deleted = match blocking(move || reports::delete_report(&a, &id)).await {
        Ok(deleted) => {
            tracing::info!("Deleted bug report draft with localID: {local_id}");
            deleted
        }
        Err(error) => {
            let text = error.message();
            tracing::error!("Failed to delete bug report draft: {text}");
            return api_error(500, "Failed to delete report draft", message(&text));
        }
    };
    if !deleted {
        return api_error(404, "Unknown localID", code("UNKNOWN_LOCAL_ID"));
    }
    Reply::base().status(204)
}

/// `ReportsListHandler.get`: a page, newest first, with a FIQL filter.
async fn list(app: Arc<App>, req: Request) -> Reply {
    let pagination = (|| {
        let page = int_argument(&req, "page")?;
        let size = int_argument(&req, "size")?;
        let negative = |n: &Result<i64, bool>| matches!(n, Ok(v) if *v < 0) || *n == Err(true);
        if negative(&page) || matches!(size, Ok(0)) || negative(&size) {
            return Err("page must be >= 0 and size must be > 0".to_string());
        }
        Ok((page, size))
    })();
    let (page, size) = match pagination {
        Ok(pagination) => pagination,
        Err(error) => return api_error(400, &format!("Invalid pagination params: {error}"), None),
    };
    let filter_text = query_argument(&req, "filter");
    let filter = match rows::parse_fiql(filter_text.as_deref()) {
        Ok((_, true)) => return api_error(400, "Filter contains no valid fields", None),
        Ok((filter, false)) => filter,
        Err(_) => return uncaught(&req),
    };
    // PARITY: Python's ints do not overflow, so a huge page passes the
    // checks above and SQLite refuses it (OverflowError, a 500).
    let (Ok(page), Ok(size)) = (page, size) else {
        return uncaught(&req);
    };
    if page
        .checked_add(1)
        .and_then(|p| p.checked_mul(size))
        .is_none()
    {
        return uncaught(&req);
    }
    match blocking(move || reports::list_report_page(&app, page, size, filter.as_ref())).await {
        Ok(page) => Reply::base().json(page),
        Err(_) => uncaught(&req),
    }
}

/// `ReportsRequestHandler.post`: mint a localID for a report the mobile app
/// will dispatch. Nothing is stored: the row appears with the dispatch, so
/// an abandoned request leaves nothing to sweep.
async fn request(app: Arc<App>, req: Request) -> Reply {
    if !req.body.is_empty() {
        return api_error(400, "Request report takes no body", code("INVALID_BODY"));
    }
    Reply::base().json(json!({
        "localID": reports::new_local_id(),
        "machineID": reports::machine_serial(&app),
    }))
}

/// `_parse_dispatch_body(body)`.
pub fn parse_dispatch_body(body: &[u8]) -> Result<QueuedReport, RequestError> {
    let data = py::json_loads_body(body).map_err(RequestError::invalid_body)?;
    let unknown: Vec<&String> = data
        .keys()
        .filter(|k| !DISPATCH_KEYS.contains(&k.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(RequestError::invalid_body(format!(
            "Unknown keys: {}",
            py::sorted_join(unknown)
        )));
    }
    let local_id = reports::validate_local_id(data.get("localID").unwrap_or(&Value::Null))?;
    let ticket = data
        .get("ticket")
        .and_then(py::as_int)
        .ok_or_else(|| RequestError::invalid_body("ticket must be an integer"))?;
    let issue_time = match data.get("issueTime") {
        None | Some(Value::Null) => None,
        Some(value) => Some(py::as_int(value).ok_or_else(|| {
            RequestError::invalid_body("issueTime must be an integer epoch timestamp")
        })?),
    };
    let mut texts: Vec<Option<String>> = Vec::new();
    for (key, limit) in DISPATCH_TEXT_LIMITS {
        let value = match data.get(key) {
            None | Some(Value::Null) => "",
            Some(Value::String(s)) => py::strip(s),
            Some(_) => {
                return Err(RequestError::invalid_body(format!(
                    "{key} must be a string or null"
                )));
            }
        };
        if value.chars().count() > limit {
            return Err(RequestError::invalid_body(format!(
                "{key} must be at most {limit} characters"
            )));
        }
        texts.push((!value.is_empty()).then(|| value.to_string()));
    }
    let mut texts = texts.into_iter();
    let (description, name, email) = (
        texts.next().flatten(),
        texts.next().flatten(),
        texts.next().flatten(),
    );
    let email = match email {
        None => None,
        Some(email) => match crate::api::settings::normalize_report_contact_mail(&json!(email)) {
            Ok(Value::String(normalized)) => Some(normalized),
            Ok(_) => None,
            Err(text) => return Err(RequestError::invalid_body(text)),
        },
    };
    Ok(QueuedReport {
        local_id,
        ticket: json!(ticket),
        issue_time,
        description,
        name,
        email,
    })
}

/// `ReportsDispatchHandler.get`: the dispatches still waiting for the dial,
/// which it reads after (re)connecting in case it missed the event.
async fn dispatch_get(app: Arc<App>, req: Request) -> Reply {
    match blocking(move || reports::queued_report_rows(&app)).await {
        Ok(rows) => Reply::base().json(json!({
            "content": rows.iter().map(BugReportRow::dispatch_payload).collect::<Vec<_>>(),
        })),
        Err(_) => uncaught(&req),
    }
}

/// `ReportsDispatchHandler.post`: store the ticket and contact details as a
/// `queued` row, tell the dial (`upload_report`), answer 202.
async fn dispatch_post(app: Arc<App>, req: Request) -> Reply {
    let dispatch = match parse_dispatch_body(&req.body) {
        Ok(dispatch) => dispatch,
        Err(error) => return request_error(&error),
    };
    let a = app.clone();
    let now = app.reports.now();
    let local_id = dispatch.local_id.clone();
    let state = app.reports.hold_state().await;
    let row = match blocking(move || reports::insert_queued_report(&a, &dispatch, now)).await {
        Ok(row) => row,
        Err(Fail::Request(error)) => return request_error(&error),
        Err(error) => {
            let text = error.message();
            tracing::error!("Failed to queue dispatched bug report: {text}");
            return api_error(500, "Failed to queue report", message(&text));
        }
    };
    let payload = row.dispatch_payload();
    // Both apps' bars start at `queued` before the Dial is told.
    let queued = StageUpdate {
        origin: Some(Origin::Mobile),
        ticket: row.ticket_number.clone(),
        ..StageUpdate::default()
    };
    reports::report_progress(&app, &local_id, Stage::Queued, queued);
    drop(state);
    reports::emit_upload_report(&app, &payload);
    Reply::base().status(202).json(payload)
}

/// `_parse_submit_body(body)`.
#[derive(Debug, Clone, PartialEq)]
pub struct SubmitBody {
    pub local_id: String,
    pub event_id: String,
    pub submission_time: i64,
    /// `Some` when the body had `ticket` (possibly `null`).
    pub ticket: Option<Value>,
}

/// `_parse_submit_body(body)`; `submissionTime` defaults to `now`.
pub fn parse_submit_body(body: &[u8], now: i64) -> Result<SubmitBody, RequestError> {
    let data = py::json_loads_body(body).map_err(RequestError::invalid_body)?;
    let unknown: Vec<&String> = data
        .keys()
        .filter(|k| !SUBMIT_KEYS.contains(&k.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(RequestError::invalid_body(format!(
            "Unknown keys: {}",
            py::sorted_join(unknown)
        )));
    }
    let local_id = reports::validate_local_id(data.get("localID").unwrap_or(&Value::Null))?;
    let event_id = match data.get("eventID") {
        Some(Value::String(s)) if !py::strip(s).is_empty() => s.clone(),
        _ => {
            return Err(RequestError::invalid_body(
                "eventID must be a non-empty string",
            ));
        }
    };
    let submission_time = match data.get("submissionTime") {
        None => now,
        Some(value) => py::as_int(value).ok_or_else(|| {
            RequestError::invalid_body("submissionTime must be an integer epoch timestamp")
        })?,
    };
    let ticket = data.get("ticket").cloned();
    if let Some(ticket) = &ticket
        && !ticket.is_null()
        && py::as_int(ticket).is_none()
    {
        return Err(RequestError::invalid_body(
            "ticket must be an integer or null",
        ));
    }
    Ok(SubmitBody {
        local_id,
        event_id,
        submission_time,
        ticket,
    })
}

/// `ReportsSubmitHandler.post`: the dial reports the Sentry event it sent.
async fn submit(app: Arc<App>, req: Request) -> Reply {
    let body = match parse_submit_body(&req.body, app.reports.now()) {
        Ok(body) => body,
        Err(error) => return request_error(&error),
    };
    // From the cancel check to `submitted` nothing awaits in the Python.
    let _state = app.reports.hold_state().await;
    let (a, id) = (app.clone(), body.local_id.clone());
    match blocking(move || reports::is_cancelled(&a, &id)).await {
        Ok(true) => return request_error(&RequestError::report_cancelled()),
        Ok(false) => {}
        Err(_) => return uncaught(&req),
    }
    let local_id = body.local_id.clone();
    reports::report_progress(&app, &local_id, Stage::Finishing, StageUpdate::default());
    let ticket = body.ticket.clone().unwrap_or(Value::Null);
    let a = app.clone();
    let updated = blocking(move || {
        reports::mark_report_submitted(
            &a,
            &body.local_id,
            &body.event_id,
            body.submission_time,
            body.ticket,
        )
    })
    .await;
    match updated {
        Ok(true) => {
            let submitted = StageUpdate {
                ticket,
                ..StageUpdate::default()
            };
            reports::report_progress(&app, &local_id, Stage::Submitted, submitted);
            Reply::base().status(200)
        }
        Ok(false) | Err(Fail::NotFound(_)) => {
            api_error(404, "Unknown localID", code("UNKNOWN_LOCAL_ID"))
        }
        Err(error) => {
            let text = error.message();
            tracing::error!("Failed to submit bug report draft: {text}");
            api_error(500, "Failed to submit report draft", message(&text))
        }
    }
}

/// `_parse_progress_update(body)`: `(stage, failure)` from a
/// ReportProgressUpdate body, `{stage}` or `{failed}`.
pub fn parse_progress_update(body: &[u8]) -> Result<(Stage, Option<Failure>), RequestError> {
    let data = py::json_loads_body(body).map_err(RequestError::invalid_body)?;
    let only = |key: &str| data.len() == 1 && data.contains_key(key);
    if only("stage") {
        let stages = Stage::UPLOADER.map(Stage::as_str).join(", ");
        return data["stage"]
            .as_str()
            .and_then(Stage::uploader)
            .map(|stage| (stage, None))
            .ok_or_else(|| RequestError::invalid_body(format!("stage must be one of {stages}")));
    }
    if only("failed") {
        let failure = data["failed"]
            .as_object()
            .filter(|failed| failed.len() == 2)
            .and_then(|failed| match (failed.get("code"), failed.get("message")) {
                (Some(Value::String(code)), Some(Value::String(message))) => Some(Failure {
                    code: code.chars().take(64).collect(),
                    message: message.chars().take(1000).collect(),
                }),
                _ => None,
            });
        return failure
            .map(|failure| (Stage::Failed, Some(failure)))
            .ok_or_else(|| {
                RequestError::invalid_body("failed must be an object with string code and message")
            });
    }
    Err(RequestError::invalid_body(
        "Body must be {stage} or {failed}",
    ))
}

/// `ReportsProgressHandler.get`: the reports in flight and the ones that
/// ended in the last minute.
async fn progress(app: Arc<App>, req: Request) -> Reply {
    let _state = app.reports.hold_state().await;
    let a = app.clone();
    match blocking(move || reports::progress_entries(&a)).await {
        Ok(content) => Reply::base().json(json!({"content": content})),
        Err(_) => uncaught(&req),
    }
}

/// `ReportProgressUpdateHandler.post`: the uploader's own stages (asking for
/// the ticket, sending to Sentry) and its failure.
async fn progress_update(app: Arc<App>, req: Request) -> Reply {
    let local_id = match validated_path_id(&req) {
        Ok(id) => id,
        Err(reply) => return reply,
    };
    let (stage, failure) = match parse_progress_update(&req.body) {
        Ok(update) => update,
        Err(error) => return request_error(&error),
    };
    let _state = app.reports.hold_state().await;
    let (a, id) = (app.clone(), local_id.clone());
    let known = blocking(move || -> Result<bool, Fail> {
        if reports::is_cancelled(&a, &id)? {
            return Err(RequestError::report_cancelled().into());
        }
        Ok(reports::progress_for_row(&a, &id)?.is_some())
    })
    .await;
    match known {
        Ok(true) => {}
        Ok(false) => return request_error(&RequestError::unknown_local_id()),
        Err(Fail::Request(error)) => return request_error(&error),
        Err(_) => return uncaught(&req),
    }
    let update = StageUpdate {
        failure,
        ..StageUpdate::default()
    };
    match reports::report_progress(&app, &local_id, stage, update) {
        Some(entry) => Reply::base().json(entry.payload()),
        None => uncaught(&req),
    }
}

/// `ReportCancelHandler.post`: cancel a report from either app, until it is
/// being uploaded.
async fn cancel(app: Arc<App>, req: Request) -> Reply {
    let local_id = match validated_path_id(&req) {
        Ok(id) => id,
        Err(reply) => return reply,
    };
    if !req.body.is_empty() {
        return request_error(&RequestError::invalid_body("Cancel takes no body"));
    }
    let _state = app.reports.hold_state().await;
    let (a, id) = (app.clone(), local_id.clone());
    let entry = match blocking(move || reports::progress_for_row(&a, &id)).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return request_error(&RequestError::unknown_local_id()),
        Err(_) => return uncaught(&req),
    };
    if entry.stage == Stage::Cancelled {
        // Cancelling twice is not an error; both apps may press it.
        return Reply::base().json(entry.payload());
    }
    if !entry.cancellable() {
        return request_error(&RequestError::too_late_to_cancel());
    }
    // A create still collecting removes the draft and answers the Dial.
    let collection = app.reports.collection(&local_id);
    let collecting = collection.is_some();
    if let Some(collection) = collection {
        collection.cancel(true);
    }
    let (a, id) = (app.clone(), local_id.clone());
    let stored = blocking(move || -> Result<(), Fail> {
        if !collecting {
            reports::remove_report_representation(&reports::draft_path(&a.reports.cfg(), &id)?)?;
        }
        if reports::get_report_row(&a, &id)?.is_some() {
            let cancelled = BugReportValues {
                status: Some(json!(REPORT_STATUS_CANCELLED)),
                ..BugReportValues::default()
            };
            reports::update_report_db(&a, &id, &cancelled)?;
        }
        Ok(())
    })
    .await;
    if stored.is_err() {
        return uncaught(&req);
    }
    match reports::report_progress(&app, &local_id, Stage::Cancelled, StageUpdate::default()) {
        Some(entry) => Reply::base().json(entry.payload()),
        None => uncaught(&req),
    }
}

/// `ReportsPreflightHandler.get`: whether a report can be collected now,
/// and why not, with the reachability of the given HTTPS endpoints.
async fn preflight(app: Arc<App>, req: Request) -> Reply {
    let mut probes = reports::dedupe(&query_arguments(&req, "probe"));
    probes.truncate(PREFLIGHT_MAX_PROBES);
    let mut blockers: Vec<&str> = Vec::new();
    let serial = reports::machine_serial(&app);
    let machine_id = if crate::config::python_truthy(&serial) {
        serial
    } else {
        Value::Null
    };
    if machine_id.is_null() {
        blockers.push("NO_SERIAL_NUMBER");
    }
    let cfg = app.reports.cfg();
    let required = cfg.min_free_bytes;
    let free = match blocking(move || reports::disk_free_bytes(&cfg)).await {
        Ok(free) => free,
        Err(_) => return uncaught(&req),
    };
    let disk_ok = free >= required;
    if !disk_ok {
        blockers.push("INSUFFICIENT_DISK_SPACE");
    }
    let busy = app.reports.collection_in_progress();
    if busy {
        blockers.push("COLLECTION_IN_PROGRESS");
    }
    let results: Vec<Value> = match app.reports.probe_client() {
        Ok(client) => {
            futures::future::join_all(
                probes
                    .iter()
                    .map(|url| reports::watcher::probe(&client, url)),
            )
            .await
        }
        Err(error) => probes
            .iter()
            .map(|_| {
                json!({"reachable": false, "status": null, "latencyMs": null, "error": error.type_name})
            })
            .collect(),
    };
    let mut network = Map::new();
    for (url, result) in probes.iter().zip(results) {
        network.insert(url.clone(), result);
    }
    if !network.is_empty() && !network.values().all(|r| r["reachable"] == json!(true)) {
        blockers.push("NETWORK_UNREACHABLE");
    }
    Reply::base().json(json!({
        "ok": blockers.is_empty(),
        "blockers": blockers,
        "machineID": machine_id,
        "disk": {"freeBytes": free, "requiredBytes": required, "ok": disk_ok},
        "collectionInProgress": busy,
        "network": Value::Object(network),
    }))
}

#[cfg(test)]
mod tests;
