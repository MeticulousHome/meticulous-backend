import os
import sys
import types

import jsonschema
import pytest

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

import profiles  # noqa: E402
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
