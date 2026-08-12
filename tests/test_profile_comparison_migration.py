import json
from copy import deepcopy
from pathlib import Path

import jsonschema
import pytest

import profiles
from config import (
    CONFIG_PROFILES,
    CONFIG_USER,
    PROFILE_LAST,
    PROFILE_ORDER,
    MeticulousConfig,
)
from profile_converter.simplified_json import SimplifiedJson
from profile_migrations import (
    canonicalize_exit_trigger_comparisons,
    migrate_legacy_exit_trigger_comparisons,
)
from profiles import ProfileManager


AFFECTED_TRIGGER_KINDS = {
    "weight": "weight_value_trigger",
    "time": "timer_trigger",
    "pressure": "pressure_value_trigger",
    "flow": "flow_value_trigger",
    "piston_position": "piston_position_trigger",
    "power": "piston_power_value_trigger",
}
CONVERTER_TRIGGER_KINDS = {
    **AFFECTED_TRIGGER_KINDS,
    "temperature": "temperature_value_trigger",
}


def _convert_exit_trigger(trigger_type, comparison_marker):
    trigger = {"type": trigger_type, "value": 0, "relative": False}
    if comparison_marker is not None:
        trigger["comparison"] = comparison_marker
    profile = {
        "name": "Comparison test",
        "temperature": 93,
        "final_weight": 0,
        "stages": [
            {
                "name": "Test stage",
                "type": "flow",
                "dynamics": {
                    "points": [[0, 1]],
                    "over": "time",
                    "interpolation": "linear",
                },
                "limits": [],
                "exit_triggers": [trigger],
            }
        ],
    }

    converted_stage = SimplifiedJson(profile).to_complex(1000, 2000)[0]
    triggers = [
        trigger
        for node in converted_stage["nodes"]
        for trigger in node["triggers"]
        if trigger.get("kind") == CONVERTER_TRIGGER_KINDS[trigger_type]
    ]
    assert triggers
    return triggers[0]


@pytest.mark.parametrize("trigger_type", AFFECTED_TRIGGER_KINDS)
@pytest.mark.parametrize("comparison", [None, ">", ">="])
def test_affected_exit_triggers_convert_to_strict_greater_than(trigger_type, comparison):
    assert _convert_exit_trigger(trigger_type, comparison)["operator"] == ">"


def test_less_than_or_equal_is_unchanged_at_conversion():
    assert _convert_exit_trigger("weight", "<=")["operator"] == "<="


def test_zero_gram_weight_trigger_preserves_zero_and_uses_strict_greater_than():
    converted = _convert_exit_trigger("weight", ">=")

    assert converted["operator"] == ">"
    assert converted["value"] == 0


@pytest.mark.parametrize(
    ("comparison", "expected_operator"),
    [(None, ">="), (">=", ">="), (">", ">")],
)
def test_unaffected_temperature_comparison_is_not_reinterpreted(comparison, expected_operator):
    assert _convert_exit_trigger("temperature", comparison)["operator"] == expected_operator


def test_migration_is_pure_targeted_and_idempotent():
    affected_with_legacy_comparison = [
        {
            "type": trigger_type,
            "comparison": ">=",
            "value": index,
            "relative": bool(index % 2),
            "extra": {"preserve": index},
        }
        for index, trigger_type in enumerate(AFFECTED_TRIGGER_KINDS)
    ]
    affected_without_comparison = [
        {
            "type": trigger_type,
            "value": index + len(AFFECTED_TRIGGER_KINDS),
            "relative": bool(index % 2),
            "extra": {"preserve": index},
        }
        for index, trigger_type in enumerate(AFFECTED_TRIGGER_KINDS)
    ]
    profile = {
        "id": "profile-id",
        "last_changed": 1234.5,
        "final_weight": 0,
        "display": {
            "image": "/api/v1/profile/image/preserved.png",
            "accentColor": "#123456",
            "nested": {"preserve": True},
        },
        "variables": [{"key": "target", "type": "weight", "value": 0}],
        "stages": [
            {
                "key": "stage-key",
                "limits": [{"type": "pressure", "value": 8, "comparison": ">="}],
                "exit_triggers": affected_with_legacy_comparison
                + affected_without_comparison
                + [
                    {"type": "weight", "comparison": "<=", "value": 1},
                    {"type": "temperature", "comparison": ">=", "value": 93},
                    {"type": "user_interaction", "comparison": ">=", "value": 0},
                    {"type": "temperature"},
                    {"kind": "timer_trigger", "operator": ">=", "value": 2},
                    "malformed",
                ],
            },
            "malformed",
            {"exit_triggers": "malformed"},
        ],
    }
    original = deepcopy(profile)
    expected = deepcopy(profile)
    for trigger in expected["stages"][0]["exit_triggers"][:12]:
        trigger["comparison"] = ">"

    migrated = migrate_legacy_exit_trigger_comparisons(profile)

    assert profile == original
    assert migrated is not profile
    assert migrated == expected
    assert [
        trigger["comparison"] for trigger in migrated["stages"][0]["exit_triggers"][:12]
    ] == [">"] * 12
    assert migrated["stages"][0]["exit_triggers"][12:] == original["stages"][0][
        "exit_triggers"
    ][12:]
    assert migrated["stages"][0]["limits"] == original["stages"][0]["limits"]
    assert migrated["id"] == original["id"]
    assert migrated["last_changed"] == original["last_changed"]
    assert migrated["final_weight"] == original["final_weight"]
    assert migrated["display"] == original["display"]
    assert migrated["variables"] == original["variables"]
    assert migrate_legacy_exit_trigger_comparisons(migrated) == migrated

    migrated_with_flag, changed = canonicalize_exit_trigger_comparisons(profile)
    canonical_with_flag, changed_again = canonicalize_exit_trigger_comparisons(
        migrated_with_flag
    )
    assert changed is True
    assert changed_again is False
    assert canonical_with_flag == migrated_with_flag


