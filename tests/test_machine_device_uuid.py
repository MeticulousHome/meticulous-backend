import asyncio
import importlib
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

# GPIO access is available only on the target machine. Isolate it here so this
# machine-level test can exercise the UUID synchronization logic with CI's dev
# dependencies, without adding a hardware-only package to that environment.
sys.modules.setdefault("gpiod", MagicMock())
machine = importlib.import_module("machine")
Machine = machine.Machine


FIRST_UUID = "123e4567-e89b-42d3-a456-426614174000"
SECOND_UUID = "987e6543-e21b-45d3-b654-426614174999"


@pytest.fixture(autouse=True)
def reset_machine_device_uuid_state(monkeypatch):
    monkeypatch.setattr(Machine, "pending_device_uuid_assignment", None)
    monkeypatch.setattr(Machine, "device_uuid_assignment_attempts", 0)
    monkeypatch.setattr(Machine, "device_uuid_retry_timer", None)
    monkeypatch.setattr(Machine, "emulated", False)


def test_unsupported_firmware_receives_no_assignment(monkeypatch):
    monkeypatch.setattr(Machine, "writeStr", pytest.fail)
    monkeypatch.setattr(machine, "update_device_uuid_cache", pytest.fail)

    Machine.syncDeviceUUID("", False)

    assert Machine.pending_device_uuid_assignment is None


def test_missing_uuid_reuses_pending_assignment_within_boot(monkeypatch):
    writes = []
    generated_assignments = []

    def assign(reported_uuid, protocol_supported, pending_assignment):
        generated_assignments.append((reported_uuid, protocol_supported, pending_assignment))
        return pending_assignment or FIRST_UUID

    monkeypatch.setattr(Machine, "writeStr", writes.append)
    monkeypatch.setattr(machine, "get_device_uuid_assignment", assign)
    monkeypatch.setattr(machine, "update_device_uuid_cache", pytest.fail)

    Machine.syncDeviceUUID("", True)
    Machine.syncDeviceUUID("invalid", True)

    assert generated_assignments == [("", True, None)]
    assert writes == [f"device_uuid,assign,{FIRST_UUID}\x03"]
    assert Machine.pending_device_uuid_assignment == FIRST_UUID


def test_esp_boot_clears_pending_assignment(monkeypatch):
    class EndRead(Exception):
        pass

    class BootPort:
        def __init__(self):
            self.message = b"rst:0x1 (POWERON_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n"

        @property
        def in_waiting(self):
            return len(self.message)

        def reset_input_buffer(self):
            pass

        def write(self, _content):
            pass

        def read(self, _size):
            if not self.message:
                raise EndRead
            message, self.message = self.message, b""
            return message

    monkeypatch.setattr(Machine, "_connection", SimpleNamespace(port=BootPort()))
    monkeypatch.setattr(Machine, "_stopESPcomm", False)
    monkeypatch.setattr(Machine, "infoReady", False)
    monkeypatch.setattr(Machine, "reset_count", 0)
    monkeypatch.setattr(Machine, "esp_restart_request", True)
    Machine.pending_device_uuid_assignment = FIRST_UUID

    with pytest.raises(EndRead):
        asyncio.run(Machine._read_data())

    assert Machine.pending_device_uuid_assignment is None


def test_confirmed_uuid_updates_cache_without_restart_when_unchanged(monkeypatch):
    Machine.pending_device_uuid_assignment = SECOND_UUID
    cache_updates = []
    monkeypatch.setattr(
        machine,
        "update_device_uuid_cache",
        lambda device_uuid: cache_updates.append(device_uuid) and False,
    )
    monkeypatch.setattr(machine.subprocess, "run", pytest.fail)

    Machine.syncDeviceUUID(FIRST_UUID, True)

    assert cache_updates == [FIRST_UUID]
    assert Machine.pending_device_uuid_assignment is None


def test_changed_cache_starts_background_updater_restart(monkeypatch):
    started_threads = []

    class FakeThread:
        def __init__(self, name, target, daemon):
            started_threads.append((name, target, daemon, self))

        def start(self):
            self.started = True

    monkeypatch.setattr(machine, "update_device_uuid_cache", lambda _device_uuid: True)
    monkeypatch.setattr(machine, "NamedThread", FakeThread)
    monkeypatch.setattr(machine.subprocess, "run", pytest.fail)

    Machine.syncDeviceUUID(FIRST_UUID, True)

    assert len(started_threads) == 1
    name, target, daemon, thread = started_threads[0]
    assert name == "HawkbitRestart"
    assert target == Machine._restartHawkbitUpdater
    assert daemon is True
    assert thread.started


