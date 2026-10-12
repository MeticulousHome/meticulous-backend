"""The backend's handshake with the ESP32: action,info answered with ESPInfo.

Until ESPInfo arrives the backend knows neither the firmware version nor the
machine's identity, and it skips the settings it syncs to the ESP when ESPInfo
comes (partial retraction, auto purge, tare behaviour). Guards three rules of
machine.py: the port is flushed before the request, not after it (the flush
could throw the reply away); nothing but terminated messages is written (stray
bytes before a message make the firmware reject it, contracts/uart/protocol.json
in EspressoFirmware); and the request is repeated every
ESP_INFO_RETRY_INTERVAL_SECONDS until ESPInfo arrives.

The lines are the ones the real firmware sends (recorded from fika-sil).
"""

import asyncio
import time as system_time
from types import SimpleNamespace

import pytest

import machine as machine_module
from esp_observability import ESPObservability
from machine import ESP_INFO_RETRY_INTERVAL_SECONDS, AlarmManager, Machine

DATA_IDLE = "Data,0.00,0.00,0.00,S,21.93,idle,idle,none,0.00,none,0.00,false,0.00,none,0.00,none,0.00,Idle\n"
ESP_INFO = "ESPInfo,0.9.10-sil,off,120,black,SIL-TM-0001,0025,2025-11-14,acaia,45.33,false,after_retraction,\n"
ROM_BOOT_BANNER = "rst:0x3 (SW_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n"
LINE_PERIOD_S = 0.5  # firmware time between two lines the script feeds


class StopReadLoop(Exception):
    pass


class RecordingPort:
    def __init__(self):
        self.calls = []

    def reset_input_buffer(self):
        self.calls.append(("reset_input_buffer",))

    def write(self, data):
        self.calls.append(("write", data))

    @property
    def written(self) -> bytes:
        return b"".join(call[1] for call in self.calls if call[0] == "write")


@pytest.fixture
def esp(monkeypatch):
    """A scripted ESP32 on the UART and a clock that advances LINE_PERIOD_S per line."""
    clock = {"now": 100.0}
    port = RecordingPort()
    monkeypatch.setattr(
        machine_module,
        "time",
        SimpleNamespace(monotonic=lambda: clock["now"], time=system_time.time),
    )
    monkeypatch.setattr(Machine, "_connection", SimpleNamespace(port=port))
    for attribute, value in {
        "esp_info": None,
        "infoReady": False,
        "profileReady": False,
        "_stopESPcomm": False,
        "_info_requested_at": None,
        "reset_count": 0,
        "startTime": None,
        "firmware_running": None,
        "firmware_available": None,
        "firmware_available_string": None,
        "esp_restart_request": False,
    }.items():
        monkeypatch.setattr(Machine, attribute, value)
    monkeypatch.setattr(Machine, "esp_observability", ESPObservability(now=clock["now"]))
    for name in (
        "requestMileage",
        "setPartialRetraction",
        "setAutoPurgeAfterShot",
        "setTareBehavior",
        "syncDeviceUUID",
        "toggle_manufacturing_mode",
        "startUpdate",
    ):
        monkeypatch.setattr(Machine, name, lambda *args, **kwargs: None)
    monkeypatch.setattr(AlarmManager, "is_alarm_set", lambda _alarm: None)
    monkeypatch.setattr("machine.MeticulousConfig.save", lambda *args, **kwargs: None)

    def run(lines):
        script = iter(line.encode() for line in lines)

        class ScriptedUART:
            def __init__(self, _port):
                pass

            def readline(self, timeout=None):
                try:
                    line = next(script)
                except StopIteration as error:
                    raise StopReadLoop from error
                clock["now"] += LINE_PERIOD_S
                return line

        monkeypatch.setattr(Machine, "ReadLine", ScriptedUART)
        with pytest.raises(StopReadLoop):
            asyncio.run(Machine._read_data())

    run.port = port
    run.clock = clock
    return run


def info_requests(port: RecordingPort) -> int:
    return port.written.count(b"action,info\x03")


def test_greeting_flushes_the_port_before_asking_for_info(esp):
    Machine._greet()

    assert esp.port.calls == [
        ("reset_input_buffer",),
        ("write", b"\x03"),
        ("write", b"action,info\x03"),
    ]
    assert Machine._info_requested_at == esp.clock["now"]


def test_the_reader_writes_nothing_before_the_firmware_speaks(esp):
    Machine._greet()
    greeting = esp.port.written

    esp([ESP_INFO])

    # Only the greeting: no flush that could drop the reply, no stray bytes.
    assert esp.port.written == greeting
    assert [call for call in esp.port.calls if call[0] == "reset_input_buffer"] == [
        ("reset_input_buffer",)
    ]
    assert Machine.esp_info.serialNumber == "SIL-TM-0001"


def test_info_is_asked_again_every_interval_until_espinfo_arrives(esp):
    Machine._greet()
    lines_per_interval = int(ESP_INFO_RETRY_INTERVAL_SECONDS / LINE_PERIOD_S)
    unanswered = 3 * lines_per_interval

    esp([DATA_IDLE] * unanswered + [ESP_INFO] + [DATA_IDLE] * (2 * lines_per_interval))

    # The greeting, then one request per interval while the firmware does not answer.
    assert info_requests(esp.port) == 1 + 3
    assert Machine.esp_info is not None
    assert Machine.esp_info.firmwareV == "0.9.10-sil"


def test_no_request_is_repeated_once_espinfo_has_arrived(esp):
    Machine._greet()

    esp([ESP_INFO] + [DATA_IDLE] * 20)

    assert info_requests(esp.port) == 1


def test_an_esp_reboot_makes_the_backend_ask_again_at_once(esp):
    Machine._greet()

    # Greeting at t=100; unanswered, it is repeated at t=102 (the 4th Data line).
    # The ESP reboots right after: once it sends Data again the backend asks at
    # once, without waiting until t=104 for the retry interval.
    esp([DATA_IDLE] * 4 + [ROM_BOOT_BANNER, DATA_IDLE, DATA_IDLE])

    assert info_requests(esp.port) == 3
    assert esp.clock["now"] - 102 < ESP_INFO_RETRY_INTERVAL_SECONDS
    assert Machine.esp_info is None
