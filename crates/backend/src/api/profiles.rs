//! `api/profiles.py`.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::app::App;
use crate::http::{Kind, Reply, Request, Route, StaticOptions};
use crate::profiles::ProfileError;

pub fn routes(app: &App) -> Vec<Route> {
    let m = "api.profiles";
    vec![
        Route::new("/profile/list", Kind::Base, m, "ListHandler").get(list),
        Route::new("/profile/save", Kind::Base, m, "SaveProfileHandler").post(save),
        Route::new(
            "/profile/from_manual",
            Kind::Base,
            m,
            "CreateProfileFromManualHandler",
        )
        .post(from_manual),
        // LoadProfileHandler defines get and post for both of its routes;
        // each route serves one (the other was a 500, now a 405).
        Route::new("/profile/load", Kind::Base, m, "LoadProfileHandler")
            .post(load_posted)
            .class_methods(&["get", "post"]),
        Route::new("/profile/defaults", Kind::Base, m, "ListDefaultsHandler").get(defaults),
        Route::new("/profile/image([/]*)", Kind::Base, m, "ListImagesHandler").get(list_images),
        Route::new(
            "/profile/image/(.*)",
            Kind::Static(StaticOptions {
                root: app.env.paths.images.clone(),
                default_filename: None,
            }),
            m,
            "StaticFileHandler",
        ),
        Route::new(
            "/profile/load/([0-9a-fA-F-]+)",
            Kind::Base,
            m,
            "LoadProfileHandler",
        )
        .get(load_by_id)
        .class_methods(&["get", "post"]),
        Route::new(
            "/profile/get/([0-9a-fA-F-]+)",
            Kind::Base,
            m,
            "GetProfileHandler",
        )
        .get(get),
        Route::new(
            "/profile/delete/([0-9a-fA-F-]+)",
            Kind::Base,
            m,
            "DeleteProfileHandler",
        )
        .get(delete)
        .delete(delete),
        Route::new("/profile/changes", Kind::Base, m, "ChangesHandler").get(changes),
        Route::new("/profile/last", Kind::Base, m, "LastProfileHandler").get(last),
        Route::new("/profile/selected", Kind::Base, m, "SelectedProfileHandler").get(selected),
        Route::new("/profile/legacy", Kind::Base, m, "LegacyProfileHandler").post(legacy),
    ]
}

fn error(status: u16, body: Value) -> Reply {
    Reply::base().status(status).json(body)
}

fn busy() -> Reply {
    error(409, json!({"status": "error", "error": "machine is busy"}))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

async fn list(app: Arc<App>, req: Request) -> Reply {
    let full = req.get_argument_or("full", "false").to_lowercase() == "true";
    if crate::limited_access::is_limited_access(&app.config) {
        return Reply::base().dumps(Value::Array(app.simple_profile.list(full)));
    }
    let profiles: Vec<Value> = app
        .profiles
        .list_profiles()
        .into_iter()
        // Manual profiles are reached from the manual-mode setup flow.
        .filter(|p| p.get("manual") != Some(&Value::Bool(true)))
        .map(|mut p| {
            if !full && let Some(map) = p.as_object_mut() {
                map.shift_remove("stages");
            }
            p
        })
        .collect();
    Reply::base().dumps(Value::Array(profiles))
}

async fn defaults(app: Arc<App>, _req: Request) -> Reply {
    Reply::base().dumps(app.simple_profile.defaults())
}

/// `json.loads(body, parse_constant=...)` that refuses NaN and Infinity.
fn parse_profile_body(body: &[u8]) -> Result<Value, String> {
    let text = std::str::from_utf8(body).map_err(|e| e.to_string())?;
    match serde_json::from_str::<Value>(text) {
        Ok(value) => Ok(value),
        Err(_) => {
            for token in ["-Infinity", "Infinity", "NaN"] {
                if bare_token(text, token) {
                    return Err(format!("Invalid number {token}"));
                }
            }
            crate::http::parse_python_json(body).map_err(|e| e.message)
        }
    }
}

/// Whether `token` appears outside a string.
fn bare_token(text: &str, token: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if text[i..].starts_with(token) {
            return true;
        }
    }
    false
}

