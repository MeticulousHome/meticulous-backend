//! `api/wifi.py`: Wi-Fi settings, status, the join QR code, the network list,
//! joining, repairing and forgetting networks.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::app::App;
use crate::config::{WIFI_MODE_AP, WIFI_MODE_CLIENT, keys};
use crate::http::{Kind, Reply, Request, Route};
use crate::py::{PyError, PyResult, contains, subscript};
use crate::wifi::WifiType;

pub fn routes() -> Vec<Route> {
    let m = "api.wifi";
    vec![
        Route::new("/wifi/config", Kind::Base, m, "WiFiConfigHandler")
            .get(get_config)
            .post(post_config),
        Route::new("/wifi/config/qr.png", Kind::Base, m, "WiFiQRHandler").get(get_qr),
        Route::new("/wifi/list", Kind::Base, m, "WiFiListHandler").get(get_list),
        Route::new("/wifi/connect", Kind::Base, m, "WiFiConnectHandler").post(post_connect),
        Route::new("/wifi/repair", Kind::Base, m, "WiFiRepairHandler").post(post_repair),
        Route::new("/wifi/delete", Kind::Base, m, "WiFiDeleteHandler").post(post_delete),
    ]
}

/// `loop.run_in_executor(None, f)`.
async fn blocking<T: Send + 'static>(
    app: &Arc<App>,
    f: impl FnOnce(&App) -> PyResult<T> + Send + 'static,
) -> PyResult<T> {
    let app = app.clone();
    tokio::task::spawn_blocking(move || f(&app))
        .await
        .unwrap_or_else(|e| Err(PyError::new("RuntimeError", e.to_string())))
}

/// What tornado does with an exception the handler does not catch.
pub fn uncaught(req: &Request, error: &PyError) -> Reply {
    tracing::error!(
        target: "tornado.application",
        "Uncaught exception {} {} ({})\n{}: {}",
        req.method,
        req.uri,
        req.remote_ip,
        error.kind,
        error.message
    );
    Reply::error(Kind::Base, 500, req)
}

/// The JSON body, or the exception `json.loads(self.request.body)` raises.
pub fn json_body(req: &Request) -> PyResult<Value> {
    if std::str::from_utf8(&req.body).is_err() {
        return Err(PyError::new(
            "UnicodeDecodeError",
            "'utf-8' codec can't decode bytes: invalid utf-8",
        ));
    }
    req.json()
        .map_err(|e| PyError::new("JSONDecodeError", e.message))
}

/// `WiFiConfigHandler.getWifiConfig()`.
pub fn wifi_config(app: &App) -> PyResult<Value> {
    let config = json!({
        "mode": app.config.get(keys::WIFI, keys::WIFI_MODE),
        "apName": app.config.get(keys::WIFI, keys::WIFI_AP_NAME),
        "apPassword": app.config.get(keys::WIFI, keys::WIFI_AP_PASSWORD),
    });
    let current = app.wifi.get_current_config()?;
    let status = current.to_json();
    let health = app.wifi.get_health_status(Some(current), false, false)?;
    Ok(json!({
        "config": config,
        "status": status,
        "health": health.to_json(),
        "known_wifis": Value::Object(app.wifi.get_known_wifis()?),
    }))
}

async fn get_config(app: Arc<App>, req: Request) -> Reply {
    match blocking(&app, wifi_config).await {
        Ok(config) => Reply::base().json(config),
        Err(error) => uncaught(&req, &error),
    }
}

fn text(status: u16, body: &str) -> Reply {
    let mut reply = Reply::base().status(status);
    reply.write_str(body);
    reply
}

async fn post_config(app: Arc<App>, req: Request) -> Reply {
    let data = match json_body(&req) {
        Ok(data) => data,
        Err(error) if error.kind == "JSONDecodeError" => {
            tracing::warn!("Failed to parse passed JSON: JSONDecodeError");
            return text(400, "Invalid JSON");
        }
        Err(error) => return failed_to_write(&error),
    };
    match apply_config(&app, &data).await {
        Ok(reply) => reply,
        Err(error) => failed_to_write(&error),
    }
}