def test_updater_restart_is_enqueued_for_inactive_unit_and_is_bounded(monkeypatch):
    calls = []
    monkeypatch.setattr(
        machine.subprocess,
        "run",
        lambda *args, **kwargs: calls.append((args, kwargs))
        or subprocess.CompletedProcess(args[0], 0, "", ""),
    )

    Machine._restartHawkbitUpdater()

    assert calls == [
        (
            (
                [
                    "systemctl",
                    "--no-block",
                    "restart",
                    "rauc-hawkbit-updater.service",
                ],
            ),
            {
                "capture_output": True,
                "text": True,
                "timeout": Machine.HAWKBIT_UPDATER_RESTART_TIMEOUT_SECONDS,
            },
        )
    ]


@pytest.mark.parametrize(
    "error",
    [OSError("systemctl unavailable"), subprocess.TimeoutExpired("systemctl", 5)],
)
def test_updater_restart_command_errors_do_not_escape(monkeypatch, error):
    log = MagicMock()

    def fail(*_args, **_kwargs):
        raise error

    monkeypatch.setattr(machine, "logger", log)
    monkeypatch.setattr(machine.subprocess, "run", fail)

    Machine._restartHawkbitUpdater()

    log.exception.assert_called_once()


def test_updater_restart_nonzero_result_is_logged(monkeypatch):
    log = MagicMock()
    monkeypatch.setattr(machine, "logger", log)
    monkeypatch.setattr(
        machine.subprocess,
        "run",
        lambda *args, **_kwargs: subprocess.CompletedProcess(args[0], 1, "", "failed"),
    )

    Machine._restartHawkbitUpdater()

    log.warning.assert_called_once()


@pytest.mark.parametrize(
    ("response", "level"),
    [
        ("SUCCESS", "info"),
        ("ALREADY_ASSIGNED", "info"),
        ("ERROR_INVALID_FORMAT", "error"),
        ("ERROR_INVALID_UUID", "error"),
        ("UNEXPECTED_RESPONSE", "error"),
    ],
)
def test_each_firmware_response_is_logged_exactly(monkeypatch, response, level):
    log = MagicMock()
    monkeypatch.setattr(machine, "logger", log)
    Machine.pending_device_uuid_assignment = FIRST_UUID
    Machine.device_uuid_assignment_attempts = 1

    Machine.handleDeviceUUIDResponse(response)

    messages = [str(call) for call in getattr(log, level).call_args_list]
    assert any(response in message for message in messages)


def test_write_failure_retries_same_candidate_after_non_busy_delay(monkeypatch):
    log = MagicMock()
    writes = []
    timers = []

    class FakeTimer:
        def __init__(self, delay, callback):
            self.delay = delay
            self.callback = callback
            self.daemon = False
            self.started = False
            timers.append(self)

        def start(self):
            self.started = True

        def cancel(self):
            pass

    monkeypatch.setattr(machine.threading, "Timer", FakeTimer)
    monkeypatch.setattr(machine, "logger", log)
    monkeypatch.setattr(Machine, "writeStr", writes.append)
    Machine.pending_device_uuid_assignment = FIRST_UUID
    Machine.device_uuid_assignment_attempts = 1

    Machine.handleDeviceUUIDResponse("ERROR_WRITE_FAILED")

    assert writes == []
    assert len(timers) == 1
    assert timers[0].delay == Machine.DEVICE_UUID_RETRY_DELAY_SECONDS
    assert timers[0].started
    assert timers[0].daemon
    assert "ERROR_WRITE_FAILED" in str(log.warning.call_args)

    timers[0].callback()

    assert writes == [f"device_uuid,assign,{FIRST_UUID}\x03"]
    assert Machine.device_uuid_assignment_attempts == 2


def test_persistent_write_failures_stop_after_bounded_attempts(monkeypatch):
    log = MagicMock()
    monkeypatch.setattr(machine, "logger", log)
    monkeypatch.setattr(machine.threading, "Timer", pytest.fail)
    monkeypatch.setattr(Machine, "writeStr", pytest.fail)
    Machine.pending_device_uuid_assignment = FIRST_UUID
    Machine.device_uuid_assignment_attempts = Machine.DEVICE_UUID_MAX_ASSIGNMENT_ATTEMPTS

    Machine.handleDeviceUUIDResponse("ERROR_WRITE_FAILED")

    assert "ERROR_WRITE_FAILED" in str(log.error.call_args)
    assert "exhausted" in str(log.error.call_args)
