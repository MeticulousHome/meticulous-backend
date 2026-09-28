import copy
import json
import sys
import types

import pytest
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

try:
    import pyprctl  # noqa: F401
except Exception:
    sys.modules["pyprctl"] = types.SimpleNamespace(set_name=lambda _name: None)

try:
    import gpiod  # noqa: F401
except Exception:
    sys.modules["gpiod"] = types.SimpleNamespace(
        line_request=types.SimpleNamespace(DIRECTION_OUTPUT=1)
    )

import limited_access
from config import (
    CONFIG_SYSTEM,
    CONFIG_USER,
    MACHINE_SERIAL_NUMBER,
    UPDATE_CHANNEL,
    MeticulousConfig,
)
from limited_access import UnlockThrottle


@pytest.fixture(autouse=True)
def restore_config():
    original = copy.deepcopy(dict(MeticulousConfig))
    UnlockThrottle.reset()
    yield
    MeticulousConfig.clear()
    MeticulousConfig.update(original)
    UnlockThrottle.reset()


def js_code(serial_int):
    return str((serial_int ^ 0xC0FFEE) % 99999)


@pytest.mark.parametrize(
    "serial, expected",
    [
        ("12345", js_code(12345)),
        (12345, js_code(12345)),
        (" 42abc", js_code(42)),
        ("ME1234", None),
        ("١٢٣", None),
        ("NOT_ASSIGNED", None),
        (None, None),
        ("", None),
        ("-42", None),
    ],
)
def test_serial_unlock_code_matches_javascript_parse_int(serial, expected):
    assert limited_access.serial_unlock_code(serial) == expected


def test_validate_unlock_code_accepts_master_and_serial_code():
    MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] = "12345"
    assert limited_access.validate_unlock_code("met")
    assert limited_access.validate_unlock_code(" MET ")
    assert limited_access.validate_unlock_code(js_code(12345))
    assert not limited_access.validate_unlock_code("")
    assert not limited_access.validate_unlock_code(None)
    assert not limited_access.validate_unlock_code("00000")


def test_limited_access_depends_only_on_update_channel():
    MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] = "factory"
    assert limited_access.is_limited_access()
    MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] = "stable"
    assert not limited_access.is_limited_access()


def test_throttle_locks_after_more_than_ten_failures():
    for _ in range(limited_access.UNLOCK_MAX_FAILURES):
        UnlockThrottle.record_failure(now=100.0)
    assert UnlockThrottle.retry_after(now=100.0) == 0
    UnlockThrottle.record_failure(now=100.0)
    assert 0 < UnlockThrottle.retry_after(now=100.0) <= limited_access.UNLOCK_LOCKOUT_S + 1
    assert UnlockThrottle.retry_after(now=131.0) == 0


class LimitedAccessApiTests(AsyncHTTPTestCase):
    def setUp(self):
        super().setUp()
        self._original = copy.deepcopy(dict(MeticulousConfig))
        self.channel_changes = []
        self.events = []
        self.saves = []
        from ota import UpdateManager
        from profiles import ProfileManager

        self._patches = [
            (UpdateManager, "setChannel", UpdateManager.setChannel),
            (ProfileManager, "_emit_profile_event", ProfileManager._emit_profile_event),
            (ProfileManager, "list_profiles", ProfileManager.list_profiles),
            (MeticulousConfig, "save", MeticulousConfig.save),
        ]
        UpdateManager.setChannel = staticmethod(self.channel_changes.append)
        ProfileManager._emit_profile_event = staticmethod(
            lambda change, *args, **kwargs: self.events.append(change.value)
        )
        ProfileManager.list_profiles = staticmethod(
            lambda: [{"id": "user-1", "name": "User", "stages": []}]
        )
        MeticulousConfig.save = lambda: self.saves.append(True)
        UnlockThrottle.reset()

    def tearDown(self):
        for target, name, original in self._patches:
            setattr(target, name, original)
        MeticulousConfig.clear()
        MeticulousConfig.update(self._original)
        UnlockThrottle.reset()
        super().tearDown()

    def get_app(self):
        from api.machine import MachineUnlockHandler
        from api.profiles import GetProfileHandler, ListDefaultsHandler, ListHandler

        return Application(
            [
                (r"/api/v1/profile/list", ListHandler),
                (r"/api/v1/profile/defaults", ListDefaultsHandler),
                (r"/api/v1/profile/get/([0-9a-fA-F-]+)", GetProfileHandler),
                (r"/api/v1/machine/unlock", MachineUnlockHandler),
            ]
        )

    def post_unlock(self, body):
        return self.fetch(
            "/api/v1/machine/unlock",
            method="POST",
            headers={"Content-Type": "application/json"},
            body=body if isinstance(body, str) else json.dumps(body),
        )

    def test_limited_list_defaults_and_get(self):
        MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] = "factory"
        profiles = json.loads(self.fetch("/api/v1/profile/list?full=true").body)
        assert len(profiles) == 1
        assert profiles[0]["id"] == "f5db85cc-ab0d-4c81-aee8-bcac7da77141"
        assert profiles[0]["stages"]
        assert "stages" not in json.loads(self.fetch("/api/v1/profile/list").body)[0]
        defaults = json.loads(self.fetch("/api/v1/profile/defaults").body)
        assert len(defaults["default"]) == 1 and defaults["community"] == []
        response = self.fetch("/api/v1/profile/get/f5db85cc-ab0d-4c81-aee8-bcac7da77141")
        assert response.code == 200

    def test_unlock_statuses_and_side_effects(self):
        MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] = "factory"
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] = "12345"
        assert self.post_unlock("{").code == 400
        malformed = self.fetch(
            "/api/v1/machine/unlock",
            method="POST",
            headers={"Content-Type": "application/json"},
            body=b"\x80",
        )
        assert malformed.code == 400
        assert json.loads(malformed.body)["data"]["code"] == "INVALID_BODY"
        assert self.post_unlock({"code": "00000"}).code == 403
        response = self.post_unlock({"code": js_code(12345)})
        assert response.code == 200
        assert json.loads(response.body) == {
            "status": "ok",
            "update_channel": "stable",
            "limited_access": False,
        }
        assert self.channel_changes == ["stable"]
        assert self.saves == [True]
        assert self.events == ["full_reload"]

    def test_unlock_throttles_after_eleven_failed_attempts(self):
        for _ in range(limited_access.UNLOCK_MAX_FAILURES + 1):
            assert self.post_unlock({"code": "00000"}).code == 403
        response = self.post_unlock({"code": "met"})
        assert response.code == 429
        assert response.headers["Retry-After"]
        assert json.loads(response.body)["data"]["code"] == "UNLOCK_THROTTLED"
