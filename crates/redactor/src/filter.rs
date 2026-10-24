//! Emit-time redaction of log records: a port of `log_redaction_filter.py`.
//!
//! The watcher redacts at read time, when a bug report is built. That leaves
//! plaintext on the device (the journal, the shot-debug log files, Sentry
//! events built from error logs), so the backend also redacts every record
//! before any handler sees it, with the same rules and the same per-device
//! key, so a value gets the same token in both services.
//!
//! This is a backstop, not the primary defence: values are still kept out of
//! messages at the call site (`get_reportable_config`, `redact_ssid`,
//! `redact_ip`, `redact_timezone`, logging an error's type instead of its
//! text). Those cover classes no regex can reach.
//!
//! [`LogRedactor`] holds the Python filter's state. It takes `&mut self`; the
//! backend keeps one instance behind a mutex and calls [`LogRedactor::filter`]
//! for every record, from every logger, before formatting or sending it.

use std::borrow::Cow;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use aho_corasick::{AhoCorasick, Input, MatchKind};

use crate::pychar::{char_len_at, is_word_or_dash, next_char, prev_char, py_len, py_repr};
use crate::rules::{
    KeyError, MIN_SWEEP_LEN, REDACTED, RedactionState, Rules, is_already_redacted, pseudonym,
};

/// `_REFRESH_INTERVAL_SECONDS`: upper bound on how stale the config-seeded
/// sweep may be.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// Why a record could not be redacted. The filter never fails loudly; the
/// record's content is replaced by a placeholder naming this.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{exception_type}")]
pub struct RedactionFailure {
    /// The Python exception class the same failure raises in the Python
    /// backend (`FileNotFoundError`, `ValueError`, `RuntimeError`, ...): it is
    /// the only detail the placeholder keeps.
    pub exception_type: String,
}

impl RedactionFailure {
    /// A failure reported as the Python exception class `exception_type`.
    pub fn new(exception_type: impl Into<String>) -> RedactionFailure {
        RedactionFailure {
            exception_type: exception_type.into(),
        }
    }
}

impl From<KeyError> for RedactionFailure {
    fn from(error: KeyError) -> RedactionFailure {
        RedactionFailure::new(error.python_exception())
    }
}

/// Supplies the per-device key for each record: `load_cached_key(path)`.
///
/// The Python filter asks for the key on every record and the cache only
/// keeps a successful load, so a missing key is retried until it appears.
/// Implementations do the IO (read `REDACTION_KEY_PATH`, falling back to
/// `/root/.redaction_key`, creating it if absent, as `load_key` does) and
/// cache the bytes once loaded.
pub trait KeySource: Send {
    /// The key, or the failure that replaces the record with a placeholder.
    fn key(&mut self) -> Result<&[u8], RedactionFailure>;
}

impl KeySource for Vec<u8> {
    fn key(&mut self) -> Result<&[u8], RedactionFailure> {
        Ok(self)
    }
}

impl KeySource for Result<Vec<u8>, RedactionFailure> {
    fn key(&mut self) -> Result<&[u8], RedactionFailure> {
        match self {
            Ok(key) => Ok(key),
            Err(failure) => Err(failure.clone()),
        }
    }
}

/// The live config values the sweep removes from every record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepValues {
    /// Known network names: pseudonymised.
    pub ssids: Vec<String>,
    /// Passwords and keys: replaced by `[REDACTED]`.
    pub credentials: Vec<String>,
}

/// The value provider the Python config registers with
/// `set_value_provider`: called when the sweep is rebuilt.
///
/// It runs while the redactor is borrowed, so it must not log through it
/// (the Python filter used an `RLock` to survive that; a mutex would
/// deadlock).
pub trait ValueProvider: Send {
    /// The current values, or the failure that replaces the record.
    fn values(&mut self) -> Result<SweepValues, RedactionFailure>;
}

impl<F> ValueProvider for F
where
    F: FnMut() -> Result<SweepValues, RedactionFailure> + Send,
{
    fn values(&mut self) -> Result<SweepValues, RedactionFailure> {
        self()
    }
}

/// The parts of a Python `logging.LogRecord` the filter reads and rewrites.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogRecord {
    /// `record.name`, the logger name. Survives a failure.
    pub name: String,
    /// `record.getMessage()`: the message with its arguments applied. Python
    /// rewrites `msg` and clears `args`; here the caller formats first.
    pub msg: String,
    /// The rendered exception, `"".join(traceback.format_exception(...))`,
    /// when the record carries one (`exc_info`). Left unredacted, as Python
    /// leaves `exc_info` for Sentry; handlers must print `exc_text`.
    pub exc_info: Option<String>,
    /// `record.exc_text`: what every formatter prints for the exception.
    pub exc_text: Option<String>,
    /// `record.stack_info`.
    pub stack_info: Option<String>,
    /// The `_meticulous_redacted` mark: the record was already filtered.
    pub redacted: bool,
}