@pytest.mark.parametrize("malformed", [None, [], "profile", {"stages": None}, {"stages": {}}])
def test_migration_tolerates_malformed_or_unrelated_data(malformed):
    assert migrate_legacy_exit_trigger_comparisons(malformed) == malformed


def test_migration_canonicalizes_affected_trigger_even_when_other_fields_are_missing():
    profile = {"stages": [{"exit_triggers": [{"type": "weight"}]}]}

    migrated, changed = canonicalize_exit_trigger_comparisons(profile)

    assert changed is True
    assert migrated == {
        "stages": [{"exit_triggers": [{"type": "weight", "comparison": ">"}]}]
    }


@pytest.mark.parametrize("trigger_type", AFFECTED_TRIGGER_KINDS)
@pytest.mark.parametrize("comparison", [None, ">", ">="])
def test_schema_accepts_canonical_and_legacy_ingress(trigger_type, comparison):
    profile = _legacy_profile()
    trigger = profile["stages"][0]["exit_triggers"][0]
    trigger["type"] = trigger_type
    if comparison is None:
        trigger.pop("comparison")
    else:
        trigger["comparison"] = comparison

    schema_path = Path(__file__).parents[1] / "profile_schema" / "schema.json"
    jsonschema.validate(profile, json.loads(schema_path.read_text()))


def _legacy_profile(profile_id="profile-id"):
    return {
        "id": profile_id,
        "name": "Legacy comparison",
        "author": "",
        "author_id": "00000000-0000-0000-0000-000000000000",
        "last_changed": 1234.5,
        "display": {
            "image": "/api/v1/profile/image/test.png",
            "accentColor": "#000000",
        },
        "temperature": 93,
        "final_weight": 0,
        "stages": [
            {
                "key": "stage-key",
                "name": "Test stage",
                "type": "flow",
                "dynamics": {
                    "points": [[0, 1]],
                    "over": "time",
                    "interpolation": "linear",
                },
                "limits": [],
                "exit_triggers": [
                    {
                        "type": "weight",
                        "comparison": ">=",
                        "value": 0,
                        "relative": False,
                    }
                ],
            }
        ],
    }


def _comparison(profile):
    return profile["stages"][0]["exit_triggers"][0]["comparison"]


@pytest.fixture
def isolated_profile_manager(monkeypatch, tmp_path):
    original_config = deepcopy(dict(MeticulousConfig))
    ProfileManager._known_profiles = {}
    ProfileManager._last_profile_changes = []
    ProfileManager._loop = None
    ProfileManager._profile_default_images = []
    ProfileManager._profile_default_images_accent_colors = {}

    MeticulousConfig[CONFIG_USER][PROFILE_ORDER] = []
    MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST] = None
    monkeypatch.setattr(MeticulousConfig, "save", lambda: None)
    monkeypatch.setattr(
        profiles.asyncio,
        "run_coroutine_threadsafe",
        lambda coroutine, loop: coroutine.close(),
    )
    monkeypatch.setattr(profiles, "PROFILE_PATH", str(tmp_path / "profiles"))
    monkeypatch.setattr(profiles, "DEFAULT_PROFILES_PATH", str(tmp_path / "defaults"))
    (tmp_path / "profiles").mkdir()
    (tmp_path / "defaults").mkdir()

    yield tmp_path

    MeticulousConfig.clear()
    MeticulousConfig.update(original_config)


