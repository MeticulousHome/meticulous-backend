//! tests/test_machine_device_uuid.py: what the backend does once the ESP32
//! confirmed its device UUID: the OTA identity cache, and a restart of
//! rauc-hawkbit-updater when the cache changed. The enrollment protocol
//! itself (assignments, retries, responses) is ported with the `esp` crate
//! (`crates/esp/tests/python_machine.rs`).

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::app::App;
use crate::bug_report::test_support::capture_logs;
use crate::host::{CommandOutput, CommandRunner};

const FIRST_UUID: &str = "123e4567-e89b-42d3-a456-426614174000";
const RESTART: [&str; 4] = [
    "systemctl",
    "--no-block",
    "restart",
    "rauc-hawkbit-updater.service",
];

/// `subprocess.run` as the Python tests replace it: every call with its
/// timeout and the thread it ran on, and one answer for all of them.
struct Systemctl {
    answer: Result<CommandOutput, std::io::ErrorKind>,
    calls: Mutex<Vec<Call>>,
}

#[derive(Debug, Clone, PartialEq)]
struct Call {
    argv: Vec<String>,
    timeout: Option<Duration>,
    thread: Option<String>,
}

impl Systemctl {
    fn answering(answer: Result<CommandOutput, std::io::ErrorKind>) -> Arc<Systemctl> {
        Arc::new(Systemctl {
            answer,
            calls: Mutex::new(Vec::new()),
        })
    }

    fn record(&self, argv: &[&str], timeout: Option<Duration>) -> std::io::Result<CommandOutput> {
        self.calls.lock().push(Call {
            argv: argv.iter().map(|a| a.to_string()).collect(),
            timeout,
            thread: std::thread::current().name().map(str::to_string),
        });
        match &self.answer {
            Ok(output) => Ok(output.clone()),
            Err(kind) => Err(std::io::Error::new(*kind, "systemctl unavailable")),
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().clone()
    }
}

impl CommandRunner for Systemctl {
    fn run(&self, argv: &[&str], _stdin: Option<&[u8]>) -> std::io::Result<CommandOutput> {
        self.record(argv, None)
    }

    fn run_with_timeout(&self, argv: &[&str], timeout: Duration) -> std::io::Result<CommandOutput> {
        self.record(argv, Some(timeout))
    }

    fn spawn(&self, _argv: &[&str]) -> std::io::Result<()> {
        Ok(())
    }

    fn which(&self, _name: &str) -> Option<std::path::PathBuf> {
        None
    }
}

/// A machine (not emulated, so the updater is restarted) whose host
/// commands are recorded.
fn machine(
    answer: Result<CommandOutput, std::io::ErrorKind>,
) -> (tempfile::TempDir, Arc<App>, Arc<Systemctl>) {
    let (dir, app) = crate::testing::app_with(crate::env::BackendMode::Fika, |_| {});
    assert!(!app.machine.emulated);
    let systemctl = Systemctl::answering(answer);
    app.host.replace(systemctl.clone());
    (dir, app, systemctl)
}

fn restart_call() -> Call {
    Call {
        argv: RESTART.iter().map(|a| a.to_string()).collect(),
        timeout: Some(Duration::from_secs(5)),
        thread: None,
    }
}

/// tests/test_machine_device_uuid.py::test_confirmed_uuid_updates_cache_without_restart_when_unchanged
#[test]
fn confirmed_uuid_updates_cache_without_restart_when_unchanged() {
    let (_dir, app, systemctl) = machine(Ok(CommandOutput::ok()));
    let cache = app.env.paths.device_uuid_cache.clone();
    std::fs::create_dir_all(cache.parent().expect("parent")).expect("cache directory");
    std::fs::write(&cache, format!("{FIRST_UUID}\n")).expect("cache");
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    // An assignment of this boot is in flight when the ESP32 confirms a UUID.
    app.machine.sync_device_uuid(&app, "", true);
    assert!(app.machine.pending_device_uuid().is_some());
    let logs = capture_logs();

    app.machine.sync_device_uuid(&app, FIRST_UUID, true);

    // The cache was updated with the confirmed UUID (it already held it: only
    // its permissions were enforced), and the updater was left alone.
    assert_eq!(
        std::fs::read_to_string(&cache).expect("cache"),
        format!("{FIRST_UUID}\n")
    );
    let mode = std::fs::metadata(&cache)
        .expect("cache")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(app.machine.pending_device_uuid(), None);
    assert!(
        logs.messages(tracing::Level::INFO, "machine")
            .iter()
            .all(|m| !m.starts_with("Updated OTA device UUID cache"))
    );
    assert!(systemctl.calls().is_empty());
}

/// tests/test_machine_device_uuid.py::test_changed_cache_starts_background_updater_restart
#[test]
fn changed_cache_starts_background_updater_restart() {
    let (_dir, app, systemctl) = machine(Ok(CommandOutput::ok()));

    app.machine.sync_device_uuid(&app, FIRST_UUID, true);

    assert_eq!(
        std::fs::read_to_string(&app.env.paths.device_uuid_cache).expect("cache"),
        format!("{FIRST_UUID}\n")
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while systemctl.calls().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    // One restart, on its own thread: the caller (the UART thread) never
    // waits for systemctl.
    let calls = systemctl.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].thread.as_deref(), Some("HawkbitRestart"));
    assert_ne!(
        calls[0].thread.as_deref(),
        std::thread::current().name(),
        "the restart ran on the caller's thread"
    );
    assert_eq!(
        Call {
            thread: None,
            ..calls[0].clone()
        },
        restart_call()
    );
}

/// tests/test_machine_device_uuid.py::test_updater_restart_is_enqueued_for_inactive_unit_and_is_bounded
#[test]
fn updater_restart_is_enqueued_for_inactive_unit_and_is_bounded() {
    let (_dir, app, systemctl) = machine(Ok(CommandOutput::ok()));

    crate::machine::restart_hawkbit_updater(&app.host);

    let calls: Vec<Call> = systemctl
        .calls()
        .into_iter()
        .map(|call| Call {
            thread: None,
            ..call
        })
        .collect();
    assert_eq!(calls, vec![restart_call()]);
}

/// tests/test_machine_device_uuid.py::test_updater_restart_command_errors_do_not_escape
/// (OSError, FileNotFoundError and TimeoutExpired).
#[test]
fn updater_restart_command_errors_do_not_escape() {
    for error in [
        std::io::ErrorKind::Other,
        std::io::ErrorKind::NotFound,
        std::io::ErrorKind::TimedOut,
    ] {
        let (_dir, app, systemctl) = machine(Err(error));
        let logs = capture_logs();

        crate::machine::restart_hawkbit_updater(&app.host);

        assert_eq!(systemctl.calls().len(), 1, "{error:?}");
        assert_eq!(
            logs.messages(tracing::Level::ERROR, "machine").len(),
            1,
            "{error:?}"
        );
    }
}

/// tests/test_machine_device_uuid.py::test_updater_restart_nonzero_result_is_logged
#[test]
fn updater_restart_nonzero_result_is_logged() {
    let (_dir, app, _systemctl) = machine(Ok(CommandOutput {
        status: Some(1),
        stdout: Vec::new(),
        stderr: b"failed".to_vec(),
    }));
    let logs = capture_logs();

    crate::machine::restart_hawkbit_updater(&app.host);

    let warnings = logs.messages(tracing::Level::WARN, "machine");
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].ends_with("exit=1 stderr='failed'"),
        "{warnings:?}"
    );
    assert!(logs.messages(tracing::Level::ERROR, "machine").is_empty());
}
