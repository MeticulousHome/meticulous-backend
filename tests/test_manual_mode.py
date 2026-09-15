import os
import sys
import types

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

import profiles  # noqa: E402
from api.profiles import ListHandler  # noqa: E402
from profiles import (  # noqa: E402
    MANUAL_MODE_AUTHOR,
    MANUAL_MODE_AUTHOR_ID,
    MANUAL_MODE_FINAL_WEIGHT_SENTINEL,
    MANUAL_MODE_FLOW_STAGE_KEY,
    MANUAL_MODE_FLOW_STAGE_NAME,
    MANUAL_MODE_LEGACY_FINAL_WEIGHT_SENTINEL,
    MANUAL_MODE_PRESSURE_STAGE_KEY,
    MANUAL_MODE_PRESSURE_STAGE_NAME,
    MANUAL_MODE_PROFILE_ID,
    MANUAL_MODE_PROFILE_NAME,
    MANUAL_MODE_TEMPERATURE,
    ProfileManager,
)


@pytest.fixture
def profile_schema():
    import json

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

    assert profile["final_weight"] == 2000.0

    assert len(profile["stages"]) == 2
    pressure, flow = profile["stages"]
    assert (pressure["name"], pressure["key"], pressure["type"]) == (
        MANUAL_MODE_PRESSURE_STAGE_NAME,
        MANUAL_MODE_PRESSURE_STAGE_KEY,
        "pressure",
    )
    assert (flow["name"], flow["key"], flow["type"]) == (
        MANUAL_MODE_FLOW_STAGE_NAME,
        MANUAL_MODE_FLOW_STAGE_KEY,
        "flow",
    )
    for stage in profile["stages"]:
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


@pytest.fixture
def recording_save(monkeypatch):
    """Capture what ensure_manual_mode_profile() writes, without touching disk."""
    saved = []

    def fake_save_profile(data, set_last_changed=False, change_id=None, skip_validation=False):
        saved.append(data)
        return {"profile": data, "change_id": "change"}

    monkeypatch.setattr(ProfileManager, "save_profile", fake_save_profile)
    return saved


def _seeded_stages(order=("pressure", "flow")):
    by_type = {stage["type"]: stage for stage in ProfileManager.manual_mode_stages()}
    return [by_type[kind] for kind in order]


def test_ensure_manual_mode_profile_is_a_no_op_when_already_seeded(
    known_profiles, recording_save
):
    known_profiles[MANUAL_MODE_PROFILE_ID] = {
        "id": MANUAL_MODE_PROFILE_ID,
        "stages": _seeded_stages(),
    }

    assert ProfileManager.ensure_manual_mode_profile() is False
    assert recording_save == []


def test_ensure_manual_mode_profile_keeps_a_reordered_stage_pair(
    known_profiles, recording_save
):
    """Stage order records the start choice, so it is the user's setup, not damage."""
    known_profiles[MANUAL_MODE_PROFILE_ID] = {
        "id": MANUAL_MODE_PROFILE_ID,
        "stages": _seeded_stages(order=("flow", "pressure")),
    }

    assert ProfileManager.ensure_manual_mode_profile() is False
    assert recording_save == []


def test_ensure_manual_mode_profile_keeps_edited_stage_values(known_profiles, recording_save):
    edited = _seeded_stages()
    edited[0]["dynamics"]["points"] = [[0, 6.5]]
    known_profiles[MANUAL_MODE_PROFILE_ID] = {
        "id": MANUAL_MODE_PROFILE_ID,
        "stages": edited,
    }

    assert ProfileManager.ensure_manual_mode_profile() is False
    assert recording_save == []


def test_ensure_manual_mode_profile_migrates_the_v1_single_stage(
    known_profiles, recording_save, profile_schema
):
    known_profiles[MANUAL_MODE_PROFILE_ID] = {
        "id": MANUAL_MODE_PROFILE_ID,
        "name": "My manual",
        "author": "Someone",
        "author_id": "11111111-1111-1111-1111-111111111111",
        "previous_authors": [],
        "temperature": 93.5,
        "final_weight": MANUAL_MODE_LEGACY_FINAL_WEIGHT_SENTINEL,
        "variables": [],
        "display": {"accentColor": "#abcdef"},
        "manual": True,
        "stages": [
            {
                "name": "Manual",
                "key": MANUAL_MODE_PRESSURE_STAGE_KEY,
                "type": "pressure",
                "dynamics": {"points": [[0, 0]], "over": "time", "interpolation": "none"},
                "exit_triggers": [{"type": "user_interaction", "value": 1}],
                "limits": [],
            }
        ],
    }

    assert ProfileManager.ensure_manual_mode_profile() is True

    (written,) = recording_save
    jsonschema.validate(instance=written, schema=profile_schema)
    # The user's own fields survive the migration.
    assert written["name"] == "My manual"
    assert written["temperature"] == 93.5
    assert written["author"] == "Someone"
    assert written["author_id"] == "11111111-1111-1111-1111-111111111111"
    assert written["display"] == {"accentColor": "#abcdef"}
    # The v1 sentinel was reachable with a big enough carafe.
    assert written["final_weight"] == MANUAL_MODE_FINAL_WEIGHT_SENTINEL == 2000.0
    assert written["manual"] is True
    assert [stage["key"] for stage in written["stages"]] == [
        MANUAL_MODE_PRESSURE_STAGE_KEY,
        MANUAL_MODE_FLOW_STAGE_KEY,
    ]


