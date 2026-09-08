import os
import sys
import types
import uuid

import jsonschema
import pytest
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

os.environ.setdefault("MOTOR_ENERGY_PATH", "/tmp/meticulous-test/motor-energy")

try:
    import pyprctl  # noqa: F401
except Exception:
    sys.modules["pyprctl"] = types.SimpleNamespace(set_name=lambda _name: None)

try:
    import gpiod  # noqa: F401
except Exception:

    class _LineRequest:
        DIRECTION_OUTPUT = 1

    sys.modules["gpiod"] = types.SimpleNamespace(line_request=_LineRequest)

# machine.py imports monitoring.motor_power_monitoring, whose module-level
# EnergyCalculator starts a non-daemon thread that loops forever. Importing it
# from a test run leaves the pytest process unable to exit. Nothing here
# exercises motor energy, so stub the module before the import chain reaches it.
sys.modules.setdefault(
    "monitoring.motor_power_monitoring",
    types.SimpleNamespace(
        motor_energy_calculator=types.SimpleNamespace(
            calculate_motor_energy=lambda *args, **kwargs: 0.0,
            total_energy=0.0,
        ),
        MAX_ENERGY_ALLOWED=25000,
    ),
)

import json  # noqa: E402
import manual_mode  # noqa: E402
import profiles  # noqa: E402
from api.profiles import CreateProfileFromManualHandler  # noqa: E402
from profiles import (  # noqa: E402
    MANUAL_MODE_AUTHOR,
    MANUAL_MODE_AUTHOR_ID,
    MANUAL_MODE_FINAL_WEIGHT_SENTINEL,
    MANUAL_MODE_PROFILE_ID,
    MANUAL_MODE_PROFILE_NAME,
    MANUAL_MODE_STAGE_KEY,
    MANUAL_MODE_STAGE_NAME,
    MANUAL_MODE_TEMPERATURE,
    ProfileManager,
)


@pytest.fixture
def profile_schema():
    schema_path = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "profile_schema",
        "schema.json",
    )
    with open(schema_path, "r") as schema_file:
        return json.load(schema_file)


@pytest.fixture
def known_profiles(monkeypatch):
    """Isolate ProfileManager._known_profiles from other tests."""
    profiles_by_id = {}
    monkeypatch.setattr(ProfileManager, "_known_profiles", profiles_by_id)
    return profiles_by_id


def test_build_manual_mode_profile_matches_the_contract(profile_schema):
    profile = ProfileManager.build_manual_mode_profile()

    jsonschema.validate(instance=profile, schema=profile_schema)

    assert profile["id"] == MANUAL_MODE_PROFILE_ID
    assert profile["name"] == MANUAL_MODE_PROFILE_NAME
    assert profile["author"] == MANUAL_MODE_AUTHOR
    assert profile["author_id"] == MANUAL_MODE_AUTHOR_ID
    assert profile["previous_authors"] == []
    assert profile["temperature"] == MANUAL_MODE_TEMPERATURE
    assert profile["final_weight"] == MANUAL_MODE_FINAL_WEIGHT_SENTINEL
    assert profile["variables"] == []
    assert profile["display"] == {}
    assert profile["manual"] is True

    assert len(profile["stages"]) == 1
    stage = profile["stages"][0]
    assert stage["name"] == MANUAL_MODE_STAGE_NAME
    assert stage["key"] == MANUAL_MODE_STAGE_KEY
    assert stage["type"] == "pressure"
    assert stage["dynamics"] == {
        "points": [[0, 0]],
        "over": "time",
        "interpolation": "none",
    }
    assert stage["exit_triggers"] == [{"type": "user_interaction", "value": 1}]
    assert stage["limits"] == []


def test_build_manual_mode_profile_returns_a_fresh_copy():
    first = ProfileManager.build_manual_mode_profile()
    first["stages"][0]["dynamics"]["points"][0][1] = 9.0
    first["name"] = "renamed"

    second = ProfileManager.build_manual_mode_profile()

    assert second["name"] == MANUAL_MODE_PROFILE_NAME
    assert second["stages"][0]["dynamics"]["points"] == [[0, 0]]


