//! `api/history.py`: the shot history, its files and the debug shots.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value, json};

use history::SearchParams;
use history::files::{self, HistoryFile};
use history::search::{SearchError as ErrorDetail, ValidationError};

use crate::app::App;
use crate::config::keys;
use crate::http::{Kind, Reply, Request, Route, serve_file};

const LAST_VERSION_PATH: &str = "/api/v1";

pub fn routes() -> Vec<Route> {
    let m = "api.history";
    vec![
        Route::new("/history/search", Kind::Base, m, "ProfileSearchHandler").get(profile_search),
        Route::new("/history/current", Kind::Base, m, "CurrentShotHandler").get(current),
        Route::new("/history/last", Kind::Base, m, "LastShotHandler").get(last),
        Route::new(
            "/history/upload-index",
            Kind::Base,
            m,
            "UploadHistoryIndexHandler",
        )
        .prepare(super::local_only)
        .get(upload_index),
        Route::new("/history/stats", Kind::Base, m, "StatisticsHandler").get(stats),
        Route::new("/history", Kind::Base, m, "HistoryHandler")
            .get(search_get)
            .post(search_post),
        Route::new(
            "/history/last-debug-file",
            Kind::Base,
            m,
            "LastDebugFileHandler",
        )
        .get(last_debug_file),
        Route::new("/history/rating/(.*)", Kind::Base, m, "ShotRatingHandler")
            .get(rating_get)
            .post(rating_post),
        Route::new(
            "/history/debug",
            Kind::Redirect(format!("{LAST_VERSION_PATH}/history/debug/")),
            m,
            "RedirectHandler",
        ),
        Route::new(
            "/history/debug.zip",
            Kind::Base,
            m,
            "CompressedDebugHistoryHandler",
        )
        .get(debug_zip),
        Route::new("/history/debug/(.*)", Kind::Base, m, "ZstdHistoryHandler").get(debug_file),
        Route::new(
            "/history/files",
            Kind::Redirect(format!("{LAST_VERSION_PATH}/history/files/")),
            m,
            "RedirectHandler",
        ),
        Route::new("/history/files/(.*)", Kind::Base, m, "ZstdHistoryHandler").get(shot_file),
    ]
}

/// `int(text)` for a query argument: sign, digits and single underscores.
pub fn py_int(text: &str) -> Option<i64> {
    let text = text.trim();
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    let valid = !digits.is_empty()
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__")
        && digits.chars().all(|c| c.is_ascii_digit() || c == '_');
    if !valid {
        return None;
    }
    text.replace('_', "").parse().ok()
}

/// `self.redirect(url)`: a temporary redirect from a `BaseHandler`.
fn found(url: &str) -> Reply {
    Reply::base().status(302).with_header("Location", url)
}

fn internal_error() -> Reply {
    Reply::base()
        .status(500)
        .json(json!({"status": "error", "error": "Internal server error"}))
}

async fn profile_search(app: Arc<App>, req: Request) -> Reply {
    let query = req.get_argument_or("query", "");
    match app.history.db().and_then(|db| {
        db.autocomplete_profile_name(&query)
            .map_err(|e| e.to_string())
    }) {
        Ok(profiles) => Reply::base().json(json!({"profiles": profiles})),
        Err(error) => {
            tracing::error!("Profile name search failed: {error}");
            Reply::error(Kind::Base, 500, &req)
        }
    }
}

async fn current(app: Arc<App>, _req: Request) -> Reply {
    Reply::base().dumps(app.shots.current_shot())
}

async fn last(app: Arc<App>, req: Request) -> Reply {
    let worker = app.clone();
    match tokio::task::spawn_blocking(move || worker.shots.last_shot(&worker)).await {
        Ok(last) => Reply::base().dumps(last),
        Err(_) => Reply::error(Kind::Base, 500, &req),
    }
}

async fn upload_index(app: Arc<App>, req: Request) -> Reply {
    let Some(max_results) = py_int(
        &req.get_query_argument("max_results")
            .unwrap_or("200".into()),
    ) else {
        return Reply::base()
            .status(400)
            .json(json!({"status": "error", "error": "max_results must be an integer"}));
    };
    let max_results = max_results.clamp(1, 200);
    let after = req.get_query_argument("after");
    let result = tokio::task::spawn_blocking(move || {
        app.history.db().and_then(|db| {
            db.list_history_files(after.as_deref(), max_results)
                .map_err(|e| e.to_string())
        })
    })
    .await;
    match result {
        Ok(Ok(history)) => Reply::base().json(json!({"history": history})),
        _ => Reply::error(Kind::Base, 500, &req),
    }
}

async fn stats(app: Arc<App>, req: Request) -> Reply {
    match app.history.statistics() {
        Ok(Value::Object(mut results)) => {
            // The ESP's lifetime shot count, not reduced when shots are
            // deleted; None until the ESP has reported it.
            results.insert("mileage".into(), json!(app.machine.mileage()));
            Reply::base().json(Value::Object(results))
        }
        Ok(_) | Err(_) => Reply::error(Kind::Base, 500, &req),
    }
}

