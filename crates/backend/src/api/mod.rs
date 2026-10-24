//! The HTTP API (`api/*.py`): one module per Python module, assembled into
//! tornado's route list.

pub mod action;
pub mod bug_report;
pub mod community_upload_diagnostics;
pub mod emulation;
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
pub mod wifi;

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

/// `api/wifi.py`'s routes; on the emulator without NetworkManager,
/// `register_emulation_handlers()` replaces those with the same path.
fn wifi_routes(app: &App) -> Vec<Route> {
    if !(app.emulated() && !app.wifi.networking_available()) {
        return wifi::routes();
    }
    let emulated = emulation::routes();
    let mut routes: Vec<Route> = wifi::routes()
        .into_iter()
        .filter(|r| !emulated.iter().any(|e| e.pattern == r.pattern))
        .collect();
    routes.extend(emulated);
    routes
}

/// `API.get_routes()`: every API route under `/api/v1`, sorted by pattern.
pub fn api_routes(app: &App) -> Vec<Route> {
    with_prefix(app, wifi_routes(app))
}

/// The routes the contract snapshot records: the API, with both the real
/// and the emulated Wi-Fi handlers (`extract.py` reads every module).
pub fn contract_routes(app: &App) -> Vec<Route> {
    let mut wifi = wifi::routes();
    wifi.extend(emulation::routes());
    with_prefix(app, wifi)
}

fn with_prefix(app: &App, wifi: Vec<Route>) -> Vec<Route> {
    let mut routes: Vec<Route> = wifi;
    routes.extend(action::routes());
    routes.extend(bug_report::routes());
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