def test_ensure_manual_mode_profile_seeds_when_absent(monkeypatch, known_profiles):
    saved = []

    def fake_save_profile(data, set_last_changed=False, change_id=None, skip_validation=False):
        saved.append((data, set_last_changed))
        return {"profile": data, "change_id": "change"}

    monkeypatch.setattr(ProfileManager, "save_profile", fake_save_profile)

    assert ProfileManager.ensure_manual_mode_profile() is True
    assert len(saved) == 1
    data, set_last_changed = saved[0]
    assert data["id"] == MANUAL_MODE_PROFILE_ID
    assert data["manual"] is True
    assert set_last_changed is True


def test_ensure_manual_mode_profile_is_a_no_op_when_present(monkeypatch, known_profiles):
    known_profiles[MANUAL_MODE_PROFILE_ID] = {"id": MANUAL_MODE_PROFILE_ID}
    calls = []

    monkeypatch.setattr(
        ProfileManager,
        "save_profile",
        lambda *args, **kwargs: calls.append(args) or {},
    )

    assert ProfileManager.ensure_manual_mode_profile() is False
    assert calls == []


def test_ensure_manual_mode_profile_survives_a_failing_save(monkeypatch, known_profiles):
    def failing_save(*args, **kwargs):
        raise OSError("read-only filesystem")

    monkeypatch.setattr(ProfileManager, "save_profile", failing_save)

    assert ProfileManager.ensure_manual_mode_profile() is False


def test_init_seeds_the_manual_mode_profile_after_refreshing():
    import inspect

    source = inspect.getsource(profiles.ProfileManager.init)
    refresh_index = source.index("refresh_profile_list()")
    seed_index = source.index("ensure_manual_mode_profile()")
    delete_index = source.index("_delete_unused_images()")

    assert refresh_index < seed_index < delete_index


# --- profile construction from a recorded manual brew -----------------------

SHOT_TIME = 1757193600.0


def _sample(profile_time, setpoints, weight=0.0, use_profile_time=True):
    key = "profile_time" if use_profile_time else "time"
    sample = {
        "shot": {
            "pressure": 6.0,
            "flow": 1.2,
            "weight": weight,
            "gravimetric_flow": 1.1,
            "setpoints": setpoints,
        },
        "time": profile_time,
        "status": "Manual",
    }
    if use_profile_time:
        sample[key] = profile_time
    return sample


def _pressure_sample(profile_time, pressure, weight=0.0, use_profile_time=True):
    return _sample(
        profile_time,
        {"active": "pressure", "pressure": pressure},
        weight=weight,
        use_profile_time=use_profile_time,
    )


def _manual_shot(data, shot_time=SHOT_TIME, profile=None):
    if profile is None:
        profile = {
            "id": MANUAL_MODE_PROFILE_ID,
            "db_key": 7,
            "author": "Barista",
            "author_id": "11111111-2222-4333-8444-555555555555",
            "display": {},
            "final_weight": 1000.0,
            "last_changed": SHOT_TIME,
            "name": MANUAL_MODE_PROFILE_NAME,
            "temperature": 93.5,
            "stages": [],
            "variables": [],
            "previous_authors": [],
        }
    return {
        "id": "8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6",
        "db_key": 12,
        "time": shot_time,
        "file": "2026-09-07/10:00:00.shot.json.zst",
        "debug_file": None,
        "name": MANUAL_MODE_PROFILE_NAME,
        "data": data,
        "profile": profile,
    }


def _mixed_shot():
    return _manual_shot(
        [
            # heating: the ESP reports the temperature controller, not pressure
            _sample(0, {"active": "temperature", "temperature": 93.5}),
            _pressure_sample(1000, 6.0, weight=0.0),
            _pressure_sample(2000, 8.456, weight=12.0),
            # a repeated timestamp must not add a second point
            _pressure_sample(2000, 9.0, weight=13.0),
            # retracting: the ESP reports no controller at all
            _sample(3000, {"active": None}),
            # a malformed sample with no pressure key at all
            _sample(3100, {"active": "pressure"}),
            _pressure_sample(3500, 4.0, weight=36.24),
        ]
    )


