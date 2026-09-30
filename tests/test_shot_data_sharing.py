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
    SHOT_DATA_SHARING_ID,
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


def test_default_config_has_sharing_disabled_and_no_sharing_id():
    assert DefaultConfiguration_V1[CONFIG_USER][SHOT_DATA_SHARING] is False
    assert DefaultConfiguration_V1[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] is None


def test_anonymize_strips_identifiers_and_keeps_sensor_data():
    original = _debug_shot()
    snapshot = copy.deepcopy(original)

    anonymized = ShotDataSharing.anonymize(original)

    assert original == snapshot
    for key in shot_data_sharing.MACHINE_IDENTIFYING_KEYS:
        assert key not in anonymized["machine"]
    for key in shot_data_sharing.CONFIG_IDENTIFYING_KEYS:
        assert key not in anonymized["config"]
    assert anonymized["machine"]["firmware_version"] == "1.2.3"
    assert anonymized["machine"]["image_build_channel"] == "beta"
    assert anonymized["config"]["heat_on_boot"] is True
    assert anonymized["data"] == original["data"]
    assert anonymized["profile"] == original["profile"]


def test_legacy_jwt_key_is_also_sent_as_bearer_token(monkeypatch):
    monkeypatch.setattr(shot_data_sharing, "SUPABASE_ANON_KEY", "eyJhbGciOiJIUzI1NiJ9.x.y")
    headers = ShotDataSharing.build_headers()
    assert headers["apikey"] == "eyJhbGciOiJIUzI1NiJ9.x.y"
    assert headers["Authorization"] == "Bearer eyJhbGciOiJIUzI1NiJ9.x.y"

    monkeypatch.setattr(shot_data_sharing, "SUPABASE_ANON_KEY", "sb_publishable_abc")
    headers = ShotDataSharing.build_headers()
    assert headers["apikey"] == "sb_publishable_abc"
    assert "Authorization" not in headers


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
