//! `api/action.py`.

use std::sync::Arc;

use serde_json::json;

use crate::app::App;
use crate::http::{Kind, Reply, Request, Route};
use crate::machine::{ALLOWED_BACKEND_ACTIONS, ALLOWED_ESP_ACTIONS};

pub fn routes() -> Vec<Route> {
    vec![
        Route::new(
            "/action/(.*)",
            Kind::Base,
            "api.action",
            "ExecuteActionHandler",
        )
        .post(execute)
        .get(execute),
    ]
}

async fn execute(app: Arc<App>, req: Request) -> Reply {
    let action = req.arg(0).unwrap_or("").to_string();
    tokio::task::spawn_blocking(move || {
        if ALLOWED_ESP_ACTIONS.contains(&action.as_str()) {
            if app.machine.action(&app, &action) {
                Reply::base().dumps(json!({"action": action, "status": "ok"}))
            } else {
                Reply::base()
                    .status(409)
                    .dumps(json!({"status": "error", "action": action}))
            }
        } else if action == "reset" {
            app.machine.reset();
            Reply::base().dumps(json!({"action": action, "status": "ok"}))
        } else if action == "abort" {
            app.machine.end_profile(&app);
            Reply::base()
        } else {
            let allowed: Vec<&str> = ALLOWED_BACKEND_ACTIONS
                .iter()
                .chain(ALLOWED_ESP_ACTIONS.iter())
                .copied()
                .collect();
            Reply::base()
                .status(400)
                .dumps(json!({"status": "error", "action": action, "allowed_actions": allowed}))
        }
    })
    .await
    .unwrap_or_else(|_| Reply::base().status(500))
}
