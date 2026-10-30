//! Error reports to Meticulous' Sentry (`back.py`, `machine.py`).
//!
//! Two projects: the backend's, which receives every ERROR log record (the
//! Python SDK's logging integration) and explicit captures, and the ESP32's,
//! which receives what the firmware reports through the backend. Every event
//! goes through the privacy scrubber before it leaves the machine, automatic
//! breadcrumbs are off, and nothing is sent in manufacturing mode.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::RwLock;
use sentry::protocol::{Context, Event, Level};
use serde_json::Value;

const BACKEND_DSN: &str =
    "https://39a03a6899dcf043dc520bc93d9e966c@sentry.meticulousespresso.com/3";
const ESP_DSN: &str = "https://57bd3ab95e32eda7af5fca189527c235@sentry.meticulousespresso.com/6";

type Scrubber = Arc<dyn Fn(Value) -> Option<Value> + Send + Sync>;

/// What `sentry_sdk.set_tag` and `set_context` put on the global scope,
/// which every event of both projects carries.
#[derive(Default)]
struct GlobalScope {
    tags: BTreeMap<String, String>,
    contexts: BTreeMap<String, BTreeMap<String, Value>>,
}

/// Shared state of both clients.
pub struct Reporting {
    enabled: bool,
    /// `toggle_sentry`: manufacturing mode silences reports.
    paused: AtomicBool,
    scrubber: RwLock<Option<Scrubber>>,
    /// Added to every event in `before_send` rather than kept in a sentry
    /// scope: `configure_scope` changes only the calling thread's hub, and
    /// another thread's hub is a copy taken when it first reports, so a
    /// context set later (the config, on every save) would miss most events.
    global: RwLock<GlobalScope>,
    esp: Option<Arc<sentry::Client>>,
    _guard: Option<sentry::ClientInitGuard>,
}

impl Reporting {
    /// Reporting off, quietly (tools that only build the app).
    pub fn disabled() -> Arc<Reporting> {
        Arc::new(Reporting {
            enabled: false,
            paused: AtomicBool::new(false),
            scrubber: RwLock::new(None),
            global: RwLock::default(),
            esp: None,
            _guard: None,
        })
    }

