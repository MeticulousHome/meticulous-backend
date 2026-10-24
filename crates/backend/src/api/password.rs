//! `api/password_handler.py`.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::api::local_only;
use crate::app::App;
use crate::config::keys;
use crate::http::{Kind, Reply, Request, Route};

pub fn routes() -> Vec<Route> {
    vec![
        Route::new(
            "/machine/root-password",
            Kind::Base,
            "api.password_handler",
            "RootPasswordHandler",
        )
        .get(get),
    ]
}

async fn get(app: Arc<App>, req: Request) -> Reply {
    if let Some(refused) = local_only(&req) {
        return refused;
    }
    let password = match app.config.get(keys::SYSTEM, keys::ROOT_PASSWORD) {
        Value::Null => Value::String("root".into()),
        other => other,
    };
    Reply::base().json(json!({"status": "success", "root_password": password}))
}
