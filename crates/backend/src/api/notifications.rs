//! `api/notifications.py`.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::app::App;
use crate::http::{Kind, Reply, Request, Route};

pub fn routes() -> Vec<Route> {
    let module = "api.notifications";
    ["/notifications", "/notifications/acknowledge"]
        .into_iter()
        .map(|path| {
            Route::new(path, Kind::Base, module, "GetNotificationsHandler")
                .get(list)
                .post(acknowledge)
        })
        .collect()
}

async fn list(app: Arc<App>, req: Request) -> Reply {
    let include_acknowledged =
        req.get_argument_or("acknowledged", "false").to_lowercase() == "true";
    let notifications = if include_acknowledged {
        app.notifications.all()
    } else {
        app.notifications.unacknowledged()
    };
    let entries: Vec<Value> = notifications.iter().map(|n| n.to_value()).collect();
    Reply::base().dumps(Value::Array(entries))
}

async fn acknowledge(app: Arc<App>, req: Request) -> Reply {
    let data = match req.json() {
        Ok(data) => data,
        Err(error) => {
            // PARITY: the Python handler does not catch the decode error, so
            // tornado answers 500.
            tracing::error!("Uncaught exception POST {}: {error}", req.uri);
            return Reply::error(Kind::Base, 500, &req);
        }
    };
    // Deliberate fix: Python's `data.get` raises AttributeError on a body
    // that is JSON but not an object, which tornado answers 500; here such
    // a body has no id and gets the 404.
    let id = data.get("id").map(value_text);
    tracing::info!("acknowledge {}", id.as_deref().unwrap_or("None"));
    let response = data.get("response").and_then(|r| match r {
        Value::Null => None,
        other => Some(value_text(other)),
    });
    let known = match &id {
        Some(id) => app.notifications.acknowledge(id, response),
        None => false,
    };
    if known {
        Reply::base().json(json!({"status": "success"}))
    } else {
        let ids: Vec<Value> = app
            .notifications
            .all()
            .into_iter()
            .map(|n| Value::String(n.id))
            .collect();
        let known_ids = pyfmt::dumps_default(&Value::Array(ids));
        tracing::info!("acknoledge failed. Known notifications: {known_ids}");
        Reply::base()
            .status(404)
            .json(json!({"status": "failure", "message": "Notification not found"}))
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