fn failed_to_write(error: &PyError) -> Reply {
    tracing::warn!("Failed to accept passed config: {}", error.kind);
    text(400, "Failed to write config")
}

async fn apply_config(app: &Arc<App>, data: &Value) -> PyResult<Reply> {
    let mut changed = false;
    let mut mode = None;
    let mut ap_password = None;
    if contains(data, "mode")? {
        let requested = subscript(data, "mode")?;
        if matches!(requested.as_str(), Some(WIFI_MODE_AP | WIFI_MODE_CLIENT)) {
            tracing::info!("Changing wifi mode");
            mode = Some(requested.clone());
            changed = true;
        }
    }
    if contains(data, "apPassword")? {
        tracing::info!("Changing wifi ap password");
        // `None` means "keep the current one" to applyWifiSettings.
        ap_password = Some(subscript(data, "apPassword")?.clone()).filter(|p| !p.is_null());
        changed = true;
    }
    if changed {
        let success = blocking(app, move |app| {
            app.wifi.apply_wifi_settings(mode, ap_password)
        })
        .await?;
        if !success {
            let error = app.wifi.last_health_error_message();
            let error = if error.is_empty() {
                "Failed to apply Wi-Fi settings."
            } else {
                error
            };
            let code = app.wifi.last_health_error();
            let config = blocking(app, wifi_config).await?;
            return Ok(Reply::base().status(400).json(json!({
                "status": "error",
                "error": error,
                "code": code,
                "config": config,
            })));
        }
    }
    let config = blocking(app, wifi_config).await?;
    Ok(Reply::base().json(config))
}

/// `escape_wifi_qr_value`.
fn escape_wifi_qr_value(value: &Value) -> PyResult<String> {
    let Value::String(value) = value else {
        return Err(PyError::attribute_error(format!(
            "'{}' object has no attribute 'replace'",
            crate::py::type_name(value)
        )));
    };
    Ok(value
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace(':', "\\:")
        .replace('"', "\\\""))
}

/// The text the QR code encodes: join credentials in hotspot mode, else the
/// machine's URL.
pub fn qr_contents(app: &App) -> PyResult<String> {
    let config = app.wifi.get_current_config()?;
    let port = app.env.port;
    if config.is_hotspot() {
        let ssid = escape_wifi_qr_value(&app.config.get(keys::WIFI, keys::WIFI_AP_NAME))?;
        let password = escape_wifi_qr_value(&app.config.get(keys::WIFI, keys::WIFI_AP_PASSWORD))?;
        return Ok(format!("WIFI:S:{ssid};T:WPA2;P:{password};H:false;;"));
    }
    if let Some(ip) = config.ips.first() {
        return Ok(match ip.addr {
            std::net::IpAddr::V6(v6) => format!("http://[{v6}]:{port}"),
            std::net::IpAddr::V4(v4) => format!("http://{v4}:{port}"),
        });
    }
    Ok(format!("http://{}.local:{port}", config.hostname))
}

/// `WiFiQRHandler.generate_wifi_qr()`.
fn generate_wifi_qr(app: &App) -> PyResult<Vec<u8>> {
    let contents = qr_contents(app)?;
    let (width, modules) = crate::qr::create(&contents)?;
    Ok(crate::qr::png(width, &modules, 8, 2))
}

pub async fn get_qr(app: Arc<App>, req: Request) -> Reply {
    match blocking(&app, generate_wifi_qr).await {
        Ok(png) => {
            let mut reply = Reply::base().with_header("Content-Type", "image/png");
            reply.write_bytes(&png);
            reply
        }
        Err(error) => uncaught(&req, &error),
    }
}