def test_build_profile_from_manual_shot_replays_the_pressure_targets(profile_schema):
    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(_mixed_shot(), None)

    jsonschema.validate(instance=profile, schema=profile_schema)

    assert len(profile["stages"]) == 1
    stage = profile["stages"][0]
    assert stage["name"] == "Pressure"
    assert stage["type"] == "pressure"
    assert stage["limits"] == []
    assert stage["dynamics"]["over"] == "time"
    assert stage["dynamics"]["interpolation"] == "none"

    # only the pressure samples survive, y is rounded to 2 decimals
    assert stage["dynamics"]["points"] == [[0.0, 6.0], [1.0, 8.46], [2.5, 4.0]]

    x_values = [point[0] for point in stage["dynamics"]["points"]]
    assert x_values[0] == 0.0
    assert x_values == sorted(x_values)
    assert len(set(x_values)) == len(x_values)

    assert stage["exit_triggers"] == [
        {"type": "time", "value": 2.5, "relative": True, "comparison": ">="}
    ]

    assert profile["final_weight"] == 36.2
    assert profile["temperature"] == 93.5
    assert profile["author"] == "Barista"
    assert profile["author_id"] == "11111111-2222-4333-8444-555555555555"
    assert profile["previous_authors"] == []
    assert profile["variables"] == []
    assert profile["display"] == {}
    assert "manual" not in profile
    assert uuid.UUID(profile["id"])
    assert uuid.UUID(stage["key"])


def test_build_profile_from_manual_shot_falls_back_to_the_sample_time():
    from manual_mode import build_profile_from_manual_shot

    shot = _manual_shot(
        [
            _pressure_sample(4000, 5.0, use_profile_time=False),
            _pressure_sample(6250, 7.0, use_profile_time=False),
        ]
    )

    points = build_profile_from_manual_shot(shot, None)["stages"][0]["dynamics"]["points"]

    assert points == [[0.0, 5.0], [2.25, 7.0]]


def test_build_profile_from_manual_shot_uses_the_last_target_bearing_weight():
    from manual_mode import build_profile_from_manual_shot

    # The final sample carries a usable target but a repeated timestamp, so it
    # adds no point while still being the newest weight reading.
    shot = _manual_shot(
        [
            _pressure_sample(1000, 6.0, weight=5.0),
            _pressure_sample(2000, 7.0, weight=30.0),
            _pressure_sample(2000, 7.0, weight=31.5),
        ]
    )

    profile = build_profile_from_manual_shot(shot, None)

    assert profile["stages"][0]["dynamics"]["points"] == [[0.0, 6.0], [1.0, 7.0]]
    assert profile["final_weight"] == 31.5


@pytest.mark.parametrize("weight", [float("nan"), 0.4, None, "36.2"])
def test_build_profile_from_manual_shot_uses_the_weight_sentinel(weight):
    from manual_mode import build_profile_from_manual_shot

    shot = _manual_shot([_pressure_sample(1000, 6.0, weight=weight)])

    profile = build_profile_from_manual_shot(shot, None)

    assert profile["final_weight"] == MANUAL_MODE_FINAL_WEIGHT_SENTINEL


@pytest.mark.parametrize("temperature", [None, float("inf"), "93.5"])
def test_build_profile_from_manual_shot_falls_back_to_the_default_temperature(temperature):
    from manual_mode import build_profile_from_manual_shot

    shot = _manual_shot([_pressure_sample(1000, 6.0)])
    shot["profile"]["temperature"] = temperature

    profile = build_profile_from_manual_shot(shot, None)

    assert profile["temperature"] == MANUAL_MODE_TEMPERATURE


def test_build_profile_from_manual_shot_defaults_the_name_to_the_shot_time():
    from datetime import datetime

    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(_mixed_shot(), None)

    expected = datetime.fromtimestamp(SHOT_TIME).strftime("Manual %Y-%m-%d %H:%M")
    assert profile["name"] == expected


@pytest.mark.parametrize("name", ["", "   ", None])
def test_build_profile_from_manual_shot_ignores_a_blank_name(name):
    from datetime import datetime

    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(_mixed_shot(), name)

    assert profile["name"] == datetime.fromtimestamp(SHOT_TIME).strftime(
        "Manual %Y-%m-%d %H:%M"
    )


def test_build_profile_from_manual_shot_trims_and_caps_the_name():
    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(_mixed_shot(), "   Morning manual   ")
    assert profile["name"] == "Morning manual"

    capped = build_profile_from_manual_shot(_mixed_shot(), "m" * 100)
    assert capped["name"] == "m" * 64


