//! `api/machine.py`: unlocking a limited-access machine, the backlight, the
//! factory reset, the OS update status and the system clock.
//!
//! `GET /machine` (`MachineInfoHandler`) belongs here too; it is added with
//! the machine runtime it reads.

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::api::{local_only, redact_ip};
use crate::app::App;
use crate::http::{Kind, Reply, Request, Route};
use crate::limited_access;

pub fn routes() -> Vec<Route> {
    let m = "api.machine";
    vec![
        Route::new("/machine/unlock", Kind::Base, m, "MachineUnlockHandler").post(unlock),
        Route::new(
            "/machine/backlight",
            Kind::Base,
            m,
            "MachineBacklightController",
        )
        .post(backlight),
        Route::new(
            "/machine/factory_reset",
            Kind::Base,
            m,
            "MachineResetHandler",
        )
        .get(factory_reset),
        Route::new("/machine/OS_update_status", Kind::Base, m, "UpdateOSStatus")
            .get(os_update_status),
        Route::new("/machine/time", Kind::Base, m, "MachineTimeHandler").post(set_time),
    ]
}

/// `cleanup_factory_reset_data`: remove every non-hidden entry of the user
/// partition (the hidden device identity cache survives), going on past
/// entries that cannot be removed.
pub fn cleanup_factory_reset_data(root: &Path) {
    cleanup_with(root, |path, is_dir| {
        if is_dir {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
    });
}

fn cleanup_with(root: &Path, remove: impl Fn(&Path, bool) -> std::io::Result<()>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
        if let Err(error) = remove(&path, is_dir) {
            tracing::warn!(
                "Could not remove factory reset entry {}: {error}",
                path.display()
            );
        }
    }
}

fn invalid_body() -> Reply {
    Reply::base().status(400).json(json!({
        "status": "error",
        "error": "body must be {\"code\": string}",
        "data": {"code": "INVALID_BODY"},
    }))
}

/// `MachineUnlockHandler.post`.
async fn unlock(app: Arc<App>, req: Request) -> Reply {
    let body = if req.body.is_empty() {
        Value::Null
    } else {
        req.json().unwrap_or(Value::Null)
    };
    let code = body.get("code").cloned().unwrap_or(Value::Null);
    let Some(text) = code.as_str() else {
        return invalid_body();
    };
    if crate::ota::py_strip(text).is_empty() {
        return invalid_body();
    }
    let throttle = &app.unlock_throttle;
    let retry_after = throttle.retry_after(throttle.now());
    if retry_after > 0 {
        return Reply::base()
            .status(429)
            .with_header("Retry-After", retry_after.to_string())
            .json(json!({
                "status": "error",
                "error": "too many failed attempts",
                "data": {"code": "UNLOCK_THROTTLED", "retryAfterSeconds": retry_after},
            }));
    }
    if !limited_access::validate_unlock_code(&app.config, &code) {
        throttle.record_failure(throttle.now());
        tracing::warn!("Rejected unlock code from {}", redact_ip(&req.remote_ip));
        return Reply::base().status(403).json(json!({
            "status": "error",
            "error": "invalid unlock code",
            "data": {"code": "INVALID_UNLOCK_CODE"},
        }));
    }
    throttle.reset();
    let unlocked = {
        let app = app.clone();
        tokio::task::spawn_blocking(move || limited_access::unlock(&app)).await
    };
    match unlocked {
        Ok(channel) => Reply::base().json(json!({
            "status": "ok",
            "update_channel": channel,
            "limited_access": false,
        })),
        Err(_) => Reply::error(Kind::Base, 500, &req),
    }
}

