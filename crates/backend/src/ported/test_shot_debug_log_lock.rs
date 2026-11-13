//! tests/test_shot_debug_log_lock.py: logging into the debug shot never
//! waits for the debug shot's data lock.
//!
//! From the second shot on, every log record is copied into the current
//! debug shot. Code holding the data lock logs (building a debug shot,
//! ingesting a sample), so a record that waited for that lock froze the
//! backend at the start of a shot in the Python, and a record that gave up
//! on it was lost. The records are read back from the debug shot file a
//! snapshot writes (`write_current_incomplete_debug_shot`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::app::App;

/// The logger name the Python test logs as, for this module.
const CALLER: &str = "ported.test_shot_debug_log_lock";

/// The `debug_shot` fixture: a running debug shot capturing the logs, and
/// the backend's logging to log through.
fn debug_shot() -> (tempfile::TempDir, Arc<App>, tracing::Dispatch) {
    let (dir, app) = crate::testing::app();
    // Building a debug shot reads the network configuration.
    app.wifi.set_host(crate::wifi::fake::FakeNetwork::new());
    app.debug_shots.start(&app);
    let dispatch = crate::logging::dispatch_for_tests(app.log.clone());
    (dir, app, dispatch)
}

fn logged(dispatch: &tracing::Dispatch, log: impl FnOnce()) {
    tracing::dispatcher::with_default(dispatch, log);
}

/// A snapshot of the running shot under `root`: its file.
fn save_copy(app: &App, root: &Path) -> PathBuf {
    let relative = app
        .debug_shots
        .write_current_incomplete(app, root)
        .expect("a debug shot is running");
    root.join(relative)
}

fn logs_in(file: &Path) -> Vec<Value> {
    let compressed = std::fs::read(file).expect("debug shot file");
    let raw = zstd::decode_all(compressed.as_slice()).expect("zstd");
    let shot: Value = serde_json::from_slice(&raw).expect("JSON");
    shot["logs"].as_array().cloned().expect("logs")
}

fn messages(logs: &[Value]) -> Vec<String> {
    logs.iter()
        .map(|entry| entry["log_message"].as_str().expect("message").to_string())
        .collect()
}

/// tests/test_shot_debug_log_lock.py::test_a_log_record_reaches_the_debug_shot
#[test]
fn a_log_record_reaches_the_debug_shot() {
    let (dir, app, dispatch) = debug_shot();

    logged(&dispatch, || tracing::warn!("pressure sensor slow"));

    let logs = logs_in(&save_copy(&app, dir.path()));
    assert_eq!(logs.len(), 1);
    let entry = &logs[0];
    assert_eq!(entry["loglevel"], "WARNING");
    assert_eq!(entry["caller"], CALLER);
    assert!(
        entry["log_message"]
            .as_str()
            .expect("message")
            .ends_with("pressure sensor slow")
    );
    assert!(entry["profile_ms"].as_i64().expect("profile_ms") >= 0);
}

/// tests/test_shot_debug_log_lock.py::test_logging_does_not_wait_while_another_thread_holds_the_data_lock
#[test]
fn logging_does_not_wait_while_another_thread_holds_the_data_lock() {
    let (dir, app, dispatch) = debug_shot();
    let (logged_tx, logged_rx) = mpsc::channel();

    let worker = {
        let _data_lock = app.debug_shots.hold_data_lock();
        let dispatch = dispatch.clone();
        let worker = std::thread::spawn(move || {
            logged(&dispatch, || tracing::info!("GET /api/v1/settings"));
            let _ = logged_tx.send(());
        });
        let finished = logged_rx.recv_timeout(Duration::from_secs(2)).is_ok();
        assert!(
            finished,
            "logging blocked on the data lock: the shot log sink can deadlock"
        );
        worker
    };

    worker.join().expect("worker");
    assert_eq!(
        messages(&logs_in(&save_copy(&app, dir.path()))),
        ["GET /api/v1/settings"]
    );
}