def test_build_profile_from_manual_shot_uses_the_minimum_stage_duration():
    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(_manual_shot([_pressure_sample(1000, 6.0)]), None)

    assert profile["stages"][0]["dynamics"]["points"] == [[0.0, 6.0]]
    assert profile["stages"][0]["exit_triggers"][0]["value"] == 0.1


def test_build_profile_from_manual_shot_falls_back_to_the_zero_author():
    from manual_mode import build_profile_from_manual_shot

    shot = _manual_shot([_pressure_sample(1000, 6.0)])
    shot["profile"]["author"] = None
    shot["profile"]["author_id"] = None

    profile = build_profile_from_manual_shot(shot, None)

    assert profile["author"] == ""
    assert profile["author_id"] == MANUAL_MODE_AUTHOR_ID


@pytest.mark.parametrize(
    "data",
    [
        [],
        None,
        [_sample(0, {"active": "temperature", "temperature": 93.5})],
        [_sample(0, {"active": None})],
        [_sample(0, {"active": "pressure"})],
        [_pressure_sample(0, float("nan"))],
        [_pressure_sample(0, float("inf"))],
        ["not a sample", {"shot": "not a dict"}, {"shot": {"setpoints": None}}],
    ],
)
def test_build_profile_from_manual_shot_without_targets(data):
    from manual_mode import ManualShotHasNoTargets, build_profile_from_manual_shot

    with pytest.raises(ManualShotHasNoTargets):
        build_profile_from_manual_shot(_manual_shot(data), None)


# --- locating the manual shot ----------------------------------------------


def test_find_manual_shot_queries_the_latest_manual_profile_shot(monkeypatch):
    import manual_mode
    from shot_database import SearchOrder, ShotDataBase

    captured = {}
    expected = _manual_shot([_pressure_sample(1000, 6.0)])

    def fake_search(params):
        captured["params"] = params
        return [expected]

    monkeypatch.setattr(ShotDataBase, "search_history", fake_search)

    assert manual_mode.find_manual_shot(None) is expected

    params = captured["params"]
    assert params.ids == [MANUAL_MODE_PROFILE_ID]
    assert params.sort == SearchOrder.descending
    assert params.max_results == 1
    assert params.dump_data is True


def test_find_manual_shot_queries_the_requested_shot(monkeypatch):
    import manual_mode
    from shot_database import ShotDataBase

    captured = {}
    expected = _manual_shot([_pressure_sample(1000, 6.0)])

    def fake_search(params):
        captured["params"] = params
        return [expected]

    monkeypatch.setattr(ShotDataBase, "search_history", fake_search)

    assert manual_mode.find_manual_shot("8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6") is expected
    assert captured["params"].ids == ["8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6"]


def test_find_manual_shot_without_history(monkeypatch):
    import manual_mode
    from shot_database import ShotDataBase

    monkeypatch.setattr(ShotDataBase, "search_history", lambda params: [])

    with pytest.raises(manual_mode.ManualShotNotFound):
        manual_mode.find_manual_shot(None)


def test_find_manual_shot_rejects_a_shot_from_another_profile(monkeypatch):
    import manual_mode
    from shot_database import ShotDataBase

    other = _manual_shot([_pressure_sample(1000, 6.0)])
    other["profile"]["id"] = "99999999-8888-4777-8666-555555555555"
    monkeypatch.setattr(ShotDataBase, "search_history", lambda params: [other])

    with pytest.raises(manual_mode.ManualShotNotFound):
        manual_mode.find_manual_shot("8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6")

    # the same entry is accepted when the caller did not name a shot, because
    # the query itself filtered on the Manual mode profile id
    assert manual_mode.find_manual_shot(None) is other


# --- POST /api/v1/profile/from_manual ---------------------------------------


