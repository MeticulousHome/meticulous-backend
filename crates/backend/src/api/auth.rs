//! Device authorization's routes (no Python counterpart): pairing codes,
//! their exchange for a token, token validation, access requests answered
//! on the Dial, and "Sign out all devices"; plus the guard every API route
//! goes through. See [`crate::device_auth`].

use std::sync::Arc;

use serde_json::{Value, json};

use crate::api::{is_local, local_only, redact_ip};
use crate::app::App;
use crate::device_auth::{
    AccessRefusal, Admission, CODE_LIFETIME_S, CodeSource, REQUEST_TIMEOUT, UNNAMED_DEVICE,
    sanitize_name,
};
use crate::http::{GuardFuture, Kind, Method, Reply, Request, Route};
use crate::notifications::Notification;

pub fn routes() -> Vec<Route> {
    let m = "api.auth";
    vec![
        Route::new("/auth/pairing", Kind::Base, m, "PairingCodeHandler").post(pairing),
        Route::new("/auth/request", Kind::Base, m, "AccessRequestHandler")
            .post(request_access)
            .public(&[Method::Post]),
        Route::new(
            "/auth/request/([^/]+)",
            Kind::Base,
            m,
            "AccessRequestStatusHandler",
        )
        .get(access_request)
        .public(&[Method::Get]),
        Route::new("/auth/sign_out_all", Kind::Base, m, "SignOutAllHandler")
            .prepare(local_only)
            .post(sign_out_all),
        Route::new("/auth/token", Kind::Base, m, "TokenHandler")
            .post(token)
            .public(&[Method::Post]),
        Route::new("/auth/validate", Kind::Base, m, "ValidateTokenHandler")
            .get(validate)
            .public(&[Method::Get]),
    ]
}

/// `{"status": "error", "error": message, "data": {"code": code}}`.
fn error(status: u16, code: &str, message: &str) -> Reply {
    Reply::base().status(status).json(json!({
        "status": "error",
        "error": message,
        "data": {"code": code},
    }))
}

fn error_body(code: &str, message: &str) -> Value {
    json!({"status": "error", "error": message, "data": {"code": code}})
}

/// The answer to a client that needs a token: clients drop theirs and show
/// the pairing screen on it. A `BaseHandler` reply, so a browser on another
/// origin can read it.
pub fn unauthorized() -> Reply {
    Reply::base()
        .status(401)
        .with_header("WWW-Authenticate", "Bearer")
        .json(error_body("UNAUTHORIZED", "This device is not authorized"))
}

fn too_many_attempts(retry_after: u64) -> Reply {
    error(
        429,
        "TOO_MANY_ATTEMPTS",
        "Too many attempts, try again later",
    )
    .with_header("Retry-After", retry_after.to_string())
}

fn invalid_request(message: &str) -> Reply {
    error(400, "INVALID_REQUEST", message)
}

/// The request's JSON body; anything else reads as no body.
fn body(req: &Request) -> Value {
    if req.body.is_empty() {
        return Value::Null;
    }
    req.json().unwrap_or(Value::Null)
}

/// The router's guard: under `/api/`, a client the machine does not admit
/// (see [`Admission::admitted`]) gets the 401. Preflights pass, so a
/// browser can send the token at all; public routes never get here.
pub fn guard(app: Arc<App>, req: Request) -> GuardFuture {
    Box::pin(async move {
        if req.method == http::Method::OPTIONS || !req.path.starts_with("/api/") {
            return None;
        }
        if app.auth.admission(&req.headers).await.admitted() {
            return None;
        }
        Some(unauthorized())
    })
}

/// `POST /auth/pairing?mode=new|grant`: a code for another device. `new`
/// is the Dial's "Pair a device"; `grant` an authorized device's "Allow
/// another device", which needs its token even while authorization is off.
async fn pairing(app: Arc<App>, req: Request) -> Reply {
    let source = match req.get_query_argument("mode").as_deref() {
        Some("new") => CodeSource::New,
        Some("grant") => CodeSource::Grant,
        _ => return error(400, "INVALID_MODE", "mode must be new or grant"),
    };
    let local = is_local(&req.headers);
    match source {
        CodeSource::New if !local => {
            tracing::warn!(
                "Refused a pairing code for a new device to {}",
                redact_ip(req.header("X-Real-IP").unwrap_or_default())
            );
            return error(
                403,
                "LOCAL_ONLY",
                "This endpoint can only be accessed locally",
            );
        }
        CodeSource::Grant if !local && app.auth.device(&req.headers).is_none() => {
            return unauthorized();
        }
        _ => {}
    }
    let code = app.auth.mint_code(source, None);
    let auth = app.auth.clone();
    let for_url = code.clone();
    let Ok(pair_url) = tokio::task::spawn_blocking(move || auth.pair_url(&for_url)).await else {
        return Reply::error(Kind::Base, 500, &req);
    };
    Reply::base().json(json!({
        "code": code,
        "pair_url": pair_url,
        "expires_in": CODE_LIFETIME_S,
    }))
}

