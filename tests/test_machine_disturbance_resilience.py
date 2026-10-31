"""disturbance_resilience between the UART reader and a waiting settings write.

The ESP32 owns the setting (EspressoFirmware contracts/uart/protocol.json: NVS key
disturbance_resilience_key, ESPInfo's trailing field). The Dial shows the new
value only once POST /api/v1/settings answered, and that answer waits for the
ESP's nvs_response. These tests run the real reader, Machine._read_data, on its
own thread and event loop as on the machine, and the waiting write on another
loop, and guard:

- a SUCCESS reply resolves the write and updates the cached ESPInfo value;
- the reply old firmware sends for an unknown key, which names no key, fails the
  write instead of leaving the Dial waiting for the timeout;
- an ESP reboot fails a pending write: the reply will never come;
- an ESPInfo carrying the field puts the ESP's value into the user config and
  sends nothing back (unlike auto_purge and tare_behavior, which the backend
  pushes), and an ESPInfo without it leaves the config alone.

The lines are the ones the firmware sends (fika-sil format).
"""

import asyncio
import queue
import threading
from types import SimpleNamespace

import pytest

from config import (
    CONFIG_SYSTEM,
    CONFIG_USER,
    PROFILE_DISTURBANCE_RESILIENCE,
    MeticulousConfig,
)
from esp_observability import ESPObservability
from esp_serial.data import ESPInfo
from machine import AlarmManager, DisturbanceResilienceWriteError, Machine

ESP_INFO_PREFIX = (
    "ESPInfo,0.9.10-sil,off,120,black,SIL-TM-0001,0025,2025-11-14,acaia,45.33,false,"
    "after_retraction,"
)
ESP_INFO_WITHOUT_FIELD = ESP_INFO_PREFIX + "\n"
ROM_BOOT_BANNER = "rst:0x3 (SW_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n"
WRITE_TRUE = b"nvs_request,write,disturbance_resilience_key,true\x03"


class StopReadLoop(Exception):
    pass


class RecordingPort:
    def __init__(self):
        self.writes = []
        self.written = threading.Event()

    def reset_input_buffer(self):
        pass

    def write(self, data):
        self.writes.append(data)
        self.written.set()


@pytest.fixture
def esp(monkeypatch):
    """The real reader on a MachineSerial-like thread, fed one scripted line at a time."""
    port = RecordingPort()
    lines: queue.Queue = queue.Queue()
    monkeypatch.setattr(Machine, "_connection", SimpleNamespace(port=port))
    for attribute, value in {
        "esp_info": ESPInfo(disturbanceResilience=False),
        "infoReady": True,
        "mileage": 0,
        "profileReady": False,
        "_stopESPcomm": False,
        "_info_requested_at": None,
        "reset_count": 0,
        "startTime": None,
        "firmware_running": None,
        "firmware_available": None,
        "firmware_available_string": None,
        "esp_restart_request": False,
        "_pending_disturbance_resilience_writes": [],
        "_pending_tare_behavior_writes": [],
    }.items():
        monkeypatch.setattr(Machine, attribute, value)
    monkeypatch.setattr(Machine, "esp_observability", ESPObservability(now=0.0))
    for name in (
        "requestMileage",
        "setPartialRetraction",
        "setAutoPurgeAfterShot",
        "setTareBehavior",
        "syncDeviceUUID",
        "toggle_manufacturing_mode",
        "startUpdate",
        "_resetDeviceUUIDEnrollment",
    ):
        monkeypatch.setattr(Machine, name, lambda *args, **kwargs: None)
    monkeypatch.setattr(AlarmManager, "is_alarm_set", lambda _alarm: None)
    saves = []
    monkeypatch.setattr("machine.MeticulousConfig.save", lambda *a, **k: saves.append(1))
    # The reader stores ESPInfo's identity and the setting in the global config.
    for section in (CONFIG_USER, CONFIG_SYSTEM):
        monkeypatch.setitem(MeticulousConfig, section, dict(MeticulousConfig[section]))
    MeticulousConfig[CONFIG_USER][PROFILE_DISTURBANCE_RESILIENCE] = False

    class ScriptedUART:
        def __init__(self, _port):
            pass

        def readline(self, timeout=None):
            line = lines.get(timeout=5)
            if line is None:
                raise StopReadLoop
            return line.encode()

    monkeypatch.setattr(Machine, "ReadLine", ScriptedUART)

    def read():
        try:
            asyncio.run(Machine._read_data())
        except StopReadLoop:
            pass

    reader = threading.Thread(target=read, name="MachineSerial", daemon=True)
    reader.start()

    def feed(*script):
        for line in script:
            lines.put(line)

    def stop():
        lines.put(None)
        reader.join(timeout=5)
        assert not reader.is_alive()

    esp_harness = SimpleNamespace(port=port, feed=feed, stop=stop, saves=saves)
    yield esp_harness
    if reader.is_alive():
        stop()


def write_answered_by(esp, *reply):
    """Run writeDisturbanceResilience(True) and answer it once the request hit the port."""

    async def scenario():
        write = asyncio.ensure_future(Machine.writeDisturbanceResilience(True))
        await asyncio.get_running_loop().run_in_executor(None, esp.port.written.wait, 5)
        esp.feed(*reply)
        return await write

    try:
        return asyncio.run(scenario())
    finally:
        esp.stop()


def test_success_reply_resolves_the_write_and_updates_the_cached_value(esp):
    write_answered_by(esp, "nvs_response,disturbance_resilience_key,SUCCESS\n")

    assert esp.port.writes == [WRITE_TRUE]
    assert Machine.esp_info.disturbanceResilience is True
    assert Machine._pending_disturbance_resilience_writes == []


def test_error_reply_fails_the_write_with_its_status(esp):
    with pytest.raises(DisturbanceResilienceWriteError) as error:
        write_answered_by(esp, "nvs_response,disturbance_resilience_key,ERROR\n")

    assert str(error.value) == "failed to save disturbance_resilience on the machine: ERROR"
    assert Machine.esp_info.disturbanceResilience is False


def test_old_firmware_key_not_found_reply_fails_the_write(esp):
    with pytest.raises(DisturbanceResilienceWriteError) as error:
        write_answered_by(esp, "nvs_response,ERROR: key not found\n")

    assert str(error.value) == (
        "failed to save disturbance_resilience on the machine: ERROR: key not found"
    )
    assert Machine._pending_disturbance_resilience_writes == []


def test_esp_reboot_fails_a_pending_write(esp):
    with pytest.raises(DisturbanceResilienceWriteError) as error:
        write_answered_by(esp, ROM_BOOT_BANNER)

    assert str(error.value) == (
        "failed to save disturbance_resilience on the machine: machine restarted"
    )
    assert Machine._pending_disturbance_resilience_writes == []


def test_espinfo_value_is_adopted_into_the_config_without_writing_to_the_esp(esp):
    esp.feed(ESP_INFO_PREFIX + ",true\n")
    esp.stop()

    assert Machine.esp_info.disturbanceResilience is True
    assert MeticulousConfig[CONFIG_USER][PROFILE_DISTURBANCE_RESILIENCE] is True
    assert esp.saves
    assert not any(b"disturbance_resilience_key" in write for write in esp.port.writes)


def test_espinfo_without_the_field_leaves_the_config_alone(esp):
    esp.feed(ESP_INFO_WITHOUT_FIELD)
    esp.stop()

    assert Machine.esp_info.disturbanceResilience is None
    assert MeticulousConfig[CONFIG_USER][PROFILE_DISTURBANCE_RESILIENCE] is False
    assert not any(b"disturbance_resilience_key" in write for write in esp.port.writes)
