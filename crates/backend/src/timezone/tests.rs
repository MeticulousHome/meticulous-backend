//! The timezone parts of tests/test_api_settings.py and
//! tests/test_api_machine.py, at the `TimezoneManager` boundary: the
//! `timedatectl` commands, their errors, the IP lookup and the picker table.

use std::collections::VecDeque;

use super::*;
use crate::http_client::HttpResponse;
use crate::py::RunError;
use crate::py::fake::RecordingRunner;

struct FakeHttp {
    responses: Mutex<VecDeque<Result<HttpResponse, HttpError>>>,
    requested: Mutex<Vec<String>>,
}

impl FakeHttp {
    fn new(responses: Vec<Result<HttpResponse, HttpError>>) -> Arc<FakeHttp> {
        Arc::new(FakeHttp {
            responses: Mutex::new(responses.into()),
            requested: Mutex::new(Vec::new()),
        })
    }

    fn body(body: &str) -> Arc<FakeHttp> {
        FakeHttp::new(vec![Ok(HttpResponse {
            status: 200,
            body: body.into(),
        })])
    }
}

impl HttpClient for FakeHttp {
    fn get(
        &self,
        url: &str,
        _timeout: Duration,
        _agent: Option<&str>,
    ) -> Result<HttpResponse, HttpError> {
        self.requested.lock().push(url.into());
        self.responses
            .lock()
            .pop_front()
            .unwrap_or(Err(HttpError::Failed("no more responses".into())))
    }

    fn status(&self, url: &str, timeout: Duration, agent: Option<&str>) -> Result<u16, HttpError> {
        self.get(url, timeout, agent).map(|r| r.status)
    }
}

fn no_sleep(_: Duration) {}

/// The `FakeSubprocess` of tests/test_api_settings.py: `timedatectl
/// set-timezone` fails with `stderr` when given.
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

fn manager(runner: Arc<RecordingRunner>) -> (tempfile::TempDir, Arc<crate::app::App>) {
    let (dir, app) = crate::testing::app();
    app.timezone.set_runner(runner);
    app.timezone.set_sleep(no_sleep);
    app.config
        .set(keys::USER, keys::TIME_ZONE, json!("Etc/UTC"));
    (dir, app)
}

/// test_api_settings.py::test_post_time_zone_sets_the_system_timezone (the
/// zone is one argument here, not a shell word).
#[test]
fn update_sets_the_system_timezone() {
    let runner = timedatectl(None);
    let (_dir, app) = manager(runner.clone());
    app.timezone
        .update_timezone("America/Mexico_City")
        .expect("set");
    assert_eq!(
        runner.calls(),
        vec![
            (
                "timedatectl set-timezone America/Mexico_City".to_string(),
                false
            ),
            (TIMEZONE_STATUS_COMMAND.to_string(), true),
        ]
    );
    // Surrounding quotes are stripped.
    app.timezone
        .update_timezone("\"Etc/UTC\"")
        .expect("same zone");
    assert_eq!(runner.calls().len(), 2);
}

/// test_api_settings.py::test_post_unchanged_time_zone_does_not_call_timedatectl
#[test]
fn unchanged_zone_does_not_call_timedatectl() {
    let runner = timedatectl(None);
    let (_dir, app) = manager(runner.clone());
    app.timezone.update_timezone("Etc/UTC").expect("unchanged");
    assert!(runner.calls().is_empty());
}

/// test_api_settings.py::test_post_time_zone_rejected_by_timedatectl_returns_400_with_redacted_zone
#[test]
fn rejected_zone_error_is_redacted() {
    let (_dir, app) = manager(timedatectl(Some("Invalid time zone")));
    let error = app
        .timezone
        .update_timezone("Mars/Olympus_Mons")
        .unwrap_err();
    assert_eq!(
        error.0,
        "Error updating timezone: Error setting system time zone: Command \
         'timedatectl set-timezone Mars/*****' returned non-zero exit status 1. \
         [ Out:  | Err: Invalid time zone ]"
    );
}