/// `self.write({"history": ShotDataBase.search_history(params)})`, with the
/// shot samples transcoded from the files rather than parsed and encoded.
async fn search(app: Arc<App>, req: &Request, params: SearchParams) -> Reply {
    let result = tokio::task::spawn_blocking(move || {
        let mut body = String::from("{\"history\": ");
        app.history
            .search_json(&params, pyfmt::JsonStyle::TORNADO, &mut body)
            .map(|()| {
                body.push('}');
                body
            })
    })
    .await;
    match result {
        Ok(Ok(body)) => Reply::base().json_encoded(body),
        _ => Reply::error(Kind::Base, 500, req),
    }
}

async fn search_post(app: Arc<App>, req: Request) -> Reply {
    let data = match req.json() {
        Ok(data) => data,
        Err(error) => {
            return Reply::base()
                .status(400)
                .json(json!({"error": "Invalid JSON", "details": error.message}));
        }
    };
    // Deliberate fix: `SearchParams(**data)` raised an uncaught TypeError (a
    // 500) for a body that is not an object; it is refused like
    // `SearchParams.model_validate` would.
    let Value::Object(data) = data else {
        let detail = ErrorDetail::new(
            "model_type",
            &[],
            "Input should be a valid dictionary or instance of SearchParams",
            &data,
        )
        .with_ctx(json!({"class_name": "SearchParams"}));
        return invalid_search(&ValidationError {
            errors: vec![detail],
        });
    };
    match SearchParams::from_json(&data) {
        Ok(params) => search(app, &req, params).await,
        Err(error) => invalid_search(&error),
    }
}

/// `self.write(e.json())`: pydantic's compact rendering, as a string.
fn invalid_search(error: &ValidationError) -> Reply {
    let mut reply = Reply::base().status(422);
    reply.write_str(&serde_json::to_string(&error.to_json(true)).unwrap_or_default());
    reply
}

async fn search_get(app: Arc<App>, req: Request) -> Reply {
    let optional = |name: &str| {
        req.get_query_argument(name)
            .map_or(Value::Null, Value::String)
    };
    let list = |name: &str| {
        Value::Array(
            req.get_query_arguments(name)
                .into_iter()
                .map(Value::String)
                .collect(),
        )
    };
    let mut data = Map::new();
    data.insert("query".into(), optional("query"));
    data.insert("ids".into(), list("ids"));
    data.insert("start_date".into(), optional("start_date"));
    data.insert("end_date".into(), optional("end_date"));
    // PARITY: `get_query_arguments("order_by", [SearchOrderBy.date])` passes
    // the default as `strip`, so no order_by is an empty list.
    data.insert("order_by".into(), list("order_by"));
    data.insert(
        "sort".into(),
        req.get_query_argument("sort")
            .map_or(json!("desc"), Value::String),
    );
    data.insert(
        "max_results".into(),
        req.get_query_argument("max_results")
            .map_or(json!(20), Value::String),
    );
    data.insert(
        "dump_data".into(),
        req.get_query_argument("dump_data")
            .map_or(json!(true), Value::String),
    );
    match SearchParams::from_json(&data) {
        Ok(params) => search(app, &req, params).await,
        // Deliberate fix: the GET handler did not catch the ValidationError
        // (a 500); it answers 422 like the POST search.
        Err(error) => invalid_search(&error),
    }
}

async fn last_debug_file(app: Arc<App>, _req: Request) -> Reply {
    let debug_dir = app.env.paths.debug_history.clone();
    let latest = tokio::task::spawn_blocking(move || {
        files::latest_debug_file(&debug_dir).and_then(|path| {
            path.strip_prefix(&debug_dir)
                .ok()
                .map(|p| p.display().to_string())
        })
    })
    .await;
    match latest {
        Ok(Some(relative)) => found(&format!("{LAST_VERSION_PATH}/history/debug/{relative}")),
        Ok(None) => Reply::base()
            .status(404)
            .json(json!({"status": "error", "error": "No debug files found"})),
        Err(_) => {
            tracing::error!("Unexpected error getting last debug file: JoinError");
            Reply::base()
                .status(500)
                .json(json!({"status": "error", "error": "Internal server error"}))
        }
    }
}

async fn rating_get(app: Arc<App>, req: Request) -> Reply {
    let shot_id = req.arg(0).unwrap_or("").to_string();
    match app.history.db() {
        Ok(db) => {
            let rating = db.get_shot_rating(&shot_id);
            Reply::base().json(json!({"shot_id": shot_id, "rating": rating}))
        }
        Err(error) => {
            tracing::error!("Error getting shot rating: {error}");
            internal_error()
        }
    }
}

