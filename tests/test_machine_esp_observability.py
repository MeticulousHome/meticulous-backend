import asyncio
import itertools
import time as system_time
from types import SimpleNamespace

import pytest

from config import CONFIG_USER, DISALLOW_FIRMWARE_FLASHING, MeticulousConfig
from esp_observability import (
    ESPCommunicationPhase,
    ESPObservability,
    should_start_firmware_update,
)
from machine import AlarmManager, Machine


class StopReadLoop(Exception):
    pass


class ScriptedUART:
    lines = []

    def __init__(self, _port):
        self._lines = iter(self.lines)

    def readline(self, timeout=None):
        try:
            return next(self._lines)
        except StopIteration as error:
            raise StopReadLoop from error


class CapturedSentryScope:
    def __init__(self, captured):
        self._captured = captured
        self.tags = {}
        self.contexts = {}

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False

    def set_client(self, _client):
        pass

    def set_tag(self, key, value):
        self.tags[key] = value

    def set_context(self, key, value):
        self.contexts[key] = value

    def capture_event(self, event):
        self._captured.append({"event": event, "tags": self.tags, "contexts": self.contexts})


@pytest.fixture
def run_machine_uart(monkeypatch):
    sentry_events = []
    update_calls = []
    alarm_calls = []
    port = SimpleNamespace(reset_input_buffer=lambda: None, write=lambda _data: None)

    monkeypatch.setattr(Machine, "_connection", SimpleNamespace(port=port))
    monkeypatch.setattr(Machine, "ReadLine", ScriptedUART)
    monkeypatch.setattr(Machine, "esp_info", None)
    monkeypatch.setattr(Machine, "firmware_running", None)
    monkeypatch.setattr(Machine, "infoReady", False)
    monkeypatch.setattr(Machine, "profileReady", False)
    monkeypatch.setattr(Machine, "_stopESPcomm", False)
    monkeypatch.setattr(Machine, "shot_start_time", 0)
    monkeypatch.setattr(Machine, "startTime", None)
    monkeypatch.setattr(Machine, "esp_restart_request", False)
    monkeypatch.setattr(Machine, "reset_count", 0)
    monkeypatch.setattr(Machine, "startUpdate", lambda: update_calls.append(True))
    monkeypatch.setattr(Machine, "setPartialRetraction", lambda _value: None)
    monkeypatch.setattr(Machine, "setAutoPurgeAfterShot", lambda _value: None)
    monkeypatch.setattr(Machine, "toggle_manufacturing_mode", lambda enabled: None)
    monkeypatch.setattr(
        "machine.sentry_sdk.new_scope",
        lambda: CapturedSentryScope(sentry_events),
    )
    monkeypatch.setattr(AlarmManager, "is_alarm_set", lambda _alarm: None)
    monkeypatch.setattr(
        AlarmManager,
        "set_alarm",
        lambda alarm, *_args, **_kwargs: alarm_calls.append(alarm),
    )
    monkeypatch.setattr(AlarmManager, "clear_alarm", lambda _alarm: None)
    monkeypatch.setitem(MeticulousConfig[CONFIG_USER], DISALLOW_FIRMWARE_FLASHING, False)

    def run(lines, monitor, available_firmware, monotonic_times=None):
        clock_values = iter(monotonic_times or itertools.count(3, 0.1))
        ScriptedUART.lines = [line.encode() for line in lines]
        monkeypatch.setattr(
            "machine.time",
            SimpleNamespace(
                monotonic=lambda: next(clock_values),
                time=system_time.time,
            ),
        )
        monkeypatch.setattr(Machine, "esp_observability", monitor)
        monkeypatch.setattr(Machine, "firmware_available_string", available_firmware)
        monkeypatch.setattr(
            Machine,
            "firmware_available",
            Machine._parseVersionString(available_firmware),
        )
        with pytest.raises(StopReadLoop):
            asyncio.run(Machine._read_data())
        return sentry_events, update_calls

    run.alarm_calls = alarm_calls
    return run


def test_stale_esp_info_during_update_recovery_does_not_start_another_update():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.0.0")
    monitor.begin_update("2.0.0", "1.0.0", 1)
    monitor.finish_flashing(2)

    assert monitor.observe_valid_message("ESPInfo", 3, "1.0.0") == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_BOOT
    assert monitor.expected_firmware == "2.0.0"
    assert not should_start_firmware_update(
        available_firmware="2.0.0",
        running_firmware="1.0.0",
        update_in_progress=monitor.update_in_progress,
        flashing_disallowed=False,
    )


def test_machine_update_recovery_requires_boot_and_exact_version(run_machine_uart):
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.0.0")
    monitor.begin_update("2.0.0", "1.0.0", 1)
    monitor.finish_flashing(2)

    sentry_events, update_calls = run_machine_uart(
        [
            "ESPInfo,1.0.0,1,24.0\n",
            "rst:0x1 (POWERON_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
            "ESPInfo,2.0.0-dirty,1,24.0\n",
            "ESPInfo,2.0.0,1,24.0\n",
        ],
        monitor,
        available_firmware="2.0.0",
    )

    assert sentry_events == []
    assert update_calls == []
    assert monitor.phase == ESPCommunicationPhase.NORMAL
    assert monitor.expected_firmware is None
    assert monitor.previous_firmware == "2.0.0"
    assert Machine.firmware_running == Machine._parseVersionString("2.0.0")


