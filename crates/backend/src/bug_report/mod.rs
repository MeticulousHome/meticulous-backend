//! The report side of `api/bug_report.py`: draft directories under
//! `REPORTS_DIR`, their archives, what a report collects (machine info, the
//! watcher's logs and status, debug shots), the `bug_reports` rows (through
//! the history crate), the sweep of expired artifacts, the `upload_report`
//! event and report progress (`progress`). The HTTP handlers are in
//! `api/bug_report.rs`.
//!
//! The dial submits a report to Sentry itself; the backend only records the
//! Sentry event id it is given (`POST /reports/submit`).

pub mod archive;
pub mod progress;
pub mod py;
#[cfg(test)]
pub mod test_support;
#[cfg(test)]
mod tests;
pub mod watcher;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use history::bug_reports::{self as rows, BugReportRow, BugReportValues, ReportFilter, SweepRow};
use history::{HistoryDb, LocalZone};
use parking_lot::{Mutex, RwLock};
use regex::Regex;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::config::keys;
use crate::env::Paths;
use progress::{
    Failure, ProgressBook, REPORT_PROGRESS_EVENT, ReportProgress, Stage, StageDetail, StageUpdate,
};
use watcher::{FetchError, HttpError};

pub const REPORT_INFO_NAME: &str = "report_info.json";
pub const STATS_INFO_NAME: &str = "statistics.json";
pub const REPORT_LOG_NAME: &str = "logs_while_reporting.txt";
pub const MACHINE_INFO_NAME: &str = "machine_info.json";
pub const MACHINE_LOGS_NAME: &str = "machine_logs.txt";
pub const MACHINE_STATUS_NAME: &str = "machine_status.json";
pub const DEBUG_ARCHIVE_DIR: &str = "debug";
pub const MAX_DEBUG_SHOTS: usize = 10;
/// A report handed over by the mobile app: the row exists with its ticket
/// and contact details, nothing is collected yet.
pub const REPORT_STATUS_QUEUED: &str = rows::REPORT_STATUS_QUEUED;
pub const REPORT_STATUS_CANCELLED: &str = "cancelled";
pub const UPLOAD_REPORT_EVENT: &str = "upload_report";
const TRUNCATION_BANNER: &str = "[meticulous-backend] machine logs truncated:";
const WATCHER_LOGS_TIMEOUT: Duration = Duration::from_secs(600);
const WATCHER_STATUS_TIMEOUT: Duration = Duration::from_secs(120);
const HOUR: i64 = 60 * 60;

pub static LOCAL_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
        .expect("static regex is valid")
});

pub static DEBUG_FILE_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\d{4}-\d{2}-\d{2}/)?\d{2}:\d{2}:\d{2}\.[A-Za-z0-9_-]+\.json\.zst$")
        .expect("static regex is valid")
});

/// The module's `os.getenv` settings.
#[derive(Debug, Clone)]
pub struct ReportsConfig {
    /// `REPORTS_DIR`.
    pub reports_dir: PathBuf,
    /// `DRAFT_REPORTS_DIR` (`REPORTS_DIR/draft`).
    pub draft_dir: PathBuf,
    /// `DEBUG_HISTORY_PATH`.
    pub debug_history: PathBuf,
    /// `WATCHER_LOGS_URL`.
    pub watcher_logs_url: String,
    /// `WATCHER_STATUS_URL`.
    pub watcher_status_url: String,
    /// `REPORTS_MIN_FREE_BYTES`.
    pub min_free_bytes: i64,
    /// `REPORT_MAX_MACHINE_LOGS_BYTES`.
    pub max_machine_logs_bytes: i64,
    /// `REPORTS_DRAFT_MAX_AGE_SECONDS`.
    pub draft_max_age: i64,
    /// `REPORTS_ARCHIVE_MAX_AGE_SECONDS`.
    pub archive_max_age: i64,
}

impl ReportsConfig {
    /// The Python's defaults over `lookup` (the process environment).
    /// Deliberate: a number that `int()` rejects falls back to its default
    /// instead of failing the start like the import would.
    pub fn from_lookup(paths: &Paths, lookup: impl Fn(&str) -> Option<String>) -> ReportsConfig {
        let number = |name: &str, default: i64| {
            lookup(name)
                .and_then(|v| pyfmt::py_int(&v))
                .unwrap_or(default)
        };
        ReportsConfig {
            reports_dir: paths.reports.clone(),
            draft_dir: lookup("DRAFT_REPORTS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| paths.reports.join("draft")),
            debug_history: paths.debug_history.clone(),
            watcher_logs_url: lookup("WATCHER_LOGS_URL")
                .unwrap_or_else(|| "http://localhost/health/logs?filter=*".into()),
            watcher_status_url: lookup("WATCHER_STATUS_URL")
                .unwrap_or_else(|| "http://localhost/health/status".into()),
            min_free_bytes: number("REPORTS_MIN_FREE_BYTES", 200 * 1024 * 1024),
            max_machine_logs_bytes: number("REPORT_MAX_MACHINE_LOGS_BYTES", 20 * 1024 * 1024),
            draft_max_age: number("REPORTS_DRAFT_MAX_AGE_SECONDS", 7 * 24 * HOUR),
            archive_max_age: number("REPORTS_ARCHIVE_MAX_AGE_SECONDS", 30 * 24 * HOUR),
        }
    }
}

/// `ReportRequestError`: a status, a machine-readable code and a message.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    /// Extra keys of `data` after `code`.
    pub extra: Vec<(&'static str, Value)>,
}

impl RequestError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> RequestError {
        RequestError {
            status,
            code,
            message: message.into(),
            extra: Vec::new(),
        }
    }

    pub fn invalid_local_id() -> RequestError {
        RequestError::new(400, "INVALID_LOCAL_ID", "localID must be a UUID")
    }

    pub fn unknown_local_id() -> RequestError {
        RequestError::new(404, "UNKNOWN_LOCAL_ID", "Unknown localID")
    }

    pub fn invalid_body(message: impl Into<String>) -> RequestError {
        RequestError::new(400, "INVALID_BODY", message)
    }

    /// `DuplicateLocalID`.
    pub fn duplicate_local_id(message: &str) -> RequestError {
        RequestError::new(409, "DUPLICATE_LOCAL_ID", message)
    }

    /// `CollectionInProgress`.
    pub fn collection_in_progress() -> RequestError {
        RequestError::new(
            409,
            "COLLECTION_IN_PROGRESS",
            "A report is already being collected",
        )
    }

    /// `ReportCancelled`.
    pub fn report_cancelled() -> RequestError {
        RequestError::new(409, "REPORT_CANCELLED", "This report was cancelled")
    }

    /// `TooLateToCancel`.
    pub fn too_late_to_cancel() -> RequestError {
        RequestError::new(
            409,
            "TOO_LATE_TO_CANCEL",
            "The report is already being uploaded and cannot be cancelled",
        )
    }

    /// `InsufficientDiskSpace`.
    pub fn insufficient_disk_space(free: i64, required: i64) -> RequestError {
        RequestError {
            extra: vec![
                ("freeBytes", json!(free)),
                ("requiredBytes", json!(required)),
            ],
            ..RequestError::new(
                507,
                "INSUFFICIENT_DISK_SPACE",
                "Not enough free disk space to collect a report",
            )
        }
    }

    /// `ReportRequestError.data`.
    pub fn data(&self) -> Map<String, Value> {
        let mut data = Map::new();
        data.insert("code".into(), json!(self.code));
        for (key, value) in &self.extra {
            data.insert((*key).into(), value.clone());
        }
        data
    }
}