/// tests/test_shot_debug_log_lock.py::test_two_threads_logging_while_one_holds_the_data_lock_do_not_deadlock:
/// the shape of the freeze, one thread logs under the data lock while
/// another logs at the same time.
#[test]
fn two_threads_logging_while_one_holds_the_data_lock_do_not_deadlock() {
    let (dir, app, dispatch) = debug_shot();
    let (done_tx, done_rx) = mpsc::channel();

    let holder = {
        let (app, dispatch, done) = (app.clone(), dispatch.clone(), done_tx.clone());
        std::thread::spawn(move || {
            {
                let _data_lock = app.debug_shots.hold_data_lock();
                // The other thread is inside the log sink by now.
                std::thread::sleep(Duration::from_millis(50));
                logged(&dispatch, || {
                    tracing::warn!("/etc/build-date file not found")
                });
            }
            let _ = done.send("holder");
        })
    };
    let request = {
        let dispatch = dispatch.clone();
        std::thread::spawn(move || {
            logged(&dispatch, || {
                for _ in 0..50 {
                    tracing::info!("200 GET /api/v1/settings");
                }
            });
            let _ = done_tx.send("request");
        })
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut done = Vec::new();
    while done.len() < 2 {
        let left = deadline.saturating_duration_since(Instant::now());
        match done_rx.recv_timeout(left) {
            Ok(name) => done.push(name),
            Err(_) => break,
        }
    }
    done.sort();
    assert_eq!(done, ["holder", "request"], "the two threads deadlocked");
    holder.join().expect("holder");
    request.join().expect("request");
    assert_eq!(logs_in(&save_copy(&app, dir.path())).len(), 51);
}

/// tests/test_shot_debug_log_lock.py::test_a_record_logged_with_no_shot_running_goes_nowhere
#[test]
fn a_record_logged_with_no_shot_running_goes_nowhere() {
    let (dir, app, dispatch) = debug_shot();
    app.debug_shots.stop(&app);
    // The stopped shot is saved in the background; let it finish.
    let saved = wait_for_debug_file(&app.env.paths.debug_history);

    logged(&dispatch, || tracing::info!("between shots"));

    assert!(
        !messages(&logs_in(&saved)).contains(&"between shots".to_string()),
        "the record reached the stopped shot"
    );
    assert!(
        app.debug_shots
            .write_current_incomplete(&app, dir.path())
            .is_none()
    );
    app.debug_shots.start(&app);
    assert!(
        logs_in(&save_copy(&app, dir.path())).is_empty(),
        "a record logged between shots reached the next one"
    );
}

/// tests/test_shot_debug_log_lock.py::test_the_saved_copy_keeps_the_logs_and_later_records_do_not_change_it
#[test]
fn the_saved_copy_keeps_the_logs_and_later_records_do_not_change_it() {
    let (dir, app, dispatch) = debug_shot();
    logged(&dispatch, || tracing::warn!("before the copy"));

    let copy = save_copy(&app, &dir.path().join("copy"));
    logged(&dispatch, || tracing::warn!("after the copy"));

    assert_eq!(messages(&logs_in(&copy)), ["before the copy"]);
    let later = save_copy(&app, &dir.path().join("later"));
    assert_eq!(logs_in(&later).len(), 2);
    assert_eq!(messages(&logs_in(&copy)), ["before the copy"]);
}

fn wait_for_debug_file(debug_dir: &Path) -> PathBuf {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let found = std::fs::read_dir(debug_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|day| day.path().is_dir())
            .flat_map(|day| {
                std::fs::read_dir(day.path())
                    .into_iter()
                    .flatten()
                    .flatten()
            })
            .map(|entry| entry.path())
            .find(|path| path.to_string_lossy().ends_with(".shot.json.zst"));
        // Written in place: complete once it decodes.
        let complete = found.filter(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|bytes| zstd::decode_all(bytes.as_slice()).ok())
                .is_some_and(|raw| serde_json::from_slice::<Value>(&raw).is_ok())
        });
        if let Some(path) = complete {
            return path;
        }
        assert!(Instant::now() < deadline, "the stopped shot was not saved");
        std::thread::sleep(Duration::from_millis(10));
    }
}