async fn rating_post(app: Arc<App>, req: Request) -> Reply {
    let shot_id = req.arg(0).unwrap_or("").to_string();
    if shot_id.is_empty() {
        return Reply::base()
            .status(400)
            .json(json!({"status": "error", "error": "Invalid shot ID"}));
    }
    let data = match req.json() {
        Ok(data) => data,
        Err(_) => {
            return Reply::base()
                .status(400)
                .json(json!({"status": "error", "error": "Invalid JSON"}));
        }
    };
    // Deliberate fix: `data.get(...)` raised on a body that is not an object
    // (a 500); it is an invalid rating.
    let Value::Object(data) = data else {
        return Reply::base().status(400).json(json!({
            "status": "error",
            "error": "Invalid rating value. Use 'like', 'dislike', or null",
        }));
    };
    let rating = data.get("rating").cloned().unwrap_or(Value::Null);
    let rating = match &rating {
        Value::Null => None,
        Value::String(s) if s == "like" || s == "dislike" => Some(s.clone()),
        _ => {
            return Reply::base().status(400).json(json!({
                "status": "error",
                "error": "Invalid rating value. Use 'like', 'dislike', or null",
            }));
        }
    };
    let db = match app.history.db() {
        Ok(db) => db,
        Err(error) => {
            tracing::error!("Error updating shot rating: {error}");
            return internal_error();
        }
    };
    if db.rate_shot(&shot_id, rating.as_deref()) {
        Reply::base().json(json!({"status": "ok", "shot_id": shot_id, "rating": rating}))
    } else {
        Reply::base()
            .status(404)
            .json(json!({"status": "error", "error": "Shot not found or rating failed"}))
    }
}

async fn debug_zip(app: Arc<App>, _req: Request) -> Reply {
    match tokio::task::spawn_blocking(move || app.debug_shots.zip_all()).await {
        Ok(Ok(name)) => found(&format!("{LAST_VERSION_PATH}/history/debug/{name}")),
        Ok(Err(error)) => {
            tracing::error!("Error compressing debug file: {:?}", error.kind());
            internal_error()
        }
        Err(_) => internal_error(),
    }
}

async fn debug_file(app: Arc<App>, req: Request) -> Reply {
    let root = app.env.paths.debug_history.clone();
    zstd_history(app, req, root).await
}

async fn shot_file(app: Arc<App>, req: Request) -> Reply {
    let root = app.env.paths.shots();
    zstd_history(app, req, root).await
}

/// `"".join(MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER])`.
fn device_identifier(app: &App) -> Option<String> {
    match app.config.get(keys::SYSTEM, keys::DEVICE_IDENTIFIER) {
        Value::String(s) => Some(s),
        Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.concat()),
        _ => None,
    }
}