/// An exception of the Python code, by the class its handlers tell apart.
#[derive(Debug, Clone, PartialEq)]
pub enum Fail {
    /// `FileNotFoundError` (handlers answer 404 "Unknown localID").
    NotFound(String),
    /// `ValueError`, `json.JSONDecodeError` included.
    Value(String),
    /// Any other exception; the message is its `str()`.
    Other(String),
    /// `ReportRequestError`.
    Request(RequestError),
}

impl Fail {
    pub fn message(&self) -> String {
        match self {
            Fail::NotFound(m) | Fail::Value(m) | Fail::Other(m) => m.clone(),
            Fail::Request(e) => e.message.clone(),
        }
    }

    /// PARITY: any `FileNotFoundError` reaching a handler is "Unknown
    /// localID" (404), whichever file was missing: a draft without its
    /// `report_info.json`, or the debug history root during a PUT.
    fn io(error: &std::io::Error, path: &Path) -> Fail {
        let message = py::os_error(error, path);
        if error.kind() == std::io::ErrorKind::NotFound {
            Fail::NotFound(message)
        } else {
            Fail::Other(message)
        }
    }
}

impl From<RequestError> for Fail {
    fn from(error: RequestError) -> Fail {
        Fail::Request(error)
    }
}

impl From<history::Error> for Fail {
    fn from(error: history::Error) -> Fail {
        match error {
            history::Error::DuplicateLocalId => Fail::Request(RequestError::duplicate_local_id(
                "A report with this localID already exists",
            )),
            other => Fail::Other(other.to_string()),
        }
    }
}

/// Why `_fetch_report_files` stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum FetchFailure {
    /// The client disconnected (`asyncio.CancelledError`).
    Cancelled,
    /// An exception outside the per-stage handlers.
    Failed(String),
}

/// `FetchResult`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FetchResult {
    pub files: HashMap<String, PathBuf>,
    pub errors: Vec<String>,
    pub automatic_debug_files: Vec<String>,
    pub machine_info: bool,
    pub machine_logs: bool,
    pub machine_status: bool,
}

/// What `sweep_reports` removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepStats {
    pub tmp: i64,
    pub drafts: i64,
    pub rows: i64,
    pub archives: i64,
    pub finalized: i64,
}

impl SweepStats {
    #[cfg(test)]
    pub fn to_json(self) -> Value {
        pyfmt::object([
            ("tmp", json!(self.tmp)),
            ("drafts", json!(self.drafts)),
            ("rows", json!(self.rows)),
            ("archives", json!(self.archives)),
            ("finalized", json!(self.finalized)),
        ])
    }
}

/// Held while a report is being collected (`_collection_lock`).
pub struct CollectionGuard {
    flag: Arc<AtomicBool>,
}

impl Drop for CollectionGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

/// `CollectionCancellation`: the token a disconnect or a cancel trips, and
/// whether it was POST /reports/<localID>/cancel (the client is then still
/// connected and gets `REPORT_CANCELLED` instead of silence).
#[derive(Clone)]
pub struct CollectionCancellation {
    pub token: CancellationToken,
    requested: Arc<AtomicBool>,
}

impl CollectionCancellation {
    pub fn new(token: CancellationToken) -> CollectionCancellation {
        CollectionCancellation {
            token,
            requested: Arc::default(),
        }
    }

    /// `cancel(requested)`.
    pub fn cancel(&self, requested: bool) {
        if requested {
            self.requested.store(true, Ordering::Release);
        }
        self.token.cancel();
    }

    pub fn requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

type Collections = Arc<Mutex<HashMap<String, CollectionCancellation>>>;

/// A create's entry in `_collections` and its hold on `_collection_lock`:
/// dropping it is the handler's `finally`.
pub struct CollectionRegistration {
    collections: Collections,
    local_id: String,
    _guard: CollectionGuard,
}

impl Drop for CollectionRegistration {
    fn drop(&mut self) {
        self.collections.lock().remove(&self.local_id);
    }
}

/// The `progress` callback `_fetch_report_files` reports its stages to.
pub type StageReporter = Arc<dyn Fn(Stage, Option<StageDetail>) + Send + Sync>;

#[cfg(test)]
pub type FetchOverride = Arc<
    dyn Fn(
            PathBuf,
            Option<(i64, i64)>,
            bool,
            Option<CancellationToken>,
            Option<StageReporter>,
        ) -> futures::future::BoxFuture<'static, Result<FetchResult, FetchFailure>>
        + Send
        + Sync,
>;

/// What the Python suite monkeypatches.
#[cfg(test)]
#[derive(Default)]
pub struct TestHooks {
    /// Replaces `_fetch_report_files`.
    pub fetch: Mutex<Option<FetchOverride>>,
    /// Runs after each automatic debug file is copied.
    pub after_debug_copy: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Every `upload_report` and `report_progress` emit, by event.
    pub emitted: Mutex<Vec<(&'static str, Value)>>,
}

/// The report service (the module's globals in the Python).
pub struct BugReports {
    config: RwLock<ReportsConfig>,
    collecting: Arc<AtomicBool>,
    watcher_client: OnceLock<reqwest::Client>,
    probe_client: OnceLock<reqwest::Client>,
    clock: Mutex<Option<i64>>,
    progress: Mutex<ProgressBook>,
    collections: Collections,
    /// The Python's handlers share one event loop, so what one does between
    /// two awaits happens at once: a cancel cannot land between a report's
    /// state being checked and what follows from it. The stretches that read
    /// a report's state and change it hold this lock to keep that.
    state: tokio::sync::Mutex<()>,
    #[cfg(test)]
    pub hooks: TestHooks,
}

impl BugReports {
    pub fn new(config: ReportsConfig) -> BugReports {
        BugReports {
            config: RwLock::new(config),
            collecting: Arc::new(AtomicBool::new(false)),
            watcher_client: OnceLock::new(),
            probe_client: OnceLock::new(),
            clock: Mutex::new(None),
            progress: Mutex::new(ProgressBook::default()),
            collections: Collections::default(),
            state: tokio::sync::Mutex::new(()),
            #[cfg(test)]
            hooks: TestHooks::default(),
        }
    }

    /// Hold the report state still (see `state`).
    pub async fn hold_state(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.state.lock().await
    }

    /// `_collections[local_id] = cancellation`, until the registration drops.
    pub fn register_collection(
        &self,
        local_id: &str,
        cancellation: CollectionCancellation,
        guard: CollectionGuard,
    ) -> CollectionRegistration {
        self.collections
            .lock()
            .insert(local_id.to_string(), cancellation);
        CollectionRegistration {
            collections: self.collections.clone(),
            local_id: local_id.to_string(),
            _guard: guard,
        }
    }

    /// `_collections.get(local_id)`.
    pub fn collection(&self, local_id: &str) -> Option<CollectionCancellation> {
        self.collections.lock().get(local_id).cloned()
    }

    /// `_collections.pop(local_id, None)`.
    pub fn end_collection(&self, local_id: &str) {
        self.collections.lock().remove(local_id);
    }

    #[cfg(test)]
    pub fn collection_count(&self) -> usize {
        self.collections.lock().len()
    }

    pub fn cfg(&self) -> ReportsConfig {
        self.config.read().clone()
    }

    #[cfg(test)]
    pub fn configure(&self, f: impl FnOnce(&mut ReportsConfig)) {
        f(&mut self.config.write());
    }

    /// `_now_seconds()`.
    pub fn now(&self) -> i64 {
        self.clock
            .lock()
            .unwrap_or_else(|| chrono::Utc::now().timestamp())
    }

    #[cfg(test)]
    pub fn set_now(&self, now: Option<i64>) {
        *self.clock.lock() = now;
    }

    /// `_collection_lock.locked()`.
    pub fn collection_in_progress(&self) -> bool {
        self.collecting.load(Ordering::Acquire)
    }

