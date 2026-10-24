//! Logging as the Python does it: one line per record on stderr in
//! `%(asctime)s %(name)s %(levelname)s %(message)s`, every line through the
//! privacy redaction, and records copied into the running debug shot.
//!
//! Python logger names are module names (`machine`, `api.profiles`,
//! `tornado.access`); Rust targets become the same shape by dropping the crate
//! prefix and joining with dots.

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::RwLock;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;

/// A formatted record, as the debug shot stores it.
#[derive(Debug, Clone)]
pub struct Record {
    pub created: SystemTime,
    pub level: &'static str,
    pub name: String,
    /// The whole formatted line, like `logging.Formatter.format`.
    pub line: String,
}

type Redact = Arc<dyn Fn(&str) -> String + Send + Sync>;
type Sink = Arc<dyn Fn(&Record) + Send + Sync>;

/// What other services plug into logging. Shared by the layer and `App`.
#[derive(Default)]
pub struct LogHooks {
    redact: RwLock<Option<Redact>>,
    shot_sink: RwLock<Option<Sink>>,
}

impl LogHooks {
    /// Every line is passed through `f` before it is written anywhere.
    pub fn set_redactor(&self, f: impl Fn(&str) -> String + Send + Sync + 'static) {
        *self.redact.write() = Some(Arc::new(f));
    }

    /// `f` sees every record while set (the debug shot's log capture).
    pub fn set_shot_sink(&self, f: Option<Sink>) {
        *self.shot_sink.write() = f;
    }

    fn redact(&self, line: String) -> String {
        match self.redact.read().as_ref() {
            Some(f) => f(&line),
            None => line,
        }
    }
}

struct PythonLayer {
    hooks: Arc<LogHooks>,
    max_level: Level,
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={}", field.name(), value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={:?}", field.name(), value);
        }
    }
}

/// `meticulous_backend::api::profiles` → `api.profiles`.
pub fn python_name(target: &str) -> String {
    let trimmed = target
        .strip_prefix("meticulous_backend::")
        .or_else(|| target.strip_prefix("meticulous_backend"))
        .unwrap_or(target);
    if trimmed.is_empty() {
        "backend".to_string()
    } else {
        trimmed.replace("::", ".")
    }
}

pub fn level_name(level: &Level) -> &'static str {
    match *level {
        Level::ERROR => "ERROR",
        Level::WARN => "WARNING",
        Level::INFO => "INFO",
        Level::DEBUG => "DEBUG",
        Level::TRACE => "DEBUG",
    }
}

impl<S: Subscriber> Layer<S> for PythonLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() > self.max_level {
            return;
        }
        let ours =
            meta.target().starts_with("meticulous_backend") || meta.target().starts_with("tornado");
        if !ours && *meta.level() > Level::WARN {
            // Libraries log their internals at debug; Python's equivalents
            // (tornado's, socketio's) stay quiet unless asked.
            return;
        }
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let name = python_name(meta.target());
        let level = level_name(meta.level());
        let created = SystemTime::now();
        let stamp =
            chrono::DateTime::<chrono::Local>::from(created).format("%Y-%m-%d %H:%M:%S,%3f");
        let raw = format!(
            "{stamp} {name} {level} {}{}",
            visitor.message, visitor.fields
        );
        let line = self.hooks.redact(raw);
        {
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "{line}");
        }
        if let Some(sink) = self.hooks.shot_sink.read().clone() {
            sink(&Record {
                created,
                level,
                name,
                line,
            });
        }
    }
}

/// Install the global subscriber. `LOGLEVEL` sets the threshold, like the
/// Python's.
pub fn init(log_level: &str) -> Arc<LogHooks> {
    let hooks = Arc::new(LogHooks::default());
    let max_level = match log_level {
        "CRITICAL" | "ERROR" => Level::ERROR,
        "WARNING" | "WARN" => Level::WARN,
        "INFO" => Level::INFO,
        _ => Level::DEBUG,
    };
    let layer = PythonLayer {
        hooks: hooks.clone(),
        max_level,
    };
    // The Python SDK's logging integration: ERROR records become events, no
    // breadcrumbs. Without a Sentry client bound this does nothing.
    let sentry = sentry::integrations::tracing::layer().event_filter(|meta| {
        if *meta.level() == Level::ERROR {
            sentry::integrations::tracing::EventFilter::Event
        } else {
            sentry::integrations::tracing::EventFilter::Ignore
        }
    });
    let _ = tracing_subscriber::registry()
        .with(layer)
        .with(sentry)
        .try_init();
    hooks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_like_python_modules() {
        assert_eq!(
            python_name("meticulous_backend::api::profiles"),
            "api.profiles"
        );
        assert_eq!(python_name("meticulous_backend::machine"), "machine");
        assert_eq!(python_name("tornado.access"), "tornado.access");
        assert_eq!(python_name("meticulous_backend"), "backend");
    }
}