/// `ZstdHistoryHandler.get`: a directory listing, a shot decompressed, or
/// the file itself (a `.zst` as a download).
async fn zstd_history(app: Arc<App>, req: Request, root: PathBuf) -> Reply {
    let path = req.arg(0).unwrap_or("").to_string();
    let serve_compressed = req.get_query_argument("compressed").is_some();
    let worker_req = req.clone();
    let result = tokio::task::spawn_blocking(move || {
        let req = worker_req;
        let mut reply = Reply::base();
        // compute_etag returns None for directories and `.zst` files.
        reply.disable_etag();
        match files::resolve_history_file(&root, &path, serve_compressed) {
            HistoryFile::Directory(dir) => {
                tracing::info!("Request for a Directory:{}", dir.display());
                match files::list_directory(&dir) {
                    Ok(entries) => {
                        let listing: Vec<Value> = entries.iter().map(|e| e.to_json()).collect();
                        reply.set_header("Content-Type", "application/json");
                        reply.write_dumps(&Value::Array(listing));
                        reply
                    }
                    Err(_) => Reply::error(Kind::Base, 500, &req),
                }
            }
            HistoryFile::Decompressed(file) => {
                tracing::info!("Serving File: {}", file.display());
                match files::read_patched(&file) {
                    Ok(raw) => {
                        reply.set_header("Content-Type", files::decompressed_content_type(&file));
                        reply.write_bytes(&raw);
                        reply
                    }
                    Err(error) => {
                        tracing::error!("Cannot serve {}: {error}", file.display());
                        Reply::error(Kind::Base, 500, &req)
                    }
                }
            }
            HistoryFile::Raw(file) => {
                tracing::info!("File exists on disk");
                let is_zst = file.to_string_lossy().ends_with(".zst");
                let mut content_type = None;
                if is_zst {
                    let Some(device) = device_identifier(&app) else {
                        return Reply::error(Kind::Base, 500, &req);
                    };
                    let name = files::compressed_download_name(&device, &path);
                    reply.set_header(
                        "Content-Disposition",
                        format!("attachment; filename=\"{name}\""),
                    );
                    // PARITY: the handler sets application/octet-stream, then
                    // StaticFileHandler replaces it with mimetypes' guess.
                    content_type = Some("application/zstd");
                }
                serve_file(&file, &req, reply, !is_zst, content_type)
            }
            HistoryFile::NotFound => reply.status(404).json(json!({
                "status": "error",
                "error": "history entry not found",
                "path": path,
            })),
            HistoryFile::Forbidden => Reply::error(Kind::Base, 403, &req),
        }
    })
    .await;
    result.unwrap_or_else(|_| Reply::error(Kind::Base, 500, &req))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, call, call_with_headers, json as body};

    /// `record_shot`: the shot file, then the history row, the way
    /// `ShotManager.stop()` writes them.
    fn record_shot(app: &App, id: &str, start: f64, profile_name: &str) -> String {
        let db = app.history.db().expect("db");
        let (_, file) = files::shot_file_paths(db.zone(), start).expect("paths");
        let profile = json!({
            "id": format!("p-{profile_name}"),
            "author": "Meticulous",
            "author_id": "author-0001",
            "display": {"accentColor": "#FF7A00"},
            "final_weight": 36.0,
            "last_changed": 1760000000.0,
            "name": profile_name,
            "temperature": 93.0,
            "stages": [{"key": "s-0", "name": "Extraction", "type": "pressure"}],
            "variables": [],
            "previous_authors": [],
        });
        let mut shot = json!({
            "time": start,
            "profile_name": profile_name,
            "data": [{"shot": {"pressure": 9.0, "flow": 2.0, "weight": 1.0,
                     "gravimetric_flow": 1.5, "setpoints": {"active": "pressure", "pressure": 9.0}},
                     "time": 1000, "profile_time": 800, "status": "brewing"}],
            "id": id,
            "push_to_brew_time": 0,
            "profile": profile,
        });
        files::write_shot_file(&db.paths().shot_dir().join(&file), &shot).expect("write");
        shot["file"] = json!(file);
        db.insert_history(&shot).expect("insert");
        file
    }

    async fn get(app: &Arc<App>, uri: &str) -> Reply {
        call(app, http::Method::GET, uri, "").await
    }

    async fn post(app: &Arc<App>, uri: &str, body: &str) -> Reply {
        call(app, http::Method::POST, uri, body.to_string()).await
    }

    /// tests/test_api_history.py: every listing empty on a fresh machine.
    #[tokio::test]
    async fn fresh_machine_has_no_history() {
        let (_dir, app) = app();
        let reply = get(&app, "/api/v1/history").await;
        assert_eq!(reply.status_code(), 200);
        assert_eq!(body(&reply), json!({"history": []}));
        let reply = post(&app, "/api/v1/history", "{}").await;
        assert_eq!(body(&reply), json!({"history": []}));
        assert_eq!(get(&app, "/api/v1/history/current").await.body(), b"null");
        assert_eq!(get(&app, "/api/v1/history/last").await.body(), b"null");
        let reply = get(&app, "/api/v1/history/stats").await;
        assert_eq!(
            body(&reply),
            json!({"totalSavedShots": 0, "byProfile": [], "mileage": null})
        );
        let reply = get(&app, "/api/v1/history/upload-index").await;
        assert_eq!(body(&reply), json!({"history": []}));
        let reply = get(&app, "/api/v1/history/search").await;
        assert_eq!(body(&reply), json!({"profiles": []}));
    }

    /// TestHistorySearchPost: the 400 and 422 answers.
    #[tokio::test]
    async fn search_errors() {
        let (_dir, app) = app();
        let reply = post(&app, "/api/v1/history", "{not json").await;
        assert_eq!(reply.status_code(), 400);
        assert_eq!(
            body(&reply),
            json!({"error": "Invalid JSON", "details": "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"})
        );
        let reply = post(&app, "/api/v1/history", "").await;
        assert_eq!(reply.status_code(), 400);
        let reply = post(
            &app,
            "/api/v1/history",
            r#"{"sort": "sideways", "max_results": "x"}"#,
        )
        .await;
        assert_eq!(reply.status_code(), 422);
        let errors: Vec<(Value, Value)> = body(&reply)
            .as_array()
            .expect("list")
            .iter()
            .map(|e| (e["loc"].clone(), e["type"].clone()))
            .collect();
        assert_eq!(
            errors,
            vec![
                (json!(["sort"]), json!("enum")),
                (json!(["max_results"]), json!("int_parsing"))
            ]
        );
        // The strict xfails of the Python suite, fixed here.
        assert_eq!(post(&app, "/api/v1/history", "[]").await.status_code(), 422);
        assert_eq!(
            get(&app, "/api/v1/history?max_results=many")
                .await
                .status_code(),
            422
        );
        assert_eq!(
            get(&app, "/api/v1/history?sort=sideways")
                .await
                .status_code(),
            422
        );
    }

    /// Shots recorded, then listed, rated and served.
    #[tokio::test]
    async fn recorded_shots_are_listed_rated_and_served() {
        let (_dir, app) = app();
        let first = record_shot(&app, "shot-1", 1791534600.0, "Classic");
        record_shot(&app, "shot-2", 1791534660.0, "Bloom");

        let reply = get(&app, "/api/v1/history?dump_data=false").await;
        let ids: Vec<Value> = body(&reply)["history"]
            .as_array()
            .expect("list")
            .iter()
            .map(|e| e["id"].clone())
            .collect();
        assert_eq!(ids, vec![json!("shot-2"), json!("shot-1")]);
        let last = body(&get(&app, "/api/v1/history/last").await);
        assert_eq!(last["id"], json!("shot-2"));
        let stats = body(&get(&app, "/api/v1/history/stats").await);
        assert_eq!(stats["totalSavedShots"], json!(2));

        let reply = get(&app, "/api/v1/history/rating/shot-1").await;
        assert_eq!(body(&reply), json!({"shot_id": "shot-1", "rating": null}));
        let reply = post(
            &app,
            "/api/v1/history/rating/shot-1",
            r#"{"rating": "like"}"#,
        )
        .await;
        assert_eq!(
            body(&reply),
            json!({"status": "ok", "shot_id": "shot-1", "rating": "like"})
        );
        let reply = get(&app, "/api/v1/history/rating/shot-1").await;
        assert_eq!(body(&reply)["rating"], json!("like"));
        let reply = post(
            &app,
            "/api/v1/history/rating/unknown",
            r#"{"rating": "like"}"#,
        )
        .await;
        assert_eq!(reply.status_code(), 404);
        let reply = post(
            &app,
            "/api/v1/history/rating/shot-1",
            r#"{"rating": "meh"}"#,
        )
        .await;
        assert_eq!(reply.status_code(), 400);
        let reply = post(&app, "/api/v1/history/rating/shot-1", "{not json").await;
        assert_eq!(
            body(&reply),
            json!({"status": "error", "error": "Invalid JSON"})
        );
        let reply = post(&app, "/api/v1/history/rating/shot-1", r#"["like"]"#).await;
        assert_eq!(reply.status_code(), 400);
        let reply = post(&app, "/api/v1/history/rating/", r#"{"rating": "like"}"#).await;
        assert_eq!(
            body(&reply),
            json!({"status": "error", "error": "Invalid shot ID"})
        );

        let reply = get(&app, "/api/v1/history/files/").await;
        assert_eq!(
            body(&reply),
            json!([{"name": first.split("/").next(), "url": first.split("/").next()}])
        );
        assert!(reply.header("Etag").is_none());
        let reply = get(&app, &format!("/api/v1/history/files/{first}")).await;
        assert_eq!(reply.header("Content-Type"), Some("application/json"));
        assert_eq!(body(&reply)["id"], json!("shot-1"));
        let without_suffix = first.strip_suffix(".zst").expect("zst");
        let reply = get(&app, &format!("/api/v1/history/files/{without_suffix}")).await;
        assert_eq!(body(&reply)["id"], json!("shot-1"));
        let reply = get(&app, &format!("/api/v1/history/files/{first}?compressed")).await;
        assert_eq!(reply.status_code(), 200);
        assert_eq!(reply.header("Content-Type"), Some("application/zstd"));
        // The machine has no name yet: "".join([]) leads the file name.
        let (day, name) = first.split_once("/").expect("day");
        let expected = format!("attachment; filename=\"_{}_{name}\"", day.replace("-", "_"));
        assert_eq!(reply.header("Content-Disposition"), Some(expected.as_str()));
        assert!(reply.header("Etag").is_none());
        let raw = zstd::decode_all(reply.body()).expect("zstd");
        let shot: Value = serde_json::from_slice(&raw).expect("json");
        assert_eq!(shot["id"], json!("shot-1"));
    }

    /// TestShotFiles and TestDebugFiles on a fresh machine.
    #[tokio::test]
    async fn file_routes_on_a_fresh_machine() {
        let (_dir, app) = app();
        for (route, target) in [
            ("/api/v1/history/files", "/api/v1/history/files/"),
            ("/api/v1/history/debug", "/api/v1/history/debug/"),
            (
                "/api/v1/history/pour-over/files",
                "/api/v1/history/pour-over/files/",
            ),
        ] {
            let reply = get(&app, route).await;
            assert_eq!(reply.status_code(), 301);
            assert_eq!(reply.header("Location"), Some(target));
        }
        let reply = get(&app, "/api/v1/history/files/").await;
        assert_eq!(reply.status_code(), 404);
        assert_eq!(
            body(&reply),
            json!({"status": "error", "error": "history entry not found", "path": ""})
        );
        let reply = get(&app, "/api/v1/history/files/2026-10-09/missing.shot.json").await;
        assert_eq!(body(&reply)["path"], json!("2026-10-09/missing.shot.json"));
        let reply = get(&app, "/api/v1/history/files/..%2F..%2Fetc").await;
        assert_eq!(reply.status_code(), 403);
        let reply = get(&app, "/api/v1/history/last-debug-file").await;
        assert_eq!(reply.status_code(), 404);
        assert_eq!(
            body(&reply),
            json!({"status": "error", "error": "No debug files found"})
        );
        let reply = get(&app, "/api/v1/history/debug.zip").await;
        assert_eq!(reply.status_code(), 302);
        let location = reply.header("Location").expect("location").to_string();
        assert!(location.starts_with("/api/v1/history/debug/"), "{location}");
        let zip = get(&app, &location).await;
        assert_eq!(zip.status_code(), 200);
        assert_eq!(zip.header("Content-Type"), Some("application/zip"));
    }

    /// TestLastDebugFile: the redirect to the newest debug shot.
    #[tokio::test]
    async fn last_debug_file_redirects_to_the_newest() {
        let (_dir, app) = app();
        let dir = app.env.paths.debug_history.join("2026-10-09");
        files::write_compressed(&dir.join("08:30:00.debug.csv.zst"), b"a,b\n1,2\n").expect("write");
        let reply = get(&app, "/api/v1/history/last-debug-file").await;
        assert_eq!(reply.status_code(), 302);
        assert_eq!(
            reply.header("Location"),
            Some("/api/v1/history/debug/2026-10-09/08:30:00.debug.csv.zst")
        );
        let reply = get(
            &app,
            "/api/v1/history/debug/2026-10-09/08:30:00.debug.csv.zst",
        )
        .await;
        assert_eq!(reply.header("Content-Type"), Some("text/csv"));
        assert_eq!(reply.body(), b"a,b\n1,2\n");
    }

    /// TestUploadIndex: local only, max_results an integer.
    #[tokio::test]
    async fn upload_index_rules() {
        let (_dir, app) = app();
        let reply = call_with_headers(
            &app,
            http::Method::GET,
            "/api/v1/history/upload-index",
            "",
            &[("X-Real-IP", "192.168.1.50")],
        )
        .await;
        assert_eq!(reply.status_code(), 403);
        let reply = get(&app, "/api/v1/history/upload-index?max_results=lots").await;
        assert_eq!(reply.status_code(), 400);
        assert_eq!(
            body(&reply),
            json!({"status": "error", "error": "max_results must be an integer"})
        );
    }

    /// tests/test_pour_over_history.py: the HTTP answers of the Dial's
    /// Pour Over history.
    #[tokio::test]
    async fn pour_over_history_routes() {
        let (_dir, app) = app();
        let reply = get(&app, "/api/v1/history/pour-over").await;
        assert_eq!(body(&reply), json!({"history": []}));
        let reply = get(&app, "/api/v1/history/pour-over/last").await;
        assert_eq!(reply.status_code(), 404);
        assert_eq!(
            body(&reply),
            json!({"status": "error", "error": "No Pour Over records found"})
        );
        let reply = get(&app, "/api/v1/history/pour-over?mode=espresso").await;
        assert_eq!(
            body(&reply),
            json!({"status": "error", "error": "Invalid Pour Over mode"})
        );
        let reply = post(&app, "/api/v1/history/pour-over", "{not json").await;
        assert_eq!(reply.status_code(), 400);
        let reply = post(&app, "/api/v1/history/pour-over", "{}").await;
        assert_eq!(reply.status_code(), 422);
        assert_eq!(body(&reply)["error"], json!("Invalid Pour Over record"));
        let reply = call_with_headers(
            &app,
            http::Method::POST,
            "/api/v1/history/pour-over",
            "{}",
            &[("X-Real-IP", "192.168.1.50")],
        )
        .await;
        assert_eq!(reply.status_code(), 403);
        let reply = get(&app, "/api/v1/history/pour-over/files/../history.sqlite").await;
        assert_eq!(
            body(&reply),
            json!({"status": "error", "error": "history entry not found"})
        );
    }

    // -- Shots brewed through the ShotManager, the production write path. --

    const DAY: &str = "2026-10-09";
    const FIRST_SHOT_FILE: &str = "2026-10-09/08:30:00.shot.json.zst";

    /// `datetime(...).timestamp()`: a wall time on this machine.
    fn local(day: u32, hour: u32, minute: u32) -> f64 {
        use chrono::TimeZone;
        chrono::Local
            .with_ymd_and_hms(2026, 10, day, hour, minute, 0)
            .single()
            .expect("unambiguous local time")
            .timestamp() as f64
    }

    fn make_profile(id: &str, name: &str, stages: &[&str], temperature: f64) -> Value {
        let stages: Vec<Value> = stages
            .iter()
            .enumerate()
            .map(|(index, stage)| {
                json!({"key": format!("{id}-stage-{index}"), "name": stage, "type": "pressure"})
            })
            .collect();
        json!({
            "id": id,
            "author": "Meticulous",
            "author_id": "author-0001",
            "display": {"accentColor": "#FF7A00"},
            "final_weight": 36.0,
            "last_changed": 1760000000.0,
            "name": name,
            "temperature": temperature,
            "stages": stages,
            "variables": [],
            "previous_authors": [],
        })
    }

    fn classic() -> Value {
        make_profile("p-classic", "Classic", &["Preinfusion", "Extraction"], 93.0)
    }

    fn classic_v2() -> Value {
        make_profile(
            "p-classic-v2",
            "Classic",
            &["Preinfusion", "Extraction"],
            94.0,
        )
    }

    fn bloom() -> Value {
        make_profile("p-bloom", "Bloom", &["Bloom", "Pour"], 93.0)
    }

    fn brewing_point(profile: &Value, time_ms: i64, pressure: f64, weight: f64) -> esp::ShotData {
        esp::ShotData {
            pressure: esp::PyNum::Float(pressure),
            flow: esp::PyNum::Float(2.0),
            weight: esp::PyNum::Float(weight),
            temperature: esp::PyNum::Float(92.5),
            status: Some("brewing".into()),
            profile: profile["name"].as_str().map(str::to_string),
            time: time_ms,
            profile_time: time_ms - 200,
            state: "brewing".into(),
            is_extracting: true,
            gravimetric_flow: esp::PyNum::Float(1.5),
            main_controller_kind: Some("Pressure".into()),
            main_setpoint: esp::PyNum::Float(9.0),
            ..esp::ShotData::default()
        }
    }

    /// The data point layout `Shot.addShotData` writes and the Dial graphs.
    fn stored_point(time_ms: i64, pressure: f64, weight: f64) -> Value {
        json!({
            "shot": {
                "pressure": pressure,
                "flow": 2.0,
                "weight": weight,
                "gravimetric_flow": 1.5,
                "setpoints": {"active": "pressure", "pressure": 9.0},
            },
            "time": time_ms,
            "profile_time": time_ms - 200,
            "status": "brewing",
        })
    }

    fn saved_shots(app: &App) -> i64 {
        app.history
            .statistics()
            .ok()
            .and_then(|stats| stats["totalSavedShots"].as_i64())
            .unwrap_or(0)
    }

    /// Brew a shot through the ShotManager as the machine does and return
    /// its id, once it is saved.
    async fn brew(
        app: &Arc<App>,
        profile: &Value,
        start: f64,
        points: &[esp::ShotData],
        push_to_brew_time: i64,
    ) -> String {
        app.config.set(
            keys::PROFILES,
            keys::PROFILE_LAST,
            json!({"load_time": start, "profile": profile}),
        );
        let saved = saved_shots(app);
        app.shots.start(push_to_brew_time);
        // The shot file name is derived from the start time: pin it.
        app.shots.set_current_start_time(start);
        let id = app.shots.current_shot()["id"]
            .as_str()
            .expect("id")
            .to_string();
        for point in points {
            app.shots.handle_shot_data(app, point);
        }
        app.shots.stop(app);
        for _ in 0..1000 {
            if saved_shots(app) > saved {
                return id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the shot was not saved");
    }

    async fn brew_three_shots(app: &Arc<App>) -> (String, String, String) {
        let classic = classic();
        let first = brew(
            app,
            &classic,
            local(9, 8, 30),
            &[
                brewing_point(&classic, 1000, 9.0, 0.0),
                brewing_point(&classic, 1100, 8.5, 0.4),
            ],
            1500,
        )
        .await;
        let bloom = bloom();
        let second = brew(
            app,
            &bloom,
            local(9, 8, 31),
            &[brewing_point(&bloom, 1000, 2.0, 1.0)],
            0,
        )
        .await;
        let classic_v2 = classic_v2();
        let third = brew(
            app,
            &classic_v2,
            local(9, 8, 32),
            &[brewing_point(&classic_v2, 1000, 9.1, 0.0)],
            0,
        )
        .await;
        (first, second, third)
    }

    /// tests/test_api_history.py::TestCurrentShot::test_current_shot_reports_live_data_in_history_entry_shape
    #[tokio::test]
    async fn current_shot_reports_live_data_in_history_entry_shape() {
        let (_dir, app) = app();
        let first_shot_time = local(9, 8, 30);
        app.config.set(
            keys::PROFILES,
            keys::PROFILE_LAST,
            json!({"load_time": first_shot_time, "profile": classic()}),
        );
        app.shots.start(1200);
        app.shots.set_current_start_time(first_shot_time);
        app.shots
            .handle_shot_data(&app, &brewing_point(&classic(), 1000, 9.0, 0.0));
        let shot_id = app.shots.current_shot()["id"].clone();

        let reply = get(&app, "/api/v1/history/current").await;

        assert_eq!(reply.status_code(), 200);
        let mut profile = json!({"db_key": null});
        for (key, value) in classic().as_object().expect("profile") {
            profile[key] = value.clone();
        }
        assert_eq!(
            body(&reply),
            json!({
                "db_key": null,
                "file": FIRST_SHOT_FILE,
                "time": first_shot_time,
                "profile_name": "Classic",
                "data": [stored_point(1000, 9.0, 0.0)],
                "id": shot_id,
                "push_to_brew_time": 1200,
                "profile": profile,
            })
        );
    }

    /// tests/test_api_history.py::TestCurrentShot::test_current_shot_without_known_profile_reports_only_db_key
    #[tokio::test]
    async fn current_shot_without_known_profile_reports_only_db_key() {
        let (_dir, app) = app();
        app.shots.start(0);
        app.shots.set_current_start_time(local(9, 8, 30));
        app.shots
            .handle_shot_data(&app, &brewing_point(&classic(), 1000, 9.0, 0.0));

        let current = body(&get(&app, "/api/v1/history/current").await);

        assert_eq!(current["profile"], json!({"db_key": null}));
        assert_eq!(current["profile_name"], json!("Classic"));
    }

    /// The search answers with the shots' samples transcoded from their
    /// files: the same bytes as tornado encoding `search_history`'s values.
    #[tokio::test]
    async fn search_body_is_tornados_encoding_of_the_shots() {
        let (_dir, app) = app();
        brew_three_shots(&app).await;
        let params = SearchParams::from_json(&Map::new()).expect("defaults");
        let shots = app.history.search(&params).expect("search");
        assert_eq!(shots.len(), 3);
        let expected = pyfmt::tornado_json_encode(&json!({"history": shots}));

        let reply = post(&app, "/api/v1/history", "{}").await;

        assert_eq!(reply.status_code(), 200);
        assert_eq!(
            reply.header("Content-Type"),
            Some("application/json; charset=UTF-8")
        );
        assert_eq!(std::str::from_utf8(reply.body()), Ok(expected.as_str()));
    }

    /// tests/test_api_history.py::TestStatistics::test_counts_shots_and_profile_versions_and_reports_esp_mileage
    #[tokio::test]
    async fn counts_shots_and_profile_versions_and_reports_esp_mileage() {
        let (_dir, app) = app();
        brew_three_shots(&app).await;
        let classic = classic();
        brew(
            &app,
            &classic,
            local(9, 9, 0),
            &[brewing_point(&classic, 1, 1.0, 0.0)],
            0,
        )
        .await;
        // `Machine.mileage`: the ESP's lifetime shot count.
        app.machine.state.lock().mileage = Some(1234);

        let stats = body(&get(&app, "/api/v1/history/stats").await);

        assert_eq!(
            stats,
            json!({
                "totalSavedShots": 4,
                "byProfile": [
                    {"name": "Classic", "count": 3, "profileVersions": 2},
                    {"name": "Bloom", "count": 1, "profileVersions": 1},
                ],
                "mileage": 1234,
            })
        );
    }

    /// tests/test_api_history.py::TestShotFiles::test_compressed_query_serves_raw_file_as_named_attachment
    #[tokio::test]
    async fn compressed_query_serves_raw_file_as_named_attachment() {
        let (_dir, app) = app();
        app.config.set(
            keys::SYSTEM,
            keys::DEVICE_IDENTIFIER,
            json!(["Meticulous42"]),
        );
        brew_three_shots(&app).await;
        let on_disk = std::fs::read(app.env.paths.shots().join(FIRST_SHOT_FILE)).expect("shot");

        let reply = get(
            &app,
            &format!("/api/v1/history/files/{FIRST_SHOT_FILE}?compressed"),
        )
        .await;

        assert_eq!(reply.status_code(), 200);
        assert_eq!(reply.body(), on_disk.as_slice());
        // StaticFileHandler replaces the handler's octet-stream with the
        // file's mimetype.
        assert_eq!(reply.header("Content-Type"), Some("application/zstd"));
        assert_eq!(
            reply.header("Content-Disposition"),
            Some("attachment; filename=\"Meticulous42_2026_10_09_08:30:00.shot.json.zst\"")
        );
        assert!(reply.header("Etag").is_none());
    }

    /// tests/test_api_history.py::TestDebugFiles::test_debug_zip_bundles_all_debug_shots_and_redirects_to_it
    #[tokio::test]
    async fn debug_zip_bundles_all_debug_shots_and_redirects_to_it() {
        let (_dir, app) = app();
        let debug_dir = app.env.paths.debug_history.clone();
        let first = debug_dir.join(format!("{DAY}/08:30:00.shot.json.zst"));
        let second = debug_dir.join("2026-10-08/23:00:00.purge.json.zst");
        files::write_compressed(&first, br#"{"a": 1}"#).expect("first");
        files::write_compressed(&second, br#"{"b": 2}"#).expect("second");
        let stale = "debug-2026-10-01-10:00:00.zip";
        std::fs::write(debug_dir.join(stale), b"old").expect("stale zip");

        let reply = get(&app, "/api/v1/history/debug.zip").await;

        assert_eq!(reply.status_code(), 302);
        let zips: Vec<String> = std::fs::read_dir(&debug_dir)
            .expect("debug dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".zip"))
            .collect();
        assert_eq!(zips.len(), 1, "{zips:?}");
        let zip_name = &zips[0];
        assert!(zip_name.starts_with("debug-") && zip_name != stale);
        let location = format!("/api/v1/history/debug/{zip_name}");
        assert_eq!(reply.header("Location"), Some(location.as_str()));
        let zip_path = debug_dir.join(zip_name);
        let mut archive =
            zip::ZipArchive::new(std::fs::File::open(&zip_path).expect("zip")).expect("archive");
        let mut names: Vec<String> = archive
            .file_names()
            .map(|name| name.expect("entry name").into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "2026-10-08/23:00:00.purge.json.zst".to_string(),
                format!("{DAY}/08:30:00.shot.json.zst"),
            ]
        );
        for (name, file) in [
            (format!("{DAY}/08:30:00.shot.json.zst"), &first),
            ("2026-10-08/23:00:00.purge.json.zst".to_string(), &second),
        ] {
            let mut stored = Vec::new();
            std::io::Read::read_to_end(&mut archive.by_name(&name).expect("entry"), &mut stored)
                .expect("read");
            assert_eq!(stored, std::fs::read(file).expect("debug shot"), "{name}");
        }

        let download = get(&app, &location).await;
        assert_eq!(download.status_code(), 200);
        assert_eq!(
            download.body(),
            std::fs::read(&zip_path).expect("zip").as_slice()
        );
        assert_eq!(download.header("Content-Type"), Some("application/zip"));
    }

    #[test]
    fn py_int_like_python() {
        assert_eq!(py_int("200"), Some(200));
        assert_eq!(py_int(" -5 "), Some(-5));
        assert_eq!(py_int("+1_000"), Some(1000));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("_1"), None);
        assert_eq!(py_int("1.5"), None);
        assert_eq!(py_int(""), None);
    }
}
