//! What the server does with the Socket.IO events clients send
//! (`@sio.on(...)` handlers in `backend.py`), the `status` stream, and the
//! developer commands read from stdin.

use std::io::BufRead;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tstype::{Ts, field};

use crate::app::App;
use crate::config::keys;
use crate::machine::ALLOWED_ESP_ACTIONS;
use crate::notifications::Notification;

/// `connect`: the new client gets the hovered profile, from the backend.
pub fn on_connect(app: &Arc<App>, sid: &str) {
    app.profiles.emit_hover_to(sid);
}

/// `action`: ESP32 actions are forwarded, `abort` ends the profile, `reset`
/// is refused from Socket.IO, anything else is dropped.
pub fn on_action(app: &Arc<App>, data: Option<Value>) {
    app.standby.note_activity();
    let action = match &data {
        Some(Value::String(s)) => Some(s.as_str()),
        _ => None,
    };
    match action {
        Some(a) if ALLOWED_ESP_ACTIONS.contains(&a) => {
            app.machine.action(app, a);
        }
        Some("reset") => tracing::warn!("action,reset is not allowed from socket.io"),
        Some("abort") => app.machine.end_profile(app),
        _ => tracing::warn!(
            "Invalid action {}",
            data.as_ref()
                .map(crate::metconfig::py_repr)
                .unwrap_or_else(|| "None".into())
        ),
    }
}

/// `notification`: the payload is a JSON string `{"id", "response"}`.
pub fn on_notification(app: &Arc<App>, data: Option<Value>) {
    app.standby.note_activity();
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
    app.standby.note_activity();
    let data = data.unwrap_or(Value::Null);
    tracing::info!(
        "Hovering Profile {}",
        pyfmt::dumps(&data, &pyfmt::DumpOpts::indented(1))
    );
    app.profiles.handle_hover(&data, sid);
}

/// `calibrate`: write the scale calibration command with the current weight.
pub fn on_calibrate(app: &Arc<App>) {
    app.standby.note_activity();
    app.machine.calibrate();
}

/// `live()`: the machine's state with each frame the ESP sends, and `action,info`
/// every two seconds until the ESP32 answered.
///
/// The Python emitted on a 10 Hz timer of its own, up to a period behind the
/// ESP's frames (which also come every 100 ms) and repeating stale data when
/// the ESP was silent. Here a status goes out as soon as a frame is applied;
/// [`HEARTBEAT`] keeps the stream alive while the ESP sends nothing
/// (flashing, unplugged, the boot harness's silent pty), and
/// [`MIN_INTERVAL`] caps the rate when frames come faster (the emulator at
/// speed).
pub async fn live(app: Arc<App>) {
    let mut info_asked = Instant::now();
    tracing::info!("Starting to emit machine data");
    loop {
        if info_asked.elapsed() > Duration::from_secs(2) && !app.machine.info_ready() {
            info_asked = Instant::now();
            app.machine.action(&app, "info");
        }
        app.sio.emit("status", &status(&app));
        if let Some(sensors) = app.machine.sensor_sensors() {
            app.sio.emit("sensors", &sensors.to_sio_sensors());
        }
        tokio::time::sleep(MIN_INTERVAL).await;
        let _ = tokio::time::timeout(
            HEARTBEAT.saturating_sub(MIN_INTERVAL),
            app.machine.frames.notified(),
        )
        .await;
    }
}

/// The longest gap between two statuses while the ESP is silent.
pub const HEARTBEAT: Duration = Duration::from_millis(200);
/// The shortest gap between two statuses.
pub const MIN_INTERVAL: Duration = Duration::from_millis(40);