async fn save(app: Arc<App>, req: Request) -> Reply {
    let change_id = req.header("X-Change-Id").map(str::to_string);
    if crate::pour_over::looks_like_pour_over(&req.body) {
        return crate::pour_over::error_reply(&profile::PourOverProfileError::TooLarge);
    }
    let data = match parse_profile_body(&req.body) {
        Ok(data) => data,
        Err(cause) => {
            tracing::warn!("Failed to save profile: {cause}");
            return error(
                400,
                json!({"status": "error", "error": "failed to save profile", "cause": cause}),
            );
        }
    };
    let is_pour_over = data.get("brew_type").and_then(Value::as_str) == Some("pour_over");
    let result = blocking(move || {
        if is_pour_over {
            app.pour_over
                .save_profile(&app, data, change_id)
                .map_err(SaveError::PourOver)
        } else {
            app.profiles
                .save_profile(&app, data.clone(), false, change_id, false)
                .map_err(|e| SaveError::Espresso(e, data))
        }
    })
    .await;
    match result {
        Some(Ok(response)) => Reply::base().json(response),
        Some(Err(SaveError::PourOver(e))) => {
            tracing::warn!("Rejected Pour Over profile: {e}");
            crate::pour_over::error_reply(&e)
        }
        Some(Err(SaveError::Espresso(ProfileError::Validation(message), data))) => {
            let id = data.get("id").cloned().unwrap_or(Value::Null);
            tracing::warn!(
                "Rejected espresso profile {}: {message}",
                crate::metconfig::py_repr(&id)
            );
            error(
                400,
                json!({"status": "error", "error": format!("JSON validation error: {message}")}),
            )
        }
        Some(Err(SaveError::Espresso(e, _))) => {
            tracing::warn!("Failed to save profile: {e}");
            error(
                400,
                json!({"status": "error", "error": "failed to save profile", "cause": e.to_string()}),
            )
        }
        None => Reply::error(Kind::Base, 500, &req),
    }
}

enum SaveError {
    PourOver(profile::PourOverProfileError),
    Espresso(ProfileError, Value),
}

async fn from_manual(app: Arc<App>, req: Request) -> Reply {
    let data = if req.body.is_empty() {
        json!({})
    } else {
        match req.json() {
            Ok(v) => v,
            Err(e) => {
                return error(
                    400,
                    json!({"status": "error", "error": "failed to create profile from manual brew", "cause": e.message}),
                );
            }
        }
    };
    let Value::Object(map) = &data else {
        return error(
            400,
            json!({"status": "error", "error": "body must be a JSON object"}),
        );
    };
    let name = map.get("name").cloned();
    if name.as_ref().is_some_and(|n| !n.is_string()) {
        return error(
            400,
            json!({"status": "error", "error": "name must be a string"}),
        );
    }
    let shot_id = map.get("shot_id").cloned();
    if shot_id.as_ref().is_some_and(|s| !s.is_string()) {
        return error(
            400,
            json!({"status": "error", "error": "shot_id must be a string"}),
        );
    }
    let change_id = req.header("X-Change-Id").map(str::to_string);
    let outcome = blocking(
        move || -> Result<Value, crate::manual_mode::FromManualError> {
            let shot = crate::manual_mode::find_manual_shot(
                &app,
                shot_id.as_ref().and_then(Value::as_str),
            )?;
            let profile = crate::manual_mode::build_profile_from_manual_shot(
                &shot,
                name.as_ref().and_then(Value::as_str),
            )?;
            app.profiles
                .save_profile(&app, profile, false, change_id, false)
                .map_err(crate::manual_mode::FromManualError::Profile)
        },
    )
    .await;
    use crate::manual_mode::FromManualError as E;
    match outcome {
        Some(Ok(response)) => Reply::base().json(response),
        Some(Err(E::NotFound(_))) => error(
            404,
            json!({"status": "error", "error": "no manual brew found"}),
        ),
        Some(Err(E::NoTargets)) => error(
            409,
            json!({"status": "error", "error": "shot has no target samples"}),
        ),
        Some(Err(E::Profile(ProfileError::Validation(message)))) => error(
            400,
            json!({"status": "error", "error": format!("JSON validation error: {message}")}),
        ),
        Some(Err(other)) => {
            tracing::warn!("Failed to create profile from manual brew: {other}");
            error(
                400,
                json!({"status": "error", "error": "failed to create profile from manual brew", "cause": other.to_string()}),
            )
        }
        None => Reply::error(Kind::Base, 500, &req),
    }
}

