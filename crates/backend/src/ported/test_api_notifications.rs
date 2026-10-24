//! Port of `tests/test_api_notifications.py`: the notifications the Dial
//! lists and acknowledges, and the post-update smoke validation state.

use std::path::PathBuf;
use std::sync::Arc;

use http::Method;
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::app::App;
use crate::config::keys;
use crate::http::Reply;
use crate::notifications::{Notification, now_seconds, responses};
use crate::testing::{self, call};

const TTL_SECONDS: f64 = 3600.0;

/// `make_notification`: a fixed timestamp makes the isoformat exact.
fn make_notification(message: &str, responses: &[&str], image: Option<&str>) -> Notification {
    let mut notification = Notification::with_responses(message, responses);
    notification.image = image.map(str::to_string);
    notification.timestamp = chrono::NaiveDate::from_ymd_opt(2026, 10, 9)
        .and_then(|d| d.and_hms_micro_opt(8, 30, 15, 123_456))
        .expect("timestamp");
    notification
}

fn serialized(notification: &Notification) -> Value {
    json!({
        "id": notification.id,
        "message": notification.message,
        "image": notification.image,
        "responses": notification.responses,
        "timestamp": "2026-10-09T08:30:15.123456",
    })
}

/// `TestNotificationsAPI.setUp`: two pending notifications and one
/// acknowledged a moment ago.
struct Notifications {
    _dir: tempfile::TempDir,
    app: Arc<App>,
    update: Notification,
    descale: Notification,
    recently_acknowledged: Notification,
}

fn notifications() -> Notifications {
    notifications_with(|_| {})
}

fn notifications_with(adjust: impl FnOnce(&mut Notification)) -> Notifications {
    let (dir, app) = testing::app();
    app.config.set(
        keys::SYSTEM,
        keys::NOTIFICATION_KEEPALIVE,
        json!(TTL_SECONDS as i64),
    );
    let update = make_notification(
        "Update available",
        &[responses::UPDATE, responses::SKIP],
        Some("data:image/png;base64,AAAA"),
    );
    let descale = make_notification("Descale soon", &[responses::OK], None);
    let mut recently_acknowledged = make_notification("Old news", &[responses::OK], None);
    recently_acknowledged.acknowledged = true;
    recently_acknowledged.response = Some(responses::OK.to_string());
    recently_acknowledged.acknowledged_at = Some(now_seconds());
    adjust(&mut recently_acknowledged);
    app.notifications.replace_all(vec![
        update.clone(),
        descale.clone(),
        recently_acknowledged.clone(),
    ]);
    Notifications {
        _dir: dir,
        app,
        update,
        descale,
        recently_acknowledged,
    }
}

impl Notifications {
    async fn get(&self, uri: &str) -> Reply {
        call(&self.app, Method::GET, uri, "").await
    }

    async fn acknowledge(&self, payload: &str, path: &str) -> Reply {
        call(&self.app, Method::POST, path, payload.to_string()).await
    }

    fn listed(&self, id: &str) -> Option<Notification> {
        self.app
            .notifications
            .all()
            .into_iter()
            .find(|n| n.id == id)
    }
}

const ACKNOWLEDGE: &str = "/api/v1/notifications/acknowledge";

