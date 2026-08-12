import copy
import json
import operator

import jsonschema
import pytest

import profiles
from profile_converter.simplified_json import SimplifiedJson
from profiles import ProfileManager


NUMERIC_TRIGGER_KINDS = {
    "weight": "weight_value_trigger",
    "time": "timer_trigger",
    "pressure": "pressure_value_trigger",
    "flow": "flow_value_trigger",
    "piston_position": "piston_position_trigger",
    "power": "piston_power_value_trigger",
}

COMPARISON_FUNCTIONS = {
    ">": operator.gt,
    "<": operator.lt,
    ">=": operator.ge,
    "<=": operator.le,
}

EXPECTED_BOUNDARIES = {
    ">": (False, False, True),
    "<": (True, False, False),
    ">=": (False, True, True),
    "<=": (True, True, False),
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
    converted_triggers = [
        trigger
        for node in converted_stage["nodes"]
        for trigger in node["triggers"]
        if trigger.get("kind") == NUMERIC_TRIGGER_KINDS[trigger_type]
        and (trigger_type != "weight" or trigger.get("source") == "Weight Average")
    ]
    assert len(converted_triggers) == 1
    return converted_triggers[0]


def _profile_with_all_numeric_triggers(comparison):
    triggers = [
        {
            "type": trigger_type,
            "value": index + 1,
            "relative": index % 2 == 0,
            "comparison": comparison,
            "variables": [f"variable-{index}"],
            "extension": {"preserve": index},
        }
        for index, trigger_type in enumerate(NUMERIC_TRIGGER_KINDS)
    ]
    return {
        "id": "72f90b46-ea76-43df-8e97-9fcafb26d3fc",
        "name": "Comparison persistence test",
        "author": "Test author",
        "author_id": "5b25f0f1-d25a-494a-a8c2-b3a8f74beab6",
        "last_changed": 1,
        "temperature": 93,
        "final_weight": 36,
        "display": {
            "image": "/api/v1/profile/image/test.png",
            "accentColor": "#123456",
        },
        "variables": [
            {
                "name": "Preserve",
                "key": "preserve",
                "type": "weight",
                "value": 42,
            }
        ],
        "stages": [
            {
                "name": "Test stage",
                "key": "test-stage",
                "type": "flow",
                "dynamics": {
                    "points": [[0, 1]],
                    "over": "time",
                    "interpolation": "linear",
                },
                "limits": [],
                "exit_triggers": triggers,
            }
        ],
    }


@pytest.mark.parametrize("trigger_type", NUMERIC_TRIGGER_KINDS)
@pytest.mark.parametrize("comparison", COMPARISON_FUNCTIONS)
def test_numeric_exit_trigger_preserves_comparison_and_boundary_semantics(
    trigger_type, comparison
):
    converted = _convert_exit_trigger(trigger_type, comparison)

    assert converted["operator"] == comparison

    compare = COMPARISON_FUNCTIONS[converted["operator"]]
    actual_boundaries = (
        compare(-0.1, 0),
        compare(0, 0),
        compare(0.1, 0),
    )
    assert actual_boundaries == EXPECTED_BOUNDARIES[comparison]


@pytest.mark.parametrize("comparison", COMPARISON_FUNCTIONS)
def test_explicit_comparisons_validate_persist_and_convert_unchanged(
    comparison, tmp_path, monkeypatch
):
    profile = _profile_with_all_numeric_triggers(comparison)
    original = copy.deepcopy(profile)

    with open("profile_schema/schema.json", encoding="utf-8") as schema_file:
        schema = json.load(schema_file)
    jsonschema.validate(profile, schema)

    monkeypatch.setattr(profiles, "PROFILE_PATH", str(tmp_path))
    monkeypatch.setattr(ProfileManager, "_schema", schema)
    monkeypatch.setattr(ProfileManager, "_known_profiles", {profile["id"]: profile})
    monkeypatch.setattr(ProfileManager, "_emit_profile_event", lambda *args: None)

    ProfileManager.save_profile(profile)
    ProfileManager.refresh_profile_list()
    persisted = ProfileManager.get_profile(profile["id"])

    assert persisted == original
    assert json.loads((tmp_path / f'{profile["id"]}.json').read_text()) == original

    converted_stage = SimplifiedJson(persisted).to_complex(1000, 2000)[0]
    converted_triggers = {
        trigger["kind"]: trigger
        for node in converted_stage["nodes"]
        for trigger in node["triggers"]
        if trigger.get("kind") in NUMERIC_TRIGGER_KINDS.values()
        and (
            trigger.get("kind") != NUMERIC_TRIGGER_KINDS["weight"]
            or trigger.get("source") == "Weight Average"
        )
    }
    assert set(converted_triggers) == set(NUMERIC_TRIGGER_KINDS.values())
    assert all(trigger["operator"] == comparison for trigger in converted_triggers.values())

    assert persisted["variables"] == original["variables"]
    assert persisted["stages"][0]["exit_triggers"] == original["stages"][0]["exit_triggers"]


def test_weight_strict_greater_than_zero_waits_for_positive_weight():
    converted = _convert_exit_trigger("weight", ">")
    compare = COMPARISON_FUNCTIONS[converted["operator"]]

    assert compare(0, converted["value"]) is False
    assert compare(0.1, converted["value"]) is True


@pytest.mark.parametrize("trigger_type", NUMERIC_TRIGGER_KINDS)
def test_omitted_comparison_retains_greater_than_or_equal_fallback(trigger_type):
    assert _convert_exit_trigger(trigger_type, None)["operator"] == ">="


def test_unsupported_comparison_retains_documented_compatibility_fallback(capsys):
    converted = _convert_exit_trigger("weight", "==")

    assert converted["operator"] == ">="
    assert (
        "Comparison: == not supported. Using default value: >= ."
        in capsys.readouterr().out
    )
