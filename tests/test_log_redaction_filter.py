"""Tests for the emit-time redaction filter.

Everything here drives a real logging.Logger, because the properties worth
testing are properties of the logging machinery: what a handler downstream
actually receives, that the caller never sees an exception, and that the record
is already clean by the time callHandlers() runs.
"""

import logging
import os
import tempfile
import time
import unittest

from log_redaction_filter import LogRedactionFilter, reset_key_cache
from log_redactor import pseudonym

TEST_KEY = bytes(range(32))


class CapturingHandler(logging.Handler):
    """Records what reached a handler, and what the record looked like there."""

    def __init__(self):
        super().__init__()
        self.messages = []
        self.records = []

    def emit(self, record):
        self.messages.append(record.getMessage())
        self.records.append(record)


class FilterTestCase(unittest.TestCase):
    counter = 0

    def setUp(self):
        self.tempdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tempdir.cleanup)
        self.key_path = os.path.join(self.tempdir.name, ".redaction_key")
        with open(self.key_path, "wb") as handle:
            handle.write(TEST_KEY)
        reset_key_cache()
        self.addCleanup(reset_key_cache)

        self.filter = LogRedactionFilter(key_path=self.key_path)

        # A uniquely named logger per test: logging.Logger objects are process
        # global, so a shared name would leak filters between tests.
        FilterTestCase.counter += 1
        self.logger = logging.getLogger(f"redaction_test_{FilterTestCase.counter}")
        self.logger.setLevel(logging.DEBUG)
        self.logger.propagate = False
        self.logger.addFilter(self.filter)
        self.handler = CapturingHandler()
        self.logger.addHandler(self.handler)

    def seed(self, ssids=(), credentials=()):
        self.filter.set_value_provider(lambda: (list(ssids), list(credentials)))

    @property
    def last(self):
        return self.handler.messages[-1]


class ShapeRuleTests(FilterTestCase):
    def test_credential_key_value_is_redacted(self):
        self.logger.info("CONF:   root_password: s3cr3tvalue")
        self.assertEqual(self.last, "CONF:   root_password: [REDACTED]")

    def test_mac_and_ipv6_become_tokens(self):
        self.logger.info("authenticate with aa:bb:cc:dd:ee:ff via fe80::1122:3344:5566:7788")
        self.assertNotIn("aa:bb:cc:dd:ee:ff", self.last)
        self.assertNotIn("fe80::1122:3344:5566:7788", self.last)
        self.assertRegex(self.last, r"with \[MAC_[0-9a-f]{8}\] via \[IPV6_[0-9a-f]{8}\]")

    def test_anchored_ssid_is_redacted_without_seeding(self):
        self.logger.info("Trying to associate with SSID 'HomeNet'")
        self.assertNotIn("HomeNet", self.last)

    def test_timezone_is_coarsened(self):
        self.logger.info("Changed time zone to 'America/Mexico_City' (CST).")
        self.assertEqual(self.last, "Changed time zone to 'America/*****' (*****).")