/// `LoadProfileHandler.get(profile_id)`.
async fn load_by_id(app: Arc<App>, req: Request) -> Reply {
    if !app.machine.is_idle() {
        return busy();
    }
    let id = req.arg(0).unwrap_or("").to_string();
    let limited = if crate::limited_access::is_limited_access(&app.config) {
        app.simple_profile.get_by_id(&id)
    } else {
        None
    };
    if let Some(limited) = limited {
        let outcome = blocking({
            let app = app.clone();
            move || app.profiles.send_profile_to_esp32(&app, limited)
        })
        .await;
        return match outcome {
            Some(Ok(Some(profile))) => {
                Reply::base().json(json!({"name": profile.get("name"), "id": profile.get("id")}))
            }
            Some(Ok(None)) => error(
                403,
                json!({"status": "error", "error": "high strain on motor"}),
            ),
            Some(Err(ProfileError::Validation(message))) => error(
                400,
                json!({"status": "error", "error": format!("JSON validation error: {message}")}),
            ),
            // PARITY: other errors are uncaught on this path in Python.
            _ => Reply::error(Kind::Base, 500, &req),
        };
    }
    let outcome = blocking({
        let app = app.clone();
        let id = id.clone();
        move || -> Result<Option<Value>, ProfileError> {
            if app.profiles.get_profile(&id).is_none() {
                return Ok(None);
            }
            app.profiles.load_profile_and_send(&app, &id)
        }
    })
    .await;
    match outcome {
        Some(Ok(Some(profile))) => {
            Reply::base().json(json!({"name": profile.get("name"), "id": profile.get("id")}))
        }
        Some(Ok(None)) => error(
            404,
            json!({"status": "error", "error": "profile not found", "id": id}),
        ),
        Some(Err(ProfileError::Validation(message))) => error(
            400,
            json!({"status": "error", "error": format!("JSON validation error: {message}")}),
        ),
        Some(Err(e)) => {
            tracing::warn!("Failed to execute profile in place: {e}");
            error(
                400,
                json!({"status": "error", "error": format!("failed to load profile {e}")}),
            )
        }
        None => Reply::error(Kind::Base, 500, &req),
    }
}

/// `LoadProfileHandler.post()`: load the profile in the body.
async fn load_posted(app: Arc<App>, req: Request) -> Reply {
    if !app.machine.is_idle() {
        return busy();
    }
    let data = match req.json() {
        Ok(data) => data,
        Err(e) => {
            tracing::warn!("JSON decode error: {}", e.message);
            return error(
                400,
                json!({"status": "error", "error": format!("Invalid JSON: {}", e.message)}),
            );
        }
    };
    tracing::warn!("Parsed data: {}", crate::metconfig::py_repr(&data));
    let outcome = blocking({
        let app = app.clone();
        move || app.profiles.send_profile_to_esp32(&app, data)
    })
    .await;
    match outcome {
        Some(Ok(Some(profile))) => {
            Reply::base().json(json!({"name": profile.get("name"), "id": profile.get("id")}))
        }
        Some(Ok(None)) => error(
            403,
            json!({"status": "error", "error": "high strain on motor"}),
        ),
        Some(Err(ProfileError::Validation(message))) => error(
            400,
            json!({"status": "error", "error": format!("JSON validation error: {message}")}),
        ),
        Some(Err(ProfileError::Variable { class, message })) => error(
            400,
            json!({"status": "error", "error": format!("variable error: {class}:{message}")}),
        ),
        Some(Err(e)) => {
            tracing::warn!("Failed to execute profile in place: {e}");
            error(
                400,
                json!({"status": "error", "error": format!("failed to load profile {e}")}),
            )
        }
        None => Reply::error(Kind::Base, 500, &req),
    }
}

