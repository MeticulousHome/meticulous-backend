"""Redact personal data out of log records before any handler sees them.

The watcher redacts at *read* time: it filters the journal when a bug report is
built. That leaves the plaintext on the device -- in the journal on disk, in the
shot-debug log files that ship inside the report, and in the Sentry events that
LoggingIntegration builds from logger.error() calls. This filter closes that
window by redacting at *emit* time, using the same rules and the same
/root/.redaction_key, so a value gets the same token in both services.

Installed at logger level by log.py. logging.Logger.handle() runs logger filters
before callHandlers(), and sentry_sdk's LoggingIntegration patches callHandlers
(sentry_sdk/integrations/logging.py), so mutating the record here reaches the
journal, every handler, and Sentry's logentry.message -- in that order, always.

This is a backstop, not the primary defence. Values are still kept out of log
messages at the call site (reportable_config.get_reportable_config for the config
dump, wifi.redact_ssid, base_handler.redact_ip, TimezoneManager.redact_timezone,
`type(e).__name__` instead of `{e}`). Those cover classes no regex can reach --
an allowlist over structured config, IPv4 addresses, MACs embedded in DBus object
paths with underscores. Do not remove them because this filter exists.
"""

import logging
import os
import re
import threading
import time
import traceback

from log_redactor import (
    DEFAULT_KEY_PATH,
    MIN_SWEEP_LEN,
    REDACTED,
    RE_ALREADY_DONE,
    RedactionState,
    load_key,
    pseudonym,
    redact,
)

# Set on records this filter has already processed. The filter is installed both
# on loggers and on the root handlers so third-party loggers are covered too, and
# a record that took both routes must not be worked twice.
_MARK = "_meticulous_redacted"

# Upper bound on how stale the config-seeded sweep may be. Rebuilding walks the
# config, and DEBUG logging is hot enough that doing so per record would show up.
_REFRESH_INTERVAL_SECONDS = 2.0

_SSID = "SSID"
_CRED = "CRED"

# The service runs as root on the machine and shares the watcher's key, which is
# what makes the tokens match. Overridable for tests and dev hosts, which are not
# root and would otherwise have every record replaced by the failure placeholder.
# Not a secret: the key is per-device and reveals nothing about any other device.
KEY_PATH = os.getenv("REDACTION_KEY_PATH", DEFAULT_KEY_PATH)

_key_cache = {}
_key_cache_lock = threading.Lock()


def load_cached_key(path=None):
    """Load the per-device key once per path, per process.

    load_key() reads the file and chmods it on every call. Both this filter and
    wifi.redact_ssid() are on hot paths -- a scan result per network, a log record
    per line -- so the result is cached. Raises on failure; callers decide what
    that means for them.
    """
    path = KEY_PATH if path is None else path
    with _key_cache_lock:
        key = _key_cache.get(path)
        if key is None:
            key = _key_cache[path] = load_key(path)
        return key


def reset_key_cache():
    """Forget cached keys. For tests that swap the key file underneath."""
    with _key_cache_lock:
        _key_cache.clear()