def test_machine_routes_one_bounded_panic_with_firmware_context(run_machine_uart):
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")

    sentry_events, update_calls = run_machine_uart(
        [
            "Guru Meditation Error: Core 1 panic'ed (Unhandled debug exception).\n",
            "Debug exception reason: Stack canary watchpoint triggered (Read ADC)\n",
            "Register dump:\n",
            "Backtrace: 0x42000000:0x3fc00000\n",
            "rst:0x3 (SW_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
            "ESPBoot,PANIC,4\n",
        ],
        monitor,
        available_firmware="1.2.3",
    )

    assert update_calls == []
    assert len(sentry_events) == 1
    captured = sentry_events[0]
    event = captured["event"]
    context = captured["contexts"]["esp-diagnostic"]
    assert event["message"] == "ESP32 firmware panic detected"
    assert event["release"] == "espresso-firmware@1.2.3"
    assert event["fingerprint"] == ["esp32-firmware-panic-stack-canary"]
    assert captured["tags"]["firmware-version"] == "1.2.3"
    assert context["previous_firmware"] == "1.2.3"
    assert context["reset_reason"] == "PANIC"
    assert context["core"] == "1"
    assert context["panic_reason"] == "Unhandled debug exception"
    assert context["exception_reason"] == "Stack canary watchpoint triggered"
    assert context["panic_location"] == "Read ADC"
    assert context["backtrace"] == "Backtrace: 0x42000000:0x3fc00000"
    assert len(context["panic_output"].encode()) <= monitor.MAX_PANIC_BYTES


def test_machine_routes_one_bounded_abort_with_firmware_context(run_machine_uart):
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")

    sentry_events, update_calls = run_machine_uart(
        [
            "abort() was called at PC 0x42000000 on core 0\n",
            "Register dump:\n",
            "Backtrace: 0x42000000:0x3fc00000\n",
            "rst:0x3 (SW_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
            "ESPBoot,PANIC,4\n",
        ],
        monitor,
        available_firmware="1.2.3",
    )

    assert update_calls == []
    assert len(sentry_events) == 1
    captured = sentry_events[0]
    event = captured["event"]
    context = captured["contexts"]["esp-diagnostic"]
    assert event["message"] == "ESP32 firmware panic detected"
    assert event["release"] == "espresso-firmware@1.2.3"
    assert event["fingerprint"] == ["esp32-firmware-panic-abort"]
    assert captured["tags"]["firmware-version"] == "1.2.3"
    assert context["previous_firmware"] == "1.2.3"
    assert context["reset_reason"] == "PANIC"
    assert context["core"] == "0"
    assert context["panic_reason"] == "abort"
    assert context["backtrace"] == "Backtrace: 0x42000000:0x3fc00000"
    assert context["panic_output"].startswith("abort() was called")
    assert len(context["panic_output"].encode()) <= monitor.MAX_PANIC_BYTES


@pytest.mark.parametrize(
    "message",
    [
        "assert failed in old diagnostic text",
        "Backtrace: is a troubleshooting label",
        "heap corruption is not what this log reports",
    ],
)
def test_valid_log_marker_text_does_not_start_raw_panic(run_machine_uart, message):
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")

    sentry_events, update_calls = run_machine_uart(
        [f"Log,info,{message}\n"],
        monitor,
        available_firmware="1.2.3",
    )

    assert sentry_events == []
    assert update_calls == []
    assert run_machine_uart.alarm_calls == []


def test_start_update_reports_pending_panic_before_automatic_reflash_cleanup(
    monkeypatch,
):
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.observe_raw_line("Guru Meditation Error: Core 1 panic'ed (LoadProhibited).", 1)
    captured = []

    monkeypatch.setattr(Machine, "esp_observability", monitor)
    monkeypatch.setattr(Machine, "esp_info", SimpleNamespace(firmwareV="1.2.3"))
    monkeypatch.setattr(Machine, "firmware_available_string", "2.0.0")
    monkeypatch.setattr(Machine, "_connection", SimpleNamespace(sendUpdate=lambda: None))
    monkeypatch.setattr(Machine, "_report_esp_diagnostics", captured.extend)
    monkeypatch.setattr("machine.time.monotonic", lambda: 2)

    Machine.startUpdate()

    assert [event.title for event in captured] == ["ESP32 firmware panic detected"]
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_BOOT


def test_machine_missing_expected_version_cannot_complete_or_reflash(run_machine_uart):
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.0.0")
    monitor.begin_update(None, "1.0.0", 1)
    monitor.finish_flashing(2)

    sentry_events, update_calls = run_machine_uart(
        [
            "rst:0x1 (POWERON_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
            "ESPInfo,2.0.0,1,24.0\n",
            "waiting for expected artifact version\n",
        ],
        monitor,
        available_firmware=None,
        monotonic_times=[3, 3, 4, 4, 123, 123],
    )

    assert update_calls == []
    assert len(sentry_events) == 1
    captured = sentry_events[0]
    assert captured["event"]["message"] == "ESP32 did not recover after firmware update"
    assert captured["event"]["fingerprint"] == ["esp32-firmware-update-recovery-failed"]
    assert captured["contexts"]["esp-diagnostic"]["expected_firmware"] is None
    assert monitor.phase == ESPCommunicationPhase.NORMAL


@pytest.mark.parametrize(
    (
        "available_firmware",
        "running_firmware",
        "update_in_progress",
        "flashing_disallowed",
        "expected",
    ),
    [
        ("2.0.0", "1.0.0", False, False, True),
        ("2.0.0", "1.0.0", True, False, False),
        ("2.0.0", "2.0.0", False, False, False),
        ("2.0.0", "1.0.0", False, True, False),
        (None, "1.0.0", False, False, False),
    ],
)
def test_should_start_firmware_update(
    available_firmware,
    running_firmware,
    update_in_progress,
    flashing_disallowed,
    expected,
):
    assert (
        should_start_firmware_update(
            available_firmware,
            running_firmware,
            update_in_progress,
            flashing_disallowed,
        )
        is expected
    )