def test_migration_keeps_a_final_weight_the_user_chose(known_profiles, recording_save):
    known_profiles[MANUAL_MODE_PROFILE_ID] = {
        "id": MANUAL_MODE_PROFILE_ID,
        "final_weight": 36.0,
        "stages": [],
    }

    assert ProfileManager.ensure_manual_mode_profile() is True
    assert recording_save[0]["final_weight"] == 36.0


def test_migration_does_not_mutate_the_known_profile(known_profiles, recording_save):
    stored = {"id": MANUAL_MODE_PROFILE_ID, "stages": []}
    known_profiles[MANUAL_MODE_PROFILE_ID] = stored

    assert ProfileManager.ensure_manual_mode_profile() is True
    assert stored["stages"] == []


@pytest.mark.parametrize(
    "stages",
    [
        [],
        None,
        "not a list",
        [{"key": MANUAL_MODE_PRESSURE_STAGE_KEY, "type": "pressure"}],
        # Right keys, wrong types.
        [
            {"key": MANUAL_MODE_PRESSURE_STAGE_KEY, "type": "flow"},
            {"key": MANUAL_MODE_FLOW_STAGE_KEY, "type": "pressure"},
        ],
        # Two pressure stages.
        [
            {"key": MANUAL_MODE_PRESSURE_STAGE_KEY, "type": "pressure"},
            {"key": MANUAL_MODE_PRESSURE_STAGE_KEY, "type": "pressure"},
        ],
        # A third stage a client added.
        [
            {"key": MANUAL_MODE_PRESSURE_STAGE_KEY, "type": "pressure"},
            {"key": MANUAL_MODE_FLOW_STAGE_KEY, "type": "flow"},
            {"key": "extra", "type": "flow"},
        ],
    ],
)
def test_ensure_manual_mode_profile_migrates_a_broken_stage_list(
    known_profiles, recording_save, stages
):
    known_profiles[MANUAL_MODE_PROFILE_ID] = {
        "id": MANUAL_MODE_PROFILE_ID,
        "stages": stages,
    }

    assert ProfileManager.ensure_manual_mode_profile() is True
    assert recording_save[0]["stages"] == ProfileManager.manual_mode_stages()


def test_manual_profile_has_seeded_stages_accepts_either_order():
    assert ProfileManager.manual_profile_has_seeded_stages({"stages": _seeded_stages()})
    assert ProfileManager.manual_profile_has_seeded_stages(
        {"stages": _seeded_stages(order=("flow", "pressure"))}
    )


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


# --- GET /api/v1/profile/list ------------------------------------------------


class TestListHandlerHidesManualProfiles(AsyncHTTPTestCase):
    """Contract section 2: the list omits manual profiles in both modes."""

    def setUp(self):
        self.profiles = [
            {"id": "regular", "name": "Regular", "manual": False, "stages": [{"name": "a"}]},
            {"id": "manual", "name": "Manual mode", "manual": True, "stages": [{"name": "b"}]},
            {"id": "plain", "name": "Plain", "stages": [{"name": "c"}]},
        ]
        self.previous = ProfileManager.list_profiles
        ProfileManager.list_profiles = lambda: self.profiles
        super().setUp()

    def tearDown(self):
        super().tearDown()
        ProfileManager.list_profiles = self.previous

    def get_app(self):
        return Application([(r"/api/v1/profile/list", ListHandler)])

    def listed(self, query=""):
        response = self.fetch("/api/v1/profile/list" + query)
        assert response.code == 200
        return json.loads(response.body)

    def test_the_summary_list_omits_the_manual_profile(self):
        listed = self.listed()

        assert [p["id"] for p in listed] == ["regular", "plain"]
        assert all("stages" not in p for p in listed)

    def test_the_full_list_omits_the_manual_profile(self):
        listed = self.listed("?full=true")

        assert [p["id"] for p in listed] == ["regular", "plain"]
        assert all("stages" in p for p in listed)

    def test_a_non_boolean_manual_flag_does_not_hide_a_profile(self):
        """`manual` is a boolean; anything else is not a manual profile."""
        self.profiles = [{"id": "odd", "manual": "true", "stages": []}]

        assert [p["id"] for p in self.listed()] == ["odd"]

    def test_the_list_does_not_mutate_the_stored_profiles(self):
        self.listed()

        assert all("stages" in p for p in self.profiles)