/// test_api_settings.py::test_post_automatic_timezone_sync_applies_the_fetched_zone
#[test]
fn automatic_sync_applies_the_fetched_zone() {
    let runner = timedatectl(None);
    let (_dir, app) = manager(runner.clone());
    let http = FakeHttp::body("{\"tz\": \"Europe/Berlin\"}");
    app.timezone.set_http(http.clone());

    let zone = app.timezone.request_and_sync_tz().expect("synced");

    assert_eq!(zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(*http.requested.lock(), vec![TZ_GETTER_URL.to_string()]);
    assert_eq!(
        runner.calls()[0],
        ("timedatectl set-timezone Europe/Berlin".to_string(), false)
    );
}

/// test_api_settings.py::test_post_automatic_timezone_sync_without_known_zone_keeps_the_zone
#[test]
fn automatic_sync_without_known_zone_keeps_the_zone() {
    let runner = timedatectl(None);
    let (_dir, app) = manager(runner.clone());
    app.timezone.set_http(FakeHttp::body("{\"tz\": null}"));

    assert_eq!(app.timezone.request_and_sync_tz(), Ok(None));
    assert!(runner.calls().is_empty());
}

/// test_api_settings.py::test_post_automatic_timezone_sync_failing_to_apply_returns_400
#[test]
fn automatic_sync_failing_to_apply() {
    let (_dir, app) = manager(timedatectl(Some("Access denied")));
    app.timezone
        .set_http(FakeHttp::body("{\"tz\": \"Europe/Berlin\"}"));

    assert_eq!(
        app.timezone.request_and_sync_tz(),
        Err(TimezoneSyncError(
            "failed to set the provided timezone".into()
        ))
    );
}

/// A non-200 answer is retried; a connection error and a body that is not
/// JSON escape as they are.
#[test]
fn automatic_sync_retries_and_errors() {
    let (_dir, app) = manager(timedatectl(None));
    let http = FakeHttp::new(vec![
        Ok(HttpResponse {
            status: 503,
            body: String::new(),
        }),
        Ok(HttpResponse {
            status: 200,
            body: "{\"tz\": null}".into(),
        }),
    ]);
    app.timezone.set_http(http.clone());
    assert_eq!(app.timezone.request_and_sync_tz(), Ok(None));
    assert_eq!(http.requested.lock().len(), 2);

    app.timezone
        .set_http(FakeHttp::new(vec![Err(HttpError::Failed(
            "dns failed".into(),
        ))]));
    assert_eq!(
        app.timezone.request_and_sync_tz(),
        Err(TimezoneSyncError("dns failed".into()))
    );
    app.timezone.set_http(FakeHttp::body("<html>"));
    assert_eq!(
        app.timezone.request_and_sync_tz(),
        Err(TimezoneSyncError(
            "Expecting value: line 1 column 1 (char 0)".into()
        ))
    );
}

/// `tz_background_update`: five failed attempts at most; a success stops
/// it; manual sync never asks.
#[test]
fn background_update_gives_up_after_five_failures() {
    let (_dir, app) = manager(timedatectl(None));
    let http = FakeHttp::new(Vec::new());
    app.timezone.set_http(http.clone());
    for _ in 0..7 {
        app.timezone.tz_background_update();
    }
    assert_eq!(http.requested.lock().len(), 5);

    let (_dir, app) = manager(timedatectl(None));
    let http = FakeHttp::body("{\"tz\": null}");
    app.timezone.set_http(http.clone());
    app.timezone.tz_background_update();
    app.timezone.tz_background_update();
    assert_eq!(http.requested.lock().len(), 1);

    let (_dir, app) = manager(timedatectl(None));
    app.config
        .set(keys::USER, keys::TIMEZONE_SYNC, json!("manual"));
    let http = FakeHttp::body("{\"tz\": null}");
    app.timezone.set_http(http.clone());
    app.timezone.tz_background_update();
    assert!(http.requested.lock().is_empty());
}

fn write_zoneinfo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("dir");
    std::fs::write(
        dir.join("iso3166.tab"),
        "# comment\nMX\tMexico\nAR\tArgentina\nDE\tGermany\n",
    )
    .expect("iso");
    std::fs::write(
        dir.join("zone.tab"),
        "# comment\nMX\t+1924-09909\tAmerica/Mexico_City\tCentral\n\
         AR\t-3436-05827\tAmerica/Argentina/Buenos_Aires\n\
         DE\t+5230+01322\tEurope/Berlin\n",
    )
    .expect("zone");
}

fn list_timezones() -> Arc<RecordingRunner> {
    RecordingRunner::new(|command, shell| {
        Ok(match (command, shell) {
            ("timedatectl list-timezones", false) => Completed {
                stdout: "Etc/UTC\nAmerica/Mexico_City\nAmerica/Argentina/Buenos_Aires\n\
                         Europe/Berlin\nUS/Eastern\nUTC\n"
                    .into(),
                ..Completed::default()
            },
            (TIMEZONE_STATUS_COMMAND, true) => Completed {
                stdout: " Etc/UTC \n".into(),
                ..Completed::default()
            },
            _ => Completed::default(),
        })
    })
}

