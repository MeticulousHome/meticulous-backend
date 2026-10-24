//! Test doubles for the HTTP services reports talk to (the watcher, the
//! preflight targets, Supabase Storage) and a capture of log records.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};

/// One request as the server read it.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    /// The request target (path and query) as sent.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// What the server does with a request.
pub enum ServerReply {
    Respond(u16, Vec<u8>),
    /// Never answer; wait for the client to close the connection.
    Hang,
}

impl From<(i32, String)> for ServerReply {
    fn from((status, body): (i32, String)) -> ServerReply {
        ServerReply::Respond(status as u16, body.into_bytes())
    }
}

impl From<(i32, Vec<u8>)> for ServerReply {
    fn from((status, body): (i32, Vec<u8>)) -> ServerReply {
        ServerReply::Respond(status as u16, body)
    }
}

type Handler = Arc<dyn Fn(&RecordedRequest) -> ServerReply + Send + Sync>;

/// A plain HTTP/1.1 server on `127.0.0.1` with its own threads, so it works
/// from synchronous and asynchronous tests alike.
pub struct TestServer {
    port: u16,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    closed_while_hanging: Arc<AtomicUsize>,
}

fn read_request(stream: &TcpStream) -> Option<RecordedRequest> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    let length = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(RecordedRequest {
        method,
        target,
        headers,
        body,
    })
}

impl TestServer {
    pub fn start<R: Into<ServerReply>>(
        handler: impl Fn(&RecordedRequest) -> R + Send + Sync + 'static,
    ) -> TestServer {
        let handler: Handler = Arc::new(move |request| handler(request).into());
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let port = listener.local_addr().expect("local address").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(AtomicUsize::new(0));
        let (requests2, closed2) = (requests.clone(), closed.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (handler, requests, closed) =
                    (handler.clone(), requests2.clone(), closed2.clone());
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let Some(request) = read_request(&stream) else {
                        return;
                    };
                    requests.lock().unwrap().push(request.clone());
                    match handler(&request) {
                        ServerReply::Respond(status, body) => {
                            let reason = http::StatusCode::from_u16(status)
                                .ok()
                                .and_then(|s| s.canonical_reason())
                                .unwrap_or("Unknown");
                            let head = format!(
                                "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = stream.write_all(head.as_bytes());
                            if request.method != "HEAD" {
                                let _ = stream.write_all(&body);
                            }
                            let _ = stream.flush();
                        }
                        ServerReply::Hang => {
                            let mut buffer = [0u8; 1024];
                            loop {
                                match stream.read(&mut buffer) {
                                    Ok(0) | Err(_) => break,
                                    Ok(_) => {}
                                }
                            }
                            closed.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                });
            }
        });
        TestServer {
            port,
            requests,
            closed_while_hanging: closed,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// How many hanging requests the client gave up on (closed the socket).
    pub fn closed_while_hanging(&self) -> usize {
        self.closed_while_hanging.load(Ordering::SeqCst)
    }

    /// A local port nothing listens on.
    pub fn closed_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("local address").port()
    }
}

/// Log records seen on this thread while the capture lives.
pub struct LogCapture {
    records: Arc<Mutex<Vec<(tracing::Level, String, String)>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

impl LogCapture {
    /// The messages at `level` from the module `meticulous_backend::<module>`.
    pub fn messages(&self, level: tracing::Level, module: &str) -> Vec<String> {
        let target = format!("meticulous_backend::{module}");
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|(l, t, _)| *l == level && *t == target)
            .map(|(_, _, m)| m.clone())
            .collect()
    }
}

struct CaptureLayer {
    records: Arc<Mutex<Vec<(tracing::Level, String, String)>>>,
}

#[derive(Default)]
struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0.push_str(&format!("{value:?}"));
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        self.records.lock().unwrap().push((
            *event.metadata().level(),
            event.metadata().target().to_string(),
            visitor.0,
        ));
    }
}

/// Capture what this thread logs (`#[tokio::test]` runs on one thread).
pub fn capture_logs() -> LogCapture {
    let records = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(CaptureLayer {
        records: records.clone(),
    });
    LogCapture {
        records,
        _guard: tracing::subscriber::set_default(subscriber),
    }
}

/// The Python suite's `report_module` fixture: an app on temporary
/// directories with the debug history and draft directories created.
pub fn report_app(backend: crate::env::BackendMode) -> (tempfile::TempDir, Arc<crate::app::App>) {
    let (dir, app) = crate::testing::app_with(backend, |_| {});
    let cfg = app.reports.cfg();
    std::fs::create_dir_all(&cfg.debug_history).unwrap();
    std::fs::create_dir_all(&cfg.draft_dir).unwrap();
    (dir, app)
}