class TestCreateProfileFromManualHandler(AsyncHTTPTestCase):
    def setUp(self):
        self.saved = []
        self.found = []
        self.shot = _manual_shot([_pressure_sample(1000, 6.0)])
        self.built = {"id": "built-profile", "name": "Manual 2026-09-07 00:00"}

        def fake_find(shot_id):
            self.found.append(shot_id)
            return self.shot

        def fake_build(shot, name):
            self.built["name"] = name or self.built["name"]
            return self.built

        def fake_save(data, set_last_changed=False, change_id=None, skip_validation=False):
            self.saved.append((data, change_id))
            return {"profile": data, "change_id": change_id or "generated"}

        self.find_manual_shot = fake_find
        self.build_profile_from_manual_shot = fake_build
        self.save_profile = fake_save

        self.previous = (
            manual_mode.find_manual_shot,
            manual_mode.build_profile_from_manual_shot,
            ProfileManager.save_profile,
        )
        manual_mode.find_manual_shot = lambda shot_id: self.find_manual_shot(shot_id)
        manual_mode.build_profile_from_manual_shot = (
            lambda shot, name: self.build_profile_from_manual_shot(shot, name)
        )
        ProfileManager.save_profile = lambda data, **kwargs: self.save_profile(data, **kwargs)
        super().setUp()

    def tearDown(self):
        super().tearDown()
        (
            manual_mode.find_manual_shot,
            manual_mode.build_profile_from_manual_shot,
            ProfileManager.save_profile,
        ) = self.previous

    def get_app(self):
        return Application([(r"/api/v1/profile/from_manual", CreateProfileFromManualHandler)])

    def post(self, body="", headers=None):
        return self.fetch(
            "/api/v1/profile/from_manual",
            method="POST",
            body=body,
            headers=headers,
        )

    def test_an_empty_body_saves_the_latest_manual_brew(self):
        response = self.post()

        assert response.code == 200
        assert json.loads(response.body) == {
            "profile": self.built,
            "change_id": "generated",
        }
        assert self.found == [None]
        assert self.saved == [(self.built, None)]

    def test_the_change_id_header_is_forwarded_to_save_profile(self):
        response = self.post(headers={"X-Change-Id": "change-42"})

        assert response.code == 200
        assert json.loads(response.body)["change_id"] == "change-42"
        assert self.saved == [(self.built, "change-42")]

    def test_the_requested_shot_id_is_forwarded(self):
        response = self.post(body=json.dumps({"shot_id": "shot-7", "name": "Morning"}))

        assert response.code == 200
        assert self.found == ["shot-7"]
        assert self.built["name"] == "Morning"

    def test_a_non_object_body_is_rejected(self):
        response = self.post(body=json.dumps(["not", "an", "object"]))

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "body must be a JSON object",
        }
        assert self.found == []

    def test_a_non_string_name_is_rejected(self):
        response = self.post(body=json.dumps({"name": 7}))

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "name must be a string",
        }
        assert self.found == []

    def test_a_non_string_shot_id_is_rejected(self):
        response = self.post(body=json.dumps({"shot_id": 7}))

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "shot_id must be a string",
        }
        assert self.found == []

    def test_a_missing_manual_brew_is_reported_as_not_found(self):
        def missing(shot_id):
            raise manual_mode.ManualShotNotFound("no manual brew in history")

        self.find_manual_shot = missing

        response = self.post()

        assert response.code == 404
        assert json.loads(response.body) == {
            "status": "error",
            "error": "no manual brew found",
        }

    def test_a_brew_without_targets_is_reported_as_a_conflict(self):
        def without_targets(shot, name):
            raise manual_mode.ManualShotHasNoTargets("no pressure targets")

        self.build_profile_from_manual_shot = without_targets

        response = self.post()

        assert response.code == 409
        assert json.loads(response.body) == {
            "status": "error",
            "error": "manual brew has no pressure targets",
        }

    def test_a_schema_violation_is_reported_like_profile_save(self):
        def invalid(data, **kwargs):
            raise jsonschema.exceptions.ValidationError("'stages' is a required property")

        self.save_profile = invalid

        response = self.post()

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "JSON validation error: 'stages' is a required property",
        }

    def test_an_unexpected_failure_is_reported_with_its_cause(self):
        def broken(data, **kwargs):
            raise OSError("read-only filesystem")

        self.save_profile = broken

        response = self.post()

        assert response.code == 400
        body = json.loads(response.body)
        assert body["status"] == "error"
        assert body["error"] == "failed to create profile from manual brew"
        assert body["cause"] == "read-only filesystem"

    def test_a_malformed_body_is_rejected(self):
        response = self.post(body="{not json")

        assert response.code == 400
        assert json.loads(response.body)["status"] == "error"
        assert self.found == []