fn ack_body(id: &str, response: &str) -> String {
    json!({"id": id, "response": response}).to_string()
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_get_lists_only_unacknowledged_notifications
#[tokio::test]
async fn get_lists_only_unacknowledged_notifications() {
    let n = notifications();

    let reply = n.get("/api/v1/notifications").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.header("Content-Type"), Some("application/json"));
    let expected = json!([serialized(&n.update), serialized(&n.descale)]);
    assert_eq!(testing::json(&reply), expected);
    // `json.dumps` of the list: the keys in this order, Python's separators.
    assert_eq!(reply.body(), pyfmt::dumps_default(&expected).as_bytes());
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_get_with_acknowledged_true_includes_acknowledged_ones
#[tokio::test]
async fn get_with_acknowledged_true_includes_acknowledged_ones() {
    let n = notifications();
    for flag in ["true", "TRUE"] {
        let reply = n
            .get(&format!("/api/v1/notifications?acknowledged={flag}"))
            .await;

        assert_eq!(reply.status_code(), 200, "{flag}");
        assert_eq!(
            testing::json(&reply),
            json!([
                serialized(&n.update),
                serialized(&n.descale),
                serialized(&n.recently_acknowledged),
            ]),
            "{flag}"
        );
    }
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_get_with_acknowledged_other_than_true_hides_acknowledged_ones
#[tokio::test]
async fn get_with_acknowledged_other_than_true_hides_acknowledged_ones() {
    let n = notifications();

    let reply = n.get("/api/v1/notifications?acknowledged=1").await;

    assert_eq!(
        testing::json(&reply),
        json!([serialized(&n.update), serialized(&n.descale)])
    );
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_get_drops_notifications_acknowledged_longer_than_the_ttl_ago
#[tokio::test]
async fn get_drops_notifications_acknowledged_longer_than_the_ttl_ago() {
    let n = notifications_with(|old| {
        old.acknowledged_at = Some(now_seconds() - TTL_SECONDS - 1.0);
    });

    let reply = n.get("/api/v1/notifications?acknowledged=true").await;

    assert_eq!(
        testing::json(&reply),
        json!([serialized(&n.update), serialized(&n.descale)])
    );
    // Gone from the list itself, not only from the answer.
    assert_eq!(
        n.app
            .notifications
            .is_acknowledged(&n.recently_acknowledged.id),
        None
    );
    assert_eq!(
        n.app.notifications.is_acknowledged(&n.update.id),
        Some(false)
    );
    assert_eq!(
        n.app.notifications.is_acknowledged(&n.descale.id),
        Some(false)
    );
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_get_with_no_notifications_returns_an_empty_list
#[tokio::test]
async fn get_with_no_notifications_returns_an_empty_list() {
    let n = notifications();
    n.app.notifications.replace_all(Vec::new());

    let reply = n.get("/api/v1/notifications").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(testing::json(&reply), json!([]));
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_get_on_the_acknowledge_route_also_lists_notifications
#[tokio::test]
async fn get_on_the_acknowledge_route_also_lists_notifications() {
    let n = notifications();

    // Both patterns map to GetNotificationsHandler, so GET works on either.
    let reply = n.get(ACKNOWLEDGE).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        testing::json(&reply),
        json!([serialized(&n.update), serialized(&n.descale)])
    );
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_post_acknowledges_with_the_chosen_response
#[tokio::test]
async fn post_acknowledges_with_the_chosen_response() {
    let n = notifications();

    let reply = n
        .acknowledge(&ack_body(&n.update.id, "Skip"), ACKNOWLEDGE)
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(testing::json(&reply), json!({"status": "success"}));
    let update = n.listed(&n.update.id).expect("update listed");
    assert!(update.acknowledged);
    assert_eq!(update.response.as_deref(), Some("Skip"));
    assert_eq!(
        n.app.notifications.is_acknowledged(&n.descale.id),
        Some(false)
    );
    let listed = n.get("/api/v1/notifications").await;
    assert_eq!(testing::json(&listed), json!([serialized(&n.descale)]));
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_post_on_the_list_route_also_acknowledges
#[tokio::test]
async fn post_on_the_list_route_also_acknowledges() {
    let n = notifications();

    let reply = n
        .acknowledge(&ack_body(&n.descale.id, "Ok"), "/api/v1/notifications")
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(testing::json(&reply), json!({"status": "success"}));
    assert_eq!(
        n.app.notifications.is_acknowledged(&n.descale.id),
        Some(true)
    );
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_post_unknown_id_returns_404
#[tokio::test]
async fn post_unknown_id_returns_404() {
    let n = notifications();

    let reply = n
        .acknowledge(&ack_body("not-a-notification", "Ok"), ACKNOWLEDGE)
        .await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        testing::json(&reply),
        json!({"status": "failure", "message": "Notification not found"})
    );
    assert_eq!(
        n.app.notifications.is_acknowledged(&n.update.id),
        Some(false)
    );
    assert_eq!(
        n.app.notifications.is_acknowledged(&n.descale.id),
        Some(false)
    );
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_post_for_an_already_acknowledged_notification_returns_404
#[tokio::test]
async fn post_for_an_already_acknowledged_notification_returns_404() {
    let n = notifications();

    let reply = n
        .acknowledge(&ack_body(&n.recently_acknowledged.id, "Ok"), ACKNOWLEDGE)
        .await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        testing::json(&reply),
        json!({"status": "failure", "message": "Notification not found"})
    );
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_post_without_id_returns_404
#[tokio::test]
async fn post_without_id_returns_404() {
    let n = notifications();

    let reply = n
        .acknowledge(&json!({"response": "Ok"}).to_string(), ACKNOWLEDGE)
        .await;

    assert_eq!(reply.status_code(), 404);
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_post_invalid_json_returns_500
#[tokio::test]
async fn post_invalid_json_returns_500() {
    let n = notifications();

    // json.loads is not guarded in the handler, so tornado answers 500.
    let reply = n.acknowledge("{not json", ACKNOWLEDGE).await;

    assert_eq!(reply.status_code(), 500);
}

/// tests/test_api_notifications.py::TestNotificationsAPI::test_post_runs_the_notification_callback_once
///
/// PARITY: a strict xfail in the Python (acknowledge_notification calls the
/// callback after Notification.acknowledge already did, notifications.py
/// 71-72 and 126-128), not a crash, so the port runs it twice too. Flip
/// this when notifications.py is fixed.
#[tokio::test]
async fn post_runs_the_notification_callback_once() {
    let n = notifications();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let manager = Arc::downgrade(&n.app.notifications);
    let id = n.update.id.clone();
    let seen = calls.clone();
    let mut update = n.update.clone();
    update.callback = Some(Arc::new(move || {
        let response = manager
            .upgrade()
            .and_then(|m| m.all().into_iter().find(|x| x.id == id))
            .and_then(|x| x.response);
        seen.lock().push(response);
    }));
    n.app
        .notifications
        .replace_all(vec![update, n.descale.clone()]);

    let reply = n
        .acknowledge(&ack_body(&n.update.id, "Update"), ACKNOWLEDGE)
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        *calls.lock(),
        vec![Some("Update".to_string()), Some("Update".to_string())]
    );
}

// TestSmokeValidationAPI

/// `TestSmokeValidationAPI.setUp`: the image version on disk, an ESP32 that
/// reported its info, an idle machine.
struct Smoke {
    _dir: tempfile::TempDir,
    app: Arc<App>,
    state_file: PathBuf,
    version_file: PathBuf,
}

fn smoke() -> Smoke {
    let (dir, app) = testing::app();
    let state_file = app.env.paths.smoke_validation_state.clone();
    let version_file = app.env.paths.smoke_validation_version.clone();
    std::fs::create_dir_all(version_file.parent().expect("parent")).expect("version folder");
    std::fs::write(&version_file, "2026M1446-beta\n").expect("version");
    {
        let mut machine = app.machine.state.lock();
        machine.info_ready = true;
        machine.esp_info = Some(esp::EspInfo {
            firmware_v: "1.4.2".into(),
            ..esp::EspInfo::default()
        });
        machine.data_sensors.state = "idle".into();
        machine.data_sensors.status = Some("idle".into());
    }
    Smoke {
        _dir: dir,
        app,
        state_file,
        version_file,
    }
}

impl Smoke {
    async fn get(&self) -> Reply {
        call(&self.app, Method::GET, "/api/v1/smoke-validation", "").await
    }

    fn write_state(&self) {
        let state = json!({
            "image_version": "2026M1446-beta",
            "post_update_shot_completed": true,
            "post_update_shot_completed_at": "2026-10-09T07:00:00+00:00",
            "post_update_shot_completed_version": "2026M1446-beta",
        });
        std::fs::write(&self.state_file, state.to_string()).expect("state");
    }
}

/// tests/test_api_notifications.py::TestSmokeValidationAPI::test_fresh_image_reports_no_post_update_shot_yet
#[tokio::test]
async fn fresh_image_reports_no_post_update_shot_yet() {
    let s = smoke();

    let reply = s.get().await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        testing::json(&reply),
        json!({
            "backend_initialized": true,
            "esp32_responding": true,
            "machine_state": "idle",
            "machine_status": "idle",
            "firmware_version": "1.4.2",
            "image_version": "2026M1446-beta",
            "post_update_shot_completed": false,
            "post_update_shot_completed_at": null,
            "post_update_shot_completed_version": null,
        })
    );
    // A missing state file is answered from defaults without being created.
    assert!(!s.state_file.exists());
}

/// tests/test_api_notifications.py::TestSmokeValidationAPI::test_completed_shot_on_the_current_image_is_reported
#[tokio::test]
async fn completed_shot_on_the_current_image_is_reported() {
    let s = smoke();
    s.write_state();
    {
        let mut machine = s.app.machine.state.lock();
        machine.data_sensors.state = "brewing".into();
        machine.data_sensors.status = Some("heating".into());
    }

    let reply = s.get().await;

    let body = testing::json(&reply);
    assert_eq!(reply.status_code(), 200);
    assert_eq!(body["machine_state"], json!("brewing"));
    assert_eq!(body["machine_status"], json!("heating"));
    assert_eq!(body["post_update_shot_completed"], json!(true));
    assert_eq!(
        body["post_update_shot_completed_at"],
        json!("2026-10-09T07:00:00+00:00")
    );
    assert_eq!(
        body["post_update_shot_completed_version"],
        json!("2026M1446-beta")
    );
}

/// tests/test_api_notifications.py::TestSmokeValidationAPI::test_esp32_without_info_is_reported_as_not_responding
#[tokio::test]
async fn esp32_without_info_is_reported_as_not_responding() {
    let s = smoke();
    s.app.machine.state.lock().info_ready = false;

    let reply = s.get().await;

    let body = testing::json(&reply);
    assert_eq!(body["esp32_responding"], json!(false));
    assert_eq!(body["firmware_version"], json!("1.4.2"));
}

/// tests/test_api_notifications.py::TestSmokeValidationAPI::test_missing_version_file_reports_unknown_image
#[tokio::test]
async fn missing_version_file_reports_unknown_image() {
    let s = smoke();
    std::fs::remove_file(&s.version_file).expect("remove");

    let reply = s.get().await;

    assert_eq!(testing::json(&reply)["image_version"], json!("unknown"));
}

/// tests/test_api_notifications.py::TestSmokeValidationAPI::test_unparseable_state_file_falls_back_to_defaults
#[tokio::test]
async fn unparseable_state_file_falls_back_to_defaults() {
    let s = smoke();
    std::fs::write(&s.state_file, "{truncated").expect("state");

    let reply = s.get().await;

    let body = testing::json(&reply);
    assert_eq!(reply.status_code(), 200);
    assert_eq!(body["post_update_shot_completed"], json!(false));
    assert_eq!(body["image_version"], json!("2026M1446-beta"));
}
