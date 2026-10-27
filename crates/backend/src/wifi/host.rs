//! Everything `wifi.py` reaches outside the process, in one trait: the
//! `nmcli` package, `subprocess.run`, DNS and HTTP probes, the clock and its
//! threads. The Python tests replace exactly these (`wifi.nmcli`,
//! `wifi.subprocess`, `wifi.socket`, `wifi.time`, `wifi.NamedThread`); the
//! Rust tests implement this trait instead.

use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::http_client::{HttpClient, HttpError};
use crate::py::{Completed, PyResult, RunError, run_command};
use crate::wifi::nmcli::{Nmcli, NmcliCli};

/// A background job (`NamedThread(name, target=...)`): an error ends it the
/// way an exception ends the Python thread.
pub type Task = Box<dyn FnOnce() -> PyResult<()> + Send>;

pub trait WifiHost: Send + Sync {
    fn nmcli(&self) -> &dyn Nmcli;
    /// `subprocess.run(command, capture_output=True, text=True, timeout=...)`.
    fn run(&self, argv: &[String], timeout: Option<f64>) -> Result<Completed, RunError>;
    /// `shutil.which(name) is not None`.
    fn which(&self, name: &str) -> bool;
    /// `os.path.exists(path)`.
    fn path_exists(&self, path: &str) -> bool;
    /// `socket.getaddrinfo(host, port)`, the error as `str(e)`.
    fn getaddrinfo(&self, host: &str, port: u16) -> Result<(), String>;
    /// `urllib.request.urlopen(Request(url, headers=...), timeout=...)`:
    /// the final status after redirects.
    fn http_status(&self, url: &str, timeout: f64, user_agent: &str) -> Result<u16, HttpError>;
    /// `socket.gethostname()`.
    fn gethostname(&self) -> String;
    /// `time.time()`.
    fn time(&self) -> f64;
    /// `time.monotonic()`.
    fn monotonic(&self) -> f64;
    /// `time.sleep(seconds)`.
    fn sleep(&self, seconds: f64);
    /// `NamedThread(name, target=task).start()`.
    fn spawn(&self, name: &str, task: Task);
}

/// The machine.
pub struct SystemWifiHost {
    nmcli: NmcliCli,
    http: Arc<dyn HttpClient>,
    started: std::time::Instant,
}

impl SystemWifiHost {
    pub fn new(http: Arc<dyn HttpClient>) -> SystemWifiHost {
        SystemWifiHost {
            nmcli: NmcliCli::new(),
            http,
            started: std::time::Instant::now(),
        }
    }
}

impl WifiHost for SystemWifiHost {
    fn nmcli(&self) -> &dyn Nmcli {
        &self.nmcli
    }

    fn run(&self, argv: &[String], timeout: Option<f64>) -> Result<Completed, RunError> {
        run_command(argv, timeout.map(Duration::from_secs_f64), &[])
    }

    fn which(&self, name: &str) -> bool {
        crate::py::which(name).is_some()
    }

    fn path_exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }

    fn getaddrinfo(&self, host: &str, port: u16) -> Result<(), String> {
        (host, port)
            .to_socket_addrs()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn http_status(&self, url: &str, timeout: f64, user_agent: &str) -> Result<u16, HttpError> {
        self.http
            .status(url, Duration::from_secs_f64(timeout), Some(user_agent))
    }

    fn gethostname(&self) -> String {
        nix::unistd::gethostname()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn time(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    fn monotonic(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn sleep(&self, seconds: f64) {
        if seconds > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(seconds));
        }
    }

    fn spawn(&self, name: &str, task: Task) {
        spawn_named(name, task);
    }
}

/// `NamedThread.run`: log the start, and an exception as a crash.
pub fn spawn_named(name: &str, task: Task) {
    let thread_name = name.to_string();
    let result = std::thread::Builder::new()
        .name(name.chars().take(15).collect())
        .spawn(move || {
            tracing::debug!("Thread {thread_name} started");
            if let Err(error) = task() {
                tracing::error!(
                    "Thread {thread_name} crashed: {}: {}",
                    error.kind,
                    error.message
                );
            }
        });
    if let Err(error) = result {
        tracing::error!("Could not start thread {name}: {error}");
    }
}
