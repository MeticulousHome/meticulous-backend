//! The HTTP API (`api/*.py`): one module per Python module, assembled into
//! tornado's route list.

pub mod action;
pub mod history;
pub mod machine;
pub mod notifications;
pub mod password;
pub mod pour_over_history;
pub mod pour_over_profiles;
pub mod profiles;
pub mod serial;
pub mod settings;
pub mod smoke_validation;
pub mod sounds;
pub mod update;
pub mod web_ui;

use std::net::IpAddr;

use serde_json::json;

use crate::app::App;
use crate::http::{Reply, Request, Route, Router};

/// `LocalAccessHandler.prepare`: the backend only listens on loopback, so a
/// request either comes from the machine itself (no `X-Real-IP`) or through
/// nginx, which sets `X-Real-IP` to the client. Only that address decides.
/// `Some(reply)` when the request must be refused.
pub fn local_only(req: &Request) -> Option<Reply> {
    let remote = req.header("X-Real-IP")?;
    if is_loopback(remote) {
        return None;
    }
    tracing::warn!(
        "Unauthorized access to {} from remote IP: {}",
        req.uri,
        redact_ip(remote)
    );
    Some(Reply::base().status(403).json(json!({
        "status": "error",
        "error": "This endpoint can only be accessed locally",
    })))
}

/// `is_loopback_address`: 127.0.0.0/8, `::1` and IPv4-mapped loopback.
pub fn is_loopback(address: &str) -> bool {
    match address.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_loopback(),
        Ok(IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.is_loopback(),
            None => v6.is_loopback(),
        },
        Err(_) => false,
    }
}

/// `redact_ip`: keep the first two segments of an address.
pub fn redact_ip(ip: &str) -> String {
    let separator = if ip.contains(':') { ':' } else { '.' };
    ip.split(separator)
        .enumerate()
        .map(|(i, segment)| {
            if i < 2 {
                segment.to_string()
            } else {
                "x".repeat(segment.len().max(1))
            }
        })
        .collect::<Vec<_>>()
        .join(&separator.to_string())
}

/// `API.get_routes()`: every API route under `/api/v1`, sorted by pattern.
pub fn api_routes(app: &App) -> Vec<Route> {
    let mut routes: Vec<Route> = Vec::new();
    routes.extend(action::routes());
    routes.extend(history::routes());
    routes.extend(machine::routes());
    routes.extend(notifications::routes());
    routes.extend(serial::routes());
    routes.extend(settings::routes());
    routes.extend(update::routes());
    routes.extend(password::routes());
    routes.extend(pour_over_history::routes());
    routes.extend(pour_over_profiles::routes());
    routes.extend(profiles::routes(app));
    routes.extend(smoke_validation::routes());
    routes.extend(sounds::routes());
    for route in &mut routes {
        route.pattern = format!("/api/v1{}", route.pattern);
        route.regex = regex::Regex::new(&format!("^{}$", route.pattern))
            .unwrap_or_else(|e| panic!("bad route {}: {e}", route.pattern));
    }
    routes.sort_by(|a, b| a.pattern.cmp(&b.pattern));
    routes
}

/// The routes the contract snapshot records: the API, whatever the mode.
pub fn contract_routes(app: &App) -> Vec<Route> {
    api_routes(app)
}

/// `API.get_routes()` plus `WEB_UI_HANDLER`, in the order tornado tries them:
/// the API routes sorted by pattern, then the web UI.
pub fn router(app: &App) -> Router {
    let mut routes = api_routes(app);
    tracing::info!("API Routes registered:");
    tracing::info!("  /api/v1");
    for route in &routes {
        tracing::info!("    {}", route.pattern);
    }
    routes.extend(web_ui::routes(app));
    Router::new(routes)
}