class ConfigSeededSweepTests(FilterTestCase):
    def test_bare_known_ssid_is_redacted(self):
        """The case no shape rule can reach -- wifi.py's tryAutoConnect message.

        There is no anchored phrasing here and nothing for the watcher's literal
        sweep to have learned from, so this only works because the backend seeds
        the sweep from its own config.
        """
        self.seed(ssids=["HomeNet"])
        self.logger.info("Found known WIFI HomeNet. Connecting")
        self.assertEqual(
            self.last,
            f"Found known WIFI {pseudonym('SSID', 'HomeNet', TEST_KEY)}. Connecting",
        )

    def test_bare_credential_is_destroyed_not_pseudonymised(self):
        self.seed(credentials=["sup3rs3cret"])
        self.logger.info("nmcli failed: connection needs sup3rs3cret to activate")
        self.assertNotIn("sup3rs3cret", self.last)
        self.assertIn("[REDACTED]", self.last)

    def test_credential_wins_when_a_value_is_both(self):
        """A password that happens to equal a network name is still a password."""
        self.seed(ssids=["sharedvalue"], credentials=["sharedvalue"])
        self.logger.info("saw sharedvalue in the wild")
        self.assertEqual(self.last, "saw [REDACTED] in the wild")

    def test_longest_value_wins_at_the_same_position(self):
        self.seed(ssids=["Coffee", "Coffee Bar"])
        self.logger.info("joined Coffee Bar now")
        self.assertEqual(self.last, f"joined {pseudonym('SSID', 'Coffee Bar', TEST_KEY)} now")

    def test_short_values_are_not_swept(self):
        """Below the sweep floor, a name collides with ordinary log text."""
        self.seed(ssids=["up"])
        self.logger.info("link is up and running")
        self.assertEqual(self.last, "link is up and running")

    def test_word_boundaries_are_respected(self):
        self.seed(ssids=["Home"])
        self.logger.info("Homeless value and Home itself")
        self.assertIn("Homeless", self.last)
        self.assertNotIn(" Home itself", self.last)

    def test_invalidate_picks_up_a_new_network(self):
        networks = ["FirstNet"]
        self.filter.set_value_provider(lambda: (list(networks), []))
        self.logger.info("saw FirstNet and SecondNet")
        self.assertNotIn("FirstNet", self.last)
        self.assertIn("SecondNet", self.last)

        networks.append("SecondNet")
        self.filter.invalidate()
        self.logger.info("saw FirstNet and SecondNet")
        self.assertNotIn("SecondNet", self.last)

    def test_sweep_refreshes_without_an_explicit_invalidate(self):
        """The cache has a ceiling, so a missed invalidate cannot persist."""
        networks = []
        self.filter.set_value_provider(lambda: (list(networks), []))
        self.logger.info("nothing yet")

        networks.append("LateNet")
        # Reach past the refresh interval rather than sleeping through it.
        self.filter._sweep_deadline = time.monotonic() - 1
        self.logger.info("saw LateNet")
        self.assertNotIn("LateNet", self.last)

    def test_provider_failure_does_not_leak_or_raise(self):
        def broken():
            raise RuntimeError("config unavailable")

        self.filter.set_value_provider(broken)
        self.logger.info("Found known WIFI HomeNet. Connecting")
        self.assertNotIn("HomeNet", self.last)
        self.assertIn("redaction failed", self.last)

    def test_provider_failure_keeps_failing_instead_of_degrading(self):
        """A broken provider must not fall back to shape rules alone.

        The refresh deadline used to be extended before the provider was called,
        so one record failed loudly and every record for the next two seconds was
        redacted by the shape rules only -- which put a bare network name in the
        journal in the clear, with nothing in the log to say so.
        """

        def broken():
            raise RuntimeError("config unavailable")

        self.filter.set_value_provider(broken)
        for _ in range(3):
            self.logger.info("Found known WIFI HomeNet. Connecting")

        for message in self.handler.messages:
            self.assertNotIn("HomeNet", message)
            self.assertIn("redaction failed", message)

    def test_recovery_after_a_provider_failure(self):
        state = {"broken": True}

        def provider():
            if state["broken"]:
                raise RuntimeError("config unavailable")
            return ["HomeNet"], []

        self.filter.set_value_provider(provider)
        self.logger.info("Found known WIFI HomeNet. Connecting")
        self.assertIn("redaction failed", self.last)

        state["broken"] = False
        self.logger.info("Found known WIFI HomeNet. Connecting")
        self.assertNotIn("HomeNet", self.last)
        self.assertNotIn("redaction failed", self.last)


class RecordShapeTests(FilterTestCase):
    def test_percent_args_are_formatted_then_redacted(self):
        self.seed(ssids=["HomeNet"])
        self.logger.info("connecting to %s on %s", "HomeNet", "wlan0")
        self.assertNotIn("HomeNet", self.last)
        self.assertIn("on wlan0", self.last)
        # Left un-consumed, a handler would try to re-apply them and raise.
        self.assertIsNone(self.handler.records[-1].args)

    def test_non_string_msg_survives(self):
        self.logger.info({"root_password": "s3cr3tvalue"})
        self.assertNotIn("s3cr3tvalue", self.last)

    def test_multiline_record_is_redacted_line_by_line(self):
        self.logger.info("first: aa:bb:cc:dd:ee:ff\nsecond: root_password: s3cr3tvalue")
        self.assertNotIn("aa:bb:cc:dd:ee:ff", self.last)
        self.assertNotIn("s3cr3tvalue", self.last)
        self.assertEqual(len(self.last.splitlines()), 2)

    def test_traceback_text_is_redacted(self):
        self.seed(ssids=["HomeNet"])
        try:
            raise ValueError("could not reach HomeNet at aa:bb:cc:dd:ee:ff")
        except ValueError:
            self.logger.error("wifi failed", exc_info=True)

        record = self.handler.records[-1]
        rendered = logging.Formatter().format(record)
        self.assertNotIn("HomeNet", rendered)
        self.assertNotIn("aa:bb:cc:dd:ee:ff", rendered)
        self.assertIn("ValueError", rendered)
        # exc_info is left in place so sentry_privacy still gets an exception to
        # work with; exc_text is what every Formatter renders.
        self.assertIsNotNone(record.exc_info)

    def test_stack_info_is_redacted(self):
        self.seed(credentials=["s3cr3tvalue"])
        self.logger.info("saving s3cr3tvalue", stack_info=True)
        record = self.handler.records[-1]
        self.assertNotIn("s3cr3tvalue", record.stack_info)
        self.assertNotIn("s3cr3tvalue", logging.Formatter().format(record))

    def test_output_is_idempotent(self):
        self.seed(ssids=["HomeNet"])
        self.logger.info("Found known WIFI HomeNet. Connecting")
        once = self.last
        self.logger.info(once)
        self.assertEqual(self.last, once)

    def test_an_already_redacted_record_is_not_reprocessed(self):
        """redact_ssid() output must survive the filter unchanged."""
        token = pseudonym("SSID", "HomeNet", TEST_KEY)
        self.logger.info(f"Connecting to wifi: {token}")
        self.assertEqual(self.last, f"Connecting to wifi: {token}")


