import asyncio
import copy
import json
import sys
import types
from datetime import datetime
from pathlib import Path

import pytest
import zstandard as zstd

import shot_data_sharing
from config import (
    CONFIG_SYSTEM,
    CONFIG_USER,
    DefaultConfiguration_V1,
    MeticulousConfig,
    SHOT_DATA_SHARING,
    SHOT_DATA_SHARING_DEFAULT,
    SHOT_DATA_SHARING_ID,
    SHOT_DATA_SHARING_ID_DEFAULT,
)
from shot_data_sharing import ShotDataSharing, ShotDataUploadError

SHOT_START = datetime(2026, 9, 29, 8, 12, 33)
SHOT_FILE = Path("/meticulous-user/history/debug/2026-09-29/08:12:33.shot.json.zst")


def _debug_shot() -> dict:
    return {
        "time": SHOT_START.timestamp(),
        "type": "shot",
        "profile_name": "Espresso",
        "machine": {
            "name": "MeticulousRoastedRobusta",
            "hostname": "meticulousRoastedRobusta-123",
            "serial_number": "123",
            "batch_number": "B1",
            "build_date": "2025-01-01",
            "color": "black",
            "software_version": "2026-09-01 00:00:00",
            "image_build_channel": "beta",
            "firmware_version": "1.2.3",
        },
        "profile": {"name": "Espresso"},
        "nodeJSON": {},
        "config": {
            "hostname_override": "my-machine",
            "machine_name": ["Roasted", "Robusta"],
            "wifi": {},
            "heat_on_boot": True,
        },
        "data": [{"shot": {"pressure": 9.0}, "sensors": {"motor_temp": 40.0}}],
        "logs": [],
    }


@pytest.fixture
def config(monkeypatch):
    fresh = copy.deepcopy(DefaultConfiguration_V1)
    monkeypatch.setitem(MeticulousConfig, CONFIG_USER, fresh[CONFIG_USER])
    monkeypatch.setitem(MeticulousConfig, CONFIG_SYSTEM, fresh[CONFIG_SYSTEM])
    monkeypatch.setattr(MeticulousConfig, "save", lambda: None)
    return MeticulousConfig


@pytest.fixture
def configured(monkeypatch):
    monkeypatch.setattr(shot_data_sharing, "SUPABASE_URL", "https://project.supabase.co")
    monkeypatch.setattr(shot_data_sharing, "SUPABASE_BUCKET", "brew shots")
    monkeypatch.setattr(shot_data_sharing, "SUPABASE_ANON_KEY", "sb_publishable_test")


class FakeResponse:
    def __init__(self, status, text=""):
        self.status = status
        self._text = text

    async def __aenter__(self):
        return self

    async def __aexit__(self, *exc_info):
        return None

    async def text(self):
        return self._text


def _fake_session(captured, status=200, text=""):
    class FakeSession:
        def __init__(self, timeout=None):
            captured["timeout"] = timeout

        async def __aenter__(self):
            return self

        async def __aexit__(self, *exc_info):
            return None

        def post(self, url, data=None, headers=None):
            captured["url"] = url
            captured["data"] = data
            captured["headers"] = headers
            return FakeResponse(status, text)

    return FakeSession


def test_default_config_has_sharing_unanswered_and_no_sharing_id():
    # The nested default dicts are shared with the loaded config, so check the
    # constants rather than the (possibly merged) DefaultConfiguration_V1.
    assert SHOT_DATA_SHARING_DEFAULT is None
    assert SHOT_DATA_SHARING_ID_DEFAULT is None
    assert SHOT_DATA_SHARING in DefaultConfiguration_V1[CONFIG_USER]
    assert SHOT_DATA_SHARING_ID in DefaultConfiguration_V1[CONFIG_SYSTEM]


def test_anonymize_keeps_only_allowlisted_fields():
    original = _debug_shot()
    snapshot = copy.deepcopy(original)

    anonymized = ShotDataSharing.anonymize(original)

    assert original == snapshot
    assert set(anonymized) == {
        "time",
        "type",
        "profile_name",
        "machine",
        "profile",
        "nodeJSON",
        "config",
        "data",
        "logs",
    }
    for key in ("name", "hostname", "serial_number", "batch_number", "build_date", "color"):
        assert key not in anonymized["machine"]
    assert anonymized["machine"] == {
        "software_version": "2026-09-01 00:00:00",
        "image_build_channel": "beta",
        "firmware_version": "1.2.3",
    }
    assert anonymized["config"] == {"heat_on_boot": True}
    assert anonymized["profile"] == {"name": "Espresso"}
    assert anonymized["data"] == original["data"]
    assert anonymized["logs"] == []


