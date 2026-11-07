//! Port of `tests/test_motor_strain.py`, the case that is not already
//! handled: a `start` is refused while the motor rests, like a `purge`.

use http::Method;
use serde_json::json;

use crate::alarms::AlarmType;
use crate::testing::{self, call};

/// tests/test_motor_strain.py::test_start_is_refused_while_the_motor_rests
#[tokio::test]
async fn start_is_refused_while_the_motor_rests() {
    let (_dir, app) = testing::app();
    app.alarms.set(AlarmType::MotorStressed, None, true, true);
    app.machine.state.lock().profile_ready = true;

    let reply = call(&app, Method::POST, "/api/v1/action/start", "").await;

    assert_eq!(reply.status_code(), 409);
    assert_eq!(
        testing::json(&reply),
        json!({"status": "error", "action": "start"})
    );
    assert_eq!(app.machine.uart_writes.lock().concat(), Vec::<u8>::new());
}
