//! `api/pour_over_profiles.py`.

use std::sync::Arc;

use serde_json::json;

use crate::app::App;
use crate::http::{Kind, Reply, Request, Route};
use crate::pour_over::error_reply;

pub fn routes() -> Vec<Route> {
    let m = "api.pour_over_profiles";
    vec![
        Route::new(
            "/pour-over/profile/list",
            Kind::Base,
            m,
            "ListPourOverProfilesHandler",
        )
        .get(list),
        Route::new(
            "/pour-over/profile/get/([0-9a-fA-F-]+)",
            Kind::Base,
            m,
            "GetPourOverProfileHandler",
        )
        .get(get),
        Route::new(
            "/pour-over/profile/image/([0-9a-fA-F-]+)",
            Kind::Base,
            m,
            "GetPourOverProfileImageHandler",
        )
        .get(image),
        Route::new(
            "/pour-over/profile/delete/([0-9a-fA-F-]+)",
            Kind::Base,
            m,
            "DeletePourOverProfileHandler",
        )
        .delete(delete),
        Route::new(
            "/pour-over/profile/schema",
            Kind::Base,
            m,
            "PourOverProfileSchemaHandler",
        )
        .get(schema),
    ]
}

fn not_found(what: &str) -> Reply {
    Reply::base()
        .status(404)
        .json(json!({"status": "error", "error": what}))
}

async fn list(app: Arc<App>, _req: Request) -> Reply {
    match app.pour_over.list_profiles() {
        Ok(profiles) => Reply::base().json(json!({"profiles": profiles})),
        Err(error) => error_reply(&error),
    }
}

async fn get(app: Arc<App>, req: Request) -> Reply {
    let id = req.arg(0).unwrap_or("").to_string();
    let include_image = req.get_argument_or("include_image", "true").to_lowercase() == "true";
    match tokio::task::spawn_blocking(move || app.pour_over.get_profile(&id, include_image)).await {
        Ok(Ok(Some(profile))) => Reply::base().json(profile),
        Ok(Ok(None)) => not_found("Pour Over profile not found"),
        Ok(Err(error)) => error_reply(&error),
        Err(_) => Reply::error(Kind::Base, 500, &req),
    }
}

async fn image(app: Arc<App>, req: Request) -> Reply {
    let id = req.arg(0).unwrap_or("").to_string();
    match tokio::task::spawn_blocking(move || app.pour_over.get_profile_image(&id)).await {
        Ok(Ok(Some(bytes))) => {
            let mut reply = Reply::base();
            reply.set_header("Content-Type", "image/jpeg");
            reply.set_header("Cache-Control", "private, max-age=300");
            reply.write_bytes(&bytes);
            reply
        }
        Ok(Ok(None)) => not_found("Pour Over profile image not found"),
        Ok(Err(error)) => error_reply(&error),
        Err(_) => Reply::error(Kind::Base, 500, &req),
    }
}

async fn delete(app: Arc<App>, req: Request) -> Reply {
    let id = req.arg(0).unwrap_or("").to_string();
    let change_id = req.header("X-Change-Id").map(str::to_string);
    let worker = app.clone();
    match tokio::task::spawn_blocking(move || {
        worker.pour_over.delete_profile(&worker, &id, change_id)
    })
    .await
    {
        Ok(Ok(Some(result))) => Reply::base().json(result),
        Ok(Ok(None)) => not_found("Pour Over profile not found"),
        Ok(Err(error)) => error_reply(&error),
        Err(_) => Reply::error(Kind::Base, 500, &req),
    }
}

async fn schema(app: Arc<App>, _req: Request) -> Reply {
    match app.pour_over.schema_text() {
        Ok(text) => {
            let mut reply = Reply::base();
            reply.set_header("Content-Type", "application/schema+json");
            reply.write_str(&text);
            reply
        }
        Err(error) => error_reply(&error),
    }
}
