//! Privacy controls for everything that leaves the machine: log text, Sentry
//! events and the diagnostic config.
//!
//! - [`Redactor`] / [`pseudonym`]: the shared redaction rules of the
//!   `log_redactor` submodule (`redactor.py`, `REDACTION_SPEC.md`). The
//!   watcher runs the Python copy over the bug-report journal; the output here
//!   is byte-for-byte the same for the same key, which is what makes one
//!   network carry one token across both services.
//! - [`LogRedactor`]: the backend's emit-time log filter
//!   (`log_redaction_filter.py`): the shared rules with block state carried
//!   across records, plus a sweep of SSIDs and credentials from the live
//!   config.
//! - [`sanitize_sentry_event`] / [`drop_breadcrumb`] (`sentry_privacy.py`),
//!   [`get_reportable_config`] (`reportable_config.py`), the
//!   `sensitive_logging.py` helpers and the radio recovery event of
//!   `radio_diagnostics.py`.
//!
//! Pure: no IO, no clocks (the filter takes `now`), no globals. Loading the
//! key file (`REDACTION_KEY_PATH`, default [`DEFAULT_KEY_PATH`]) is the
//! caller's job, through a [`KeySource`].

mod config_values;
mod filter;
mod pychar;
#[rustfmt::skip]
mod pyunicode;
mod radio_diagnostics;
mod reportable_config;
mod rules;
mod sensitive_logging;
mod sentry;

pub use config_values::sweep_values_from_config;
pub use filter::{
    KeySource, LogRecord, LogRedactor, REFRESH_INTERVAL, RedactionFailure, SweepValues,
    ValueProvider,
};
pub use radio_diagnostics::{RadioRecoveryEvent, radio_recovery_message, safe_code};
pub use reportable_config::get_reportable_config;
pub use rules::{
    DEFAULT_KEY_PATH, KEY_SIZE, KeyError, MIN_SWEEP_LEN, REDACTED, Redacted, RedactionCancelled,
    RedactionState, Redactor, TZ_MASK, check_key, is_already_redacted, pseudonym,
};
pub use sensitive_logging::{
    command_metadata, credential_metadata, exception_metadata, payload_metadata,
    payload_parts_metadata,
};
pub use sentry::{drop_breadcrumb, sanitize_sentry_event};