    /// Take the collection lock unless a collection holds it.
    pub fn try_begin_collection(&self) -> Option<CollectionGuard> {
        self.collecting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| CollectionGuard {
                flag: self.collecting.clone(),
            })
    }

    fn client(
        slot: &OnceLock<reqwest::Client>,
        follow_redirects: bool,
    ) -> Result<reqwest::Client, HttpError> {
        if let Some(client) = slot.get() {
            return Ok(client.clone());
        }
        let client = watcher::build_client(follow_redirects)?;
        Ok(slot.get_or_init(|| client).clone())
    }

    pub fn watcher_client(&self) -> Result<reqwest::Client, HttpError> {
        Self::client(&self.watcher_client, true)
    }

    pub fn probe_client(&self) -> Result<reqwest::Client, HttpError> {
        Self::client(&self.probe_client, false)
    }
}

/// `_ensure_database_initialized()`: the history database `backend.main()`
/// opened and migrated.
fn db(app: &App) -> Result<&HistoryDb, Fail> {
    app.history.db().map_err(Fail::Other)
}

/// Run blocking work off the event loop; a panic there is a panic here.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(f).await {
        Ok(value) => value,
        Err(error) => std::panic::resume_unwind(error.into_panic()),
    }
}

/// `_new_local_id()`: a UUID v7.
pub fn new_local_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// `_validate_local_id`.
pub fn validate_local_id(value: &Value) -> Result<String, RequestError> {
    match value.as_str() {
        Some(id) if LOCAL_ID_RE.is_match(id) => Ok(id.to_string()),
        _ => Err(RequestError::invalid_local_id()),
    }
}

/// `Path.resolve()` (non-strict): symlinks of the part that exists are
/// resolved, the rest is appended with `..` applied.
pub fn resolve(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    let mut components = path.components();
    let last = components.next_back();
    let parent = components.as_path();
    let resolved_parent = || {
        if parent.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_default()
        } else {
            resolve(parent)
        }
    };
    match last {
        Some(Component::Normal(name)) => resolved_parent().join(name),
        Some(Component::ParentDir) => {
            let p = resolved_parent();
            p.parent().map(Path::to_path_buf).unwrap_or(p)
        }
        Some(Component::CurDir) => resolved_parent(),
        _ => path.to_path_buf(),
    }
}

/// `_reports_child(base, name)`: `base/name`, refused unless it resolves to a
/// direct child of `base`.
fn reports_child(base: &Path, name: &str) -> Result<PathBuf, RequestError> {
    let target = base.join(name);
    let parent = resolve(&target).parent().map(Path::to_path_buf);
    if parent.as_deref() != Some(resolve(base).as_path()) {
        return Err(RequestError::invalid_local_id());
    }
    Ok(target)
}

/// `_draft_path(local_id)`.
pub fn draft_path(cfg: &ReportsConfig, local_id: &str) -> Result<PathBuf, RequestError> {
    reports_child(&cfg.draft_dir, local_id)
}

/// `_finalized_draft_path(local_id)`.
pub fn finalized_draft_path(cfg: &ReportsConfig, local_id: &str) -> Result<PathBuf, RequestError> {
    reports_child(&cfg.draft_dir, &format!("{local_id}.zstd"))
}

/// `_safe_draft_file_path`: a member path that stays inside the draft.
fn safe_draft_file_path(draft_dir: &Path, archive_name: &str) -> Result<PathBuf, String> {
    let target = resolve(&draft_dir.join(archive_name));
    if target.starts_with(resolve(draft_dir)) {
        Ok(target)
    } else {
        Err(format!("Unsafe draft member: {archive_name}"))
    }
}

/// `_safe_archive_name(path)`: `<day>/<file>`.
pub fn safe_archive_name(path: &Path) -> String {
    let name = |p: Option<&Path>| {
        p.and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    format!("{}/{}", name(path.parent()), name(Some(path)))
}

/// `_debug_archive_name(file_name)`.
pub fn debug_archive_name(file_name: &str) -> String {
    format!("{DEBUG_ARCHIVE_DIR}/{file_name}")
}

/// `_dedupe`.
pub fn dedupe<T: PartialEq + Clone>(values: &[T]) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for value in values {
        if !out.contains(value) {
            out.push(value.clone());
        }
    }
    out
}

/// `_read_draft_report_info`.
pub fn read_draft_report_info(draft_dir: &Path) -> Result<Value, Fail> {
    let path = draft_dir.join(REPORT_INFO_NAME);
    let bytes = std::fs::read(&path).map_err(|e| Fail::io(&e, &path))?;
    let text = std::str::from_utf8(&bytes).map_err(|e| Fail::Value(py::utf8_error(&bytes, &e)))?;
    py::json_loads(text).map_err(Fail::Value)
}

fn write_json(path: &Path, value: &Value) -> Result<(), Fail> {
    std::fs::write(path, pyfmt::dumps(value, &pyfmt::DumpOpts::unicode()))
        .map_err(|e| Fail::io(&e, path))
}

/// `_write_draft_report_info`: `json.dumps(report_info, ensure_ascii=False)`.
pub fn write_draft_report_info(draft_dir: &Path, report_info: &Value) -> Result<(), Fail> {
    write_json(&draft_dir.join(REPORT_INFO_NAME), report_info)
}

/// `_write_draft_machine_stats`.
pub fn write_draft_machine_stats(draft_dir: &Path, statistics: &Value) -> Result<(), Fail> {
    write_json(&draft_dir.join(STATS_INFO_NAME), statistics)
}

/// `_draft_files(draft_dir)`: every file but the report info, by archive name.
#[cfg(test)]
pub fn draft_files(draft_dir: &Path) -> HashMap<String, PathBuf> {
    archive::draft_files_sorted(draft_dir)
        .into_iter()
        .filter(|(_, path)| path.file_name() != Some(REPORT_INFO_NAME.as_ref()))
        .map(|(name, path)| (String::from_utf8_lossy(&name).into_owned(), path))
        .collect()
}

/// `_append_reporting_log_to_dir`: one timestamped line per error.
pub fn append_reporting_log(draft_dir: &Path, errors: &[String]) -> Result<(), Fail> {
    use std::io::Write;
    if errors.is_empty() {
        return Ok(());
    }
    let path = draft_dir.join(REPORT_LOG_NAME);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .map_err(|e| Fail::io(&e, &path))?;
    for error in errors {
        writeln!(file, "{} {error}", py::isoformat_utc(chrono::Utc::now()))
            .map_err(|e| Fail::io(&e, &path))?;
    }
    Ok(())
}

/// `shutil.copy2(source, target)`: the content, the permission bits and the
/// access and modification times.
fn copy2(source: &Path, target: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(source).map_err(|e| py::os_error(&e, source))?;
    let mut input = std::fs::File::open(source).map_err(|e| py::os_error(&e, source))?;
    let mut output = std::fs::File::create(target).map_err(|e| py::os_error(&e, target))?;
    std::io::copy(&mut input, &mut output).map_err(|e| py::os_error(&e, target))?;
    output
        .set_permissions(meta.permissions())
        .map_err(|e| py::os_error(&e, target))?;
    let mut times = std::fs::FileTimes::new();
    if let Ok(accessed) = meta.accessed() {
        times = times.set_accessed(accessed);
    }
    if let Ok(modified) = meta.modified() {
        times = times.set_modified(modified);
    }
    output
        .set_times(times)
        .map_err(|e| py::os_error(&e, target))
}

/// `_copy_draft_file(draft_dir, archive_name, source_path)`.
pub fn copy_draft_file(
    draft_dir: &Path,
    archive_name: &str,
    source: &Path,
) -> Result<PathBuf, String> {
    let target = safe_draft_file_path(draft_dir, archive_name)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| py::os_error(&e, parent))?;
    }
    copy2(source, &target)?;
    Ok(target)
}

