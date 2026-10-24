//! What the server does with the Socket.IO events clients send
//! (`@sio.on(...)` handlers in `backend.py`).

use std::sync::Arc;

use serde_json::Value;

use crate::app::App;

/// `connect`: the new client gets the hovered profile, from the backend.
pub fn on_connect(app: &Arc<App>, sid: &str) {
    let _ = (app, sid);
}

/// `action`: ESP32 actions are forwarded, `abort` ends the profile, `reset`
/// is refused from Socket.IO, anything else is dropped.
pub fn on_action(app: &Arc<App>, data: Option<Value>) {
    let _ = (app, data);
}

/// `notification`: the payload is a JSON string `{"id", "response"}`.
pub fn on_notification(app: &Arc<App>, data: Option<Value>) {
    let parsed = match data {
        Some(Value::String(text)) => serde_json::from_str::<Value>(&text).ok(),
        Some(other) => {
            // PARITY: Python calls json.loads on the payload and fails on
            // anything but a string.
            tracing::error!("notification event payload is not a JSON string: {other}");
            None
        }
        None => None,
    };
    let Some(Value::Object(map)) = parsed else {
        return;
    };
    if let (Some(id), Some(response)) = (map.get("id"), map.get("response")) {
        let id = match id {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let response = match response {
            Value::String(s) => Some(s.clone()),
            Value::Null => None,
            other => Some(other.to_string()),
        };
        app.notifications.acknowledge(&id, response);
    }
}

/// `profileHover`: remember the hovered profile and tell the other clients.
pub fn on_profile_hover(app: &Arc<App>, sid: &str, data: Option<Value>) {
    let _ = (app, sid, data);
}

/// `calibrate`: write the scale calibration command with the current weight.
pub fn on_calibrate(app: &Arc<App>) {
    let _ = app;
}