def test_save_migrates_before_validation_and_persists_without_mutating_input(
    isolated_profile_manager, monkeypatch
):
    legacy = _legacy_profile()
    original = deepcopy(legacy)
    validated = []
    monkeypatch.setattr(
        ProfileManager,
        "validate_profile",
        lambda data: validated.append(deepcopy(data)),
    )
    monkeypatch.setattr(ProfileManager, "_emit_profile_event", lambda *args: None)

    result = ProfileManager.save_profile(legacy)
    persisted = json.loads(
        (isolated_profile_manager / "profiles" / "profile-id.json").read_text()
    )

    assert legacy == original
    assert _comparison(validated[0]) == ">"
    assert _comparison(result["profile"]) == ">"
    assert _comparison(persisted) == ">"
    assert persisted["last_changed"] == original["last_changed"]


def test_direct_send_and_load_migrate_before_validation_and_cache(
    isolated_profile_manager, monkeypatch
):
    legacy = _legacy_profile()
    validated = []
    sent = []
    monkeypatch.setattr(
        ProfileManager,
        "validate_profile",
        lambda data: validated.append(deepcopy(data)),
    )
    monkeypatch.setattr(ProfileManager, "handle_image", lambda data: None)
    monkeypatch.setattr(ProfileManager, "_emit_profile_event", lambda *args: None)
    monkeypatch.setattr(ProfileManager, "_set_last_profile", lambda data: None)
    monkeypatch.setattr(profiles.AlarmManager, "is_alarm_set", lambda alarm: None)
    monkeypatch.setattr(profiles.ProfilePreprocessor, "processVariables", lambda data: data)
    monkeypatch.setattr(
        profiles.Machine,
        "send_json_with_hash",
        lambda data: sent.append(deepcopy(data)),
    )
    ProfileManager._known_profiles[legacy["id"]] = legacy

    loaded = ProfileManager.load_profile_and_send(legacy["id"])

    assert _comparison(validated[0]) == ">"
    assert _comparison(sent[0]) == ">"
    assert _comparison(loaded) == ">"
    assert _comparison(ProfileManager._known_profiles[legacy["id"]]) == ">"


def test_on_disk_refresh_migrates_before_validation_without_changing_timestamp(
    isolated_profile_manager, monkeypatch
):
    legacy = _legacy_profile()
    profile_path = isolated_profile_manager / "profiles" / "profile-id.json"
    profile_path.write_text(json.dumps(legacy))
    validated = []
    monkeypatch.setattr(
        ProfileManager,
        "validate_profile",
        lambda data: validated.append(deepcopy(data)),
    )
    monkeypatch.setattr(ProfileManager, "_emit_profile_event", lambda *args: None)

    ProfileManager.refresh_profile_list()
    persisted = json.loads(profile_path.read_text())

    assert _comparison(validated[0]) == ">"
    assert _comparison(persisted) == ">"
    assert persisted["last_changed"] == legacy["last_changed"]


def test_on_disk_refresh_keeps_saved_metadata_after_pure_migration(
    isolated_profile_manager, monkeypatch
):
    legacy = _legacy_profile()
    legacy.pop("last_changed")
    profile_path = isolated_profile_manager / "profiles" / "profile-id.json"
    profile_path.write_text(json.dumps(legacy))
    monkeypatch.setattr(ProfileManager, "validate_profile", lambda data: None)
    monkeypatch.setattr(ProfileManager, "_emit_profile_event", lambda *args: None)

    ProfileManager.refresh_profile_list()

    persisted = json.loads(profile_path.read_text())
    assert persisted["last_changed"] > 0
    assert ProfileManager._known_profiles["profile-id"] == persisted
    assert _comparison(persisted) == ">"


def test_default_and_community_reads_migrate_in_memory(isolated_profile_manager):
    defaults_path = isolated_profile_manager / "defaults"
    community_path = defaults_path / "community"
    community_path.mkdir()
    (defaults_path / "default.json").write_text(json.dumps(_legacy_profile("default")))
    (community_path / "community.json").write_text(
        json.dumps(_legacy_profile("community"))
    )

    ProfileManager.refresh_default_profile_list()
    loaded = ProfileManager.list_default_profiles()

    assert _comparison(loaded["default"][0]) == ">"
    assert _comparison(loaded["community"][0]) == ">"


def test_cached_last_profile_migrates_on_read_and_write(
    isolated_profile_manager, monkeypatch
):
    save_calls = []
    monkeypatch.setattr(MeticulousConfig, "save", lambda: save_calls.append(True))
    MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST] = {
        "load_time": 42,
        "profile": _legacy_profile(),
        "unrelated": "preserved",
    }

    first_read = ProfileManager.get_last_profile()
    second_read = ProfileManager.get_last_profile()

    assert _comparison(first_read["profile"]) == ">"
    assert first_read["load_time"] == 42
    assert first_read["unrelated"] == "preserved"
    assert second_read == first_read
    assert len(save_calls) == 1

    ProfileManager._set_last_profile(_legacy_profile("new-last"))
    written = MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST]
    assert _comparison(written["profile"]) == ">"
    assert len(save_calls) == 2
