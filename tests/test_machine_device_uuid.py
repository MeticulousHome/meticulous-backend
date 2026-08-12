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
        generated_assignments.append(
            (reported_uuid, protocol_supported, pending_assignment)
        )
        return pending_assignment or FIRST_UUID

    monkeypatch.setattr(Machine, "writeStr", writes.append)
    monkeypatch.setattr(machine, "get_device_uuid_assignment", assign)
    monkeypatch.setattr(machine, "update_device_uuid_cache", pytest.fail)

    Machine.syncDeviceUUID("", True)
    Machine.syncDeviceUUID("invalid", True)

    assert generated_assignments == [("", True, None), ("invalid", True, FIRST_UUID)]
    assert writes == [
        f"device_uuid,assign,{FIRST_UUID}\x03",
        f"device_uuid,assign,{FIRST_UUID}\x03",
    ]
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


def test_changed_cache_queues_bounded_updater_restart(monkeypatch):
    calls = []
    monkeypatch.setattr(machine, "update_device_uuid_cache", lambda _device_uuid: True)
    monkeypatch.setattr(
        machine.subprocess,
        "run",
        lambda *args, **kwargs: calls.append((args, kwargs))
        or subprocess.CompletedProcess(args[0], 0),
    )

    Machine.syncDeviceUUID(FIRST_UUID, True)

    assert calls == [
        (
            (
                [
                    "systemctl",
                    "--no-block",
                    "try-restart",
                    "rauc-hawkbit-updater.service",
                ],
            ),
            {"capture_output": True, "text": True, "timeout": 5},
        )
    ]


def test_updater_restart_timeout_does_not_escape(monkeypatch):
    monkeypatch.setattr(machine, "update_device_uuid_cache", lambda _device_uuid: True)

    def timeout(*args, **kwargs):
        raise subprocess.TimeoutExpired(args[0], kwargs["timeout"])

    monkeypatch.setattr(machine.subprocess, "run", timeout)

    Machine.syncDeviceUUID(FIRST_UUID, True)
