//! `UpdateOSStatus` from `api/machine.py`: the progress of the OS update
//! RAUC and rauc-hawkbit-updater report over D-Bus, served by
//! `GET /machine/OS_update_status` and pushed to clients as `OSUpdate`.

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::sio::Sio;

/// `OSStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsStatus {
    Idle,
    Downloading,
    Installing,
    Complete,
    Failed,
}

impl OsStatus {
    /// `OSStatus.to_string`.
    pub fn as_str(self) -> &'static str {
        match self {
            OsStatus::Idle => "IDLE",
            OsStatus::Downloading => "DOWNLOADING",
            OsStatus::Installing => "INSTALLING",
            OsStatus::Complete => "COMPLETE",
            OsStatus::Failed => "FAILED",
        }
    }
}

struct State {
    is_recovery_update: bool,
    /// A Python number: an int from RAUC, a float from Hawkbit.
    last_progress: f64,
    last_status: OsStatus,
    last_extra_info: Option<String>,
}

pub struct OsUpdateStatus {
    sio: Arc<Sio>,
    state: Mutex<State>,
}

impl OsUpdateStatus {
    pub fn new(sio: Arc<Sio>) -> OsUpdateStatus {
        OsUpdateStatus {
            sio,
            state: Mutex::new(State {
                is_recovery_update: false,
                last_progress: 0.0,
                last_status: OsStatus::Idle,
                last_extra_info: None,
            }),
        }
    }

    /// `UpdateOSStatus.to_json`.
    pub fn to_json(&self) -> Value {
        let state = self.state.lock();
        let info = match &state.last_extra_info {
            Some(extra) => format!(" : {extra}"),
            None => String::new(),
        };
        json!({
            "progress": python_round(state.last_progress),
            "status": state.last_status.as_str(),
            "info": info,
        })
    }

    /// `UpdateOSStatus.markAsRecoveryUpdate`: a USB recovery update shows its
    /// progress as notifications too.
    pub fn mark_as_recovery_update(&self, is_recovery: bool) {
        self.state.lock().is_recovery_update = is_recovery;
        tracing::info!(
            "Marking update as{} recovery",
            if is_recovery { "" } else { " not" }
        );
    }

    pub fn is_recovery_update(&self) -> bool {
        self.state.lock().is_recovery_update
    }

    /// `UpdateOSStatus.sendStatus`: record the state and emit `OSUpdate`.
    pub fn send_status(&self, status: OsStatus, progress: f64, extra_info: Option<&str>) {
        // PARITY: sendStatus never stores extra_info, so the failure reason
        // dbus_monitor passes never reaches the clients (the xfail in
        // tests/test_api_machine.py::test_failure_reason_passed_to_send_status_is_reported).
        let _ = extra_info;
        {
            let mut state = self.state.lock();
            state.last_progress = progress;
            state.last_status = status;
        }
        self.sio.emit("OSUpdate", &self.to_json());
    }

    /// The class attribute the Python tests set directly.
    #[cfg(test)]
    pub fn set_last_extra_info(&self, info: Option<&str>) {
        self.state.lock().last_extra_info = info.map(str::to_string);
    }

    #[cfg(test)]
    pub fn set_last(&self, status: OsStatus, progress: f64) {
        let mut state = self.state.lock();
        state.last_status = status;
        state.last_progress = progress;
    }
}

/// Python's `round(x)`: half to even, an int.
pub fn python_round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, call, json as body};

    /// tests/test_api_machine.py::TestOSUpdateStatusRoute::test_idle_status_is_reported_before_any_update
    #[tokio::test]
    async fn idle_status_is_reported_before_any_update() {
        let (_dir, app) = app();
        let reply = call(
            &app,
            http::Method::GET,
            "/api/v1/machine/OS_update_status",
            "",
        )
        .await;
        assert_eq!(reply.status_code(), 200);
        assert_eq!(
            body(&reply),
            json!({"progress": 0, "status": "IDLE", "info": ""})
        );
    }

    /// tests/test_api_machine.py::TestOSUpdateStatusRoute::test_reported_progress_is_rounded_to_a_whole_percent
    #[tokio::test]
    async fn reported_progress_is_rounded_to_a_whole_percent() {
        let (_dir, app) = app();
        app.os_update.send_status(OsStatus::Downloading, 42.6, None);
        let reply = call(
            &app,
            http::Method::GET,
            "/api/v1/machine/OS_update_status",
            "",
        )
        .await;
        assert_eq!(
            body(&reply),
            json!({"progress": 43, "status": "DOWNLOADING", "info": ""})
        );
        assert_eq!(python_round(42.5), 42);
        assert_eq!(python_round(43.5), 44);
    }

    /// tests/test_api_machine.py::TestOSUpdateStatusRoute::test_extra_info_is_appended_after_a_separator
    #[tokio::test]
    async fn extra_info_is_appended_after_a_separator() {
        let (_dir, app) = app();
        app.os_update.set_last(OsStatus::Complete, 100.0);
        app.os_update.set_last_extra_info(Some("rebooting"));
        let reply = call(
            &app,
            http::Method::GET,
            "/api/v1/machine/OS_update_status",
            "",
        )
        .await;
        assert_eq!(
            body(&reply),
            json!({"progress": 100, "status": "COMPLETE", "info": " : rebooting"})
        );
    }

    /// tests/test_api_machine.py::TestOSUpdateStatusRoute::test_failure_reason_passed_to_send_status_is_reported
    /// is an xfail in Python: the reason is dropped. Kept as is (PARITY).
    #[tokio::test]
    async fn failure_reason_passed_to_send_status_is_dropped() {
        let (_dir, app) = app();
        app.os_update
            .send_status(OsStatus::Failed, 0.0, Some("Installation error"));
        let reply = call(
            &app,
            http::Method::GET,
            "/api/v1/machine/OS_update_status",
            "",
        )
        .await;
        assert_eq!(
            body(&reply),
            json!({"progress": 0, "status": "FAILED", "info": ""})
        );
    }
}