/// One `status` event: the machine's data plus the loaded profile.
pub fn status(app: &App) -> Value {
    let mut status = match app.machine.data_sensors().to_sio() {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    // Deliberate fix: a last profile without "profile" or "name" raises in
    // Python and ends the status stream for good.
    let profile = app
        .profiles
        .last_profile()
        .and_then(|entry| entry.get("profile").cloned());
    match profile {
        Some(profile) => {
            let name = profile.get("name").cloned().unwrap_or(Value::Null);
            // The emulator does not know the profile; the placeholder stands in.
            if app.emulated()
                && status.get("profile").and_then(Value::as_str)
                    == Some(crate::machine::emulator::PROFILE_PLACEHOLDER)
            {
                let shown = if app.machine.profile_ready() {
                    name.clone()
                } else {
                    json!("default")
                };
                status.insert("profile".into(), shown);
            }
            status.insert("loaded_profile".into(), name);
            status.insert(
                "id".into(),
                profile.get("id").cloned().unwrap_or(Value::Null),
            );
        }
        None => {
            status.insert("loaded_profile".into(), Value::Null);
            status.insert("id".into(), Value::Null);
        }
    }
    Value::Object(status)
}

/// The TypeScript type of [`status`].
pub fn status_type() -> Ts {
    let mut fields = esp::ShotData::sio_fields();
    fields.push(
        field("loaded_profile", Ts::String.nullable())
            .doc("The name of the last profile loaded; null before the first."),
    );
    fields.push(field("id", Ts::String.nullable()).doc("The id of that profile."));
    Ts::named(
        "StatusPayload",
        "The `status` event: the machine's last frame and the loaded profile.",
        Ts::object(fields),
    )
}

/// `send_data()`: developer commands typed into the backend's terminal.
/// The service has no stdin, so it stops at once there.
pub fn start_stdin_commands(app: Arc<App>) {
    let _ = std::thread::Builder::new()
        .name("SendSocketIO".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut lines = stdin.lock().lines();
            loop {
                print!("> ");
                let Some(Ok(input)) = lines.next() else {
                    tracing::warn!("no STDIN attached, not listening to commands!");
                    return;
                };
                run_command(&app, input.trim_end_matches(['\r', '\n']));
            }
        });
}

fn run_command(app: &Arc<App>, input: &str) {
    let button = |name: &str| {
        if let Some(event) = esp::ButtonEventData::from_args(&[name]) {
            app.sio.emit("button", &event.to_sio());
        }
    };
    match input {
        "reset" => app.machine.reset(),
        "show" => {
            app.config
                .set_and_save(keys::LOGGING, keys::LOGGING_SENSOR_MESSAGES, json!(true))
        }
        "hide" => {
            app.config
                .set_and_save(keys::LOGGING, keys::LOGGING_SENSOR_MESSAGES, json!(false))
        }
        "tare" | "stop" | "purge" | "home" | "start" => {
            app.machine.action(app, input);
        }
        "test" => {
            let previous = app.config.get(keys::LOGGING, keys::LOGGING_SENSOR_MESSAGES);
            app.config
                .set(keys::LOGGING, keys::LOGGING_SENSOR_MESSAGES, json!(true));
            for i in 0..10 {
                app.machine.write(b"action,purge\x03");
                std::thread::sleep(Duration::from_secs(15));
                tracing::info!("action,purge\x03");
                app.machine.write(b"action,home\x03");
                std::thread::sleep(Duration::from_secs(15));
                tracing::info!("action,home\x03");
                tracing::info!("Numero de prueba: {}", i + 1);
            }
            app.config
                .set(keys::LOGGING, keys::LOGGING_SENSOR_MESSAGES, previous);
        }
        s if s.starts_with("calibration") => {
            app.machine.write(format!("action,{s}\x03").as_bytes())
        }
        s if s.starts_with("update") => {
            app.machine.start_update(app);
        }
        s if s.starts_with("notification") => {
            app.notifications
                .add(Notification::new(s.get(12..).unwrap_or("")));
        }
        "l" | "CCW" => button("CCW"),
        "r" | "CW" => button("CW"),
        "e" | "push" => button("push"),
        "d" | "pu_d" => button("pu_d"),
        "t" | "ta_d" => button("ta_d"),
        "s" | "ta_l" => button("ta_l"),
        "ta_sl" => button("ta_sl"),
        "pr" => button("encoder_button_pressed"),
        "re" => button("encoder_button_released"),
        _ => {}
    }
}
