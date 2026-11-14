//! Port of `tests/test_actions.py`: how the action handler routes `finish`
//! (to the ESP32) and `abort` (the backend ends the profile).
//!
//! The Python replaces `Machine.action` and `Machine.end_profile` with
//! recorders. Here the machine runs for real on a pty, the UART hook the
//! boot harness uses, and the test reads what reached the ESP32's end.

use std::io::Read as _;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use http::Method;
use serde_json::json;

use crate::app::App;
use crate::config::keys;
use crate::env::BackendMode;
use crate::machine::{ALLOWED_BACKEND_ACTIONS, ALLOWED_ESP_ACTIONS};
use crate::testing::{self, call};

/// A machine whose ESP32 is the other end of a pty.
struct Esp {
    _dir: tempfile::TempDir,
    app: Arc<App>,
    esp: std::fs::File,
    _device: OwnedFd,
    received: Vec<u8>,
}

fn esp() -> Esp {
    let pty = nix::pty::openpty(None, None).expect("pty");
    let device = nix::unistd::ttyname(&pty.slave).expect("pty name");
    let flags = nix::fcntl::fcntl(&pty.master, nix::fcntl::FcntlArg::F_GETFL).expect("flags");
    let flags = nix::fcntl::OFlag::from_bits_truncate(flags) | nix::fcntl::OFlag::O_NONBLOCK;
    nix::fcntl::fcntl(&pty.master, nix::fcntl::FcntlArg::F_SETFL(flags)).expect("non-blocking");
    let (dir, app) = testing::app_custom(BackendMode::Emulator, |env| {
        env.hooks.esp_uart = Some(device);
        // The motor energy saver writes every second for as long as the
        // test process runs; give it a folder it cannot create, so nothing
        // reappears once the temporary directory is gone.
        env.paths.motor_energy = PathBuf::from("/dev/null/motor-energy");
    });
    // A minute in, an ESP32 that never sent its info would be flashed.
    app.config
        .set(keys::USER, keys::DISALLOW_FIRMWARE_FLASHING, json!(true));
    app.machine.init(&app).expect("machine init");
    Esp {
        _dir: dir,
        app,
        esp: std::fs::File::from(pty.master),
        _device: pty.slave,
        received: Vec::new(),
    }
}

impl Esp {
    /// Every message written to the ESP32 up to `marker`, without the
    /// greeting (a bare terminator and `action,info`). Writes go out in
    /// order, so nothing written before the marker can arrive after it.
    fn messages_until(&mut self, marker: &str) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut chunk = [0u8; 1024];
            match self.esp.read(&mut chunk) {
                Ok(n) => self.received.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("reading the ESP32 side: {e}"),
            }
            let mut complete: Vec<String> = self
                .received
                .split(|b| *b == 0x03)
                .map(|m| String::from_utf8_lossy(m).into_owned())
                .collect();
            // What follows the last terminator is not a message yet.
            complete.pop();
            if let Some(end) = complete.iter().position(|m| m == marker) {
                return complete
                    .into_iter()
                    .take(end + 1)
                    .filter(|m| !m.is_empty() && m != "action,info")
                    .collect();
            }
            assert!(
                Instant::now() < deadline,
                "{marker} never reached the ESP32: {:?}",
                String::from_utf8_lossy(&self.received)
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A shot in its extraction stage, where ending the profile sends `home`.
    fn brewing(&self) {
        let mut state = self.app.machine.state.lock();
        state.data_sensors.state = "brewing".into();
        state.data_sensors.status = Some("Preinfusion".into());
    }

    async fn action(&self, action: &str) -> crate::http::Reply {
        call(
            &self.app,
            Method::POST,
            &format!("/api/v1/action/{action}"),
            "",
        )
        .await
    }
}

/// tests/test_actions.py::test_finish_is_an_allowed_esp_action
#[test]
fn finish_is_an_allowed_esp_action() {
    assert!(ALLOWED_ESP_ACTIONS.contains(&"finish"));
}

/// tests/test_actions.py::test_abort_is_an_allowed_backend_action
#[test]
fn abort_is_an_allowed_backend_action() {
    assert!(ALLOWED_BACKEND_ACTIONS.contains(&"abort"));
    // The handler checks the ESP list first, so abort must stay out of it.
    assert!(!ALLOWED_ESP_ACTIONS.contains(&"abort"));
}

/// tests/test_actions.py::test_finish_is_forwarded_to_the_esp
///
/// During a shot, so ending the profile would have sent `home`.
#[tokio::test]
async fn finish_is_forwarded_to_the_esp() {
    let mut esp = esp();
    esp.brewing();

    let reply = esp.action("finish").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        testing::json(&reply),
        json!({"action": "finish", "status": "ok"})
    );
    esp.action("tare").await;
    assert_eq!(
        esp.messages_until("action,tare"),
        vec!["action,finish", "action,tare"]
    );
}

/// tests/test_actions.py::test_abort_ends_the_profile
///
/// Ending a shot in its extraction stage sends `home`; `abort` itself never
/// reaches the ESP32.
#[tokio::test]
async fn abort_ends_the_profile() {
    let mut esp = esp();
    esp.brewing();

    let reply = esp.action("abort").await;

    assert_eq!(reply.status_code(), 200);
    esp.action("tare").await;
    assert_eq!(
        esp.messages_until("action,tare"),
        vec!["action,home", "action,tare"]
    );
}