impl LogRecord {
    /// A record from logger `name` with message `msg`.
    pub fn new(name: impl Into<String>, msg: impl Into<String>) -> LogRecord {
        LogRecord {
            name: name.into(),
            msg: msg.into(),
            ..LogRecord::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ssid,
    Credential,
}

struct SweepAlternative {
    value: String,
    kind: Kind,
    lookbehind: bool,
    lookahead: bool,
}

/// The compiled config-seeded sweep.
struct Sweep {
    finder: AhoCorasick,
    alternatives: Vec<SweepAlternative>,
}

impl Sweep {
    /// `_rebuild_sweep_locked` minus the provider call.
    fn build(values: SweepValues) -> Option<Sweep> {
        let mut seen = HashSet::new();
        let mut kinds: Vec<(String, Kind)> = Vec::new();
        for value in values.credentials {
            if is_sweepable(&value) && seen.insert(value.clone()) {
                kinds.push((value, Kind::Credential));
            }
        }
        // A credential that equals a network name stays a credential.
        for value in values.ssids {
            if is_sweepable(&value) && seen.insert(value.clone()) {
                kinds.push((value, Kind::Ssid));
            }
        }
        if kinds.is_empty() {
            return None;
        }
        // Longest first (stable, in code points), as the Python sorts.
        kinds.sort_by_key(|(value, _)| std::cmp::Reverse(py_len(value)));
        let last = kinds.len() - 1;
        let alternatives: Vec<SweepAlternative> = kinds
            .into_iter()
            .enumerate()
            .map(|(index, (value, kind))| SweepAlternative {
                value,
                kind,
                // PARITY: Python compiles rf"(?<![\w-]){a|b|c}(?![\w-])"
                // without a group, so the lookbehind binds to the first
                // (longest) alternative only, the lookahead to the last
                // (shortest) only, and the ones in between match anywhere.
                // That only ever over-redacts.
                lookbehind: index == 0,
                lookahead: index == last,
            })
            .collect();
        let finder = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostFirst)
            .build(alternatives.iter().map(|alt| alt.value.as_str()))
            .expect("a literal set of sweepable values always builds");
        Some(Sweep {
            finder,
            alternatives,
        })
    }

    /// `_sweep_pattern.sub(_replace, text)`.
    fn apply<'t>(&self, text: &'t str, key: &[u8]) -> Cow<'t, str> {
        let mut out: Option<String> = None;
        let mut copied = 0;
        let mut pos = 0;
        while pos < text.len() {
            let Some(found) = self.finder.find(Input::new(text).range(pos..)) else {
                break;
            };
            let start = found.start();
            let behind_ok = !prev_char(text, start).is_some_and(is_word_or_dash);
            let matched = self.alternatives.iter().find_map(|alt| {
                if !text[start..].starts_with(alt.value.as_str()) || (alt.lookbehind && !behind_ok)
                {
                    return None;
                }
                let end = start + alt.value.len();
                if alt.lookahead && next_char(text, end).is_some_and(is_word_or_dash) {
                    return None;
                }
                Some((alt, end))
            });
            let Some((alt, end)) = matched else {
                pos = start + char_len_at(text, start);
                continue;
            };
            let o = out.get_or_insert_with(|| String::with_capacity(text.len() + 16));
            o.push_str(&text[copied..start]);
            match alt.kind {
                Kind::Ssid => o.push_str(&pseudonym("SSID", &alt.value, key)),
                Kind::Credential => o.push_str(REDACTED),
            }
            copied = end;
            pos = end;
        }
        match out {
            None => Cow::Borrowed(text),
            Some(mut o) => {
                o.push_str(&text[copied..]);
                Cow::Owned(o)
            }
        }
    }
}

/// `_is_sweepable`: non-empty, at least [`MIN_SWEEP_LEN`] code points, not a
/// token. (Python also skips non-`str` values; see
/// [`crate::sweep_values_from_config`].)
fn is_sweepable(value: &str) -> bool {
    !value.is_empty() && py_len(value) >= MIN_SWEEP_LEN && !is_already_redacted(value)
}

/// The config-seeded part of the filter: provider, compiled sweep, deadline.
#[derive(Default)]
struct Seeds {
    provider: Option<Box<dyn ValueProvider>>,
    sweep: Option<Sweep>,
    /// `None` is Python's `_sweep_deadline = 0.0`: rebuild on the next record.
    deadline: Option<Instant>,
}

impl Seeds {
    /// `_sweep_locked` up to the substitution: the sweep to apply, rebuilt
    /// first if the deadline has passed.
    fn current(&mut self, now: Instant) -> Result<Option<&Sweep>, RedactionFailure> {
        if self.deadline.is_none_or(|deadline| now >= deadline) {
            self.rebuild(now)?;
        }
        Ok(self.sweep.as_ref())
    }

    /// `_rebuild_sweep_locked`.
    fn rebuild(&mut self, now: Instant) -> Result<(), RedactionFailure> {
        self.sweep = None;
        let Some(provider) = self.provider.as_mut() else {
            self.deadline = Some(now + REFRESH_INTERVAL);
            return Ok(());
        };
        // Before the deadline moves: a failing provider must fail every record
        // until it recovers, not once followed by two seconds of shape rules
        // only (spec section 13.2).
        let values = provider.values()?;
        self.sweep = Sweep::build(values);
        self.deadline = Some(now + REFRESH_INTERVAL);
        Ok(())
    }
}