fn sorted_children(dir: &Path) -> Result<Vec<PathBuf>, Fail> {
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| Fail::io(&e, dir))?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    children.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    Ok(children)
}

/// `_find_debug_file(file_name)`: a debug file the user picked, by
/// `<day>/<file>` or by file name in the newest day that has it.
pub fn find_debug_file(cfg: &ReportsConfig, file_name: &str) -> Result<Option<PathBuf>, Fail> {
    if !DEBUG_FILE_NAME_RE.is_match(file_name) {
        return Ok(None);
    }
    let root = resolve(&cfg.debug_history);
    let candidate = root.join(file_name);
    if candidate.is_file() && resolve(&candidate).starts_with(&root) {
        return Ok(Some(candidate));
    }
    if file_name.contains('/') {
        return Ok(None);
    }
    for day_dir in sorted_children(&root)?.into_iter().filter(|p| p.is_dir()) {
        let candidate = day_dir.join(file_name);
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// `_debug_file_timestamp(path)`: the local time in `<day>/<HH:MM:SS>.*`.
pub fn debug_file_timestamp(path: &Path) -> Option<i64> {
    let day = path.parent()?.file_name()?.to_str()?;
    let name = path.file_name()?.to_str()?;
    let clock = name.split('.').next().unwrap_or(name);
    let wall =
        chrono::NaiveDateTime::parse_from_str(&format!("{day} {clock}"), "%Y-%m-%d %H:%M:%S")
            .ok()?;
    Some(LocalZone::System.local_to_utc(wall.and_utc().timestamp()))
}

/// `_select_debug_files(limit, start_time, end_time)`: the newest debug
/// files, or for a range the ones in it (newest first) topped up with older
/// ones.
pub fn select_debug_files(
    cfg: &ReportsConfig,
    limit: usize,
    range: Option<(i64, i64)>,
) -> Result<(Vec<PathBuf>, Vec<String>), Fail> {
    let root = &cfg.debug_history;
    if !root.exists() {
        return Ok((
            Vec::new(),
            vec![format!(
                "Debug history directory does not exist: {}",
                root.display()
            )],
        ));
    }
    let mut selected: Vec<PathBuf> = Vec::new();
    match range {
        None => {
            for directory in sorted_children(root)?.into_iter().filter(|p| p.is_dir()) {
                for path in sorted_children(&directory)? {
                    if path.is_file() {
                        selected.push(path);
                        if selected.len() >= limit {
                            return Ok((selected, Vec::new()));
                        }
                    }
                }
            }
        }
        Some((start, end)) => {
            let mut timestamped: Vec<(i64, PathBuf)> = archive::draft_files_sorted(root)
                .into_iter()
                .filter_map(|(_, path)| debug_file_timestamp(&path).map(|t| (t, path)))
                .collect();
            timestamped.sort_by(|a, b| (b.0, b.1.as_os_str()).cmp(&(a.0, a.1.as_os_str())));
            selected.extend(
                timestamped
                    .iter()
                    .filter(|(t, _)| start <= *t && *t <= end)
                    .map(|(_, p)| p.clone()),
            );
            if selected.len() < limit {
                selected.extend(
                    timestamped
                        .iter()
                        .filter(|(t, _)| *t < start)
                        .map(|(_, p)| p.clone()),
                );
            }
            selected.truncate(limit);
        }
    }
    let mut errors = Vec::new();
    if selected.len() < limit {
        errors.push(format!(
            "Only found {} debug files while reporting; requested {limit}.",
            selected.len()
        ));
    }
    Ok((selected, errors))
}

/// `_emulated_machine_logs`.
pub fn emulated_machine_logs(range: Option<(i64, i64)>) -> String {
    let reference = match range {
        Some((start, end)) => format!("start_time={start}, end_time={end}"),
        None => "latest".into(),
    };
    format!(
        "{} INFO meticulous-backend Emulated machine logs generated for bug report ({reference}).\n",
        py::isoformat_utc(chrono::Utc::now())
    )
}

/// `_emulated_machine_status`.
pub fn emulated_machine_status(now: i64) -> String {
    pyfmt::dumps(
        &json!({"emulated": true, "status": "ok", "source": "meticulous-backend", "timestamp": now}),
        &pyfmt::DumpOpts::unicode(),
    )
}

/// The watcher request `_fetch_machine_logs` makes: the logs URL with the
/// range as whole hours back from now, its timeout and the size cap.
pub fn machine_logs_request(
    cfg: &ReportsConfig,
    range: Option<(i64, i64)>,
    now: i64,
) -> (String, Duration, Option<i64>) {
    let mut url = cfg.watcher_logs_url.clone();
    if let Some((start, end)) = range {
        let start_hours = (now - start + 3599).div_euclid(3600).max(0);
        let end_hours = (now - end).div_euclid(3600).max(0);
        let separator = if url.contains('?') { '&' } else { '?' };
        url = format!("{url}{separator}since={start_hours}&until={end_hours}");
    }
    (url, WATCHER_LOGS_TIMEOUT, Some(cfg.max_machine_logs_bytes))
}

/// The watcher request `_fetch_machine_status` makes.
pub fn machine_status_request(cfg: &ReportsConfig) -> (String, Duration, Option<i64>) {
    (cfg.watcher_status_url.clone(), WATCHER_STATUS_TIMEOUT, None)
}

async fn fetch_watcher(
    app: &App,
    request: (String, Duration, Option<i64>),
    cancellation: Option<&CancellationToken>,
) -> Result<String, FetchError> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Err(FetchError::Cancelled);
    }
    let client = app.reports.watcher_client().map_err(FetchError::Failed)?;
    let (url, timeout, max_bytes) = request;
    watcher::fetch_text(&client, &url, timeout, cancellation, max_bytes).await
}

/// `_fetch_machine_logs`.
pub async fn fetch_machine_logs(
    app: &App,
    range: Option<(i64, i64)>,
    cancellation: Option<&CancellationToken>,
) -> Result<String, FetchError> {
    if app.machine.emulated {
        return Ok(emulated_machine_logs(range));
    }
    let request = machine_logs_request(&app.reports.cfg(), range, app.reports.now());
    fetch_watcher(app, request, cancellation).await
}

/// `_fetch_machine_status`.
pub async fn fetch_machine_status(
    app: &App,
    cancellation: Option<&CancellationToken>,
) -> Result<String, FetchError> {
    if app.machine.emulated {
        return Ok(emulated_machine_status(app.reports.now()));
    }
    fetch_watcher(
        app,
        machine_status_request(&app.reports.cfg()),
        cancellation,
    )
    .await
}

fn raise_if_disconnected(cancellation: Option<&CancellationToken>) -> Result<(), FetchFailure> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        Err(FetchFailure::Cancelled)
    } else {
        Ok(())
    }
}