class LogRedactionFilter(logging.Filter):
    """Rewrites record.msg through the shared redaction rules.

    Two sources of coverage:

    * The rules in log_redactor, which match on shape -- credentials in key:value
      form, MACs, IPv6, anchored SSID phrasings, IANA timezones.
    * A literal sweep of values read from the live config. This is the part the
      watcher cannot do. Shape rules cannot catch a bare SSID in an arbitrary
      message ("Found known WIFI HomeNet. Connecting"), and at emit time there is
      no surrounding anchored line to learn one from. The backend does not have
      to guess: it holds the real known-network names and credentials.
    """

    def __init__(self, key_path=None):
        super().__init__()
        key_path = KEY_PATH if key_path is None else key_path
        self._key_path = key_path
        # Reentrant on purpose. Handler.handle() runs filters before taking the
        # handler's I/O lock, so nothing else serialises access to this state --
        # and the value provider reads the config, so if anything on that path
        # ever logs, a plain Lock would deadlock the caller instead of just
        # producing a slightly odd nested redaction.
        self._lock = threading.RLock()
        self._state = RedactionState()
        self._value_provider = None
        self._sweep_pattern = None
        self._sweep_kinds = {}
        self._sweep_deadline = 0.0

    # -- configuration -----------------------------------------------------

    def set_value_provider(self, provider):
        """Register a callable returning (ssids, credentials) from live config.

        log.py cannot import config.py -- config.py imports log -- so the
        dependency is inverted: config registers itself here once its singleton
        exists. Until then the filter runs on shape rules alone.
        """
        with self._lock:
            self._value_provider = provider
            self._sweep_deadline = 0.0

    def invalidate(self):
        """Drop the cached sweep. Called when the config is saved."""
        with self._lock:
            self._sweep_deadline = 0.0

    # -- sweep -------------------------------------------------------------

    def _rebuild_sweep_locked(self):
        self._sweep_pattern = None
        self._sweep_kinds = {}

        if self._value_provider is None:
            self._sweep_deadline = time.monotonic() + _REFRESH_INTERVAL_SECONDS
            return

        # Deliberately before the deadline is extended: if the provider raises,
        # the deadline stays in the past so the next record retries and fails
        # again. Extending it first would mean one placeholder followed by two
        # seconds of records redacted by the shape rules alone -- a bare network
        # name would go to the journal in the clear with nothing to show for it.
        ssids, credentials = self._value_provider()

        kinds = {}
        for value in credentials:
            if self._is_sweepable(value):
                kinds[str(value)] = _CRED
        for value in ssids:
            # A credential that happens to equal a network name stays a
            # credential: destroyed outright rather than pseudonymised.
            if self._is_sweepable(value) and str(value) not in kinds:
                kinds[str(value)] = _SSID

        if kinds:
            # One alternation rather than a substitution per value: a wifi scan in
            # a dense building can put dozens of names in the config, and this
            # runs on every log record. Longest first so a name that contains a
            # shorter one wins at the same position.
            alternatives = "|".join(
                re.escape(value) for value in sorted(kinds, key=len, reverse=True)
            )
            self._sweep_pattern = re.compile(rf"(?<![\w-]){alternatives}(?![\w-])")
            self._sweep_kinds = kinds

        self._sweep_deadline = time.monotonic() + _REFRESH_INTERVAL_SECONDS

    @staticmethod
    def _is_sweepable(value):
        if not isinstance(value, str) or not value:
            return False
        # Same floor the rules use for learned SSIDs: anything shorter collides
        # with ordinary log text. Short values are still caught by the shape
        # rules when they appear in a key:value or anchored form.
        if len(value) < MIN_SWEEP_LEN:
            return False
        return not RE_ALREADY_DONE.match(value)

    def _sweep_locked(self, text, key):
        if time.monotonic() >= self._sweep_deadline:
            self._rebuild_sweep_locked()
        if self._sweep_pattern is None:
            return text

        def _replace(match):
            value = match.group(0)
            if self._sweep_kinds.get(value) == _SSID:
                return pseudonym(_SSID, value, key)
            return REDACTED

        return self._sweep_pattern.sub(_replace, text)

    # -- filtering ---------------------------------------------------------

    def _redact(self, text, key):
        with self._lock:
            redacted, _ = redact(text, key, state=self._state)
            return self._sweep_locked(redacted, key)

    def filter(self, record):
        if record.__dict__.get(_MARK):
            return True

        try:
            key = load_cached_key(self._key_path)

            record.msg = self._redact(record.getMessage(), key)
            record.args = None

            if record.exc_info and not record.exc_text:
                # Render it here so the traceback text can be redacted. A
                # Formatter reuses exc_text and skips formatting exc_info, so
                # every handler gets the redacted rendering. exc_info itself is
                # left in place: Sentry builds its exception from it, and
                # scrubbing the structured event is sentry_privacy's job.
                rendered = "".join(traceback.format_exception(*record.exc_info))
                if rendered.endswith("\n"):
                    rendered = rendered[:-1]
                record.exc_text = self._redact(rendered, key)

            if record.stack_info:
                record.stack_info = self._redact(record.stack_info, key)
        except Exception as exception:
            # A filter that raises propagates into whatever called logger.info(),
            # so failure can never be loud. Keep the fact that something was
            # logged and where, drop the content. Never log from in here.
            record.msg = (
                f"<redaction failed for a {record.name!r} record: "
                f"{type(exception).__name__}>"
            )
            record.args = None
            record.exc_info = None
            record.exc_text = None
            record.stack_info = None

        record.__dict__[_MARK] = True
        return True
