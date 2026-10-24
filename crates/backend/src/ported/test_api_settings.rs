//! Port of `tests/test_api_settings.py`: the settings, manufacturing,
//! timezone picker, serial number and root password routes through the real
//! handlers, with the ESP32 port, systemd, `timedatectl` and the IP lookup
//! replaced at their boundary.
//!
//! The Python suite points the config at a file that does not exist yet and
//! asserts it is never created; here the store writes its file when it
//! opens, so "not saved" means the file kept its bytes and no `settings`
//! event went out.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use http::Method;
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::app::App;
use crate::config::keys;
use crate::http::Reply;
use crate::http_client::{HttpClient, HttpError, HttpResponse};
use crate::py::Completed;
use crate::py::fake::RecordingRunner;
use crate::sounds::SoundEvent;
use crate::system_services::{RecordingSystemd, SystemdCall};
use crate::testing::{self, call, call_with_headers, json as body};
use crate::timezone::TIMEZONE_STATUS_COMMAND;

const TIMEZONE_URL: &str = "https://analytics.meticulousespresso.com/timezone_ip";

/// Values every test starts from (`USER_BASELINE`).
fn user_baseline() -> Value {
    json!({
        "enable_sounds": true,
        "clock_format_24_hour": true,
        "debug_shot_data_retention_days": 31,
        "heating_timeout": 10,
        "partial_retraction": 45.33,
        "auto_purge_after_shot": false,
        "tare_behavior": "after_retraction",
        "shot_data_sharing": null,
        "report_contact_mail": null,
        "update_channel": "",
        "usb_mode": "host",
        "timezone_sync": "automatic",
        "time_zone": "Etc/UTC",
        "ssh_enabled": true,
        "profile_order": [],
    })
}

/// `SYSTEM_BASELINE`.
fn system_baseline() -> Value {
    json!({
        "shot_data_sharing_id": null,
        "root_password": null,
        "serial": null,
        "color": null,
        "batch_number": null,
        "build_date": null,
    })
}

/// `MANUFACTURING_BASELINE`.
fn manufacturing_baseline() -> Value {
    json!({"enabled": false, "last_boot_mode": null, "skip_stage": false})
}

/// The `aiohttp.ClientSession` stand-in: each request gets the next body.
#[derive(Default)]
struct FakeHttp {
    bodies: Mutex<VecDeque<String>>,
    requested: Mutex<Vec<String>>,
}

impl HttpClient for FakeHttp {
    fn get(
        &self,
        url: &str,
        _timeout: Duration,
        _user_agent: Option<&str>,
    ) -> Result<HttpResponse, HttpError> {
        self.requested.lock().push(url.to_string());
        match self.bodies.lock().pop_front() {
            Some(body) => Ok(HttpResponse { status: 200, body }),
            None => Err(HttpError::Failed("no answer prepared".into())),
        }
    }

    fn status(
        &self,
        url: &str,
        timeout: Duration,
        user_agent: Option<&str>,
    ) -> Result<u16, HttpError> {
        self.get(url, timeout, user_agent).map(|r| r.status)
    }
}

/// `FakeSubprocess`: `timedatectl set-timezone` fails with `stderr` when given.
fn timedatectl(set_timezone_stderr: Option<&'static str>) -> Arc<RecordingRunner> {
    RecordingRunner::new(move |command, _shell| match set_timezone_stderr {
        Some(stderr) if command.starts_with("timedatectl set-timezone") => Ok(Completed {
            returncode: 1,
            stdout: String::new(),
            stderr: stderr.into(),
        }),
        _ => Ok(Completed::default()),
    })
}

/// `MachineBoundaryTestCase`: real handlers; only the ESP32 port, the host
/// and the network are faked.
struct Bench {
    _dir: tempfile::TempDir,
    app: Arc<App>,
    timedatectl: Arc<RecordingRunner>,
    http: Arc<FakeHttp>,
    systemd: Arc<RecordingSystemd>,
    /// The settings file as it was before the request.
    file_before: Vec<u8>,
}

impl Bench {
    fn new() -> Bench {
        let (dir, app) = testing::app();
        for (section, baseline) in [
            (keys::USER, user_baseline()),
            (keys::SYSTEM, system_baseline()),
            (keys::MANUFACTURING, manufacturing_baseline()),
        ] {
            for (key, value) in baseline.as_object().expect("baseline") {
                app.config.set(section, key, value.clone());
            }
        }
        app.machine.record_writes();
        let timedatectl = timedatectl(None);
        app.timezone.set_runner(timedatectl.clone());
        let http = Arc::new(FakeHttp::default());
        app.timezone.set_http(http.clone());
        app.timezone.set_sleep(|_| {});
        let systemd = Arc::new(RecordingSystemd::default());
        app.system_services.replace_systemd(systemd.clone());
        app.host
            .replace(Arc::new(crate::host::fake::FakeRunner::default()));
        app.sio.take_recorded();
        let file_before = std::fs::read(app.config.path()).expect("config file");
        Bench {
            _dir: dir,
            app,
            timedatectl,
            http,
            systemd,
            file_before,
        }
    }

    fn fail_set_timezone(&mut self, stderr: &'static str) {
        self.timedatectl = timedatectl(Some(stderr));
        self.app.timezone.set_runner(self.timedatectl.clone());
    }

    fn answer_timezone_lookup(&self, body: Value) {
        self.http.bodies.lock().push_back(body.to_string());
    }

    async fn get(&self, uri: &str) -> Reply {
        call(&self.app, Method::GET, uri, "").await
    }

    async fn post(&self, uri: &str, body: &str) -> Reply {
        call(&self.app, Method::POST, uri, body.to_string()).await
    }

    async fn post_settings(&self, payload: Value) -> Reply {
        self.post("/api/v1/settings", &payload.to_string()).await
    }

    fn user(&self) -> Value {
        self.app.config.section(keys::USER)
    }

    fn set(&self, section: &str, key: &str, value: Value) {
        self.app.config.set(section, key, value);
    }