fn draft_tag(draft_dir: &Path) -> String {
    draft_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn stage_failed(result: &mut FetchResult, tag: &str, message: String) {
    tracing::warn!("[{tag}] {message}");
    result.errors.push(message);
}

/// `_record_machine_info`.
async fn record_machine_info(app: &Arc<App>, result: &mut FetchResult, draft_dir: &Path) {
    let path = draft_dir.join(MACHINE_INFO_NAME);
    let app2 = app.clone();
    let target = path.clone();
    let written = blocking(move || -> Result<(), String> {
        let info = crate::machine_info::machine_info(&app2);
        std::fs::write(&target, pyfmt::dumps(&info, &pyfmt::DumpOpts::unicode()))
            .map_err(|e| py::os_error(&e, &target))
    })
    .await;
    match written {
        Ok(()) => {
            result.files.insert(MACHINE_INFO_NAME.into(), path);
            result.machine_info = true;
        }
        Err(error) => stage_failed(
            result,
            &draft_tag(draft_dir),
            format!("Failed to fetch machine info: {error}"),
        ),
    }
}

/// The community upload diagnostics snapshot.
async fn record_upload_diagnostics(app: &Arc<App>, result: &mut FetchResult, draft_dir: &Path) {
    let path = draft_dir.join(crate::api::community_upload_diagnostics::NAME);
    let config_dir = app.env.paths.config_dir.clone();
    let now = app.reports.now();
    let target = path.clone();
    let written = blocking(move || {
        let snapshot = crate::api::community_upload_diagnostics::collect(&config_dir, now);
        std::fs::write(&target, pyfmt::dumps_default(&snapshot))
    })
    .await;
    match written {
        Ok(()) => {
            result
                .files
                .insert(crate::api::community_upload_diagnostics::NAME.into(), path);
        }
        Err(_) => result
            .errors
            .push("Unable to write community upload diagnostics".into()),
    }
}

/// `_record_machine_logs` and `_record_machine_status`.
async fn record_watcher_text(
    result: &mut FetchResult,
    draft_dir: &Path,
    name: &str,
    what: &str,
    fetched: Result<String, FetchError>,
    max_bytes: i64,
) -> Result<bool, FetchFailure> {
    let tag = draft_tag(draft_dir);
    let text = match fetched {
        Ok(text) => text,
        Err(FetchError::Cancelled) => return Err(FetchFailure::Cancelled),
        Err(FetchError::Failed(error)) => {
            stage_failed(result, &tag, format!("Failed to fetch {what}: {error}"));
            return Ok(false);
        }
    };
    let path = draft_dir.join(name);
    let target = path.clone();
    let truncated = text.starts_with(TRUNCATION_BANNER);
    match blocking(move || std::fs::write(&target, text).map_err(|e| py::os_error(&e, &target)))
        .await
    {
        Ok(()) => {
            if truncated && name == MACHINE_LOGS_NAME {
                result.errors.push(format!(
                    "Machine logs truncated to the newest {max_bytes} bytes"
                ));
            }
            result.files.insert(name.into(), path);
            Ok(true)
        }
        Err(error) => {
            stage_failed(result, &tag, format!("Failed to fetch {what}: {error}"));
            Ok(false)
        }
    }
}

/// `_record_active_debug_shot`: the running shot's debug file, if any;
/// returns how much the debug-file limit shrinks.
async fn record_active_debug_shot(
    app: &Arc<App>,
    result: &mut FetchResult,
    draft_dir: &Path,
) -> usize {
    let app2 = app.clone();
    let debug_dir = draft_dir.join(DEBUG_ARCHIVE_DIR);
    let captured = tokio::task::spawn_blocking(move || {
        app2.debug_shots.write_current_incomplete(&app2, &debug_dir)
    })
    .await;
    match captured {
        Err(error) => {
            stage_failed(
                result,
                &draft_tag(draft_dir),
                format!("Failed to capture active debug shot: {error}"),
            );
            0
        }
        Ok(None) => 0,
        Ok(Some(name)) => {
            let archive_name = debug_archive_name(&name);
            result
                .files
                .insert(archive_name.clone(), draft_dir.join(&archive_name));
            result.automatic_debug_files.push(name);
            1
        }
    }
}

/// `_copy_selected_debug_files` of one file: a disconnect stops before the
/// copy.
async fn copy_selected_debug_file(
    app: &App,
    result: &mut FetchResult,
    draft_dir: &Path,
    path: PathBuf,
    cancellation: Option<&CancellationToken>,
) -> Result<(), FetchFailure> {
    #[cfg(not(test))]
    let _ = app;
    raise_if_disconnected(cancellation)?;
    let archive_name = debug_archive_name(&safe_archive_name(&path));
    let (dir, name, source) = (draft_dir.to_path_buf(), archive_name.clone(), path.clone());
    let copied = blocking(move || copy_draft_file(&dir, &name, &source)).await;
    #[cfg(test)]
    {
        let hook = app.reports.hooks.after_debug_copy.lock().clone();
        if let Some(hook) = hook {
            hook();
        }
    }
    match copied {
        Ok(target) => {
            result.files.insert(archive_name, target);
            result.automatic_debug_files.push(safe_archive_name(&path));
        }
        Err(error) => {
            // No traceback here: this runs once per file, and the line has to
            // name which file.
            let message = format!("Failed to copy debug file {}: {error}", path.display());
            tracing::warn!("[{}] {message}", draft_tag(draft_dir));
            result.errors.push(message);
        }
    }
    Ok(())
}

/// `_fetch_report_files(draft_dir, start_time, end_time,
/// capture_active_debug_shot, cancellation, progress)`: everything a report
/// bundles, each failure recorded in the bundle and the journal; a
/// disconnect is noticed at each stage boundary and during a watcher
/// request. Each stage is reported to `progress` as it starts, and each debug
/// shot copied.
pub async fn fetch_report_files(
    app: &Arc<App>,
    draft_dir: &Path,
    range: Option<(i64, i64)>,
    capture_active_debug_shot: bool,
    cancellation: Option<&CancellationToken>,
    progress: Option<StageReporter>,
) -> Result<FetchResult, FetchFailure> {
    #[cfg(test)]
    let fetch_override = app.reports.hooks.fetch.lock().clone();
    #[cfg(test)]
    if let Some(fetch) = fetch_override {
        return fetch(
            draft_dir.to_path_buf(),
            range,
            capture_active_debug_shot,
            cancellation.cloned(),
            progress,
        )
        .await;
    }
    let stage = |name: Stage, detail: Option<StageDetail>| {
        if let Some(progress) = &progress {
            progress(name, detail);
        }
    };
    let mut result = FetchResult::default();
    let tag = draft_tag(draft_dir);
    let dir = draft_dir.to_path_buf();
    blocking(move || std::fs::create_dir_all(&dir).map_err(|e| py::os_error(&e, &dir)))
        .await
        .map_err(FetchFailure::Failed)?;

    stage(Stage::FetchingMachineInfo, None);
    record_machine_info(app, &mut result, draft_dir).await;
    record_upload_diagnostics(app, &mut result, draft_dir).await;

    raise_if_disconnected(cancellation)?;
    stage(Stage::FetchingLogs, None);
    let max_bytes = app.reports.cfg().max_machine_logs_bytes;
    let logs = fetch_machine_logs(app, range, cancellation).await;
    result.machine_logs = record_watcher_text(
        &mut result,
        draft_dir,
        MACHINE_LOGS_NAME,
        "machine logs",
        logs,
        max_bytes,
    )
    .await?;

    raise_if_disconnected(cancellation)?;
    stage(Stage::FetchingMachineStatus, None);
    let status = fetch_machine_status(app, cancellation).await;
    result.machine_status = record_watcher_text(
        &mut result,
        draft_dir,
        MACHINE_STATUS_NAME,
        "machine status",
        status,
        max_bytes,
    )
    .await?;

    raise_if_disconnected(cancellation)?;
    stage(Stage::CollectingDebugShots, None);
    let mut debug_limit = MAX_DEBUG_SHOTS;
    let mut captured = 0;
    if capture_active_debug_shot {
        captured = record_active_debug_shot(app, &mut result, draft_dir).await;
        debug_limit -= captured;
    }

    raise_if_disconnected(cancellation)?;
    let cfg = app.reports.cfg();
    let (debug_files, debug_errors) =
        blocking(move || select_debug_files(&cfg, debug_limit, range))
            .await
            .map_err(|e| FetchFailure::Failed(e.message()))?;
    for message in &debug_errors {
        // info, not warning: a machine that has not brewed ten shots yet
        // reports a short count on every report.
        tracing::info!("[{tag}] {message}");
    }
    result.errors.extend(debug_errors);
    let total = captured + debug_files.len();
    let copied = |current| Some(StageDetail { current, total });
    stage(Stage::CollectingDebugShots, copied(captured));
    for (index, path) in debug_files.into_iter().enumerate() {
        copy_selected_debug_file(app, &mut result, draft_dir, path, cancellation).await?;
        stage(Stage::CollectingDebugShots, copied(captured + index + 1));
    }

    let dir = draft_dir.to_path_buf();
    let errors = result.errors.clone();
    let log_exists = blocking(move || {
        append_reporting_log(&dir, &errors).map(|()| dir.join(REPORT_LOG_NAME).exists())
    })
    .await
    .map_err(|e| FetchFailure::Failed(e.message()))?;
    if log_exists {
        result
            .files
            .insert(REPORT_LOG_NAME.into(), draft_dir.join(REPORT_LOG_NAME));
    }
    Ok(result)
}

/// `_collection_range(issue_time, now)`: twelve hours around an issue at
/// least twelve hours old, otherwise the last day.
pub fn collection_range(issue_time: i64, now: i64) -> (i64, i64) {
    if now >= issue_time + 12 * HOUR {
        (issue_time - 12 * HOUR, issue_time + 12 * HOUR)
    } else {
        (now - 24 * HOUR, now)
    }
}

/// `MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]`.
pub fn machine_serial(app: &App) -> Value {
    app.config.get(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER)
}

/// `ShotDataBase.statistics()`.
pub fn statistics(app: &App) -> Result<Value, Fail> {
    Ok(db(app)?.statistics()?)
}

/// `_get_report_row(local_id)`.
pub fn get_report_row(app: &App, local_id: &str) -> Result<Option<BugReportRow>, Fail> {
    Ok(db(app)?.get_report_row(local_id)?)
}

/// `_insert_report(report_info)`.
pub fn insert_report(app: &App, report_info: &Value) -> Result<(), Fail> {
    Ok(db(app)?.insert_report(report_info)?)
}

/// `_insert_queued_report(dispatch, now)`.
pub fn insert_queued_report(
    app: &App,
    dispatch: &rows::QueuedReport,
    now: i64,
) -> Result<BugReportRow, Fail> {
    let machine_id = machine_serial(app);
    Ok(db(app)?.insert_queued_report(dispatch, now, &machine_id)?)
}

/// `_queued_report_rows()`.
pub fn queued_report_rows(app: &App) -> Result<Vec<BugReportRow>, Fail> {
    Ok(db(app)?.queued_report_rows()?)
}

/// `_update_report_db(local_id, values)`.
pub fn update_report_db(app: &App, local_id: &str, values: &BugReportValues) -> Result<bool, Fail> {
    Ok(db(app)?.update_report(local_id, values)?)
}

/// `_list_report_page(page, size, condition)`.
pub fn list_report_page(
    app: &App,
    page: i64,
    size: i64,
    filter: Option<&ReportFilter>,
) -> Result<Value, Fail> {
    Ok(db(app)?.list_report_page(page, size, filter)?)
}

fn sweep_rows(app: &App) -> Result<Vec<SweepRow>, Fail> {
    Ok(db(app)?.sweep_rows()?)
}

/// `_remove_report_representation(path)`.
pub fn remove_report_representation(path: &Path) -> Result<(), Fail> {
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(|e| Fail::io(&e, path))
    } else if path.exists() || path.is_symlink() {
        std::fs::remove_file(path).map_err(|e| Fail::io(&e, path))
    } else {
        Ok(())
    }
}