/// `_redact(text, key)`: the shape rules with the carried block state, then
/// the config-seeded sweep.
fn redact_record_text(
    rules: &Rules,
    state: &mut RedactionState,
    seeds: &mut Seeds,
    key: &[u8],
    text: &str,
    now: Instant,
) -> Result<String, RedactionFailure> {
    let redacted = match rules.redact(text, key, state, None) {
        Ok(redacted) => redacted.text,
        Err(_) => unreachable!("no cancellation predicate was given"),
    };
    let Some(sweep) = seeds.current(now)? else {
        return Ok(redacted);
    };
    Ok(match sweep.apply(&redacted, key) {
        Cow::Borrowed(_) => redacted,
        Cow::Owned(swept) => swept,
    })
}

/// `LogRedactionFilter`: rewrites each record through the shared rules plus a
/// literal sweep of values read from the live config.
pub struct LogRedactor {
    rules: Rules,
    key_source: Box<dyn KeySource>,
    state: RedactionState,
    seeds: Seeds,
}

impl LogRedactor {
    /// A filter that takes its key from `key_source`; until a value provider
    /// is registered it runs on the shape rules alone.
    pub fn new(key_source: impl KeySource + 'static) -> LogRedactor {
        LogRedactor::with_rules(Rules::new(), key_source)
    }

    pub(crate) fn with_rules(rules: Rules, key_source: impl KeySource + 'static) -> LogRedactor {
        LogRedactor {
            rules,
            key_source: Box::new(key_source),
            state: RedactionState::default(),
            seeds: Seeds::default(),
        }
    }

    /// `set_value_provider(provider)`: register the live-config callback and
    /// rebuild the sweep on the next record.
    pub fn set_value_provider(&mut self, provider: impl ValueProvider + 'static) {
        self.seeds.provider = Some(Box::new(provider));
        self.seeds.deadline = None;
    }

    /// Seed the sweep with fixed values: a provider that always returns them.
    /// For a backend that pushes values whenever its config changes instead
    /// of registering a callback.
    pub fn set_values(&mut self, ssids: Vec<String>, credentials: Vec<String>) {
        let values = SweepValues { ssids, credentials };
        self.set_value_provider(move || Ok(values.clone()));
    }

    /// `invalidate()`: drop the cached sweep. Called when the config is saved.
    pub fn invalidate(&mut self) {
        self.seeds.deadline = None;
    }

    /// The KnownWifis block tracking carried between records.
    pub fn state(&self) -> &RedactionState {
        &self.state
    }

    /// `filter(record)`: redact the message, the exception text and the stack
    /// text in place and mark the record. Never fails: on any failure the
    /// content is replaced by `<redaction failed for a '<name>' record: <Type>>`
    /// and the exception and stack are dropped. `now` is the monotonic time,
    /// for the sweep's refresh interval.
    pub fn filter(&mut self, record: &mut LogRecord, now: Instant) {
        if record.redacted {
            return;
        }
        if let Err(failure) = self.try_filter(record, now) {
            record.msg = format!(
                "<redaction failed for a {} record: {}>",
                py_repr(&record.name),
                failure.exception_type
            );
            record.exc_info = None;
            record.exc_text = None;
            record.stack_info = None;
        }
        record.redacted = true;
    }

    fn try_filter(&mut self, record: &mut LogRecord, now: Instant) -> Result<(), RedactionFailure> {
        let LogRedactor {
            rules,
            key_source,
            state,
            seeds,
        } = self;
        let key = key_source.key()?;
        record.msg = redact_record_text(rules, state, seeds, key, &record.msg, now)?;
        if record.exc_text.as_deref().is_none_or(str::is_empty) {
            if let Some(rendered) = record.exc_info.as_deref() {
                // Rendered here so a formatter reuses the redacted exc_text.
                let rendered = rendered.strip_suffix('\n').unwrap_or(rendered);
                record.exc_text =
                    Some(redact_record_text(rules, state, seeds, key, rendered, now)?);
            }
        }
        if let Some(stack) = record.stack_info.as_deref().filter(|s| !s.is_empty()) {
            record.stack_info = Some(redact_record_text(rules, state, seeds, key, stack, now)?);
        }
        Ok(())
    }

    /// `_redact(text, key)` with the key from the key source: one piece of
    /// text through the shape rules (carrying the block state) and the sweep.
    /// [`LogRedactor::filter`] is the per-record entry point; this is for
    /// text that is not a log record but leaves the device the same way.
    pub fn redact(&mut self, text: &str, now: Instant) -> Result<String, RedactionFailure> {
        let LogRedactor {
            rules,
            key_source,
            state,
            seeds,
        } = self;
        let key = key_source.key()?;
        redact_record_text(rules, state, seeds, key, text, now)
    }
}

impl std::fmt::Debug for LogRedactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogRedactor")
            .field("state", &self.state)
            .field("has_provider", &self.seeds.provider.is_some())
            .finish_non_exhaustive()
    }
}