/// `POST /auth/token`: a pairing code for a token, once. Failures count
/// toward the same lockout as `POST /machine/unlock`.
async fn token(app: Arc<App>, req: Request) -> Reply {
    let body = body(&req);
    let Some(code) = body
        .get("code")
        .and_then(Value::as_str)
        .filter(|code| !code.trim().is_empty())
    else {
        return invalid_request("body must be {\"code\": string, \"name\": string}");
    };
    let throttle = &app.auth.token_throttle;
    let retry_after = throttle.retry_after(throttle.now());
    if retry_after > 0 {
        return too_many_attempts(retry_after);
    }
    let Some(pairing) = app.auth.take_code(code) else {
        throttle.record_failure(throttle.now());
        tracing::warn!(
            "Rejected a pairing code from {}",
            redact_ip(req.header("X-Real-IP").unwrap_or("local"))
        );
        return error(401, "INVALID_CODE", "This code is not valid");
    };
    throttle.reset();
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .and_then(sanitize_name)
        .or(pairing.name)
        .unwrap_or_else(|| UNNAMED_DEVICE.to_string());
    let (token, claims) = match app.auth.issue(&name) {
        Ok(issued) => issued,
        Err(error) => {
            tracing::error!("Could not issue a device token: {error}");
            return Reply::error(Kind::Base, 500, &req);
        }
    };
    tracing::info!(
        "Device {} paired with a code (source={})",
        claims.sub,
        pairing.source.as_str()
    );
    // Improv's code was approved on the Dial a moment ago; every other
    // code's exchange is news there (and closes its pairing screen).
    if pairing.source != CodeSource::Improv {
        app.notifications.add(
            Notification::new(format!("\u{201c}{name}\u{201d} can now use this machine")).private(),
        );
    }
    Reply::base().json(json!({
        "token": token,
        "device": {"id": claims.sub, "name": claims.name},
    }))
}

/// `GET /auth/validate`: 200 exactly when the request would get into the
/// API, 401 otherwise. nginx asks it before serving `/health` to the LAN.
async fn validate(app: Arc<App>, req: Request) -> Reply {
    let admission: Admission = app.auth.admission(&req.headers).await;
    if !admission.admitted() {
        let mut body = error_body("UNAUTHORIZED", "This device is not authorized");
        body["required"] = json!(true);
        return Reply::base()
            .status(401)
            .with_header("WWW-Authenticate", "Bearer")
            .json(body);
    }
    let device = admission
        .device
        .as_ref()
        .map(|claims| json!({"id": claims.sub, "name": claims.name}))
        .unwrap_or(Value::Null);
    Reply::base().json(json!({
        "required": admission.required,
        "local": admission.local,
        "token_valid": admission.device.is_some(),
        "device": device,
    }))
}

/// `POST /auth/request`: ask on the Dial whether this device may use the
/// machine; the requester polls the outcome.
async fn request_access(app: Arc<App>, req: Request) -> Reply {
    let Some(name) = body(&req)
        .get("name")
        .and_then(Value::as_str)
        .and_then(sanitize_name)
    else {
        return invalid_request("body must be {\"name\": string}");
    };
    match app.auth.request_access(&name) {
        Ok(id) => Reply::base().status(202).json(json!({
            "id": id,
            "expires_in": REQUEST_TIMEOUT.as_secs(),
        })),
        Err(AccessRefusal::Pending) => error(
            409,
            "REQUEST_PENDING",
            "Another device is waiting for an answer",
        ),
        Err(AccessRefusal::Throttled { retry_after }) => too_many_attempts(retry_after),
    }
}

/// `GET /auth/request/<id>`: the request's outcome, with a code once it is
/// allowed.
async fn access_request(app: Arc<App>, req: Request) -> Reply {
    let Some(view) = app.auth.access_request(req.arg(0).unwrap_or_default()) else {
        return error(404, "NOT_FOUND", "No such request");
    };
    let mut body = json!({"status": view.status.as_str()});
    if let Some((code, expires_in)) = view.code {
        body["code"] = json!(code);
        body["expires_in"] = json!(expires_in);
    }
    Reply::base().json(body)
}

/// `POST /auth/sign_out_all`: the Dial's "Sign out all devices".
async fn sign_out_all(app: Arc<App>, req: Request) -> Reply {
    let auth = app.auth.clone();
    match tokio::task::spawn_blocking(move || auth.sign_out_all()).await {
        Ok(Ok(())) => Reply::base().json(json!({"status": "ok"})),
        Ok(Err(error)) => {
            tracing::error!("Could not sign out all devices: {error}");
            Reply::error(Kind::Base, 500, &req)
        }
        Err(_) => Reply::error(Kind::Base, 500, &req),
    }
}

#[cfg(test)]
mod tests;