/// `get_organized_timezones` and the generated `UI_timezones.json`.
#[test]
fn organized_timezones() {
    let (dir, app) = crate::testing::app();
    let zoneinfo = dir.path().join("zoneinfo");
    write_zoneinfo(&zoneinfo);
    let json_path = dir.path().join("UI_timezones.json");
    let mut timezone = TimezoneManager::new(
        app.config.clone(),
        json_path.clone(),
        list_timezones(),
        FakeHttp::new(Vec::new()),
    );
    timezone.set_zoneinfo(zoneinfo);

    let expected = json!({
        "ETC": {"UTC": "Etc/UTC"},
        "Mexico": {"Mexico City": "America/Mexico_City"},
        "Argentina": {"Buenos Aires": "America/Argentina/Buenos_Aires"},
        "Germany": {"Berlin": "Europe/Berlin"},
        "LINKS": {"Eastern": "US/Eastern", "UTC": "UTC"},
    });
    assert_eq!(timezone.get_organized_timezones(), Ok(expected.clone()));
    assert_eq!(timezone.get_ui_timezones(), expected);
    let written = std::fs::read_to_string(&json_path).expect("written");
    assert!(written.starts_with("{\"ETC\": {\"UTC\": \"Etc/UTC\"}, \"Mexico\""));
}

/// tests/test_api_machine.py::TestTimezonesRoute reads the table from
/// `TIMEZONE_JSON_FILE_PATH`; a broken file reads as `{}`.
#[test]
fn ui_timezones_from_file() {
    let (dir, app) = crate::testing::app();
    let path = dir.path().join("UI_timezones.json");
    std::fs::write(&path, "{\"Mexico\": {\"Cancun\": \"America/Cancun\"}}").expect("write");
    let timezone = TimezoneManager::new(
        app.config.clone(),
        path.clone(),
        RecordingRunner::ok(),
        FakeHttp::new(Vec::new()),
    );
    assert_eq!(
        timezone.get_ui_timezones(),
        json!({"Mexico": {"Cancun": "America/Cancun"}})
    );
    std::fs::write(&path, "").expect("truncate");
    assert_eq!(timezone.get_ui_timezones(), json!({}));
}

/// `init` on a new machine: no zone configured, so Etc/UTC is applied and
/// saved (what the boot harness sees with its timedatectl stub).
#[test]
fn init_applies_utc_on_a_new_machine() {
    let (dir, app) = crate::testing::app();
    let zoneinfo = dir.path().join("zoneinfo");
    write_zoneinfo(&zoneinfo);
    let runner = list_timezones();
    let mut timezone = TimezoneManager::new(
        app.config.clone(),
        dir.path().join("UI_timezones.json"),
        runner.clone(),
        FakeHttp::new(Vec::new()),
    );
    timezone.set_zoneinfo(zoneinfo);

    timezone.init();

    assert_eq!(
        timezone.startup_system_timezone().as_deref(),
        Some("Etc/UTC")
    );
    assert_eq!(
        app.config.get(keys::USER, keys::TIME_ZONE),
        json!("Etc/UTC")
    );
    assert!(
        runner
            .calls()
            .contains(&("timedatectl set-timezone Etc/UTC".to_string(), false))
    );
    let saved = std::fs::read_to_string(app.config.path()).expect("saved");
    assert!(saved.contains("time_zone: Etc/UTC"));
}

/// A zone timedatectl refuses at start-up gives way to the system's.
#[test]
fn init_falls_back_to_the_system_zone() {
    let (dir, app) = crate::testing::app();
    std::fs::write(dir.path().join("ui.json"), "{}").expect("table");
    let runner = RecordingRunner::new(|command, shell| {
        Ok(match (command, shell) {
            (TIMEZONE_STATUS_COMMAND, true) => Completed {
                stdout: "America/Cancun\n".into(),
                ..Completed::default()
            },
            (c, false) if c.starts_with("timedatectl set-timezone") => Completed {
                returncode: 1,
                stderr: "Invalid time zone".into(),
                ..Completed::default()
            },
            _ => Completed::default(),
        })
    });
    let timezone = TimezoneManager::new(
        app.config.clone(),
        dir.path().join("ui.json"),
        runner,
        FakeHttp::new(Vec::new()),
    );
    app.config
        .set(keys::USER, keys::TIME_ZONE, json!("Mars/Base"));

    timezone.init();

    assert_eq!(
        app.config.get(keys::USER, keys::TIME_ZONE),
        json!("America/Cancun")
    );
}

/// `set_system_datetime`: `date --set`, raising when it cannot run.
#[test]
fn set_system_datetime() {
    let runner = timedatectl(None);
    let (_dir, app) = manager(runner.clone());
    let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 24)
        .and_then(|d| d.and_hms_opt(12, 30, 0))
        .expect("date");
    app.timezone.set_system_datetime(&date).expect("set");
    assert_eq!(
        runner.calls(),
        vec![("date --set 2026-10-24 12:30:00".to_string(), false)]
    );
    app.timezone.set_runner(RecordingRunner::new(|_, _| {
        Err(RunError::NotFound("date".into()))
    }));
    assert_eq!(
        app.timezone.set_system_datetime(&date).unwrap_err().kind,
        "FileNotFoundError"
    );
}

#[test]
fn redaction() {
    assert_eq!(
        redact_timezone(Some("America/Mexico_City")),
        "America/*****"
    );
    assert_eq!(redact_timezone(Some("UTC")), "UTC");
    assert_eq!(redact_timezone(None), "");
}
