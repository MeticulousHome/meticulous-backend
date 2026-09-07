import sys
import types

try:
    import pyprctl  # noqa: F401
except Exception:
    sys.modules["pyprctl"] = types.SimpleNamespace(set_name=lambda _name: None)

try:
    import gpiod  # noqa: F401
except Exception:
    line_request = type("line_request", (), {"DIRECTION_OUTPUT": 1})
    sys.modules["gpiod"] = types.SimpleNamespace(
        line_request=line_request,
        chip=lambda _number: None,
    )

from esp_serial.data import ESPInfo
from machine import Machine


class ConnectedSerial:
    port = object()


def configure_machine(monkeypatch, tare_behavior):
    writes = []
    monkeypatch.setattr(Machine, "esp_info", ESPInfo(tareBehavior=tare_behavior))
    monkeypatch.setattr(Machine, "_connection", ConnectedSerial())
    monkeypatch.setattr(Machine, "_stopESPcomm", False)
    monkeypatch.setattr(Machine, "_pending_tare_behavior_writes", [])
    monkeypatch.setattr(Machine, "write", lambda payload: writes.append(payload))
    return writes


def test_tare_behavior_sync_is_deferred_for_older_firmware(monkeypatch):
    writes = configure_machine(monkeypatch, None)

    Machine.setTareBehavior("before_retraction")

    assert writes == []
    assert Machine.esp_info.tareBehavior is None


def test_tare_behavior_sync_skips_matching_value(monkeypatch):
    writes = configure_machine(monkeypatch, "before_retraction")

    Machine.setTareBehavior("before_retraction")

    assert writes == []


def test_tare_behavior_sync_writes_supported_firmware(monkeypatch):
    writes = configure_machine(monkeypatch, "after_retraction")

    Machine.setTareBehavior("before_retraction")

    assert writes == [b"nvs_request,write,tare_behavior_key,before_retraction\x03"]
    assert Machine.esp_info.tareBehavior == "after_retraction"
    assert Machine._pending_tare_behavior_writes == ["before_retraction"]


def test_tare_behavior_sync_updates_cached_value_only_after_success(monkeypatch):
    configure_machine(monkeypatch, "after_retraction")

    Machine.setTareBehavior("before_retraction")
    Machine.handleTareBehaviorNVSResponse("SUCCESS")

    assert Machine.esp_info.tareBehavior == "before_retraction"
    assert Machine._pending_tare_behavior_writes == []


def test_tare_behavior_sync_keeps_cached_value_after_error(monkeypatch):
    configure_machine(monkeypatch, "after_retraction")

    Machine.setTareBehavior("before_retraction")
    Machine.handleTareBehaviorNVSResponse("ERROR")

    assert Machine.esp_info.tareBehavior == "after_retraction"
    assert Machine._pending_tare_behavior_writes == []


def test_tare_behavior_sync_preserves_rapid_toggle_order(monkeypatch):
    writes = configure_machine(monkeypatch, "after_retraction")

    Machine.setTareBehavior("before_retraction")
    Machine.setTareBehavior("after_retraction")

    assert writes == [
        b"nvs_request,write,tare_behavior_key,before_retraction\x03",
        b"nvs_request,write,tare_behavior_key,after_retraction\x03",
    ]

    Machine.handleTareBehaviorNVSResponse("SUCCESS")
    assert Machine.esp_info.tareBehavior == "before_retraction"
    assert Machine._pending_tare_behavior_writes == ["after_retraction"]

    Machine.handleTareBehaviorNVSResponse("SUCCESS")
    assert Machine.esp_info.tareBehavior == "after_retraction"
    assert Machine._pending_tare_behavior_writes == []