async fn legacy(app: Arc<App>, req: Request) -> Reply {
    if !app.config.get_bool(
        crate::config::keys::USER,
        crate::config::keys::ALLOW_LEGACY_JSON,
    ) {
        return Reply::base().status(404);
    }
    if !app.machine.is_idle() {
        return busy();
    }
    let data = match req.json() {
        Ok(data) => data,
        Err(e) => {
            return error(
                400,
                json!({"status": "error", "error": format!("failed to load profile {}", e.message)}),
            );
        }
    };
    let sent = data.clone();
    blocking({
        let app = app.clone();
        move || {
            app.profiles
                .remember_last_profile(&crate::profiles::legacy_dummy_profile());
            app.machine.send_json_with_hash(&app, &sent);
        }
    })
    .await;
    let missing = ["name", "id"].into_iter().find(|k| data.get(*k).is_none());
    match missing {
        Some(key) => error(
            400,
            json!({"status": "error", "error": format!("failed to load profile '{key}'")}),
        ),
        None => Reply::base().json(json!({"name": data["name"], "id": data["id"]})),
    }
}

async fn get(app: Arc<App>, req: Request) -> Reply {
    let id = req.arg(0).unwrap_or("").to_string();
    tracing::info!("Request for profile {id}");
    let limited = if crate::limited_access::is_limited_access(&app.config) {
        app.simple_profile.get_by_id(&id)
    } else {
        None
    };
    match limited.or_else(|| app.profiles.get_profile(&id)) {
        Some(profile) if crate::config::python_truthy(&profile) => Reply::base().json(profile),
        _ => error(
            404,
            json!({"status": "error", "error": "profile not found", "id": id}),
        ),
    }
}

async fn delete(app: Arc<App>, req: Request) -> Reply {
    let id = req.arg(0).unwrap_or("").to_string();
    let change_id = req.header("X-Change-Id").map(str::to_string);
    tracing::info!("Deletion for profile {id}");
    let outcome = blocking({
        let app = app.clone();
        let id = id.clone();
        move || app.profiles.delete_profile(&id, change_id)
    })
    .await;
    match outcome {
        Some(Ok(Some(data))) => {
            tracing::info!("Deleted profile: {id}");
            Reply::base().json(data)
        }
        Some(Ok(None)) => error(
            404,
            json!({"status": "error", "error": "profile not found", "id": id}),
        ),
        _ => Reply::error(Kind::Base, 500, &req),
    }
}

async fn changes(app: Arc<App>, _req: Request) -> Reply {
    Reply::base().dumps(Value::Array(app.profiles.changes()))
}

async fn last(app: Arc<App>, _req: Request) -> Reply {
    let Some(last) = app.profiles.last_profile() else {
        return Reply::base().status(204);
    };
    let mut reply = Reply::base();
    if let Some(load_time) = last.get("load_time").and_then(Value::as_f64) {
        reply.set_header(
            "Last-Modified",
            crate::http::http_date(crate::shots::local_time(load_time).with_timezone(&chrono::Utc)),
        );
    }
    reply.write_dumps(&last);
    reply
}

async fn selected(app: Arc<App>, _req: Request) -> Reply {
    let hover = app.profiles.hover();
    if hover.id.is_empty() {
        return Reply::base().status(204);
    }
    Reply::base().dumps(hover.to_value())
}

async fn list_images(app: Arc<App>, _req: Request) -> Reply {
    let images: Vec<Value> = app
        .profiles
        .default_images()
        .into_iter()
        .map(Value::String)
        .collect();
    Reply::base().dumps(Value::Array(images))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_nan_like_parse_constant() {
        assert_eq!(
            parse_profile_body(br#"{"a": NaN}"#),
            Err("Invalid number NaN".into())
        );
        assert_eq!(
            parse_profile_body(br#"{"a": -Infinity}"#),
            Err("Invalid number -Infinity".into())
        );
        assert!(parse_profile_body(br#"{"a": "NaN"}"#).is_ok());
        assert_eq!(
            parse_profile_body(b""),
            Err("Expecting value: line 1 column 1 (char 0)".into())
        );
    }
}