def test_anonymize_drops_unknown_fields_at_every_level():
    debug_shot = _debug_shot()
    debug_shot["new_top_level"] = "x"
    debug_shot["machine"]["new_machine_field"] = "x"
    debug_shot["config"]["new_setting"] = "x"
    debug_shot["profile"].update({"author": "Someone", "author_id": "u1", "id": "p1"})
    debug_shot["data"][0]["new_sample_field"] = "x"
    debug_shot["data"][0]["shot"]["new_shot_field"] = "x"
    debug_shot["data"][0]["shot"]["setpoints"] = {"active": "pressure", "pressure": 9.0, "x": 1}
    debug_shot["data"][0]["sensors"]["new_sensor"] = 1.0
    debug_shot["logs"] = [
        {"profile_ms": 1, "loglevel": "INFO", "caller": "m", "log_message": "hi", "extra": 1}
    ]

    anonymized = ShotDataSharing.anonymize(debug_shot)

    assert "new_top_level" not in anonymized
    assert "new_machine_field" not in anonymized["machine"]
    assert "new_setting" not in anonymized["config"]
    assert anonymized["profile"] == {"name": "Espresso"}
    sample = anonymized["data"][0]
    assert set(sample) == {"shot", "sensors"}
    assert sample["shot"] == {
        "pressure": 9.0,
        "setpoints": {"active": "pressure", "pressure": 9.0},
    }
    assert sample["sensors"] == {"motor_temp": 40.0}
    assert anonymized["logs"] == [
        {"profile_ms": 1, "loglevel": "INFO", "caller": "m", "log_message": "hi"}
    ]


def test_anonymize_drops_values_with_an_unexpected_shape():
    debug_shot = _debug_shot()
    debug_shot["machine"] = "not a dict"
    debug_shot["data"] = {"not": "a list"}
    debug_shot["logs"] = ["not a dict", {"loglevel": "INFO"}]

    anonymized = ShotDataSharing.anonymize(debug_shot)

    assert "machine" not in anonymized
    assert "data" not in anonymized
    assert anonymized["logs"] == [{"loglevel": "INFO"}]
    assert ShotDataSharing.anonymize({}) == {}


def test_shared_schema_names_only_real_fields():
    from dataclasses import fields

    from esp_serial.data import SensorData

    sensor_fields = {field.name for field in fields(SensorData)}
    assert set(shot_data_sharing.SHARED_SENSOR_FIELDS) <= sensor_fields
    user_settings = set(DefaultConfiguration_V1[CONFIG_USER])
    assert set(shot_data_sharing.SHARED_SHOT_SCHEMA["config"]) <= user_settings


def test_object_path_groups_by_sharing_id_and_day():
    path = ShotDataSharing.build_object_path("abc", SHOT_START, SHOT_FILE)
    assert path == "abc/2026-09-29/08:12:33.shot.json.zst"


def test_is_configured_rejects_placeholders(monkeypatch):
    monkeypatch.setattr(shot_data_sharing, "SUPABASE_URL", "https://PLACEHOLDER.supabase.co")
    monkeypatch.setattr(shot_data_sharing, "SUPABASE_ANON_KEY", "anon-key")
    assert ShotDataSharing.is_configured() is False
    monkeypatch.setattr(shot_data_sharing, "SUPABASE_URL", "https://project.supabase.co")
    monkeypatch.setattr(shot_data_sharing, "SUPABASE_ANON_KEY", "PLACEHOLDER_ANON_KEY")
    assert ShotDataSharing.is_configured() is False
    monkeypatch.setattr(shot_data_sharing, "SUPABASE_ANON_KEY", "anon-key")
    assert ShotDataSharing.is_configured() is True


def test_should_upload_requires_opt_in_real_machine_and_shot_type(config, monkeypatch):
    # should_upload imports machine lazily; stub it so the test does not pull in
    # the Linux-only machine dependencies.
    class Machine:
        emulated = False

    monkeypatch.setitem(sys.modules, "machine", types.SimpleNamespace(Machine=Machine))
    assert ShotDataSharing.should_upload("shot") is False
    config[CONFIG_USER][SHOT_DATA_SHARING] = False
    assert ShotDataSharing.should_upload("shot") is False

    config[CONFIG_USER][SHOT_DATA_SHARING] = True
    assert ShotDataSharing.should_upload("shot") is True
    assert ShotDataSharing.should_upload("purge") is False
    assert ShotDataSharing.should_upload("boot") is False

    monkeypatch.setattr(Machine, "emulated", True)
    assert ShotDataSharing.should_upload("shot") is False


def test_opt_in_rotates_sharing_id_and_opt_out_forgets_it(config):
    assert config[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] is None

    ShotDataSharing.on_setting_changed(True)
    first_id = config[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID]
    assert isinstance(first_id, str) and len(first_id) == 36

    # Re-sending True while already enabled keeps the id stable.
    config[CONFIG_USER][SHOT_DATA_SHARING] = True
    ShotDataSharing.on_setting_changed(True)
    assert config[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] == first_id

    ShotDataSharing.on_setting_changed(False)
    assert config[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] is None

    config[CONFIG_USER][SHOT_DATA_SHARING] = False
    ShotDataSharing.on_setting_changed(True)
    assert config[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] != first_id