/// `MachineBacklightController.post`.
async fn backlight(app: Arc<App>, req: Request) -> Reply {
    if std::str::from_utf8(&req.body).is_err() {
        // json.loads raises UnicodeDecodeError, which the handler lets escape.
        return Reply::error(Kind::Base, 500, &req);
    }
    let settings = match req.json() {
        Ok(settings) => settings,
        Err(error) => {
            return Reply::base().status(403).json(json!({
                "status": "error",
                "error": "invalid json",
                "json_error": error.message,
            }));
        }
    };
    // `"brightness" in settings` works on any container; `.get` only on a dict.
    let settings = match settings {
        Value::Object(map) => map,
        Value::Array(items) if items.contains(&json!("brightness")) => {
            return Reply::error(Kind::Base, 500, &req);
        }
        Value::String(text) if text.contains("brightness") => {
            return Reply::error(Kind::Base, 500, &req);
        }
        Value::Array(_) | Value::String(_) => serde_json::Map::new(),
        _ => return Reply::error(Kind::Base, 500, &req),
    };
    let Some(brightness) = settings.get("brightness").cloned() else {
        return Reply::base().status(400).json(json!({
            "status": "error",
            "error": "brightness value is required",
        }));
    };
    let interpolation = settings
        .get("interpolation")
        .cloned()
        .unwrap_or_else(|| json!("curve"));
    let animation_time = settings
        .get("animation_time")
        .cloned()
        .unwrap_or_else(|| json!(1));
    if !brightness.is_null() {
        tracing::info!("Dimming to {}", crate::ota::python_str(&brightness));
        let leave_alone = app.emulated() || app.env.hooks.fake_hardware;
        let controller = app.backlight.clone();
        let _ = tokio::task::spawn_blocking(move || {
            controller.dim(&brightness, &interpolation, &animation_time, leave_alone)
        })
        .await;
    }
    Reply::base()
}

/// `MachineResetHandler.get` (a `LocalAccessHandler`).
async fn factory_reset(app: Arc<App>, req: Request) -> Reply {
    if let Some(refused) = local_only(&req) {
        return refused;
    }
    if req.get_argument("confirm").as_deref() != Some("true") {
        return Reply::base()
            .status(400)
            .json(json!({"error": "Confirmation required. Add confirm=true"}));
    }
    if app.emulated() {
        tracing::warn!("Factory reset only simluated in emulated mode");
        return Reply::base().json(json!({"status": "success", "message": "Emulated mode"}));
    }
    tracing::warn!("Performing factory reset");
    let rebooted = {
        let app = app.clone();
        tokio::task::spawn_blocking(move || {
            cleanup_factory_reset_data(&app.env.paths.system.factory_reset_root);
            app.host.run(&["reboot"]).is_ok()
        })
        .await
        .unwrap_or(false)
    };
    if !rebooted {
        // subprocess.run("reboot") raising FileNotFoundError.
        return Reply::error(Kind::Base, 500, &req);
    }
    Reply::base()
}

/// `UpdateOSStatus.get`.
async fn os_update_status(app: Arc<App>, _req: Request) -> Reply {
    Reply::base().json(app.os_update.to_json())
}