    /// `sentry_sdk.init(...)` when `BACKEND` is FIKA or `SENTRY` is set; a
    /// fake-hardware run never sends anything.
    pub fn init(enabled: bool) -> Arc<Reporting> {
        if !enabled {
            println!("Skipping Sentry initialization");
            return Reporting::disabled();
        }
        Arc::new_cyclic(|weak: &std::sync::Weak<Reporting>| {
            println!("Initializing sentry");
            let backend_weak = weak.clone();
            let guard = sentry::init((
                BACKEND_DSN,
                options(move |event| filter(&backend_weak, event)),
            ));
            let esp_weak = weak.clone();
            // sentry::init gives the backend client its HTTP transport; a
            // client built by hand gets one only through apply_defaults, and
            // without it every ESP diagnostic was dropped.
            let esp_options: sentry::ClientOptions =
                (ESP_DSN, options(move |event| filter(&esp_weak, event))).into();
            let esp = sentry::Client::from(sentry::apply_defaults(esp_options));
            Reporting {
                enabled: true,
                paused: AtomicBool::new(false),
                scrubber: RwLock::new(None),
                global: RwLock::default(),
                esp: Some(Arc::new(esp)),
                _guard: Some(guard),
            }
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The privacy scrubber (`sentry_privacy.sanitize_sentry_event`): gets
    /// the event as JSON, returns it cleaned or `None` to drop it.
    pub fn set_scrubber(&self, f: impl Fn(Value) -> Option<Value> + Send + Sync + 'static) {
        *self.scrubber.write() = Some(Arc::new(f));
    }

    /// `toggle_sentry(enabled)`.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// `sentry_sdk.set_tag(key, value)`: on every later event of both
    /// projects.
    pub fn set_tag(&self, key: &str, value: &str) {
        if self.enabled {
            self.global.write().tags.insert(key.into(), value.into());
        }
    }

    /// `sentry_sdk.set_context(key, value)`: on every later event of both
    /// projects.
    pub fn set_context(&self, key: &str, value: &Value) {
        if !self.enabled {
            return;
        }
        let map: BTreeMap<String, Value> = match value {
            Value::Object(o) => o.clone().into_iter().collect(),
            other => BTreeMap::from([("value".to_string(), other.clone())]),
        };
        self.global.write().contexts.insert(key.into(), map);
    }

    /// `backend.main()` after `UpdateManager.init()`: the image every event
    /// comes from (`build-info`, `build-timestamp`, `build-channel`,
    /// `build-version`) and the machine's `serial`. What the image lacks is
    /// left out, as sentry-sdk drops a `None` tag.
    pub fn tag_build(&self, ota: &crate::ota::UpdateManager, serial: &Value) {
        if let Some(info) = ota.repository_info() {
            self.set_context("build-info", &info);
        }
        if let Some(timestamp) = ota.build_timestamp() {
            self.set_tag("build-timestamp", &timestamp.sentry_tag());
        }
        if let Some(channel) = ota.image_channel() {
            self.set_tag("build-channel", &channel);
        }
        if let Some(version) = ota.image_version() {
            self.set_tag("build-version", &version);
        }
        if !serial.is_null() {
            self.set_tag("serial", &crate::ota::python_str(serial));
        }
    }

    /// The global scope under the event's own: a tag or context the event
    /// already has (an ESP32 diagnostic's) wins, as a forked scope's does.
    fn apply_global(&self, event: &mut Event<'static>) {
        let global = self.global.read();
        for (key, value) in &global.tags {
            event
                .tags
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        for (key, map) in &global.contexts {
            event
                .contexts
                .entry(key.clone())
                .or_insert_with(|| Context::Other(map.clone()));
        }
    }

    /// `sentry_sdk.capture_message(message, level)` to the backend project.
    pub fn capture_message(&self, message: &str, level: Level) {
        if self.enabled {
            sentry::capture_message(message, level);
        }
    }

    /// `sentry_sdk.capture_exception(exc)` inside `sentry_sdk.new_scope()`
    /// with one tag and one context, to the backend project.
    pub fn capture_exception_with(
        &self,
        exception_type: &str,
        message: &str,
        tag: (&str, &str),
        context: (&str, &Value),
    ) {
        if !self.enabled {
            return;
        }
        let map: BTreeMap<String, Value> = match context.1 {
            Value::Object(o) => o.clone().into_iter().collect(),
            other => BTreeMap::from([("value".to_string(), other.clone())]),
        };
        sentry::with_scope(
            |scope| {
                scope.set_tag(tag.0, tag.1);
                scope.set_context(context.0, sentry::protocol::Context::Other(map));
            },
            || {
                let event = Event {
                    level: Level::Error,
                    exception: vec![sentry::protocol::Exception {
                        ty: exception_type.to_string(),
                        value: Some(message.to_string()),
                        ..Default::default()
                    }]
                    .into(),
                    ..Default::default()
                };
                sentry::capture_event(event);
            },
        );
    }

    /// An event for the ESP32's project with its own tags, context and
    /// release (`espresso-firmware@<version>`).
    pub fn capture_esp_event(
        &self,
        message: &str,
        level: Level,
        fingerprint: Option<String>,
        tags: &[(String, String)],
        context: Option<(&str, &Value)>,
        firmware_version: Option<&str>,
    ) {
        let Some(client) = &self.esp else { return };
        let hub = sentry::Hub::new(Some(client.clone()), Arc::new(sentry::Scope::default()));
        hub.with_scope(
            |scope| {
                for (key, value) in tags {
                    if !value.is_empty() {
                        scope.set_tag(key, value);
                    }
                }
                if let Some((name, value)) = context
                    && let Value::Object(o) = value
                {
                    scope.set_context(
                        name,
                        sentry::protocol::Context::Other(o.clone().into_iter().collect()),
                    );
                }
                if let Some(version) = firmware_version {
                    scope.set_tag("firmware-version", version);
                }
            },
            || {
                let mut event = Event {
                    message: Some(message.to_string()),
                    level,
                    ..Default::default()
                };
                if let Some(fingerprint) = &fingerprint {
                    event.fingerprint = vec![fingerprint.clone().into()].into();
                }
                if let Some(version) = firmware_version {
                    event.release = Some(format!("espresso-firmware@{version}").into());
                }
                hub.capture_event(event);
            },
        );
    }
}

/// What both Python clients pass: no PII, no breadcrumbs, no tracing, and
/// every event through `before_send`.
fn options(
    before_send: impl Fn(Event<'static>) -> Option<Event<'static>> + Send + Sync + 'static,
) -> sentry::ClientOptions {
    let mut options = sentry::ClientOptions::default();
    options.send_default_pii = false;
    options.max_breadcrumbs = 0;
    options.attach_stacktrace = false;
    options.before_breadcrumb = Some(Arc::new(|_| None));
    options.before_send = Some(Arc::new(before_send));
    options
}

fn filter(weak: &std::sync::Weak<Reporting>, mut event: Event<'static>) -> Option<Event<'static>> {
    let reporting = weak.upgrade()?;
    if reporting.paused.load(Ordering::Relaxed) {
        return None;
    }
    reporting.apply_global(&mut event);
    let scrubber = reporting.scrubber.read().clone()?;
    let json = serde_json::to_value(&event).ok()?;
    let cleaned = scrubber(json)?;
    serde_json::from_value(cleaned).ok()
}

/// Python level names (`"warning"`, `"error"`, …) as Sentry levels.
pub fn level_from_name(name: &str) -> Level {
    match name {
        "debug" => Level::Debug,
        "info" => Level::Info,
        "warning" | "warn" => Level::Warning,
        "fatal" | "critical" => Level::Fatal,
        _ => Level::Error,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use parking_lot::RwLock;
    use sentry::protocol::{Context, DebugImage, Event};
    use serde_json::{Value, json};

    use super::{Level, Reporting, filter, options};
    use crate::env::BackendMode;

    /// Reporting on, with the privacy scrubber and no client: events reach
    /// only a test transport.
    fn enabled() -> Arc<Reporting> {
        let reporting = Arc::new(Reporting {
            enabled: true,
            paused: AtomicBool::new(false),
            scrubber: RwLock::new(None),
            global: RwLock::default(),
            esp: None,
            _guard: None,
        });
        reporting.set_scrubber(|event| redactor::sanitize_sentry_event(&event));
        reporting
    }

    fn context(event: &Event<'static>, name: &str) -> Value {
        match event.contexts.get(name) {
            Some(Context::Other(map)) => json!(map),
            other => panic!("no {name} context: {other:?}"),
        }
    }

    /// A panic reaches Sentry with what symbolicating the stripped release
    /// binary from its uploaded debug file takes: every frame's instruction
    /// address, and the debug images with this executable's GNU build id
    /// (its code id), which Sentry files the debug file under. The privacy
    /// scrubber keeps both. The event goes to the test transport only.
    #[test]
    fn panic_event_carries_what_symbolication_needs() {
        let reporting = enabled();
        let weak = Arc::downgrade(&reporting);
        let events = sentry::test::with_captured_events_options(
            || {
                let _ = std::panic::catch_unwind(|| panic!("symbolication test panic"));
            },
            sentry::apply_defaults(options(move |event| filter(&weak, event))),
        );

        let event = events
            .iter()
            .find(|e| e.exception.values.iter().any(|x| x.ty == "panic"))
            .expect("a panic event");
        let frames = &event.exception.values[0]
            .stacktrace
            .as_ref()
            .expect("a stack trace")
            .frames;
        assert!(!frames.is_empty());
        assert!(frames.iter().all(|frame| frame.instruction_addr.is_some()));

        let exe = std::env::current_exe().expect("test executable");
        let own = event
            .debug_meta
            .images
            .iter()
            .find_map(|image| match image {
                DebugImage::Symbolic(image) if std::path::Path::new(&image.name) == exe => {
                    Some(image)
                }
                _ => None,
            })
            .expect("this executable among the debug images");
        assert!(
            own.code_id
                .as_ref()
                .is_some_and(|id| !id.to_string().is_empty())
        );
        assert!(!own.id.is_nil());
    }

    /// `sentry_sdk.set_tag`/`set_context` reach every later event, under
    /// the event's own: a tag its scope set (an ESP32 diagnostic's
    /// `firmware-version`) wins.
    #[test]
    fn global_tags_and_contexts_reach_events() {
        let reporting = enabled();
        reporting.set_tag("build-version", "2026M1495-nightly");
        reporting.set_tag("firmware-version", "from the global scope");
        reporting.set_context("build-info", &json!({"backend": {"branch": "nightly"}}));
        let weak = Arc::downgrade(&reporting);
        let events = sentry::test::with_captured_events_options(
            || {
                sentry::with_scope(
                    |scope| scope.set_tag("firmware-version", "0.2.24-477"),
                    || sentry::capture_message("an error", Level::Error),
                );
            },
            sentry::apply_defaults(options(move |event| filter(&weak, event))),
        );
        let event = &events[0];
        assert_eq!(event.tags["build-version"], "2026M1495-nightly");
        assert_eq!(event.tags["firmware-version"], "0.2.24-477");
        assert_eq!(
            context(event, "build-info"),
            json!({"backend": {"branch": "nightly"}})
        );
    }

    /// `backend.main()`'s build tags and context come from the image's
    /// files and the serial from the config; an image without them, or a
    /// machine without a serial, adds none.
    #[test]
    fn tag_build_reports_the_image_and_the_serial() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let root = dir.path();
        let config = Arc::new(crate::config::ConfigStore::open(
            root.join("config.yml"),
            crate::config::defaults(),
        ));
        let paths = crate::env::SystemPaths::under(root);
        let tags_of = |serial: Value| {
            let reporting = enabled();
            let ota = crate::ota::UpdateManager::new(config.clone(), &paths);
            reporting.tag_build(&ota, &serial);
            filter(&Arc::downgrade(&reporting), Event::default()).expect("kept")
        };

        let bare = tags_of(Value::Null);
        assert!(bare.tags.is_empty(), "{:?}", bare.tags);
        assert!(!bare.contexts.contains_key("build-info"));

        for (name, text) in [
            ("ROOTFS_BUILD_DATE", "Tue, 27 Oct 2026 20:35:40 +0000\n"),
            ("image-build-channel", "nightly\n"),
            ("image-build-version", "2026M1495-nightly\n"),
            ("summary.txt", "## backend ##\nBranch: nightly\n"),
        ] {
            let path = root.join("opt").join(name);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, text).expect("write");
        }
        let event = tags_of(json!("012810"));
        assert_eq!(event.tags["build-timestamp"], "2026-10-27T20:35:40.000000Z");
        assert_eq!(event.tags["build-channel"], "nightly");
        assert_eq!(event.tags["build-version"], "2026M1495-nightly");
        assert_eq!(event.tags["serial"], "012810");
        assert_eq!(
            context(&event, "build-info")["backend"]["branch"],
            "nightly"
        );
    }

    /// The reportable config travels with every event, as loaded and again
    /// after each save (`MeticulousConfig.save()`).
    #[test]
    fn config_context_follows_saves() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let env = crate::env::Env::for_tests(dir.path(), BackendMode::Emulator);
        let log = Arc::new(crate::logging::LogHooks::default());
        let reporting = enabled();
        let app = crate::app::App::new(env, log, reporting.clone());
        let weak = Arc::downgrade(&reporting);
        let sounds = || {
            let event = filter(&weak, Event::default()).expect("kept");
            context(&event, "config")["user"]["enable_sounds"].clone()
        };
        assert!(sounds().is_boolean());
        for enabled in [false, true] {
            app.config.set_and_save(
                crate::config::keys::USER,
                crate::config::keys::SOUNDS_ENABLED,
                json!(enabled),
            );
            assert_eq!(sounds(), json!(enabled));
        }
    }

    /// tests/test_sentry_offline.py::test_the_esp32_sentry_client_has_no_dsn_and_no_transport:
    /// no test may send events to the production Sentry projects. Every app
    /// the tests build, the production machine (FIKA) included, has neither
    /// the ESP32's client nor the backend's.
    #[test]
    fn test_apps_have_no_sentry_client() {
        let apps = [
            crate::testing::app(),
            crate::testing::app_with(BackendMode::Fika, |_| {}),
            crate::testing::app_custom(BackendMode::Fika, |env| {
                env.sentry = true;
                env.hooks.fake_hardware = false;
            }),
        ];
        for (_dir, app) in &apps {
            assert!(!app.reporting.enabled());
            assert!(app.reporting.esp.is_none());
            // ESP32 events go nowhere instead of to the ESP32's project.
            app.reporting.capture_esp_event(
                "test event",
                super::Level::Error,
                None,
                &[],
                None,
                Some("1.2.3"),
            );
        }
        assert!(sentry::Hub::current().client().is_none());
    }
}
