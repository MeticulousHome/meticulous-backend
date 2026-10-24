//! `api/smoke_validation.py`.

use std::sync::Arc;

use serde_json::json;

use crate::api::local_only;
use crate::app::App;
use crate::http::{Kind, Reply, Request, Route};
use crate::smoke_validation::MachineSnapshot;

pub fn routes() -> Vec<Route> {
    vec![
        Route::new(
            "/smoke-validation",
            Kind::Base,
            "api.smoke_validation",
            "SmokeValidationHandler",
        )
        .prepare(local_only)
        .get(get),
    ]
}

/// `SmokeValidationHandler.get` (a `LocalAccessHandler`).
async fn get(app: Arc<App>, req: Request) -> Reply {
    let status = {
        let app = app.clone();
        tokio::task::spawn_blocking(move || {
            let machine = MachineSnapshot::of(&app.machine);
            app.smoke.status(&machine)
        })
        .await
    };
    match status {
        Ok(Ok(status)) => Reply::base().json(status),
        Ok(Err(error)) => {
            tracing::warn!("Failed to build smoke validation status: {error}");
            Reply::base()
                .status(500)
                .json(json!({"status": "error", "error": error}))
        }
        Err(_) => Reply::error(Kind::Base, 500, &req),
    }
}
