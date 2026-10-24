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
use sentry::protocol::{Event, Level};
use serde_json::Value;

const BACKEND_DSN: &str =
    "https://39a03a6899dcf043dc520bc93d9e966c@sentry.meticulousespresso.com/3";
const ESP_DSN: &str = "https://57bd3ab95e32eda7af5fca189527c235@sentry.meticulousespresso.com/6";

type Scrubber = Arc<dyn Fn(Value) -> Option<Value> + Send + Sync>;

/// Shared state of both clients.
pub struct Reporting {
    enabled: bool,
    /// `toggle_sentry`: manufacturing mode silences reports.
    paused: AtomicBool,
    scrubber: RwLock<Option<Scrubber>>,
    esp: Option<Arc<sentry::Client>>,
    _guard: Option<sentry::ClientInitGuard>,
}

impl Reporting {
    /// `sentry_sdk.init(...)` when `BACKEND` is FIKA or `SENTRY` is set; a
    /// fake-hardware run never sends anything.
    pub fn init(enabled: bool) -> Arc<Reporting> {
        let reporting = Arc::new_cyclic(|weak: &std::sync::Weak<Reporting>| {
            if !enabled {
                println!("Skipping Sentry initialization");
                return Reporting {
                    enabled: false,
                    paused: AtomicBool::new(false),
                    scrubber: RwLock::new(None),
                    esp: None,
                    _guard: None,
                };
            }
            println!("Initializing sentry");
            let backend_weak = weak.clone();
            let guard = sentry::init((
                BACKEND_DSN,
                options(move |event| filter(&backend_weak, event)),
            ));
            let esp_weak = weak.clone();
            let esp =
                sentry::Client::from((ESP_DSN, options(move |event| filter(&esp_weak, event))));
            Reporting {
                enabled: true,
                paused: AtomicBool::new(false),
                scrubber: RwLock::new(None),
                esp: Some(Arc::new(esp)),
                _guard: Some(guard),
            }
        });
        reporting
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

    pub fn set_tag(&self, key: &str, value: &str) {
        if self.enabled {
            sentry::configure_scope(|scope| scope.set_tag(key, value));
        }
    }

    pub fn set_context(&self, key: &str, value: &Value) {
        if !self.enabled {
            return;
        }
        let map: BTreeMap<String, Value> = match value {
            Value::Object(o) => o.clone().into_iter().collect(),
            other => BTreeMap::from([("value".to_string(), other.clone())]),
        };
        sentry::configure_scope(|scope| {
            scope.set_context(key, sentry::protocol::Context::Other(map))
        });
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
                if let Some((name, value)) = context {
                    if let Value::Object(o) = value {
                        scope.set_context(
                            name,
                            sentry::protocol::Context::Other(o.clone().into_iter().collect()),
                        );
                    }
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

fn filter(weak: &std::sync::Weak<Reporting>, event: Event<'static>) -> Option<Event<'static>> {
    let reporting = weak.upgrade()?;
    if reporting.paused.load(Ordering::Relaxed) {
        return None;
    }
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