    fn saved_config(&self) -> Value {
        let text = std::fs::read_to_string(self.app.config.path()).expect("config file");
        serde_norway::from_str(&text).expect("yaml")
    }

    fn writes(&self) -> Vec<Vec<u8>> {
        self.app.machine.writes()
    }

    fn set_esp_info(&self, info: esp::EspInfo) {
        self.app.machine.state.lock().esp_info = Some(info);
    }

    fn enable_manufacturing(&self) {
        self.app.machine.state.lock().enable_manufacturing = true;
    }

    /// `assert_saved`: the changes are live, the file holds the whole user
    /// section and one `settings` event went out.
    fn assert_saved(&self, changes: Value) {
        let user = self.user();
        for (key, value) in changes.as_object().expect("changes") {
            assert_eq!(user[key], *value, "{key}");
        }
        assert_eq!(self.saved_config()[keys::USER], user);
        assert_eq!(self.app.sio.recorded("settings"), vec![json!({})]);
    }

    fn assert_not_saved(&self) {
        assert_eq!(
            std::fs::read(self.app.config.path()).expect("config file"),
            self.file_before,
            "the settings file changed"
        );
        assert_eq!(self.app.sio.recorded("settings"), Vec::<Value>::new());
    }

    /// `assert_rejected`: the error, the baseline still live, nothing saved.
    fn assert_rejected(&self, reply: &Reply, status: u16, error: &str) {
        assert_eq!(reply.status_code(), status);
        assert_eq!(body(reply), json!({"status": "error", "error": error}));
        let user = self.user();
        for (key, value) in user_baseline().as_object().expect("baseline") {
            assert_eq!(user[key], *value, "{key}");
        }
        self.assert_not_saved();
    }
}

fn no_writes() -> Vec<Vec<u8>> {
    Vec::new()
}

// TestSettingsAPI

/// tests/test_api_settings.py::TestSettingsAPI::test_get_returns_the_whole_user_section
#[tokio::test]
async fn get_returns_the_whole_user_section() {
    let bench = Bench::new();
    let reply = bench.get("/api/v1/settings").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.header("Content-Type"), Some("application/json"));
    assert_eq!(body(&reply), bench.user());
}

/// tests/test_api_settings.py::TestSettingsAPI::test_get_with_trailing_slash_returns_the_whole_user_section
#[tokio::test]
async fn get_with_trailing_slash_returns_the_whole_user_section() {
    let bench = Bench::new();
    let reply = bench.get("/api/v1/settings/").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply), bench.user());
}