/// `_delete_report(local_id)`: the draft and its archive, then the row.
pub fn delete_report(app: &App, local_id: &str) -> Result<bool, Fail> {
    let last = Path::new(local_id)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    if matches!(local_id, "" | "." | "..") || last.as_deref() != Some(local_id) {
        return Ok(false);
    }
    if get_report_row(app, local_id)?.is_none() {
        return Ok(false);
    }
    let cfg = app.reports.cfg();
    remove_report_representation(&draft_path(&cfg, local_id)?)?;
    remove_report_representation(&finalized_draft_path(&cfg, local_id)?)?;
    Ok(db(app)?.delete_report_row(local_id)?)
}

/// `str(name)` of a debug file name from a patch.
fn name_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => py::repr(other),
    }
}

fn setdefault<'a>(map: &'a mut Map<String, Value>, key: &str, default: Value) -> &'a mut Value {
    map.entry(key.to_string()).or_insert(default)
}

fn no_setdefault(value: &Value) -> Fail {
    let kind = match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    };
    Fail::Other(format!("'{kind}' object has no attribute 'setdefault'"))
}

/// `_apply_draft_patch(local_id, patch)`: a validated patch applied to the
/// draft's report info and row; user-picked debug files are copied in.
pub fn apply_draft_patch(
    app: &App,
    local_id: &str,
    patch: &Map<String, Value>,
) -> Result<Value, Fail> {
    let cfg = app.reports.cfg();
    let draft_dir = draft_path(&cfg, local_id)?;
    if !draft_dir.is_dir() {
        return Err(Fail::NotFound(local_id.to_string()));
    }
    let mut report_info = read_draft_report_info(&draft_dir)?;
    let Value::Object(info) = &mut report_info else {
        return Err(no_setdefault(&report_info));
    };
    let attachments = setdefault(info, "attachments", Value::Object(Map::new()));
    let Value::Object(attachments_map) = attachments else {
        return Err(no_setdefault(attachments));
    };
    let debug_files = setdefault(attachments_map, "debugFiles", Value::Object(Map::new()));
    let Value::Object(debug_files) = debug_files else {
        return Err(no_setdefault(debug_files));
    };
    setdefault(debug_files, "automatic", json!([]));
    let user_files = setdefault(debug_files, "user", json!([])).clone();

    let mut values = BugReportValues::default();
    for (key, column) in [
        ("description", &mut values.description),
        ("baseEventID", &mut values.base_event_id),
        ("ticket", &mut values.ticket_number),
        ("multimedia", &mut values.multimedia),
    ] {
        if let Some(value) = patch.get(key) {
            info.insert(key.into(), value.clone());
            *column = Some(value.clone());
        }
    }

    let mut log_errors = Vec::new();
    if patch.contains_key("attachments") {
        let incoming = patch
            .get("attachments")
            .and_then(|a| a.get("debugFiles"))
            .and_then(|d| d.as_object())
            .map(|d| d.get("user").cloned().unwrap_or(json!([])))
            .unwrap_or(json!([]));
        // PARITY: `"user": null` passes validation, then iterating None
        // raises (the handler answers 500).
        let Value::Array(incoming) = incoming else {
            return Err(Fail::Other("'NoneType' object is not iterable".into()));
        };
        let Value::Array(mut user_files) = user_files else {
            return Err(Fail::Other(format!(
                "argument of type '{}' is not iterable",
                match user_files {
                    Value::Null => "NoneType",
                    _ => "object",
                }
            )));
        };
        let tag = local_id;
        for name in incoming.iter().map(name_of) {
            if user_files.contains(&Value::String(name.clone())) {
                continue;
            }
            let Some(source) = find_debug_file(&cfg, &name)? else {
                let message = format!("User selected debug file not found: {name}");
                tracing::warn!("[{tag}] {message}");
                log_errors.push(message);
                continue;
            };
            if let Err(error) = copy_draft_file(&draft_dir, &debug_archive_name(&name), &source) {
                let message = format!("Failed to copy user selected debug file {name}: {error}");
                tracing::warn!("[{tag}] {message}");
                log_errors.push(message);
                continue;
            }
            user_files.push(Value::String(name));
        }
        if let Some(Value::Object(debug_files)) = info
            .get_mut("attachments")
            .and_then(|a| a.get_mut("debugFiles"))
        {
            debug_files.insert("user".into(), Value::Array(dedupe(&user_files)));
        }
    }

    if !log_errors.is_empty() {
        append_reporting_log(&draft_dir, &log_errors)?;
    }
    let attachments = info.get("attachments").cloned().unwrap_or(Value::Null);
    let db_values = rows::draft_db_values(&attachments);
    values.log_files = db_values.log_files;
    values.machine_logs = db_values.machine_logs;
    values.machine_info = db_values.machine_info;
    values.machine_status = db_values.machine_status;
    // PARITY: the report info is written before the row is updated, so a
    // value SQLite cannot bind (a dict description) answers 500 with the
    // draft's report info already changed.
    write_draft_report_info(&draft_dir, &report_info)?;
    update_report_db(app, local_id, &values)?;
    Ok(report_info)
}