/// An emulated app (watcher never called).
pub fn emulated_app() -> (tempfile::TempDir, Arc<crate::app::App>) {
    report_app(crate::env::BackendMode::Emulator)
}

/// A real-machine app whose watcher URLs point at `logs` and `status`.
pub fn machine_app(logs: &str, status: &str) -> (tempfile::TempDir, Arc<crate::app::App>) {
    let (dir, app) = report_app(crate::env::BackendMode::Fika);
    let (logs, status) = (logs.to_string(), status.to_string());
    app.reports.configure(|cfg| {
        cfg.watcher_logs_url = logs;
        cfg.watcher_status_url = status;
    });
    (dir, app)
}

/// A watcher answering `logs` on `/health/logs` and `status` on
/// `/health/status`.
pub fn watcher(logs: &str, status: &str) -> TestServer {
    let (logs, status) = (logs.to_string(), status.to_string());
    TestServer::start(move |request| {
        if request.target.starts_with("/health/logs") {
            (200, logs.clone())
        } else if request.target.starts_with("/health/status") {
            (200, status.clone())
        } else {
            (404, String::new())
        }
    })
}

/// `_debug_file(root, day, name)`: a debug file whose content is its name.
pub fn debug_file(root: &std::path::Path, day: &str, name: &str) -> std::path::PathBuf {
    let path = root.join(day).join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("{day}/{name}")).unwrap();
    path
}

/// A `bug_reports` row with these columns (the rest NULL).
pub fn insert_row(
    app: &crate::app::App,
    local_id: &str,
    status: &str,
    creation_time: i64,
    mut values: history::bug_reports::BugReportValues,
) {
    let info = serde_json::json!({
        "localID": local_id,
        "dateAndTime": creation_time,
        "issueTime": creation_time,
    });
    super::insert_report(app, &info).unwrap();
    values.status = Some(serde_json::json!(status));
    super::update_report_db(app, local_id, &values).unwrap();
}

/// `_report_row(local_id)`.
pub fn report_row(
    app: &crate::app::App,
    local_id: &str,
) -> Option<history::bug_reports::BugReportRow> {
    super::get_report_row(app, local_id).unwrap()
}

/// `_all_report_rows()`: by creation time, then localID.
pub fn all_rows(app: &crate::app::App) -> Vec<history::bug_reports::BugReportRow> {
    let ids: Vec<String> = app
        .reports
        .database(app)
        .with_connection(|conn| {
            let mut statement = conn.prepare(
                "SELECT \"localID\" FROM bug_reports ORDER BY \"creationTime\", \"localID\"",
            )?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .unwrap();
    ids.iter().map(|id| report_row(app, id).unwrap()).collect()
}

/// Wait (up to five seconds) until `condition` holds.
pub async fn eventually(mut condition: impl FnMut() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("condition not reached");
}

/// What a replaced `_fetch_report_files` was called with.
#[derive(Clone)]
pub struct FetchCall {
    pub range: Option<(i64, i64)>,
    pub capture_active_debug_shot: bool,
    pub cancellation: Option<tokio_util::sync::CancellationToken>,
}

/// `_Collector`: replaces `_fetch_report_files`, writes a machine status
/// and reports `automatic` as the automatic debug files.
pub fn install_collector(
    app: &crate::app::App,
    automatic: &[&str],
) -> Arc<parking_lot::Mutex<Vec<FetchCall>>> {
    let calls = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let automatic: Vec<String> = automatic.iter().map(|s| s.to_string()).collect();
    *app.reports.hooks.fetch.lock() =
        Some(Arc::new(move |draft_dir, range, capture, cancellation| {
            recorded.lock().push(FetchCall {
                range,
                capture_active_debug_shot: capture,
                cancellation,
            });
            let automatic = automatic.clone();
            Box::pin(async move {
                std::fs::create_dir_all(&draft_dir).unwrap();
                let status = draft_dir.join(super::MACHINE_STATUS_NAME);
                std::fs::write(&status, "{\"ok\": true}").unwrap();
                Ok(super::FetchResult {
                    files: std::collections::HashMap::from([(
                        super::MACHINE_STATUS_NAME.to_string(),
                        status,
                    )]),
                    automatic_debug_files: automatic,
                    machine_status: true,
                    ..Default::default()
                })
            })
        }));
    calls
}

/// A replaced `_fetch_report_files` that leaves a partial file and then
/// sees the client disconnect.
pub fn install_cancelling_collector(app: &crate::app::App) {
    *app.reports.hooks.fetch.lock() = Some(Arc::new(|draft_dir, _, _, _| {
        Box::pin(async move {
            std::fs::create_dir_all(&draft_dir).unwrap();
            std::fs::write(draft_dir.join("partial.txt"), "partial").unwrap();
            Err(super::FetchFailure::Cancelled)
        })
    }));
}