/// tests/test_api_settings.py::TestSettingsAPI::test_get_single_setting_returns_only_that_key
#[tokio::test]
async fn get_single_setting_returns_only_that_key() {
    let bench = Bench::new();
    let reply = bench.get("/api/v1/settings/heating_timeout").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply), json!({"heating_timeout": 10}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_get_accepts_repeated_slashes_and_no_slash_before_the_setting:
/// "[/]*" matches zero or more slashes, so both spellings reach the same key.
#[tokio::test]
async fn get_accepts_repeated_slashes_and_no_slash_before_the_setting() {
    let bench = Bench::new();
    for path in [
        "/api/v1/settings//enable_sounds",
        "/api/v1/settingsenable_sounds",
    ] {
        let reply = bench.get(path).await;
        assert_eq!(reply.status_code(), 200, "{path}");
        assert_eq!(body(&reply), json!({"enable_sounds": true}), "{path}");
    }
}

/// tests/test_api_settings.py::TestSettingsAPI::test_get_unknown_setting_returns_404
#[tokio::test]
async fn get_unknown_setting_returns_404() {
    let bench = Bench::new();
    let reply = bench.get("/api/v1/settings/not_a_setting").await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        body(&reply),
        json!({"status": "error", "error": "setting not found", "setting": "not_a_setting"})
    );
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_plain_setting_saves_and_returns_the_whole_section
#[tokio::test]
async fn post_plain_setting_saves_and_returns_the_whole_section() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"enable_sounds": false, "clock_format_24_hour": false}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply), bench.user());
    bench.assert_saved(json!({"enable_sounds": false, "clock_format_24_hour": false}));
    assert_eq!(bench.writes(), no_writes());
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_ignores_the_setting_name_in_the_path:
/// only the body keys are applied.
#[tokio::test]
async fn post_ignores_the_setting_name_in_the_path() {
    let bench = Bench::new();
    let reply = bench
        .post(
            "/api/v1/settings/enable_sounds",
            &json!({"clock_format_24_hour": false}).to_string(),
        )
        .await;

    assert_eq!(reply.status_code(), 200);
    bench.assert_saved(json!({"clock_format_24_hour": false, "enable_sounds": true}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_invalid_json_returns_403
#[tokio::test]
async fn post_invalid_json_returns_403() {
    let bench = Bench::new();
    let reply = bench.post("/api/v1/settings", "{not json").await;

    assert_eq!(reply.status_code(), 403);
    assert_eq!(
        body(&reply),
        json!({
            "status": "error",
            "error": "invalid json",
            "json_error": "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
        })
    );
    bench.assert_not_saved();
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_unknown_setting_returns_404_without_saving
#[tokio::test]
async fn post_unknown_setting_returns_404_without_saving() {
    let bench = Bench::new();
    let reply = bench.post_settings(json!({"not_a_setting": true})).await;

    bench.assert_rejected(&reply, 404, "'setting not_a_setting not found'");
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_wrong_type_returns_404_without_saving
#[tokio::test]
async fn post_wrong_type_returns_404_without_saving() {
    let bench = Bench::new();
    let reply = bench.post_settings(json!({"enable_sounds": "yes"})).await;

    bench.assert_rejected(
        &reply,
        404,
        "\"setting value invalid, received <class 'str'> and expected <class 'bool'>\"",
    );
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_rejects_heating_timeout_given_as_bool:
/// bool is a subclass of int, but the check compares exact types.
#[tokio::test]
async fn post_rejects_heating_timeout_given_as_bool() {
    let bench = Bench::new();
    let reply = bench.post_settings(json!({"heating_timeout": true})).await;

    bench.assert_rejected(
        &reply,
        404,
        "\"setting value invalid, received <class 'bool'> and expected <class 'int'>\"",
    );
    assert_eq!(bench.writes(), no_writes());
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_rejected_request_keeps_earlier_keys_out_of_the_config
#[tokio::test]
async fn post_rejected_request_keeps_earlier_keys_out_of_the_config() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"enable_sounds": false, "not_a_setting": 1}))
        .await;

    bench.assert_rejected(&reply, 404, "'setting not_a_setting not found'");
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_rejected_request_sends_nothing_to_the_esp32
/// (a strict xfail in Python).
#[tokio::test]
async fn post_rejected_request_sends_nothing_to_the_esp32() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"heating_timeout": 15, "not_a_setting": 1}))
        .await;

    assert_eq!(reply.status_code(), 404);
    // PARITY: each setting's side effect runs while the keys are walked, so
    // the timeout reaches the ESP32 although a later key fails the request
    // and the config keeps the old value.
    assert_eq!(bench.writes(), vec![b"heater_timeout,15\x03".to_vec()]);
    assert_eq!(bench.user()["heating_timeout"], json!(10));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_rejected_request_leaves_the_sharing_id_untouched
/// (a strict xfail in Python).
#[tokio::test]
async fn post_rejected_request_leaves_the_sharing_id_untouched() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"shot_data_sharing": true, "not_a_setting": 1}))
        .await;

    assert_eq!(reply.status_code(), 404);
    // PARITY: opting in writes a new sharing id into the live system config
    // before the later key rejects the request; it is only not saved.
    let sharing_id = bench
        .app
        .config
        .get_str(keys::SYSTEM, keys::SHOT_DATA_SHARING_ID)
        .expect("a sharing id");
    assert_eq!(
        uuid::Uuid::parse_str(&sharing_id)
            .expect("uuid")
            .get_version_num(),
        4
    );
    bench.assert_not_saved();
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_heating_timeout_writes_the_timeout_to_the_esp32
#[tokio::test]
async fn post_heating_timeout_writes_the_timeout_to_the_esp32() {
    let bench = Bench::new();
    let reply = bench.post_settings(json!({"heating_timeout": 15})).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(bench.writes(), vec![b"heater_timeout,15\x03".to_vec()]);
    bench.assert_saved(json!({"heating_timeout": 15}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_heating_timeout_above_one_hour_returns_400
#[tokio::test]
async fn post_heating_timeout_above_one_hour_returns_400() {
    let bench = Bench::new();
    let reply = bench.post_settings(json!({"heating_timeout": 61})).await;

    bench.assert_rejected(
        &reply,
        400,
        "Invalid heater timeout value: Timeout must be between 0 and 60 minutes",
    );
    assert_eq!(bench.writes(), no_writes());
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_partial_retraction_integer_is_stored_as_float_and_synced
#[tokio::test]
async fn post_partial_retraction_integer_is_stored_as_float_and_synced() {
    let bench = Bench::new();
    let reply = bench.post_settings(json!({"partial_retraction": 50})).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        bench.writes(),
        vec![b"nvs_request,write,partial_retraction_key,50.0\x03".to_vec()]
    );
    bench.assert_saved(json!({"partial_retraction": 50.0}));
    assert!(bench.user()["partial_retraction"].is_f64());
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_partial_retraction_matching_the_esp32_skips_the_write
#[tokio::test]
async fn post_partial_retraction_matching_the_esp32_skips_the_write() {
    let bench = Bench::new();
    bench.set_esp_info(esp::EspInfo {
        partial_retraction: 50.0,
        ..esp::EspInfo::default()
    });

    let reply = bench
        .post_settings(json!({"partial_retraction": 50.0}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(bench.writes(), no_writes());
    bench.assert_saved(json!({"partial_retraction": 50.0}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_partial_retraction_out_of_range_returns_400
#[tokio::test]
async fn post_partial_retraction_out_of_range_returns_400() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"partial_retraction": 68.0}))
        .await;

    bench.assert_rejected(
        &reply,
        400,
        "partial_retraction must be between 36.26 and 67.99 mm",
    );
    assert_eq!(bench.writes(), no_writes());
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_auto_purge_writes_the_flag_to_the_esp32
#[tokio::test]
async fn post_auto_purge_writes_the_flag_to_the_esp32() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"auto_purge_after_shot": true}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        bench.writes(),
        vec![b"nvs_request,write,auto_purge_after_shot_key,true\x03".to_vec()]
    );
    bench.assert_saved(json!({"auto_purge_after_shot": true}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_auto_purge_updates_the_cached_esp32_value
#[tokio::test]
async fn post_auto_purge_updates_the_cached_esp32_value() {
    let bench = Bench::new();
    bench.set_esp_info(esp::EspInfo {
        auto_purge_after_shot: true,
        ..esp::EspInfo::default()
    });

    let reply = bench
        .post_settings(json!({"auto_purge_after_shot": false}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        bench.writes(),
        vec![b"nvs_request,write,auto_purge_after_shot_key,false\x03".to_vec()]
    );
    assert!(
        !bench
            .app
            .machine
            .esp_info()
            .expect("esp info")
            .auto_purge_after_shot
    );
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_tare_behavior_is_written_when_firmware_supports_it
#[tokio::test]
async fn post_tare_behavior_is_written_when_firmware_supports_it() {
    let bench = Bench::new();
    bench.set_esp_info(esp::EspInfo {
        tare_behavior: Some("after_retraction".into()),
        ..esp::EspInfo::default()
    });

    let reply = bench
        .post_settings(json!({"tare_behavior": "before_retraction"}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        bench.writes(),
        vec![b"nvs_request,write,tare_behavior_key,before_retraction\x03".to_vec()]
    );
    assert_eq!(
        bench.app.machine.pending_tare_behavior_writes(),
        vec!["before_retraction".to_string()]
    );
    bench.assert_saved(json!({"tare_behavior": "before_retraction"}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_tare_behavior_is_saved_but_not_sent_to_older_firmware
#[tokio::test]
async fn post_tare_behavior_is_saved_but_not_sent_to_older_firmware() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"tare_behavior": "before_retraction"}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(bench.writes(), no_writes());
    bench.assert_saved(json!({"tare_behavior": "before_retraction"}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_unknown_tare_behavior_returns_400
#[tokio::test]
async fn post_unknown_tare_behavior_returns_400() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"tare_behavior": "sideways"}))
        .await;

    bench.assert_rejected(&reply, 400, "unsupported tare behavior: sideways");
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_shot_data_sharing_opt_in_generates_a_sharing_id
#[tokio::test]
async fn post_shot_data_sharing_opt_in_generates_a_sharing_id() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"shot_data_sharing": true}))
        .await;

    assert_eq!(reply.status_code(), 200);
    let sharing_id = bench
        .app
        .config
        .get_str(keys::SYSTEM, keys::SHOT_DATA_SHARING_ID)
        .expect("a sharing id");
    assert_eq!(
        uuid::Uuid::parse_str(&sharing_id)
            .expect("uuid")
            .get_version_num(),
        4
    );
    assert_eq!(
        bench.saved_config()[keys::SYSTEM][keys::SHOT_DATA_SHARING_ID],
        json!(sharing_id)
    );
    bench.assert_saved(json!({"shot_data_sharing": true}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_shot_data_sharing_opt_out_forgets_the_sharing_id
#[tokio::test]
async fn post_shot_data_sharing_opt_out_forgets_the_sharing_id() {
    let bench = Bench::new();
    bench.set(keys::USER, keys::SHOT_DATA_SHARING, json!(true));
    bench.set(
        keys::SYSTEM,
        keys::SHOT_DATA_SHARING_ID,
        json!("previous-id"),
    );

    let reply = bench
        .post_settings(json!({"shot_data_sharing": false}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        bench
            .app
            .config
            .get(keys::SYSTEM, keys::SHOT_DATA_SHARING_ID),
        Value::Null
    );
    bench.assert_saved(json!({"shot_data_sharing": false}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_shot_data_sharing_null_is_rejected
#[tokio::test]
async fn post_shot_data_sharing_null_is_rejected() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"shot_data_sharing": null}))
        .await;

    bench.assert_rejected(
        &reply,
        404,
        "\"setting value invalid, received <class 'NoneType'> and expected <class 'NoneType'>\"",
    );
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_report_contact_mail_is_trimmed
#[tokio::test]
async fn post_report_contact_mail_is_trimmed() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"report_contact_mail": "  barista@example.com "}))
        .await;

    assert_eq!(reply.status_code(), 200);
    bench.assert_saved(json!({"report_contact_mail": "barista@example.com"}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_empty_report_contact_mail_clears_it
#[tokio::test]
async fn post_empty_report_contact_mail_clears_it() {
    let bench = Bench::new();
    bench.set(
        keys::USER,
        keys::REPORT_CONTACT_MAIL,
        json!("barista@example.com"),
    );

    let reply = bench
        .post_settings(json!({"report_contact_mail": "   "}))
        .await;

    assert_eq!(reply.status_code(), 200);
    bench.assert_saved(json!({"report_contact_mail": null}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_malformed_report_contact_mail_returns_400
#[tokio::test]
async fn post_malformed_report_contact_mail_returns_400() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"report_contact_mail": "barista"}))
        .await;

    bench.assert_rejected(
        &reply,
        400,
        "report_contact_mail must look like name@domain",
    );
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_non_string_report_contact_mail_returns_400
#[tokio::test]
async fn post_non_string_report_contact_mail_returns_400() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"report_contact_mail": 42}))
        .await;

    bench.assert_rejected(&reply, 400, "report_contact_mail must be a string or null");
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_update_channel_writes_the_hawkbit_channel_file
#[tokio::test]
async fn post_update_channel_writes_the_hawkbit_channel_file() {
    let bench = Bench::new();
    let hawkbit = bench.app.env.paths.system.hawkbit_dir.clone();
    std::fs::create_dir_all(&hawkbit).expect("mkdir");

    let reply = bench.post_settings(json!({"update_channel": "beta"})).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        std::fs::read_to_string(hawkbit.join("channel")).expect("channel"),
        "beta\n"
    );
    bench.assert_saved(json!({"update_channel": "beta"}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_update_channel_without_hawkbit_dir_only_saves_the_setting
#[tokio::test]
async fn post_update_channel_without_hawkbit_dir_only_saves_the_setting() {
    let bench = Bench::new();
    let reply = bench.post_settings(json!({"update_channel": "beta"})).await;

    assert_eq!(reply.status_code(), 200);
    assert!(!bench.app.env.paths.system.hawkbit_dir.exists());
    bench.assert_saved(json!({"update_channel": "beta"}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_valid_usb_mode_is_saved:
/// switching the port is disabled and returns before touching the PD
/// controller.
#[tokio::test]
async fn post_valid_usb_mode_is_saved() {
    let bench = Bench::new();
    let reply = bench.post_settings(json!({"usb_mode": "client"})).await;

    assert_eq!(reply.status_code(), 200);
    bench.assert_saved(json!({"usb_mode": "client"}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_unknown_usb_mode_returns_400
#[tokio::test]
async fn post_unknown_usb_mode_returns_400() {
    let bench = Bench::new();
    let reply = bench.post_settings(json!({"usb_mode": "otg"})).await;

    bench.assert_rejected(
        &reply,
        400,
        "Failed to set the USB mode: 'otg' is not a valid USB_MODES",
    );
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_profile_order_emits_a_profile_reload
#[tokio::test]
async fn post_profile_order_emits_a_profile_reload() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"profile_order": ["a", "b"]}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        bench.app.sio.recorded("profile"),
        vec![json!({"change": "full_reload"})]
    );
    bench.assert_saved(json!({"profile_order": ["a", "b"]}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_ssh_disabled_stops_and_disables_the_unit
#[tokio::test]
async fn post_ssh_disabled_stops_and_disables_the_unit() {
    let bench = Bench::new();
    let reply = bench.post_settings(json!({"ssh_enabled": false})).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        *bench.systemd.calls.lock(),
        vec![
            SystemdCall::StopUnit("ssh.service".into(), "fail".into()),
            SystemdCall::DisableUnitFiles(vec!["ssh.service".into()], false),
            SystemdCall::Reload,
        ]
    );
    bench.assert_saved(json!({"ssh_enabled": false}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_ssh_enabled_enables_and_starts_the_unit
#[tokio::test]
async fn post_ssh_enabled_enables_and_starts_the_unit() {
    let bench = Bench::new();
    bench.set(keys::USER, keys::SSH_ENABLED, json!(false));

    let reply = bench.post_settings(json!({"ssh_enabled": true})).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        *bench.systemd.calls.lock(),
        vec![
            SystemdCall::EnableUnitFiles(vec!["ssh.service".into()], false, false),
            SystemdCall::StartUnit("ssh.service".into(), "fail".into()),
            SystemdCall::Reload,
        ]
    );
    bench.assert_saved(json!({"ssh_enabled": true}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_ssh_failure_returns_500_and_keeps_the_old_value
/// (a strict xfail in Python).
#[tokio::test]
async fn post_ssh_failure_returns_500_and_keeps_the_old_value() {
    let bench = Bench::new();
    bench
        .app
        .system_services
        .replace_systemd(Arc::new(RecordingSystemd {
            failure: Some("D-Bus unavailable".into()),
            ..RecordingSystemd::default()
        }));

    let reply = bench.post_settings(json!({"ssh_enabled": false})).await;

    assert_eq!(reply.status_code(), 500);
    // PARITY: the handler sets 500 but neither returns nor raises, so the
    // new value is saved anyway and the settings JSON follows the error
    // object in the body.
    let error = r#"{"status": "error", "setting": "ssh_enabled", "details": "Failed to update SSH service state"}"#;
    let text = std::str::from_utf8(reply.body()).expect("utf8");
    let settings = text.strip_prefix(error).expect("the error object first");
    assert_eq!(
        serde_json::from_str::<Value>(settings).expect("then the settings"),
        bench.user()
    );
    bench.assert_saved(json!({"ssh_enabled": false}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_time_zone_sets_the_system_timezone
/// (the zone is one argument here, not a shell word).
#[tokio::test]
async fn post_time_zone_sets_the_system_timezone() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"time_zone": "America/Mexico_City"}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        bench.timedatectl.calls(),
        vec![
            (
                "timedatectl set-timezone America/Mexico_City".to_string(),
                false
            ),
            (TIMEZONE_STATUS_COMMAND.to_string(), true),
        ]
    );
    bench.assert_saved(json!({"time_zone": "America/Mexico_City"}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_unchanged_time_zone_does_not_call_timedatectl
#[tokio::test]
async fn post_unchanged_time_zone_does_not_call_timedatectl() {
    let bench = Bench::new();
    let reply = bench.post_settings(json!({"time_zone": "Etc/UTC"})).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(bench.timedatectl.calls(), Vec::<(String, bool)>::new());
    bench.assert_saved(json!({"time_zone": "Etc/UTC"}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_time_zone_rejected_by_timedatectl_returns_400_with_redacted_zone
#[tokio::test]
async fn post_time_zone_rejected_by_timedatectl_returns_400_with_redacted_zone() {
    let mut bench = Bench::new();
    bench.fail_set_timezone("Invalid time zone");

    let reply = bench
        .post_settings(json!({"time_zone": "Mars/Olympus_Mons"}))
        .await;

    bench.assert_rejected(
        &reply,
        400,
        "Error updating timezone: Error setting system time zone: Command \
         'timedatectl set-timezone Mars/*****' returned non-zero exit status 1. \
         [ Out:  | Err: Invalid time zone ]",
    );
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_manual_timezone_sync_only_saves_the_mode
#[tokio::test]
async fn post_manual_timezone_sync_only_saves_the_mode() {
    let bench = Bench::new();
    let reply = bench
        .post_settings(json!({"timezone_sync": "manual"}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(bench.timedatectl.calls(), Vec::<(String, bool)>::new());
    bench.assert_saved(json!({"timezone_sync": "manual", "time_zone": "Etc/UTC"}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_automatic_timezone_sync_applies_the_fetched_zone
#[tokio::test]
async fn post_automatic_timezone_sync_applies_the_fetched_zone() {
    let bench = Bench::new();
    bench.answer_timezone_lookup(json!({"tz": "Europe/Berlin"}));
    bench.set(keys::USER, keys::TIMEZONE_SYNC, json!("manual"));

    let reply = bench
        .post_settings(json!({"timezone_sync": "automatic"}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(*bench.http.requested.lock(), vec![TIMEZONE_URL.to_string()]);
    assert_eq!(
        bench.timedatectl.calls()[0],
        ("timedatectl set-timezone Europe/Berlin".to_string(), false)
    );
    bench.assert_saved(json!({"timezone_sync": "automatic", "time_zone": "Europe/Berlin"}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_automatic_timezone_sync_without_known_zone_keeps_the_zone
#[tokio::test]
async fn post_automatic_timezone_sync_without_known_zone_keeps_the_zone() {
    let bench = Bench::new();
    bench.answer_timezone_lookup(json!({"tz": null}));
    bench.set(keys::USER, keys::TIMEZONE_SYNC, json!("manual"));

    let reply = bench
        .post_settings(json!({"timezone_sync": "automatic"}))
        .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(bench.timedatectl.calls(), Vec::<(String, bool)>::new());
    bench.assert_saved(json!({"timezone_sync": "automatic", "time_zone": "Etc/UTC"}));
}

/// tests/test_api_settings.py::TestSettingsAPI::test_post_automatic_timezone_sync_failing_to_apply_returns_400
#[tokio::test]
async fn post_automatic_timezone_sync_failing_to_apply_returns_400() {
    let mut bench = Bench::new();
    bench.answer_timezone_lookup(json!({"tz": "Europe/Berlin"}));
    bench.fail_set_timezone("Access denied");

    let reply = bench
        .post_settings(json!({"timezone_sync": "automatic"}))
        .await;

    bench.assert_rejected(
        &reply,
        400,
        "failed to sync timezone: failed to set the provided timezone",
    );
}

// TestManufacturingAPI

/// `dial_schema` in manufacturing.py.
fn dial_schema() -> Value {
    let options = json!([
        {"name": "enabled", "type": "boolean", "value": true},
        {"name": "disabled", "type": "boolean", "value": false},
    ]);
    json!({"Elements": [
        {"key": "skip_stage", "label": "Skip Stage", "options": options},
        {"key": "enabled", "label": "Enable Manufacturing", "options": options},
    ]})
}

/// tests/test_api_settings.py::TestManufacturingAPI::test_get_outside_manufacturing_returns_204_without_body
#[tokio::test]
async fn manufacturing_get_outside_manufacturing_returns_204_without_body() {
    let bench = Bench::new();
    let reply = bench.get("/api/v1/manufacturing").await;

    assert_eq!(reply.status_code(), 204);
    assert!(reply.body().is_empty());
}

/// tests/test_api_settings.py::TestManufacturingAPI::test_get_in_manufacturing_returns_the_dial_schema
#[tokio::test]
async fn manufacturing_get_in_manufacturing_returns_the_dial_schema() {
    let bench = Bench::new();
    bench.enable_manufacturing();

    let reply = bench.get("/api/v1/manufacturing/").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply), dial_schema());
}

/// tests/test_api_settings.py::TestManufacturingAPI::test_post_outside_manufacturing_returns_410
#[tokio::test]
async fn manufacturing_post_outside_manufacturing_returns_410() {
    let bench = Bench::new();
    let reply = bench
        .post("/api/v1/manufacturing", r#"{"skip_stage": true}"#)
        .await;

    assert_eq!(reply.status_code(), 410);
    assert_eq!(
        body(&reply),
        json!({"status": "error", "error": "no configuration available"})
    );
    assert_eq!(
        bench.app.config.section(keys::MANUFACTURING),
        manufacturing_baseline()
    );
    bench.assert_not_saved();
}

/// tests/test_api_settings.py::TestManufacturingAPI::test_post_in_manufacturing_saves_and_returns_the_section
#[tokio::test]
async fn manufacturing_post_in_manufacturing_saves_and_returns_the_section() {
    let bench = Bench::new();
    bench.enable_manufacturing();

    let reply = bench
        .post("/api/v1/manufacturing", r#"{"skip_stage": true}"#)
        .await;

    let expected = json!({"enabled": false, "last_boot_mode": null, "skip_stage": true});
    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply), expected);
    assert_eq!(bench.app.config.section(keys::MANUFACTURING), expected);
    assert_eq!(bench.saved_config()[keys::MANUFACTURING], expected);
    assert_eq!(bench.app.sio.recorded("settings"), vec![json!({})]);
}

/// tests/test_api_settings.py::TestManufacturingAPI::test_post_invalid_json_returns_403
#[tokio::test]
async fn manufacturing_post_invalid_json_returns_403() {
    let bench = Bench::new();
    bench.enable_manufacturing();

    let reply = bench.post("/api/v1/manufacturing", "{").await;

    assert_eq!(reply.status_code(), 403);
    assert_eq!(
        body(&reply),
        json!({
            "status": "error",
            "error": "invalid json",
            "json_error": "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
        })
    );
}

/// tests/test_api_settings.py::TestManufacturingAPI::test_post_unknown_key_returns_404_without_saving
#[tokio::test]
async fn manufacturing_post_unknown_key_returns_404_without_saving() {
    let bench = Bench::new();
    bench.enable_manufacturing();

    let reply = bench
        .post(
            "/api/v1/manufacturing",
            r#"{"skip_stage": true, "turbo": true}"#,
        )
        .await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        body(&reply),
        json!({"status": "error", "error": "'setting turbo not found'"})
    );
    assert_eq!(
        bench.app.config.section(keys::MANUFACTURING),
        manufacturing_baseline()
    );
    bench.assert_not_saved();
}

/// tests/test_api_settings.py::TestManufacturingAPI::test_post_wrong_type_returns_404_without_saving
#[tokio::test]
async fn manufacturing_post_wrong_type_returns_404_without_saving() {
    let bench = Bench::new();
    bench.enable_manufacturing();

    let reply = bench
        .post("/api/v1/manufacturing", r#"{"skip_stage": "yes"}"#)
        .await;

    assert_eq!(reply.status_code(), 404);
    assert_eq!(
        bench.app.config.section(keys::MANUFACTURING),
        manufacturing_baseline()
    );
    bench.assert_not_saved();
}

/// tests/test_api_settings.py::TestManufacturingAPI::test_post_wrong_type_names_the_expected_type
/// (a strict xfail in Python).
#[tokio::test]
async fn manufacturing_post_wrong_type_names_the_expected_type() {
    let bench = Bench::new();
    bench.enable_manufacturing();

    let reply = bench
        .post("/api/v1/manufacturing", r#"{"skip_stage": "yes"}"#)
        .await;

    // PARITY: the message names the type of the key (always str), not the
    // type of the stored value (bool).
    assert_eq!(
        body(&reply)["error"],
        json!("\"setting value invalid, received <class 'str'> and expected <class 'str'>\"")
    );
}

// TestTimezoneUIAPI

/// An app whose timezone picker table is the Python test's `TIMEZONES`.
fn timezone_ui_app() -> (tempfile::TempDir, Arc<App>) {
    let (dir, app) = testing::app();
    let path = &app.env.paths.timezone_json;
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        path,
        r#"{"Mexico": {"Cancun": "America/Cancun", "Mexico City": "America/Mexico_City"}, "Germany": {"Berlin": "Europe/Berlin"}}"#,
    )
    .expect("write");
    (dir, app)
}

/// tests/test_api_settings.py::TestTimezoneUIAPI::test_empty_region_lists_all_countries
#[tokio::test]
async fn timezones_empty_region_lists_all_countries() {
    let (_dir, app) = timezone_ui_app();
    let reply = call(&app, Method::GET, "/api/v1/timezones/", "").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply), json!({"countries": ["Mexico", "Germany"]}));
}

/// tests/test_api_settings.py::TestTimezoneUIAPI::test_countries_filter_matches_the_lowercase_prefix
#[tokio::test]
async fn timezones_countries_filter_matches_the_lowercase_prefix() {
    let (_dir, app) = timezone_ui_app();
    let reply = call(
        &app,
        Method::GET,
        "/api/v1/timezones/countries?filter=ge",
        "",
    )
    .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply), json!({"countries": ["Germany"]}));
}

/// tests/test_api_settings.py::TestTimezoneUIAPI::test_countries_filter_ignores_case
/// (a strict xfail in Python).
#[tokio::test]
async fn timezones_countries_filter_ignores_case() {
    let (_dir, app) = timezone_ui_app();
    let reply = call(
        &app,
        Method::GET,
        "/api/v1/timezones/countries?filter=Ge",
        "",
    )
    .await;

    // PARITY: the country is lowercased but the filter is not, so a filter
    // with an uppercase letter matches nothing.
    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply), json!({"countries": []}));
}

/// tests/test_api_settings.py::TestTimezoneUIAPI::test_cities_lists_the_zones_of_a_country
#[tokio::test]
async fn timezones_cities_lists_the_zones_of_a_country() {
    let (_dir, app) = timezone_ui_app();
    let reply = call(
        &app,
        Method::GET,
        "/api/v1/timezones/cities?filter=Mexico",
        "",
    )
    .await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body(&reply),
        json!({"cities": [{"Cancun": "America/Cancun"}, {"Mexico City": "America/Mexico_City"}]})
    );
}

/// tests/test_api_settings.py::TestTimezoneUIAPI::test_cities_of_an_unknown_country_returns_403
#[tokio::test]
async fn timezones_cities_of_an_unknown_country_returns_403() {
    let (_dir, app) = timezone_ui_app();
    let reply = call(
        &app,
        Method::GET,
        "/api/v1/timezones/cities?filter=Atlantis",
        "",
    )
    .await;

    assert_eq!(reply.status_code(), 403);
    assert_eq!(
        body(&reply),
        json!({"status": "error", "error": "invalid country requested"})
    );
}

/// tests/test_api_settings.py::TestTimezoneUIAPI::test_unknown_region_type_returns_403
#[tokio::test]
async fn timezones_unknown_region_type_returns_403() {
    let (_dir, app) = timezone_ui_app();
    let reply = call(&app, Method::GET, "/api/v1/timezones/regions", "").await;

    assert_eq!(reply.status_code(), 403);
    assert_eq!(
        body(&reply),
        json!({"status": "error", "error": "invalid region type requested"})
    );
}

/// tests/test_api_settings.py::TestTimezoneUIAPI::test_undecodable_filter_is_rejected_by_tornado_with_400:
/// tornado's get_argument refuses the value before the handler's own
/// UnicodeDecodeError branch (403) can run.
#[tokio::test]
async fn timezones_undecodable_filter_is_rejected_by_tornado_with_400() {
    let (_dir, app) = timezone_ui_app();
    let reply = call(
        &app,
        Method::GET,
        "/api/v1/timezones/countries?filter=%ff",
        "",
    )
    .await;

    assert_eq!(reply.status_code(), 400);
}

// TestSerialAPI

/// `WEBPAGE` in api/serial.py.
const WEBPAGE: &str = r#"
<!DOCTYPE html>
<html>
<head>
    <title>Product Form</title>
</head>
<body>
    <form action="serial" method="post">
        <label for="color">Color:</label>
        <input type="text" id="color" name="color" value="black"><br><br>

        <label for="serial">Serial:</label>
        <input type="text" id="serial" name="serial"><br><br>

        <label for="batch_number">Batch Number:</label>
        <input type="text" id="batch_number" name="batch_number"><br><br>

        <label for="build_date">Build Date:</label>
        <input type="date" id="build_date" name="build_date"><br><br>

        <input type="submit" value="Submit">
    </form>

    <script>
        // Set the build date to today's date
        document.addEventListener('DOMContentLoaded', () => {
            const today = new Date().toISOString().split('T')[0];
            document.getElementById('build_date').value = today;
        });
    </script>
</body>
</html>
"#;

/// `TestSerialAPI.FORM`.
const FORM: [(&str, &str); 4] = [
    ("color", "white"),
    ("serial", "MET-0042"),
    ("batch_number", "B7"),
    ("build_date", "2026-10-09"),
];

/// `post_form`: tornado's HTTP client sends a POST body as a form.
async fn post_form(bench: &Bench, form: &[(&str, &str)]) -> Reply {
    let encoded: Vec<String> = form.iter().map(|(k, v)| format!("{k}={v}")).collect();
    call_with_headers(
        &bench.app,
        Method::POST,
        "/api/v1/serial",
        encoded.join("&"),
        &[("Content-Type", "application/x-www-form-urlencoded")],
    )
    .await
}

fn form_without(field: &str) -> Vec<(&'static str, &'static str)> {
    FORM.iter().copied().filter(|(k, _)| *k != field).collect()
}

/// tests/test_api_settings.py::TestSerialAPI::test_get_serves_the_serial_form
#[tokio::test]
async fn serial_get_serves_the_serial_form() {
    let bench = Bench::new();
    let reply = bench.get("/api/v1/serial").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.header("Content-Type"), Some("text/html"));
    assert_eq!(std::str::from_utf8(reply.body()).expect("utf8"), WEBPAGE);
}

/// tests/test_api_settings.py::TestSerialAPI::test_post_writes_the_identity_to_the_esp32_nvs
#[tokio::test]
async fn serial_post_writes_the_identity_to_the_esp32_nvs() {
    let bench = Bench::new();
    let reply = post_form(&bench, &FORM).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.header("Content-Type"), Some("text/html"));
    assert_eq!(
        std::str::from_utf8(reply.body()).expect("utf8"),
        "Received Data:<br>Color: white<br>Serial: MET-0042<br>\
         Batch Number: B7<br>Build Date: 2026-10-09"
    );
    assert_eq!(
        bench.writes(),
        vec![
            b"nvs_request,write,color_key,white\x03".to_vec(),
            b"nvs_request,write,serial_number_key,MET-0042\x03".to_vec(),
            b"nvs_request,write,batch_number_key,B7\x03".to_vec(),
            b"nvs_request,write,build_date_key,2026-10-09\x03".to_vec(),
        ]
    );
}

/// tests/test_api_settings.py::TestSerialAPI::test_post_stores_the_identity_in_the_system_config
#[tokio::test]
async fn serial_post_stores_the_identity_in_the_system_config() {
    let bench = Bench::new();
    post_form(&bench, &FORM).await;

    let saved_system = bench.saved_config()[keys::SYSTEM].clone();
    for (key, value) in [
        (keys::MACHINE_SERIAL_NUMBER, "MET-0042"),
        (keys::MACHINE_COLOR, "white"),
        (keys::MACHINE_BATCH_NUMBER, "B7"),
        (keys::MACHINE_BUILD_DATE, "2026-10-09"),
    ] {
        assert_eq!(
            bench.app.config.get(keys::SYSTEM, key),
            json!(value),
            "{key}"
        );
        assert_eq!(saved_system[key], json!(value), "{key}");
    }
    assert_eq!(bench.app.sio.recorded("settings"), vec![json!({})]);
}

/// tests/test_api_settings.py::TestSerialAPI::test_post_raises_a_notification_with_the_identity
#[tokio::test]
async fn serial_post_raises_a_notification_with_the_identity() {
    let bench = Bench::new();
    post_form(&bench, &FORM).await;

    let notifications = bench.app.notifications.all();
    let [notification] = notifications.as_slice() else {
        panic!("expected one notification, got {}", notifications.len());
    };
    assert_eq!(
        notification.message,
        format!(
            "\nSerial number: MET-0042\n\nBatch number: B7\n\nColor: white\n\n\
             Build Date: 2026-10-09\n{}",
            " ".repeat(12)
        )
    );
    assert_eq!(notification.responses, vec!["Ok".to_string()]);
    // The Python queues it for the socket; here it goes out at once.
    assert_eq!(
        bench.app.sio.recorded("notification"),
        vec![Value::String(notification.to_json())]
    );
    assert_eq!(
        *bench.app.sounds.events.lock(),
        vec![SoundEvent::Notification]
    );
}

/// tests/test_api_settings.py::TestSerialAPI::test_post_without_color_defaults_to_black
#[tokio::test]
async fn serial_post_without_color_defaults_to_black() {
    let bench = Bench::new();
    let reply = post_form(&bench, &form_without("color")).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        bench.writes().first(),
        Some(&b"nvs_request,write,color_key,black\x03".to_vec())
    );
}

/// tests/test_api_settings.py::TestSerialAPI::test_post_empty_field_answers_200_with_a_message_and_writes_nothing
#[tokio::test]
async fn serial_post_empty_field_answers_200_with_a_message_and_writes_nothing() {
    let bench = Bench::new();
    for (field, message) in [
        ("color", "Color is required"),
        ("serial", "Serial is required"),
        ("batch_number", "Batch number is required"),
        ("build_date", "Build date is required"),
    ] {
        let form: Vec<(&str, &str)> = FORM
            .iter()
            .map(|&(k, v)| (k, if k == field { "" } else { v }))
            .collect();

        let reply = post_form(&bench, &form).await;

        assert_eq!(reply.status_code(), 200, "{field}");
        assert_eq!(
            std::str::from_utf8(reply.body()).expect("utf8"),
            message,
            "{field}"
        );
    }
    assert_eq!(bench.writes(), no_writes());
    bench.assert_not_saved();
}

/// tests/test_api_settings.py::TestSerialAPI::test_post_missing_field_returns_400
#[tokio::test]
async fn serial_post_missing_field_returns_400() {
    let bench = Bench::new();
    let reply = post_form(&bench, &form_without("serial")).await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(bench.writes(), no_writes());
    bench.assert_not_saved();
}

/// tests/test_api_settings.py::TestSerialAPI::test_scale_calibrate_sends_the_master_calibration_action
#[tokio::test]
async fn scale_calibrate_sends_the_master_calibration_action() {
    let bench = Bench::new();
    let reply = bench.get("/api/v1/scaleCalibrate").await;

    assert_eq!(reply.status_code(), 200);
    assert!(reply.body().is_empty());
    assert_eq!(
        bench.writes(),
        vec![b"action,scale_master_calibration\x03".to_vec()]
    );
}

// TestRootPasswordAPI

/// tests/test_api_settings.py::TestRootPasswordAPI::test_returns_the_configured_password
#[tokio::test]
async fn root_password_returns_the_configured_password() {
    let bench = Bench::new();
    bench.set(keys::SYSTEM, keys::ROOT_PASSWORD, json!("s3cretpw9"));

    let reply = bench.get("/api/v1/machine/root-password").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body(&reply),
        json!({"status": "success", "root_password": "s3cretpw9"})
    );
}
