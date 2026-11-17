//! `notifications.py`: messages for the user, shown by the Dial and the app.

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Value, json};
use tstype::{Ts, field};

use crate::config::{ConfigStore, keys};
use crate::sio::Sio;
use crate::sounds::{SoundEvent, SoundPlayer};

pub mod responses {
    pub const OK: &str = "Ok";
    pub const YES: &str = "Yes";
    pub const NO: &str = "No";
    pub const UPDATE: &str = "Update";
    pub const SKIP: &str = "Skip";
    pub const ABORT: &str = "Abort";
}

pub type Callback = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct Notification {
    pub id: String,
    pub message: String,
    pub responses: Vec<String>,
    pub image: Option<String>,
    pub acknowledged: bool,
    pub acknowledged_at: Option<f64>,
    pub response: Option<String>,
    pub callback: Option<Callback>,
    /// `datetime.now()`: local wall time, naive.
    pub timestamp: chrono::NaiveDateTime,
}

impl Notification {
    /// `Notification(message)` with the default `[Ok]` response.
    pub fn new(message: impl Into<String>) -> Notification {
        Notification::with_responses(message, &[responses::OK])
    }

    pub fn with_responses(message: impl Into<String>, responses: &[&str]) -> Notification {
        Notification {
            id: uuid::Uuid::new_v4().to_string(),
            message: message.into(),
            responses: responses.iter().map(|r| r.to_string()).collect(),
            image: None,
            acknowledged: false,
            acknowledged_at: None,
            response: None,
            callback: None,
            timestamp: chrono::Local::now().naive_local(),
        }
    }

    pub fn image(mut self, image: impl Into<String>) -> Notification {
        self.image = Some(image.into());
        self
    }

    pub fn callback(mut self, callback: Callback) -> Notification {
        self.callback = Some(callback);
        self
    }

    /// The entry the HTTP API lists and the socket event carries.
    pub fn to_value(&self) -> Value {
        json!({
            "id": self.id,
            "message": self.message,
            "image": self.image,
            "responses": self.responses,
            "timestamp": python_isoformat(&self.timestamp),
        })
    }

    /// `Notification.to_json()`: the socket event's payload is this JSON text.
    pub fn to_json(&self) -> String {
        pyfmt::dumps_default(&self.to_value())
    }

    /// The payload of the `notification` event.
    pub fn to_sio(&self) -> Value {
        Value::String(self.to_json())
    }

    /// The TypeScript type of [`Notification::to_sio`].
    pub fn sio_type() -> Ts {
        let message = Ts::named(
            "NotificationMessage",
            "A notification, as GET /notifications lists it too.",
            Ts::object([
                field("id", Ts::String),
                field("message", Ts::String),
                field("image", Ts::String.nullable()),
                field("responses", Ts::String.array()),
                field("timestamp", Ts::String)
                    .doc("Local time, `datetime.isoformat()` without an offset."),
            ]),
        );
        Ts::named(
            "NotificationPayload",
            "The `notification` event: the JSON text of a `NotificationMessage` (the Python emits\n\
             `json.dumps(...)`), not an object.",
            message.json_text(),
        )
    }
}

/// `datetime.isoformat()`: microseconds only when there are any.
pub fn python_isoformat(t: &chrono::NaiveDateTime) -> String {
    use chrono::Timelike;
    if t.nanosecond() / 1000 == 0 {
        t.format("%Y-%m-%dT%H:%M:%S").to_string()
    } else {
        t.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
    }
}

pub struct NotificationManager {
    list: Mutex<Vec<Notification>>,
    sio: Arc<Sio>,
    config: Arc<ConfigStore>,
    sounds: Arc<SoundPlayer>,
}

impl NotificationManager {
    pub fn new(
        sio: Arc<Sio>,
        config: Arc<ConfigStore>,
        sounds: Arc<SoundPlayer>,
    ) -> NotificationManager {
        NotificationManager {
            list: Mutex::new(Vec::new()),
            sio,
            config,
            sounds,
        }
    }

    /// `NotificationManager.add_notification`: a notification with the id of
    /// an unacknowledged one replaces it (an update, without the sound).
    pub fn add(&self, mut notification: Notification) {
        let updating = {
            let mut list = self.list.lock();
            let before = list.len();
            list.retain(|n| !(n.id == notification.id && !n.acknowledged));
            let updating = list.len() != before;
            notification.acknowledged = false;
            list.push(notification.clone());
            updating
        };
        let payload = notification.to_sio();
        self.sio.emit("notification", &payload);
        tracing::info!(
            "send notification: {}",
            payload.as_str().unwrap_or_default()
        );
        if updating {
            tracing::info!("Notification updated");
        } else {
            tracing::info!("Notification created");
            self.sounds.play_event(SoundEvent::Notification);
        }
    }

    /// `acknowledge_notification`: true when an unacknowledged notification
    /// with that id existed.
    pub fn acknowledge(&self, id: &str, response: Option<String>) -> bool {
        self.delete_old_acknowledged();
        let callback = {
            let mut list = self.list.lock();
            let Some(n) = list.iter_mut().find(|n| n.id == id && !n.acknowledged) else {
                return false;
            };
            n.acknowledged = true;
            n.response = response;
            n.acknowledged_at = Some(now_seconds());
            n.callback.clone()
        };
        if let Some(callback) = callback {
            // PARITY: Notification.acknowledge() runs the callback and
            // acknowledge_notification() runs it again.
            callback();
            callback();
        }
        true
    }

    /// Whether the notification with this id is acknowledged; `None` when it
    /// is not listed (never added, or acknowledged and expired).
    pub fn is_acknowledged(&self, id: &str) -> Option<bool> {
        self.list
            .lock()
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.acknowledged)
    }

    pub fn unacknowledged(&self) -> Vec<Notification> {
        self.delete_old_acknowledged();
        self.list
            .lock()
            .iter()
            .filter(|n| !n.acknowledged)
            .cloned()
            .collect()
    }

    pub fn all(&self) -> Vec<Notification> {
        self.delete_old_acknowledged();
        self.list.lock().clone()
    }

    /// Replace the list as it stands, acknowledged entries included (the
    /// Python suite patches `NotificationManager._notifications`).
    #[cfg(test)]
    pub fn replace_all(&self, notifications: Vec<Notification>) {
        *self.list.lock() = notifications;
    }

    fn delete_old_acknowledged(&self) {
        let ttl = self
            .config
            .get_f64(keys::SYSTEM, keys::NOTIFICATION_KEEPALIVE)
            .unwrap_or(3600.0);
        let now = now_seconds();
        self.list
            .lock()
            .retain(|n| !n.acknowledged || n.acknowledged_at.is_some_and(|at| now - at < ttl));
    }
}

pub fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isoformat_like_python() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 10, 24)
            .and_then(|d| d.and_hms_micro_opt(9, 5, 7, 120))
            .expect("date");
        assert_eq!(python_isoformat(&t), "2026-10-24T09:05:07.000120");
        let t = chrono::NaiveDate::from_ymd_opt(2026, 10, 24)
            .and_then(|d| d.and_hms_opt(9, 5, 7))
            .expect("date");
        assert_eq!(python_isoformat(&t), "2026-10-24T09:05:07");
    }
}
