//! `api/pour_over_history.py`: Pour Over sessions recorded by the Dial.

use std::sync::Arc;

use serde_json::json;

use history::Error;
use history::pour_over::{
    MAX_POUR_OVER_RECORD_BYTES, PourOverFile, PourOverSession, read_compressed_record,
    resolve_pour_over_file,
};

use super::history::py_int;
use crate::app::App;
use crate::http::{Kind, Reply, Request, Route};

pub fn routes() -> Vec<Route> {
    let m = "api.pour_over_history";
    vec![
        Route::new(
            "/history/pour-over",
            Kind::Base,
            m,
            "PourOverHistoryHandler",
        )
        .get(search)
        .post(save),
        Route::new(
            "/history/pour-over/last",
            Kind::Base,
            m,
            "LastPourOverHandler",
        )
        .get(last),
        Route::new(
            "/history/pour-over/files",
            Kind::Redirect("/api/v1/history/pour-over/files/".into()),
            m,
            "RedirectHandler",
        ),
        Route::new(
            "/history/pour-over/files/(.*)",
            Kind::Base,
            m,
            "PourOverFileHandler",
        )
        .get(file),
    ]
}

fn error(status: u16, message: &str) -> Reply {
    Reply::base()
        .status(status)
        .json(json!({"status": "error", "error": message}))
}

/// `post`: only the Dial writes history, so writes are local only.
async fn save(app: Arc<App>, req: Request) -> Reply {
    if let Some(refused) = super::local_only(&req) {
        return refused;
    }
    if req.body.len() > MAX_POUR_OVER_RECORD_BYTES {
        return error(413, "Pour Over record is too large");
    }
    let Ok(raw) = req.json() else {
        return error(400, "Invalid JSON");
    };
    let session = match PourOverSession::validate(&raw) {
        Ok(session) => session,
        Err(validation) => {
            return Reply::base().status(422).json(json!({
                "status": "error",
                "error": "Invalid Pour Over record",
                "details": validation.to_json(false),
            }));
        }
    };
    let result = tokio::task::spawn_blocking(move || {
        app.history
            .db()
            .map_err(Error::Storage)
            .and_then(|db| db.save_pour_over(&session))
    })
    .await;
    match result {
        Ok(Ok(saved)) => Reply::base()
            .status(if saved.created { 201 } else { 200 })
            .json(json!({
                "status": if saved.created { "created" } else { "existing" },
                "history": saved.history,
            })),
        Ok(Err(Error::PourOverTooLarge(_))) => error(413, "Pour Over record is too large"),
        Ok(Err(Error::PourOverConflict)) => {
            error(409, "A different Pour Over record already uses this id")
        }
        Ok(Err(failure)) => {
            tracing::error!("Failed to save Pour Over history: {failure}");
            error(500, "Could not save Pour Over record")
        }
        Err(_) => error(500, "Could not save Pour Over record"),
    }
}

async fn search(app: Arc<App>, req: Request) -> Reply {
    let Some(max_results) = py_int(&req.get_query_argument("max_results").unwrap_or("50".into()))
    else {
        return error(400, "max_results must be an integer");
    };
    let max_results = max_results.clamp(1, 200);
    let descending = req.get_query_argument("sort").as_deref() != Some("asc");
    let after = req.get_query_argument("after");
    let mode = req.get_query_argument("mode");
    if !matches!(mode.as_deref(), None | Some("free_pour") | Some("profile")) {
        return error(400, "Invalid Pour Over mode");
    }
    let result = tokio::task::spawn_blocking(move || {
        app.history.db().and_then(|db| {
            db.search_pour_over(max_results, descending, after.as_deref(), mode.as_deref())
                .map_err(|e| e.to_string())
        })
    })
    .await;
    match result {
        Ok(Ok(history)) => Reply::base().json(json!({"history": history})),
        _ => Reply::error(Kind::Base, 500, &req),
    }
}

async fn last(app: Arc<App>, req: Request) -> Reply {
    let result = tokio::task::spawn_blocking(move || {
        app.history
            .db()
            .and_then(|db| db.latest_pour_over().map_err(|e| e.to_string()))
    })
    .await;
    match result {
        Ok(Ok(Some(history))) => Reply::base().json(history),
        Ok(Ok(None)) => error(404, "No Pour Over records found"),
        _ => Reply::error(Kind::Base, 500, &req),
    }
}

async fn file(app: Arc<App>, req: Request) -> Reply {
    let relative = req.arg(0).unwrap_or("").to_string();
    let root = app.env.paths.pour_over_history();
    let worker_relative = relative.clone();
    let result = tokio::task::spawn_blocking(move || {
        let found = resolve_pour_over_file(&root, &worker_relative)?;
        match found {
            PourOverFile::Record(path) => {
                let raw = read_compressed_record(&path);
                Ok((PourOverFile::Record(path), Some(raw)))
            }
            other => Ok::<_, Error>((other, None)),
        }
    })
    .await;
    match result {
        Ok(Ok((PourOverFile::Outside, _))) => error(404, "history entry not found"),
        Ok(Ok((PourOverFile::NotFound, _))) => Reply::base().status(404).json(json!({
            "status": "error",
            "error": "history entry not found",
            "path": relative,
        })),
        Ok(Ok((PourOverFile::Directory(entries), _))) => Reply::base().json(
            entries
                .iter()
                .map(|e| e.to_json())
                .collect::<Vec<_>>()
                .into(),
        ),
        Ok(Ok((PourOverFile::Record(path), Some(raw)))) => match raw {
            Ok(raw) => {
                let mut reply = Reply::base();
                reply.set_header("Content-Type", "application/json");
                reply.write_bytes(&raw);
                reply
            }
            Err(_) => {
                tracing::warn!(
                    "Invalid compressed Pour Over history file: {}",
                    path.display()
                );
                error(500, "Invalid history entry")
            }
        },
        _ => Reply::error(Kind::Base, 500, &req),
    }
}