/// `MachineTimeHandler.post`: set the clock from an ISO 8601 date, aware
/// dates converted to the machine's local time.
async fn set_time(app: Arc<App>, req: Request) -> Reply {
    let data = std::str::from_utf8(&req.body)
        .ok()
        .and_then(|text| pyfmt::loads_lenient(text).ok());
    let Some(data) = data else {
        return Reply::base()
            .status(400)
            .json(json!({"error": "Invalid JSON"}));
    };
    let Value::Object(data) = data else {
        // data.get on a list or a string raises AttributeError.
        return Reply::error(Kind::Base, 500, &req);
    };
    let iso_date = data.get("date").cloned().unwrap_or(Value::Null);
    if !crate::config::python_truthy(&iso_date) {
        return Reply::base()
            .status(400)
            .json(json!({"error": "Missing 'date' in request"}));
    }
    let Value::String(iso_date) = iso_date else {
        return Reply::error(Kind::Base, 500, &req);
    };
    let Some(parsed) = crate::clock::python_fromisoformat(&iso_date.replace('Z', "+00:00")) else {
        return Reply::base()
            .status(400)
            .json(json!({"error": "Invalid ISO date format"}));
    };
    let local = parsed.in_timezone(&chrono::Local);
    let result = {
        let app = app.clone();
        tokio::task::spawn_blocking(move || crate::clock::set_system_datetime(&app.host, &local))
            .await
    };
    match result {
        Ok(Ok(())) => Reply::base().json(json!({"status": "success"})),
        Ok(Err(crate::clock::SetTimeError::Failed(_))) => Reply::base()
            .status(500)
            .json(json!({"error": "Failed to set system time"})),
        _ => Reply::error(Kind::Base, 500, &req),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::fake::FakeRunner;
    use crate::testing::{app, call, call_with_headers, json as body};

    fn fake_host(app: &App) -> Arc<FakeRunner> {
        let runner = Arc::new(FakeRunner::default());
        app.host.replace(runner.clone());
        runner
    }

    /// tests/test_factory_reset.py::test_factory_reset_cleanup_preserves_hidden_identity_cache
    #[test]
    fn cleanup_preserves_hidden_identity_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let identity = dir.path().join(".device-identity").join("device-uuid");
        std::fs::create_dir(identity.parent().expect("parent")).expect("mkdir");
        std::fs::write(&identity, "123e4567-e89b-42d3-a456-426614174000\n").expect("write");
        std::fs::write(dir.path().join("settings.json"), "user data").expect("write");
        std::fs::create_dir(dir.path().join("profiles")).expect("mkdir");
        std::fs::write(dir.path().join("profiles/profile.json"), "{}").expect("write");

        cleanup_factory_reset_data(dir.path());

        assert_eq!(
            std::fs::read_to_string(&identity).expect("read"),
            "123e4567-e89b-42d3-a456-426614174000\n"
        );
        assert!(!dir.path().join("settings.json").exists());
        assert!(!dir.path().join("profiles").exists());
    }

    /// tests/test_factory_reset.py::test_factory_reset_cleanup_continues_after_deletion_error
    #[test]
    fn cleanup_continues_after_deletion_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let undeletable = dir.path().join("undeletable.json");
        let removable = dir.path().join("removable.json");
        std::fs::write(&undeletable, "in use").expect("write");
        std::fs::write(&removable, "user data").expect("write");
        cleanup_with(dir.path(), |path, _| {
            if path == undeletable {
                Err(std::io::Error::other("file is busy"))
            } else {
                std::fs::remove_file(path)
            }
        });
        assert!(undeletable.exists());
        assert!(!removable.exists());
    }

    fn user_root(app: &App) -> std::path::PathBuf {
        let root = app.env.paths.system.factory_reset_root.clone();
        std::fs::create_dir_all(root.join("profiles")).expect("mkdir");
        std::fs::write(root.join("profiles/profile.json"), "{}").expect("write");
        std::fs::write(root.join("config.yml"), "user: {}").expect("write");
        std::fs::create_dir_all(root.join(".device-identity")).expect("mkdir");
        root
    }

    fn assert_nothing_wiped(root: &Path, runner: &FakeRunner) {
        assert!(runner.calls.lock().is_empty());
        assert!(root.join("profiles/profile.json").exists());
        assert!(root.join("config.yml").exists());
    }

    /// tests/test_api_machine.py::TestFactoryResetRoute (not emulated).
    #[tokio::test]
    async fn factory_reset_route() {
        let (_dir, app) = crate::testing::app_with(crate::env::BackendMode::Fika, |_| {});
        let runner = fake_host(&app);
        let root = user_root(&app);
        let uri = "/api/v1/machine/factory_reset";

        let reply = call(&app, http::Method::GET, uri, "").await;
        assert_eq!(reply.status_code(), 400);
        assert_eq!(
            body(&reply),
            json!({"error": "Confirmation required. Add confirm=true"})
        );
        assert_nothing_wiped(&root, &runner);

        let reply = call(&app, http::Method::GET, &format!("{uri}?confirm=yes"), "").await;
        assert_eq!(reply.status_code(), 400);
        assert_nothing_wiped(&root, &runner);

        for host in ["meticulous.local", "localhost"] {
            let reply = call_with_headers(
                &app,
                http::Method::GET,
                &format!("{uri}?confirm=true"),
                "",
                &[("Host", host), ("X-Real-IP", "192.168.1.50")],
            )
            .await;
            assert_eq!(reply.status_code(), 403);
            assert_eq!(
                body(&reply),
                json!({"status": "error", "error": "This endpoint can only be accessed locally"})
            );
            assert_nothing_wiped(&root, &runner);
        }

        let reply = call(&app, http::Method::GET, &format!("{uri}?confirm=true"), "").await;
        assert_eq!(reply.status_code(), 200);
        assert!(reply.body().is_empty());
        assert_eq!(runner.argvs(), vec![vec!["reboot".to_string()]]);
        let left: Vec<String> = std::fs::read_dir(&root)
            .expect("dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec![".device-identity".to_string()]);
    }

    /// tests/test_api_machine.py::test_confirmed_reset_is_only_simulated_on_an_emulated_machine
    #[tokio::test]
    async fn factory_reset_is_simulated_when_emulated() {
        let (_dir, app) = app();
        let runner = fake_host(&app);
        let root = user_root(&app);
        let reply = call(
            &app,
            http::Method::GET,
            "/api/v1/machine/factory_reset?confirm=true",
            "",
        )
        .await;
        assert_eq!(reply.status_code(), 200);
        assert_eq!(
            body(&reply),
            json!({"status": "success", "message": "Emulated mode"})
        );
        assert_nothing_wiped(&root, &runner);
    }

    fn backlight_files(app: &App) -> std::path::PathBuf {
        let dir = app.env.paths.system.backlight.clone();
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("brightness"), "20\n").expect("write");
        std::fs::write(dir.join("max_brightness"), "200\n").expect("write");
        dir
    }

    async fn post_backlight(app: &Arc<App>, body: &str) -> Reply {
        call(
            app,
            http::Method::POST,
            "/api/v1/machine/backlight",
            body.to_string(),
        )
        .await
    }

    /// The handler tests drive the sysfs files, so the hardware is "real".
    fn real_backlight_app() -> (tempfile::TempDir, Arc<App>) {
        crate::testing::app_custom(crate::env::BackendMode::Fika, |env| {
            env.hooks.fake_hardware = false;
        })
    }

    /// tests/test_api_machine.py::TestBacklightRoute::test_get_is_not_supported
    #[tokio::test]
    async fn backlight_get_is_not_supported() {
        let (_dir, app) = app();
        let reply = call(&app, http::Method::GET, "/api/v1/machine/backlight", "").await;
        assert_eq!(reply.status_code(), 405);
    }

    /// tests/test_api_machine.py::TestBacklightRoute::test_brightness_is_written_to_sysfs_as_a_fraction_of_max
    #[tokio::test]
    async fn brightness_is_written_as_a_fraction_of_max() {
        let (_dir, app) = real_backlight_app();
        let dir = backlight_files(&app);
        let reply = post_backlight(
            &app,
            r#"{"brightness": 0.5, "interpolation": "linear", "animation_time": 0.04}"#,
        )
        .await;
        assert_eq!(reply.status_code(), 200);
        assert!(reply.body().is_empty());
        app.backlight.wait();
        assert_eq!(
            std::fs::read_to_string(dir.join("brightness")).expect("read"),
            "100"
        );
    }

    /// tests/test_api_machine.py::TestBacklightRoute::test_brightness_above_one_is_clamped_to_max
    #[tokio::test]
    async fn brightness_above_one_is_clamped_to_max() {
        let (_dir, app) = real_backlight_app();
        let dir = backlight_files(&app);
        let reply = post_backlight(&app, r#"{"brightness": 1.7, "animation_time": 0.04}"#).await;
        assert_eq!(reply.status_code(), 200);
        app.backlight.wait();
        assert_eq!(
            std::fs::read_to_string(dir.join("brightness")).expect("read"),
            "200"
        );
    }

    /// tests/test_api_machine.py::TestBacklightRoute::test_dimming_defaults_to_a_one_second_curve:
    /// a curve over 75 steps ends at the target.
    #[tokio::test]
    async fn dimming_defaults_to_a_one_second_curve() {
        let (_dir, app) = real_backlight_app();
        let dir = backlight_files(&app);
        let started = std::time::Instant::now();
        let reply = post_backlight(&app, r#"{"brightness": 0.8}"#).await;
        assert_eq!(reply.status_code(), 200);
        app.backlight.wait();
        assert!(started.elapsed() >= std::time::Duration::from_millis(900));
        assert_eq!(
            std::fs::read_to_string(dir.join("brightness")).expect("read"),
            "160"
        );
    }

    /// tests/test_api_machine.py::TestBacklightRoute::test_emulated_machine_never_touches_sysfs
    #[tokio::test]
    async fn emulated_machine_never_touches_sysfs() {
        let (_dir, app) = app();
        let dir = backlight_files(&app);
        let reply = post_backlight(&app, r#"{"brightness": 0.5, "animation_time": 0.04}"#).await;
        assert_eq!(reply.status_code(), 200);
        assert!(!app.backlight.is_adjusting());
        assert_eq!(
            std::fs::read_to_string(dir.join("brightness")).expect("read"),
            "20\n"
        );
    }

    /// tests/test_api_machine.py::TestBacklightRoute: null brightness, a
    /// missing one, invalid JSON.
    #[tokio::test]
    async fn backlight_rejections() {
        let (_dir, app) = real_backlight_app();
        let dir = backlight_files(&app);
        let reply = post_backlight(&app, r#"{"brightness": null}"#).await;
        assert_eq!(reply.status_code(), 200);
        assert!(!app.backlight.is_adjusting());

        let reply = post_backlight(&app, r#"{"interpolation": "linear"}"#).await;
        assert_eq!(reply.status_code(), 400);
        assert_eq!(
            body(&reply),
            json!({"status": "error", "error": "brightness value is required"})
        );

        let reply = post_backlight(&app, "{brightness").await;
        assert_eq!(reply.status_code(), 403);
        assert_eq!(
            body(&reply),
            json!({
                "status": "error",
                "error": "invalid json",
                "json_error": "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
            })
        );
        assert_eq!(
            post_backlight(&app, r#""brightness""#).await.status_code(),
            500
        );
        assert_eq!(post_backlight(&app, "5").await.status_code(), 500);
        assert_eq!(post_backlight(&app, "[1]").await.status_code(), 400);
        assert_eq!(
            std::fs::read_to_string(dir.join("brightness")).expect("read"),
            "20\n"
        );
    }

    async fn post_time(app: &Arc<App>, body: &str) -> Reply {
        call(
            app,
            http::Method::POST,
            "/api/v1/machine/time",
            body.to_string(),
        )
        .await
    }

    /// tests/test_api_machine.py::TestMachineTimeRoute::test_naive_iso_date_is_passed_to_date_command_unchanged
    #[tokio::test]
    async fn naive_iso_date_is_passed_to_date_unchanged() {
        let (_dir, app) = app();
        let runner = fake_host(&app);
        let reply = post_time(&app, r#"{"date": "2026-10-09T12:34:56"}"#).await;
        assert_eq!(reply.status_code(), 200);
        assert_eq!(body(&reply), json!({"status": "success"}));
        assert_eq!(
            runner.argvs(),
            vec![vec!["date", "--set", "2026-10-09 12:34:56"]]
        );
    }

    /// tests/test_api_machine.py::TestMachineTimeRoute: rejected requests
    /// never touch the clock.
    #[tokio::test]
    async fn invalid_time_requests_are_rejected() {
        let (_dir, app) = app();
        let runner = fake_host(&app);
        for (request, error) in [
            ("{not json", "Invalid JSON"),
            (
                r#"{"time": "2026-10-09T12:34:56Z"}"#,
                "Missing 'date' in request",
            ),
            (r#"{"date": ""}"#, "Missing 'date' in request"),
            (r#"{"date": "09/10/2026 12:34"}"#, "Invalid ISO date format"),
        ] {
            let reply = post_time(&app, request).await;
            assert_eq!(reply.status_code(), 400, "{request}");
            assert_eq!(body(&reply), json!({"error": error}));
        }
        assert_eq!(post_time(&app, r#"{"date": 5}"#).await.status_code(), 500);
        assert_eq!(post_time(&app, "[]").await.status_code(), 500);
        assert!(runner.calls.lock().is_empty());
    }

    /// tests/test_api_machine.py::TestMachineTimeRoute::test_failing_date_command_reports_500
    #[tokio::test]
    async fn failing_date_command_reports_500() {
        let (_dir, app) = app();
        let runner = fake_host(&app);
        runner.exit_with("date", 1);
        let reply = post_time(&app, r#"{"date": "2026-10-09T12:34:56"}"#).await;
        assert_eq!(reply.status_code(), 500);
        assert_eq!(body(&reply), json!({"error": "Failed to set system time"}));
        runner.missing("date");
        let reply = post_time(&app, r#"{"date": "2026-10-09T12:34:56"}"#).await;
        assert_eq!(reply.status_code(), 500);
        assert_eq!(reply.header("Content-Type"), Some("application/json"));
    }
}