class KnownWifisContinuationTests(FilterTestCase):
    def test_yaml_block_spanning_records_is_tracked(self):
        """The config dump is one log record per line.

        Nothing else carries block state across calls, so this is what the
        RedactionState in log_redactor exists for. Records from another logger are
        interleaved to prove the tracking is not confused by them.
        """
        other = logging.getLogger("redaction_test_interleaved")
        other.setLevel(logging.DEBUG)
        other.propagate = False
        other.addFilter(self.filter)
        other.addHandler(self.handler)

        self.logger.debug("CONF: wifi:")
        other.info("an unrelated record arrives mid-dump")
        self.logger.debug("CONF:   KnownWifis:")
        other.info("and another one")
        self.logger.debug("CONF:     HomeNet:")
        self.logger.debug("CONF:       password: s3cr3tvalue")
        self.logger.debug("CONF:     OfficeNet:")

        joined = "\n".join(self.handler.messages)
        self.assertNotIn("HomeNet", joined)
        self.assertNotIn("OfficeNet", joined)
        self.assertNotIn("s3cr3tvalue", joined)
        # The attribute level is not an SSID -- the key itself must survive.
        self.assertIn("password: [REDACTED]", joined)


class FailureModeTests(FilterTestCase):
    def test_unreadable_key_yields_a_placeholder_and_never_raises(self):
        broken = LogRedactionFilter(key_path=os.path.join(self.tempdir.name, "nope/key"))
        self.logger.removeFilter(self.filter)
        self.logger.addFilter(broken)

        self.logger.info("Connecting to wifi: HomeNet with aa:bb:cc:dd:ee:ff")

        self.assertNotIn("HomeNet", self.last)
        self.assertNotIn("aa:bb:cc:dd:ee:ff", self.last)
        self.assertIn("redaction failed", self.last)
        # The logger name survives, which is the point of the placeholder.
        self.assertIn(self.logger.name, self.last)

    def test_failure_drops_traceback_text(self):
        broken = LogRedactionFilter(key_path=os.path.join(self.tempdir.name, "nope/key"))
        self.logger.removeFilter(self.filter)
        self.logger.addFilter(broken)

        try:
            raise ValueError("could not reach HomeNet")
        except ValueError:
            self.logger.error("wifi failed", exc_info=True)

        record = self.handler.records[-1]
        self.assertIsNone(record.exc_info)
        self.assertNotIn("HomeNet", logging.Formatter().format(record))


class OrderingTests(FilterTestCase):
    def test_record_is_clean_before_handlers_run(self):
        """Sentry's LoggingIntegration patches Logger.callHandlers and reads the
        record after the handlers have run. A logger-level filter runs inside
        Logger.handle() before callHandlers(), so if the record is already clean
        when a handler sees it, it is clean for Sentry too."""
        self.seed(ssids=["HomeNet"])
        self.logger.info("Found known WIFI HomeNet. Connecting")

        record = self.handler.records[-1]
        self.assertNotIn("HomeNet", record.msg)
        self.assertNotIn("HomeNet", record.getMessage())


class ThroughputTests(FilterTestCase):
    def test_throughput_budget(self):
        """DEBUG logging is hot on this device -- log_all_sensor_messages makes it
        hotter. A future rule must not quietly make this the bottleneck."""
        self.seed(
            ssids=[f"Network{index}" for index in range(40)],
            credentials=[f"secret{index}" for index in range(10)],
        )
        message = (
            "tornado.access INFO 304 GET /api/v1/settings/ (10.10.0.79) 4.10ms "
            "state=idle sensor=1234 profile=default"
        )
        # Warm the sweep so compilation is not counted.
        self.logger.info(message)

        # A 4,000-record window is only about 0.2 seconds on CI, so one normal
        # scheduler interruption can move an otherwise >20k records/s run below
        # the threshold. Keep the same throughput requirement, but measure it
        # over a sustained interval that is long enough to absorb runner noise.
        iterations = 20000
        started = time.perf_counter()
        for _ in range(iterations):
            self.logger.info(message)
        elapsed = time.perf_counter() - started

        rate = iterations / elapsed
        self.assertGreater(
            rate,
            20000,
            f"redaction throughput fell to {rate:.0f} records/s",
        )


if __name__ == "__main__":
    unittest.main()
