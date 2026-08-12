import operator

import pytest

from profile_converter.simplified_json import SimplifiedJson


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
    ]
    assert len(converted_triggers) == 1
    return converted_triggers[0]


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