/// `_mark_report_submitted(local_id, event_id, submission_time,
/// ticket_provided, ticket)`: record the Sentry event, archive the draft.
/// `ticket` is `Some` when the body had the key.
pub fn mark_report_submitted(
    app: &App,
    local_id: &str,
    event_id: &str,
    submission_time: i64,
    ticket: Option<Value>,
) -> Result<bool, Fail> {
    let Some(row) = get_report_row(app, local_id)? else {
        return Ok(false);
    };
    let cfg = app.reports.cfg();
    let draft_dir = draft_path(&cfg, local_id)?;
    if !draft_dir.is_dir() {
        // A retry after the first submit archived and removed the draft:
        // already recorded, so a successful no-op.
        if row.status_str() == Some("submitted") {
            return Ok(true);
        }
        return Err(Fail::NotFound(local_id.to_string()));
    }
    let mut report_info = read_draft_report_info(&draft_dir)?;
    let Value::Object(info) = &mut report_info else {
        return Err(Fail::Other(
            "'list' object does not support item assignment".into(),
        ));
    };
    info.insert("eventID".into(), json!(event_id));
    if let Some(ticket) = &ticket {
        info.insert("ticket".into(), ticket.clone());
    }
    write_draft_report_info(&draft_dir, &report_info)?;

    let archived = finalized_draft_path(&cfg, local_id)
        .map_err(|e| archive::ArchiveError {
            type_name: "ReportRequestError",
            message: e.message,
        })
        .and_then(|path| archive::write_tar_zstd_from_draft(&path, &draft_dir));
    let archive_ok = match archived {
        Ok(()) => true,
        Err(error) => {
            let message = format!("Failed to finalize draft archive: {}", error.type_name);
            tracing::warn!("[{local_id}] {message}: {error}");
            append_reporting_log(&draft_dir, &[message])?;
            false
        }
    };
    let values = BugReportValues {
        event_id: Some(json!(event_id)),
        submission_time: Some(json!(submission_time)),
        status: Some(json!("submitted")),
        ticket_number: ticket,
        ..BugReportValues::default()
    };
    let updated = update_report_db(app, local_id, &values)?;
    if updated && archive_ok {
        let _ = std::fs::remove_dir_all(&draft_dir);
    }
    Ok(updated)
}

/// `_finalize_draft_archive(local_id)`: archive a draft and remove it.
#[cfg(test)]
pub fn finalize_draft_archive(cfg: &ReportsConfig, local_id: &str) -> Result<(), Fail> {
    let draft_dir = draft_path(cfg, local_id)?;
    if !draft_dir.is_dir() {
        return Err(Fail::NotFound(local_id.to_string()));
    }
    archive::write_tar_zstd_from_draft(&finalized_draft_path(cfg, local_id)?, &draft_dir)
        .map_err(|e| Fail::Other(e.message))?;
    std::fs::remove_dir_all(&draft_dir).map_err(|e| Fail::io(&e, &draft_dir))
}

/// `_disk_free_bytes()`: what `shutil.disk_usage(REPORTS_DIR).free` says.
pub fn disk_free_bytes(cfg: &ReportsConfig) -> Result<i64, Fail> {
    std::fs::create_dir_all(&cfg.reports_dir).map_err(|e| Fail::io(&e, &cfg.reports_dir))?;
    let stat = nix::sys::statvfs::statvfs(&cfg.reports_dir)
        .map_err(|e| Fail::Other(py::os_error(&std::io::Error::from(e), &cfg.reports_dir)))?;
    let free = u128::from(stat.blocks_available()) * u128::from(stat.fragment_size());
    Ok(i64::try_from(free).unwrap_or(i64::MAX))
}

/// `_check_disk_space()`.
pub fn check_disk_space(cfg: &ReportsConfig) -> Result<(), Fail> {
    let free = disk_free_bytes(cfg)?;
    if free < cfg.min_free_bytes {
        return Err(RequestError::insufficient_disk_space(free, cfg.min_free_bytes).into());
    }
    Ok(())
}

fn sweep_draft_dir(
    app: &App,
    cfg: &ReportsConfig,
    path: &Path,
    local_id: &str,
    row: Option<&SweepRow>,
    now: i64,
    stats: &mut SweepStats,
) -> Result<(), Fail> {
    let Some(row) = row else {
        let meta = std::fs::metadata(path).map_err(|e| Fail::io(&e, path))?;
        if now - py::mtime_int(&meta) > HOUR {
            let _ = std::fs::remove_dir_all(path);
            stats.drafts += 1;
        }
        return Ok(());
    };
    if row.status.as_str() == Some("submitted") {
        let finalized = finalized_draft_path(cfg, local_id)
            .map_err(|e| e.message)
            .and_then(|out| archive::write_tar_zstd_from_draft(&out, path).map_err(|e| e.message));
        match finalized {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(path);
                stats.finalized += 1;
            }
            Err(error) => {
                tracing::warn!("[{local_id}] retry of archive finalization failed: {error}")
            }
        }
    } else if row.status.as_str() == Some(REPORT_STATUS_CANCELLED) {
        // A cancelled report never uploads: what was collected is of no use.
        let _ = std::fs::remove_dir_all(path);
        stats.drafts += 1;
    } else if now - py::int_of(&row.creation_time).unwrap_or(0) > cfg.draft_max_age
        && delete_report(app, local_id)?
    {
        stats.drafts += 1;
        stats.rows += 1;
    }
    Ok(())
}

fn sweep_entry(
    app: &App,
    cfg: &ReportsConfig,
    path: &Path,
    rows: &HashMap<String, SweepRow>,
    now: i64,
    stats: &mut SweepStats,
) -> Result<(), Fail> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.ends_with(".tmp") || name.ends_with(".cache") {
        let meta = std::fs::metadata(path).map_err(|e| Fail::io(&e, path))?;
        if now - py::mtime_int(&meta) > HOUR {
            std::fs::remove_file(path).map_err(|e| Fail::io(&e, path))?;
            stats.tmp += 1;
        }
        return Ok(());
    }
    let local_id = name.strip_suffix(".zstd").unwrap_or(&name);
    let row = rows.get(local_id);
    if path.is_dir() {
        sweep_draft_dir(app, cfg, path, local_id, row, now, stats)?;
    } else if path.is_file() && name.ends_with(".zstd") {
        let submitted = row
            .and_then(|r| py::int_of(&r.submission_time))
            .filter(|t| *t != 0);
        let reference = match submitted {
            Some(time) => time,
            None => {
                let meta = std::fs::metadata(path).map_err(|e| Fail::io(&e, path))?;
                py::mtime_int(&meta)
            }
        };
        if now - reference > cfg.archive_max_age {
            std::fs::remove_file(path).map_err(|e| Fail::io(&e, path))?;
            stats.archives += 1;
        }
    }
    Ok(())
}

