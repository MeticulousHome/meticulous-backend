import asyncio
from types import SimpleNamespace

import pytest

from config import CONFIG_USER, DISALLOW_FIRMWARE_FLASHING, MeticulousConfig
from esp_observability import ESPCommunicationPhase, ESPObservability
from machine import AlarmManager, Machine


class StopReadLoop(Exception):
    pass


class OneLineUART:
    def __init__(self, _port):
        self.lines = [b"ESPInfo,1.0.0,1,24.0\n"]

    def readline(self, timeout=None):
        if self.lines:
            return self.lines.pop(0)
        raise StopReadLoop


def test_stale_esp_info_during_update_recovery_does_not_start_another_update(
    monkeypatch,
):
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.0.0")
    monitor.begin_update("2.0.0", "1.0.0", 1)
    monitor.finish_flashing(2)
    update_calls = []

    port = SimpleNamespace(reset_input_buffer=lambda: None, write=lambda _data: None)
    monkeypatch.setattr(Machine, "_connection", SimpleNamespace(port=port))
    monkeypatch.setattr(Machine, "ReadLine", OneLineUART)
    monkeypatch.setattr(Machine, "esp_observability", monitor)
    monkeypatch.setattr(Machine, "firmware_available_string", "2.0.0")
    monkeypatch.setattr(Machine, "firmware_available", Machine._parseVersionString("2.0.0"))
    monkeypatch.setattr(Machine, "esp_info", None)
    monkeypatch.setattr(Machine, "firmware_running", None)
    monkeypatch.setattr(Machine, "infoReady", False)
    monkeypatch.setattr(Machine, "_stopESPcomm", False)
    monkeypatch.setattr(Machine, "shot_start_time", 0)
    monkeypatch.setattr(Machine, "startTime", None)
    monkeypatch.setattr(Machine, "esp_restart_request", False)
    monkeypatch.setattr(Machine, "reset_count", 0)
    monkeypatch.setattr(Machine, "startUpdate", lambda: update_calls.append(True))
    monkeypatch.setattr(Machine, "setPartialRetraction", lambda _value: None)
    monkeypatch.setattr(Machine, "setAutoPurgeAfterShot", lambda _value: None)
    monkeypatch.setattr(Machine, "toggle_manufacturing_mode", lambda enabled: None)
    monkeypatch.setattr(AlarmManager, "clear_alarm", lambda _alarm: None)
    monkeypatch.setitem(MeticulousConfig[CONFIG_USER], DISALLOW_FIRMWARE_FLASHING, False)

    with pytest.raises(StopReadLoop):
        asyncio.run(Machine._read_data())

    assert update_calls == []
    assert Machine.firmware_running == Machine._parseVersionString("1.0.0")
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_BOOT
    assert monitor.expected_firmware == "2.0.0"
