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
from api.profiles import CreateProfileFromManualHandler, ListHandler  # noqa: E402
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


def _flow_sample(profile_time, flow, weight=0.0, use_profile_time=True):
    return _sample(
        profile_time,
        {"active": "flow", "flow": flow},
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
    assert stage["name"] == "Pressure 1"
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


# --- multi-stage construction (contract section 5) --------------------------


def _switching_shot():
    """A brew that starts in pressure, moves to flow, and comes back."""
    return _manual_shot(
        [
            _pressure_sample(0, 6.0, weight=0.0),
            _pressure_sample(1000, 7.0, weight=2.0),
            _flow_sample(2000, 2.5, weight=8.0),
            _flow_sample(3000, 2.0, weight=14.0),
            _pressure_sample(4000, 4.0, weight=20.0),
        ]
    )


def test_each_run_of_a_control_becomes_its_own_stage(profile_schema):
    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(_switching_shot(), None)

    jsonschema.validate(instance=profile, schema=profile_schema)

    assert [stage["name"] for stage in profile["stages"]] == [
        "Pressure 1",
        "Flow 2",
        "Pressure 3",
    ]
    assert [stage["type"] for stage in profile["stages"]] == ["pressure", "flow", "pressure"]
    assert len({stage["key"] for stage in profile["stages"]}) == 3
    for stage in profile["stages"]:
        assert uuid.UUID(stage["key"])
        assert stage["limits"] == []
        assert stage["dynamics"]["over"] == "time"
        assert stage["dynamics"]["interpolation"] == "none"


def test_each_stage_restarts_its_own_clock():
    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(_switching_shot(), None)

    assert profile["stages"][0]["dynamics"]["points"] == [[0.0, 6.0], [1.0, 7.0]]
    assert profile["stages"][1]["dynamics"]["points"] == [[0.0, 2.5], [1.0, 2.0]]
    assert profile["stages"][2]["dynamics"]["points"] == [[0.0, 4.0]]


def test_each_stage_exits_on_its_own_duration():
    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(_switching_shot(), None)

    assert [stage["exit_triggers"] for stage in profile["stages"]] == [
        [{"type": "time", "value": 1.0, "relative": True, "comparison": ">="}],
        [{"type": "time", "value": 1.0, "relative": True, "comparison": ">="}],
        # a one-sample run still needs a duration the ESP can leave on
        [{"type": "time", "value": 0.1, "relative": True, "comparison": ">="}],
    ]


def test_a_flow_only_brew_builds_a_flow_stage(profile_schema):
    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(
        _manual_shot([_flow_sample(0, 2.0), _flow_sample(1000, 2.4, weight=18.0)]), None
    )

    jsonschema.validate(instance=profile, schema=profile_schema)

    assert [stage["name"] for stage in profile["stages"]] == ["Flow 1"]
    assert profile["stages"][0]["type"] == "flow"
    assert profile["stages"][0]["dynamics"]["points"] == [[0.0, 2.0], [1.0, 2.4]]
    assert profile["final_weight"] == 18.0


def test_samples_without_a_target_do_not_split_a_run():
    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(
        _manual_shot(
            [
                _pressure_sample(0, 6.0),
                # retracting and a malformed sample sit inside one pressure run
                _sample(1000, {"active": None}),
                _sample(1500, {"active": "pressure"}),
                _pressure_sample(2000, 7.0),
            ]
        ),
        None,
    )

    assert [stage["name"] for stage in profile["stages"]] == ["Pressure 1"]
    assert profile["stages"][0]["dynamics"]["points"] == [[0.0, 6.0], [2.0, 7.0]]


def test_the_final_weight_comes_from_the_last_target_of_the_last_run():
    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(_switching_shot(), None)

    assert profile["final_weight"] == 20.0


def test_a_brew_that_never_reached_a_meaningful_weight_uses_the_sentinel():
    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(
        _manual_shot([_pressure_sample(0, 6.0), _flow_sample(1000, 2.0, weight=0.2)]), None
    )

    assert profile["final_weight"] == MANUAL_MODE_FINAL_WEIGHT_SENTINEL == 2000.0


def test_the_sentinel_is_the_schema_maximum(profile_schema):
    """The sentinel has to be a value the schema accepts, or nothing can save it."""
    bounds = profile_schema["properties"]["final_weight"]

    assert MANUAL_MODE_FINAL_WEIGHT_SENTINEL == bounds["maximum"]
    assert MANUAL_MODE_LEGACY_FINAL_WEIGHT_SENTINEL < MANUAL_MODE_FINAL_WEIGHT_SENTINEL


def test_alternating_single_sample_runs_each_get_a_stage():
    from manual_mode import build_profile_from_manual_shot

    profile = build_profile_from_manual_shot(
        _manual_shot(
            [
                _pressure_sample(0, 6.0),
                _flow_sample(500, 2.0),
                _pressure_sample(1000, 7.0),
                _flow_sample(1500, 2.4),
            ]
        ),
        None,
    )

    assert [stage["name"] for stage in profile["stages"]] == [
        "Pressure 1",
        "Flow 2",
        "Pressure 3",
        "Flow 4",
    ]
    assert all(
        stage["dynamics"]["points"] == [[0.0, point]]
        for stage, point in zip(profile["stages"], [6.0, 2.0, 7.0, 2.4])
    )


# --- node program (contract section 8) --------------------------------------


HEAD_STAGE_NAMES = [
    "prepare",
    "purge",
    "water detection",
    "heating",
    "heating",
    "click to start",
    "retracting",
    "closing valve",
]
TAIL_STAGE_NAMES = ["retracting", "click to purge", "remove cup", "purge", "END_STAGE"]


@pytest.fixture
def manual_document():
    return ProfileManager.build_manual_mode_profile()


@pytest.fixture
def flow_first_document():
    profile = ProfileManager.build_manual_mode_profile()
    profile["stages"] = list(reversed(profile["stages"]))
    return profile


def _program_nodes(program):
    return [node for stage in program["stages"] for node in stage["nodes"]]


def _manual_program_stages(program):
    from manual_program import _is_manual_stage

    return [stage for stage in program["stages"] if _is_manual_stage(stage)]


def _main_nodes(stage):
    """The start/resume nodes of a manual stage -- everything but its init node."""
    from manual_program import MANUAL_CONTROLLER_KINDS

    return [
        node
        for node in stage["nodes"]
        if any(c["kind"] in MANUAL_CONTROLLER_KINDS for c in node["controllers"])
    ]


def test_build_manual_program_keeps_the_head_and_tail_around_the_manual_stages(
    manual_document,
):
    from manual_program import build_manual_program

    program = build_manual_program(manual_document)

    assert [stage["name"] for stage in program["stages"]] == (
        HEAD_STAGE_NAMES + ["Manual pressure", "Manual flow"] + TAIL_STAGE_NAMES
    )
    assert program["id"] == MANUAL_MODE_PROFILE_ID
    assert program["name"] == MANUAL_MODE_PROFILE_NAME


def test_a_flow_first_document_renders_its_stages_in_document_order(flow_first_document):
    from manual_program import build_manual_program

    program = build_manual_program(flow_first_document)

    manual = _manual_program_stages(program)
    assert [stage["name"] for stage in manual] == ["Manual flow", "Manual pressure"]

    # the start node -- the one entered from the head -- is the flow one
    start = [
        controller
        for controller in _main_nodes(manual[0])[0]["controllers"]
        if controller["initial"]["kind"] == "value"
    ]
    assert start[0]["kind"] == "manual_flow_controller"


def test_the_head_exits_onto_the_first_manual_stages_init_node(manual_document):
    from manual_program import END_NODE_HEAD, build_manual_program

    program = build_manual_program(manual_document)

    closing_valve = [s for s in program["stages"] if s["name"] == "closing valve"][0]
    exits = {t["next_node_id"] for n in closing_valve["nodes"] for t in n["triggers"]}
    assert exits == {END_NODE_HEAD}

    first_manual = _manual_program_stages(program)[0]
    assert first_manual["nodes"][0]["id"] == END_NODE_HEAD


def test_node_ids_are_unique_across_the_whole_program(manual_document):
    from manual_program import build_manual_program

    ids = [node["id"] for node in _program_nodes(build_manual_program(manual_document))]

    assert len(ids) == len(set(ids))


def test_every_next_node_id_resolves_to_a_node(manual_document):
    from manual_program import build_manual_program

    program = build_manual_program(manual_document)
    known = {node["id"] for node in _program_nodes(program)}

    targets = {
        trigger["next_node_id"]
        for node in _program_nodes(program)
        for trigger in node["triggers"]
    }
    assert targets <= known


def test_every_manual_main_node_carries_the_four_contract_triggers(manual_document):
    from manual_program import INIT_NODE_TAIL, build_manual_program

    program = build_manual_program(manual_document)
    manual = _manual_program_stages(program)
    resume_ids = [_main_nodes(stage)[-1]["id"] for stage in manual]

    for index, stage in enumerate(manual):
        other_resume = resume_ids[(index + 1) % len(manual)]
        for node in _main_nodes(stage):
            assert node["triggers"] == [
                {
                    "kind": "button_trigger",
                    "source": "Encoder Button",
                    "gesture": "Single Tap",
                    "next_node_id": other_resume,
                },
                {
                    "kind": "button_trigger",
                    "source": "Encoder Button",
                    "gesture": "Long Press",
                    "next_node_id": INIT_NODE_TAIL,
                },
                {
                    "kind": "weight_value_trigger",
                    "operator": ">=",
                    "value": MANUAL_MODE_FINAL_WEIGHT_SENTINEL,
                    "source": "Weight Predictive",
                    "weight_reference_id": 1,
                    "next_node_id": INIT_NODE_TAIL,
                },
                {
                    "kind": "piston_position_trigger",
                    "operator": ">=",
                    "value": 73,
                    "source": "Piston Position Raw",
                    "position_reference_id": 0,
                    "next_node_id": INIT_NODE_TAIL,
                },
            ]


def test_a_tap_hands_the_shot_to_the_other_stage(manual_document):
    from manual_program import build_manual_program

    manual = _manual_program_stages(build_manual_program(manual_document))
    pressure, flow = manual

    flow_resume = _main_nodes(flow)[-1]["id"]
    pressure_resume = _main_nodes(pressure)[-1]["id"]

    for node in _main_nodes(pressure):
        assert node["triggers"][0]["next_node_id"] == flow_resume
    for node in _main_nodes(flow):
        assert node["triggers"][0]["next_node_id"] == pressure_resume


def test_the_manual_controllers_match_the_contract(manual_document):
    from manual_program import build_manual_program

    manual = _manual_program_stages(build_manual_program(manual_document))
    pressure_nodes = _main_nodes(manual[0])
    flow_nodes = _main_nodes(manual[1])

    # first stage: a start node entered from the head, then the resume node
    assert len(pressure_nodes) == 2
    assert pressure_nodes[0]["controllers"] == [
        {
            "kind": "manual_pressure_controller",
            "algorithm": "Pressure PID v1.0",
            "step": 0.1,
            "min": 0.0,
            "max": 12.0,
            "initial": {"kind": "value", "value": 0.0},
        }
    ]
    assert pressure_nodes[1]["controllers"] == [
        {
            "kind": "manual_pressure_controller",
            "algorithm": "Pressure PID v1.0",
            "step": 0.1,
            "min": 0.0,
            "max": 12.0,
            "initial": {"kind": "sensor", "source": "Pressure Raw", "gain": 1.1},
        }
    ]

    # the second stage is only ever re-entered, so it has no start node
    assert len(flow_nodes) == 1
    assert flow_nodes[0]["controllers"] == [
        {
            "kind": "manual_flow_controller",
            "algorithm": "Flow PID v1.0",
            "step": 0.1,
            "min": 0.0,
            "max": 12.0,
            "initial": {"kind": "sensor", "source": "Flow Raw", "gain": 1.1},
        }
    ]


def test_the_start_node_begins_at_the_documents_first_point(manual_document):
    from manual_program import build_manual_program

    manual_document["stages"][0]["dynamics"]["points"] = [[0, 6.5]]

    manual = _manual_program_stages(build_manual_program(manual_document))
    start = _main_nodes(manual[0])[0]

    assert start["controllers"][0]["initial"] == {"kind": "value", "value": 6.5}


def test_each_manual_stage_opens_with_its_own_references(manual_document):
    from manual_program import build_manual_program

    manual = _manual_program_stages(build_manual_program(manual_document))

    reference_ids = []
    for stage in manual:
        init = stage["nodes"][0]
        assert [c["kind"] for c in init["controllers"]] == [
            "time_reference",
            "weight_reference",
            "position_reference",
        ]
        assert [t["kind"] for t in init["triggers"]] == ["exit"]
        reference_ids += [c["id"] for c in init["controllers"]]

    assert len(reference_ids) == len(set(reference_ids))


def test_the_documents_temperature_reaches_the_heating_curve(manual_document):
    from manual_program import build_manual_program

    manual_document["temperature"] = 94.0

    program = build_manual_program(manual_document)
    curves = [
        controller["curve"]["points"]
        for stage in program["stages"]
        if stage["name"] == "heating"
        for node in stage["nodes"]
        for controller in node["controllers"]
        if controller["kind"] == "temperature_controller"
    ]

    assert [[0, 94.0]] in curves


def test_the_documents_final_weight_is_the_weight_stop(manual_document):
    from manual_program import build_manual_program

    manual_document["final_weight"] = 42.0

    program = build_manual_program(manual_document)
    values = {
        trigger["value"]
        for stage in _manual_program_stages(program)
        for node in _main_nodes(stage)
        for trigger in node["triggers"]
        if trigger["kind"] == "weight_value_trigger"
    }

    assert values == {42.0}


@pytest.mark.parametrize(
    "auto_purge, retracting_exit",
    [(False, 30), (True, 48)],
)
def test_the_auto_purge_setting_picks_the_tail_route(
    monkeypatch, manual_document, auto_purge, retracting_exit
):
    from config import CONFIG_USER, MeticulousConfig, PROFILE_AUTO_PURGE
    from manual_program import build_manual_program

    monkeypatch.setitem(MeticulousConfig[CONFIG_USER], PROFILE_AUTO_PURGE, auto_purge)

    program = build_manual_program(manual_document)

    # the tail's "retracting" stage is the one after the manual stages
    manual_names = {stage["name"] for stage in _manual_program_stages(program)}
    names = [stage["name"] for stage in program["stages"]]
    tail_start = max(names.index(name) for name in manual_names) + 1
    tail_retracting = program["stages"][tail_start]

    assert tail_retracting["name"] == "retracting"
    exits = [t["next_node_id"] for n in tail_retracting["nodes"] for t in n["triggers"]]
    assert retracting_exit in exits


@pytest.mark.parametrize("allow_skipping", [False, True])
def test_stage_skipping_never_adds_a_trigger_to_a_manual_node(
    monkeypatch, manual_document, allow_skipping
):
    from config import CONFIG_USER, MACHINE_ALLOW_STAGE_SKIPPING, MeticulousConfig
    from manual_program import build_manual_program

    monkeypatch.setitem(
        MeticulousConfig[CONFIG_USER], MACHINE_ALLOW_STAGE_SKIPPING, allow_skipping
    )

    program = build_manual_program(manual_document)

    for stage in _manual_program_stages(program):
        for node in _main_nodes(stage):
            assert len(node["triggers"]) == 4


def test_repeated_builds_do_not_drift_the_node_ids(manual_document):
    from manual_program import build_manual_program

    first = build_manual_program(manual_document)
    second = build_manual_program(manual_document)

    assert [node["id"] for node in _program_nodes(first)] == [
        node["id"] for node in _program_nodes(second)
    ]


def test_a_document_with_an_unsupported_stage_type_is_rejected(manual_document):
    from manual_program import build_manual_program

    manual_document["stages"][0]["type"] = "temperature"

    with pytest.raises(ValueError, match="unsupported manual stage type"):
        build_manual_program(manual_document)


def test_a_document_without_stages_is_rejected(manual_document):
    from manual_program import build_manual_program

    manual_document["stages"] = []

    with pytest.raises(ValueError, match="at least one stage"):
        build_manual_program(manual_document)


def test_validate_program_rejects_a_duplicate_node_id(manual_document):
    from manual_program import build_manual_program, validate_program

    program = build_manual_program(manual_document)
    manual = _manual_program_stages(program)
    manual[1]["nodes"][0]["id"] = manual[0]["nodes"][0]["id"]

    with pytest.raises(ValueError, match="duplicate node ids"):
        validate_program(program)


def test_validate_program_rejects_a_manual_id_that_collides_with_the_head(manual_document):
    from manual_program import build_manual_program, validate_program

    program = build_manual_program(manual_document)
    _manual_program_stages(program)[0]["nodes"][0]["id"] = 23  # the head's closing valve

    with pytest.raises(ValueError, match="collide with the head or tail"):
        validate_program(program)


def test_validate_program_rejects_a_dangling_next_node_id(manual_document):
    from manual_program import build_manual_program, validate_program

    program = build_manual_program(manual_document)
    _manual_program_stages(program)[0]["nodes"][0]["triggers"][0]["next_node_id"] = 99999

    with pytest.raises(ValueError, match="unknown node 99999"):
        validate_program(program)


def test_validate_program_rejects_a_moved_tail_entry(manual_document):
    from manual_program import INIT_NODE_TAIL, build_manual_program, validate_program

    program = build_manual_program(manual_document)
    names = [stage["name"] for stage in program["stages"]]
    tail_start = names.index("closing valve") + 3
    tail_entry_node = program["stages"][tail_start]["nodes"][0]
    assert tail_entry_node["id"] == INIT_NODE_TAIL
    tail_entry_node["id"] = 6999

    with pytest.raises(ValueError, match="the tail is entered at 6999"):
        validate_program(program)


def test_validate_program_rejects_an_unreachable_start_node(manual_document):
    from manual_program import build_manual_program, validate_program

    program = build_manual_program(manual_document)
    init = _manual_program_stages(program)[0]["nodes"][0]
    # send the first init node straight to the tail instead of to the start node
    init["triggers"][0]["next_node_id"] = 7000

    with pytest.raises(ValueError, match="unreachable from the head exit"):
        validate_program(program)


# --- loading a manual profile sends the program ------------------------------


@pytest.fixture
def sent_to_esp32(monkeypatch):
    """Capture what send_profile_to_esp32 hands to the machine and to the store."""
    from api.alarms import AlarmManager

    sent = []
    last = []

    monkeypatch.setattr(AlarmManager, "is_alarm_set", staticmethod(lambda _type: None))
    monkeypatch.setattr(profiles.Machine, "send_json_with_hash", staticmethod(sent.append))
    monkeypatch.setattr(ProfileManager, "_set_last_profile", staticmethod(last.append))
    monkeypatch.setattr(
        ProfileManager, "_emit_profile_event", staticmethod(lambda *a, **k: None)
    )
    # the hover coroutine is created before the scheduler below ever sees it
    monkeypatch.setattr(
        ProfileManager, "_async_emit_profile_hover", staticmethod(lambda *a, **k: None)
    )
    monkeypatch.setattr(profiles.asyncio, "run_coroutine_threadsafe", lambda *a, **k: None)
    return sent, last


def _loadable_manual_document():
    profile = ProfileManager.build_manual_mode_profile()
    # keep handle_image off the filesystem
    profile["display"] = {"image": "/api/v1/profile/image/manual.png"}
    return profile


def test_loading_a_manual_profile_sends_the_node_program(sent_to_esp32):
    sent, last = sent_to_esp32
    document = _loadable_manual_document()

    result = ProfileManager.send_profile_to_esp32(document)

    assert len(sent) == 1
    program = sent[0]
    assert [stage["name"] for stage in program["stages"]] == (
        HEAD_STAGE_NAMES + ["Manual pressure", "Manual flow"] + TAIL_STAGE_NAMES
    )
    assert result is document


def test_loading_a_manual_profile_still_records_the_simplified_document(sent_to_esp32):
    sent, last = sent_to_esp32
    document = _loadable_manual_document()

    ProfileManager.send_profile_to_esp32(document)

    assert last == [document]
    assert last[0]["stages"][0]["key"] == MANUAL_MODE_PRESSURE_STAGE_KEY
    assert "nodes" not in json.dumps(last[0])


def test_loading_an_ordinary_profile_still_sends_the_document(sent_to_esp32):
    sent, last = sent_to_esp32
    document = _loadable_manual_document()
    del document["manual"]
    document["id"] = "22222222-3333-4444-8555-666666666666"

    ProfileManager.send_profile_to_esp32(document)

    assert sent == [document]


def test_a_profile_marked_manual_with_a_string_is_not_converted(sent_to_esp32):
    sent, last = sent_to_esp32
    document = _loadable_manual_document()
    document["manual"] = "true"

    ProfileManager.send_profile_to_esp32(document)

    assert sent == [document]


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
            "error": "shot has no target samples",
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