/// `WiFiListHandler.getWifiList()`: one entry per SSID (the one in use, else
/// the strongest), strongest first.
fn wifi_list(app: &App) -> Vec<Value> {
    let mut networks: Map<String, Value> = Map::new();
    for s in app.wifi.get_available_networks(true, true) {
        if s.ssid.is_empty() {
            continue;
        }
        let Some(wifi_type) = WifiType::from_nmcli_security(&s.security) else {
            continue;
        };
        let formatted = json!({
            "type": wifi_type.value(),
            "security": s.security,
            "ssid": s.ssid,
            "signal": s.signal,
            "rate": s.rate,
            "in_use": s.in_use,
        });
        match networks.get(&s.ssid) {
            None => {
                networks.insert(s.ssid.clone(), formatted);
            }
            Some(_) if s.in_use => {
                networks.insert(s.ssid.clone(), formatted);
            }
            Some(existing) => {
                tracing::info!("{}, {}", app.wifi.redact_ssid(&s.ssid), existing["signal"]);
                if existing["in_use"].as_bool() == Some(true) {
                    continue;
                }
                if Some(s.signal) > existing["signal"].as_i64() {
                    networks.insert(s.ssid.clone(), formatted);
                }
            }
        }
    }
    let mut list: Vec<Value> = networks.into_iter().map(|(_, v)| v).collect();
    list.sort_by(|a, b| b["signal"].as_i64().cmp(&a["signal"].as_i64()));
    list
}

async fn get_list(app: Arc<App>, _req: Request) -> Reply {
    let list = blocking(&app, |app| Ok(wifi_list(app)))
        .await
        .unwrap_or_default();
    Reply::base().dumps(Value::Array(list))
}

fn connect_error(error: &PyError) -> Reply {
    tracing::warn!("Failed to connect: {}", error.kind);
    Reply::base().status(400).json(json!({
        "status": "error",
        "error": format!("failed to connect to wifi: {}", error.kind),
    }))
}

async fn post_connect(app: Arc<App>, req: Request) -> Reply {
    let data = match json_body(&req) {
        Ok(data) => data,
        Err(error) => return connect_error(&error),
    };
    let result = blocking(&app, move |app| {
        if !app.wifi.connect_to_wifi(&data, "manual")? {
            return Ok(None);
        }
        let current = app.wifi.get_current_config()?;
        Ok(Some(app.wifi.get_health_status(
            Some(current),
            false,
            false,
        )?))
    })
    .await;
    match result {
        Ok(Some(health)) => Reply::base().json(json!({"status": "ok", "health": health.to_json()})),
        Ok(None) => {
            let (code, error) = app.wifi.last_connection_error();
            Reply::base().status(400).json(json!({
                "status": "error",
                "error": error,
                "code": code,
            }))
        }
        Err(error) => connect_error(&error),
    }
}

async fn post_repair(app: Arc<App>, _req: Request) -> Reply {
    let result = blocking(&app, |app| {
        let success = app.wifi.repair_wifi_connection("manual")?;
        let health = app.wifi.get_health_status(None, false, true)?;
        Ok((success, health))
    })
    .await;
    match result {
        Ok((true, health)) => {
            Reply::base().json(json!({"status": "ok", "health": health.to_json()}))
        }
        Ok((false, health)) => {
            let message = crate::wifi::health_error_message(&health.last_error);
            let error = if message.is_empty() {
                "Wi-Fi repair could not complete."
            } else {
                message
            };
            let code = if health.last_error.is_empty() {
                "wifi_repair_failed".to_string()
            } else {
                health.last_error.clone()
            };
            Reply::base().status(400).json(json!({
                "status": "error",
                "error": error,
                "code": code,
                "health": health.to_json(),
            }))
        }
        Err(error) => {
            tracing::warn!("Failed to repair wifi: {}: {}", error.kind, error.message);
            Reply::base().status(400).json(json!({
                "status": "error",
                "error": format!("failed to repair wifi: {}", error.message),
            }))
        }
    }
}

async fn post_delete(app: Arc<App>, req: Request) -> Reply {
    let result = match json_body(&req).and_then(|data| subscript(&data, "ssid").cloned()) {
        Ok(ssid) => blocking(&app, move |app| app.wifi.delete_wifi(&ssid)).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(true) => Reply::base().json(json!({"status": "ok"})),
        Ok(false) => Reply::base().status(400).json(json!({
            "status": "error",
            "error": "failed to delete unknown wifi",
        })),
        Err(error) => {
            tracing::warn!("Failed to delete wifi: {}", error.kind);
            Reply::base().status(400).json(json!({
                "status": "error",
                "error": format!("failed to delete wifi: {}", error.kind),
            }))
        }
    }
}

#[cfg(test)]
mod tests;