fn sweep_orphan_rows(
    app: &App,
    cfg: &ReportsConfig,
    rows: &[SweepRow],
    now: i64,
    stats: &mut SweepStats,
) -> Result<(), Fail> {
    for row in rows {
        let Some(local_id) = row.local_id.as_str() else {
            continue;
        };
        let status = row.status.as_str();
        if matches!(status, Some(REPORT_STATUS_QUEUED | REPORT_STATUS_CANCELLED)) {
            // Nothing on disk yet by design; give up on a dispatch the dial
            // never collected within the draft retention. A cancelled row is
            // kept as long, so both apps list it.
            if now - py::int_of(&row.creation_time).unwrap_or(0) > cfg.draft_max_age
                && delete_report(app, local_id)?
            {
                stats.rows += 1;
            }
            continue;
        }
        if status == Some("draft")
            && !draft_path(cfg, local_id)?.exists()
            && !finalized_draft_path(cfg, local_id)?.exists()
            && delete_report(app, local_id)?
        {
            stats.rows += 1;
        }
    }
    Ok(())
}

/// `sweep_reports(now)`: expired temporary files, drafts, archives and
/// rows; archives of submitted drafts whose finalization failed are retried.
pub fn sweep_reports(app: &App, now: Option<i64>) -> Result<SweepStats, Fail> {
    let now = now.unwrap_or_else(|| app.reports.now());
    let cfg = app.reports.cfg();
    let mut stats = SweepStats::default();
    if !cfg.draft_dir.is_dir() {
        return Ok(stats);
    }
    let all_rows = sweep_rows(app)?;
    let by_id: HashMap<String, SweepRow> = all_rows
        .iter()
        .filter_map(|row| {
            row.local_id
                .as_str()
                .map(|id| (id.to_string(), row.clone()))
        })
        .collect();
    let entries = std::fs::read_dir(&cfg.draft_dir).map_err(|e| Fail::io(&e, &cfg.draft_dir))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if let Err(error) = sweep_entry(app, &cfg, &path, &by_id, now, &mut stats) {
            tracing::warn!(
                "sweep_reports failed for {}: {}",
                entry.file_name().to_string_lossy(),
                error.message()
            );
        }
    }
    sweep_orphan_rows(app, &cfg, &all_rows, now, &mut stats)?;
    Ok(stats)
}

/// `_emit_upload_report(payload)`: announce a dispatched report to the dial.
pub fn emit_upload_report(app: &App, payload: &Value) {
    #[cfg(test)]
    {
        crate::socketio_types::assert_conforms(UPLOAD_REPORT_EVENT, payload);
        app.reports
            .hooks
            .emitted
            .lock()
            .push((UPLOAD_REPORT_EVENT, payload.clone()));
    }
    let local_id = payload.get("localID").and_then(Value::as_str).unwrap_or("");
    if !app.sio.attached() {
        tracing::warn!("[{local_id}] no socket server, {UPLOAD_REPORT_EVENT} not sent");
        return;
    }
    app.sio.emit(UPLOAD_REPORT_EVENT, payload);
}

/// `_emit_report_progress(payload)`: to both apps; without a socket server
/// it goes nowhere, silently.
fn emit_report_progress(app: &App, payload: &Value) {
    #[cfg(test)]
    app.reports
        .hooks
        .emitted
        .lock()
        .push((REPORT_PROGRESS_EVENT, payload.clone()));
    app.sio.emit(REPORT_PROGRESS_EVENT, payload);
}

fn advance_and_emit(
    app: &App,
    book: &mut ProgressBook,
    local_id: &str,
    stage: Stage,
    update: StageUpdate,
) -> Option<ReportProgress> {
    let (entry, changed) = book.advance(local_id, stage, update, app.reports.now())?;
    if changed {
        // Still under the lock, so the events leave in the order the
        // stages changed.
        emit_report_progress(app, &entry.payload());
    }
    Some(entry)
}

/// `_report_progress(local_id, stage, ...)`: move a report to `stage` and
/// broadcast it when something changed; `None` for a report without an
/// entry when `update` brings no origin.
pub fn report_progress(
    app: &App,
    local_id: &str,
    stage: Stage,
    update: StageUpdate,
) -> Option<ReportProgress> {
    advance_and_emit(
        app,
        &mut app.reports.progress.lock(),
        local_id,
        stage,
        update,
    )
}

/// A ticket a draft PUT stored, onto the report's entry if it has one; the
/// stage and its detail stay.
pub fn report_ticket(app: &App, local_id: &str, ticket: Value) {
    let mut book = app.reports.progress.lock();
    let Some(entry) = book.get(local_id) else {
        return;
    };
    let (stage, detail) = (entry.stage, entry.detail);
    let update = StageUpdate {
        ticket,
        detail,
        ..StageUpdate::default()
    };
    advance_and_emit(app, &mut book, local_id, stage, update);
}

/// `_progress_entries()`.
pub fn progress_entries(app: &App) -> Result<Vec<Value>, Fail> {
    let queued = queued_report_rows(app)?;
    Ok(app
        .reports
        .progress
        .lock()
        .listed(&queued, app.reports.now()))
}

/// `_progress_for_row(local_id)`: the entry of a known report, rebuilt from
/// its row after a restart.
pub fn progress_for_row(app: &App, local_id: &str) -> Result<Option<ReportProgress>, Fail> {
    if let Some(entry) = app.reports.progress.lock().get(local_id) {
        return Ok(Some(entry.clone()));
    }
    let Some(row) = get_report_row(app, local_id)? else {
        return Ok(None);
    };
    let now = app.reports.now();
    Ok(Some(
        app.reports.progress.lock().track_row(local_id, &row, now),
    ))
}

/// `_is_cancelled(local_id)`.
pub fn is_cancelled(app: &App, local_id: &str) -> Result<bool, Fail> {
    Ok(get_report_row(app, local_id)?
        .is_some_and(|row| row.status_str() == Some(REPORT_STATUS_CANCELLED)))
}

/// `_collection_stopped`: the progress of a collection that ended without a
/// draft.
pub fn collection_stopped(
    app: &App,
    local_id: &str,
    queued: bool,
    requested: bool,
    failure: Option<Failure>,
) {
    let (stage, update) = if requested {
        (Stage::Cancelled, StageUpdate::default())
    } else if queued {
        // A dispatched report stays queued: the Dial retries it, and reports
        // `failed` itself when it gives up.
        let rewind = StageUpdate {
            rewind: true,
            ..StageUpdate::default()
        };
        (Stage::Queued, rewind)
    } else if let Some(failure) = failure {
        let failed = StageUpdate {
            failure: Some(failure),
            ..StageUpdate::default()
        };
        (Stage::Failed, failed)
    } else {
        (Stage::Cancelled, StageUpdate::default())
    };
    report_progress(app, local_id, stage, update);
}

/// What `backend.main()` does for reports before serving: sweep once, then
/// daily. A failure is logged; the sweep also runs before each collection.
pub fn start(app: &Arc<App>) {
    if let Err(error) = sweep_reports(app, None) {
        tracing::warn!("Initial report sweep failed: {}", error.message());
    }
    let app = app.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
        interval.tick().await;
        loop {
            interval.tick().await;
            let app2 = app.clone();
            match tokio::task::spawn_blocking(move || sweep_reports(&app2, None)).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => tracing::warn!("Daily report sweep failed: {}", error.message()),
                Err(error) => tracing::warn!("Daily report sweep failed: {error}"),
            }
        }
    });
}