def test_get_sharing_id_generates_and_persists_when_missing(config, monkeypatch):
    saved = []
    monkeypatch.setattr(MeticulousConfig, "save", lambda: saved.append(True))

    sharing_id = ShotDataSharing.get_sharing_id()

    assert config[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] == sharing_id
    assert saved == [True]
    assert ShotDataSharing.get_sharing_id() == sharing_id
    assert saved == [True]


def test_upload_sends_anonymized_zstd_payload(config, configured, monkeypatch):
    config[CONFIG_USER][SHOT_DATA_SHARING] = True
    config[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] = "share-id"
    captured = {}
    monkeypatch.setattr(shot_data_sharing.aiohttp, "ClientSession", _fake_session(captured))
    reported = []
    monkeypatch.setattr(shot_data_sharing.sentry_sdk, "capture_exception", reported.append)

    ok = asyncio.run(
        ShotDataSharing.upload_debug_shot(json.dumps(_debug_shot()), SHOT_START, SHOT_FILE)
    )

    assert ok is True
    assert reported == []
    assert captured["url"] == (
        "https://project.supabase.co/storage/v1/object/brew%20shots/"
        "share-id/2026-09-29/08%3A12%3A33.shot.json.zst"
    )
    assert captured["headers"]["apikey"] == "sb_publishable_test"
    assert "Authorization" not in captured["headers"]
    assert captured["headers"]["Content-Type"] == "application/zstd"
    assert captured["headers"]["x-upsert"] == "false"
    assert captured["timeout"].total == shot_data_sharing.UPLOAD_TIMEOUT_SECONDS

    uploaded = json.loads(zstd.ZstdDecompressor().decompress(captured["data"]))
    assert uploaded == ShotDataSharing.anonymize(_debug_shot())
    assert "serial_number" not in uploaded["machine"]


def test_upload_failure_is_logged_and_reported_to_sentry(config, configured, monkeypatch):
    config[CONFIG_USER][SHOT_DATA_SHARING] = True
    config[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] = "share-id"
    captured = {}
    monkeypatch.setattr(
        shot_data_sharing.aiohttp,
        "ClientSession",
        _fake_session(captured, status=403, text="new row violates row-level security"),
    )
    reported = []
    monkeypatch.setattr(shot_data_sharing.sentry_sdk, "capture_exception", reported.append)
    logged = []
    monkeypatch.setattr(
        shot_data_sharing.logger, "error", lambda msg, *a, **k: logged.append(msg)
    )

    ok = asyncio.run(
        ShotDataSharing.upload_debug_shot(json.dumps(_debug_shot()), SHOT_START, SHOT_FILE)
    )

    assert ok is False
    assert len(reported) == 1
    assert isinstance(reported[0], ShotDataUploadError)
    assert "HTTP 403" in str(reported[0])
    assert len(logged) == 1
    assert "Failed to upload shared shot data" in logged[0]
    assert "share-id/2026-09-29/08:12:33.shot.json.zst" in logged[0]


def test_network_error_is_reported_and_does_not_raise(config, configured, monkeypatch):
    config[CONFIG_USER][SHOT_DATA_SHARING] = True
    config[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] = "share-id"

    class BrokenSession:
        def __init__(self, timeout=None):
            pass

        async def __aenter__(self):
            raise shot_data_sharing.aiohttp.ClientConnectionError("no route to host")

        async def __aexit__(self, *exc_info):
            return None

    monkeypatch.setattr(shot_data_sharing.aiohttp, "ClientSession", BrokenSession)
    reported = []
    monkeypatch.setattr(shot_data_sharing.sentry_sdk, "capture_exception", reported.append)

    ok = asyncio.run(
        ShotDataSharing.upload_debug_shot(json.dumps(_debug_shot()), SHOT_START, SHOT_FILE)
    )

    assert ok is False
    assert len(reported) == 1
    assert isinstance(reported[0], shot_data_sharing.aiohttp.ClientConnectionError)


def test_upload_is_skipped_when_target_is_not_configured(config, monkeypatch):
    config[CONFIG_USER][SHOT_DATA_SHARING] = True
    monkeypatch.setattr(shot_data_sharing, "SUPABASE_URL", "https://PLACEHOLDER.supabase.co")
    reported = []
    monkeypatch.setattr(shot_data_sharing.sentry_sdk, "capture_exception", reported.append)

    def fail(*args, **kwargs):
        raise AssertionError("no request expected")

    monkeypatch.setattr(shot_data_sharing.aiohttp, "ClientSession", fail)

    ok = asyncio.run(
        ShotDataSharing.upload_debug_shot(json.dumps(_debug_shot()), SHOT_START, SHOT_FILE)
    )

    assert ok is False
    assert reported == []
