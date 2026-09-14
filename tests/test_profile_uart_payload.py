import asyncio
import json
import sys
import types

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

from api.alarms import AlarmType
from machine import Machine
from profile_preprocessor import ProfilePreprocessor
from profiles import ESP32_PROFILE_FIELDS, PROFILE_EVENT, ProfileManager


def adaptive_profile():
    return {
        "id": "a188c420-4952-4b33-8489-a082e9500ecb",
        "name": "Adaptive (Gagné Method)",
        "author": "Corb3t",
        "author_id": "550e8400-e29b-41d4-a716-446655449999",
        "version": 9,
        "last_changed": 1776608894.8666232,
        "image": "data:image/png;base64," + "A" * 296_904,
        "display": {
            "image": "/api/v1/profile/image/ae3e6ef14875bad88cad00f09945e681.png",
            "graphs": [{"x": "time", "y": "flow", "label": "Flow"}],
            "description": "Display-only recipe guidance",
        },
        "temperature": 90,
        "final_weight": 36,
        "variables": [
            {
                "key": "pressure_PeakLimit",
                "name": "Peak Pressure Limit",
                "type": "pressure",
                "value": 8.6,
            },
            {
                "key": "flow_TargetExtract",
                "name": "Target Extraction Flow",
                "type": "flow",
                "value": 4,
            },
            {
                "key": "pressure_PILimit",
                "name": "Pre-infusion Pressure Limit",
                "type": "pressure",
                "value": 4,
            },
            {
                "key": "time_FillPause",
                "name": "Fill Pause Duration",
                "type": "time",
                "value": 4,
            },
        ],
        "stages": [
            {
                "key": "fast_fill_1",
                "name": "Fast Fill",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PILimit"}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, 8]],
                    "interpolation": "linear",
                },
                "exit_triggers": [
                    {
                        "type": "pressure",
                        "value": "$pressure_PILimit",
                        "relative": False,
                        "comparison": ">=",
                    }
                ],
            },
            {
                "key": "fill_pause_2",
                "name": "Fill Pause",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PILimit"}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, 0.5], ["$time_FillPause", 0.5]],
                    "interpolation": "linear",
                },
                "exit_triggers": [
                    {
                        "type": "time",
                        "value": "$time_FillPause",
                        "relative": True,
                        "comparison": ">=",
                    }
                ],
            },
            {
                "key": "pressure_ramp_3",
                "name": "Pressure Ramp",
                "type": "pressure",
                "limits": [{"type": "flow", "value": 6}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, "$pressure_PILimit"], [4, "$pressure_PeakLimit"]],
                    "interpolation": "linear",
                },
                "exit_triggers": [
                    {
                        "type": "time",
                        "value": 4,
                        "relative": True,
                        "comparison": ">=",
                    }
                ],
            },
            {
                "key": "adaptive_extraction_4",
                "name": "Adaptive Extraction",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PeakLimit"}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, "$flow_TargetExtract"], [35, 0.5]],
                    "interpolation": "linear",
                },
                "exit_triggers": [
                    {
                        "type": "time",
                        "value": 45,
                        "relative": True,
                        "comparison": ">=",
                    }
                ],
            },
        ],
        "community": {"downloads": 123, "featured": True},
        "unknown_future_metadata": {"large": "B" * 10_000},
    }


def test_adaptive_profile_uart_payload_collapses_to_recipe_data():
    profile = adaptive_profile()

    preprocessed = ProfilePreprocessor.processVariables(profile)
    payload = ProfileManager._profile_for_esp32(preprocessed)

    assert len(json.dumps(profile).encode("utf-8")) > 300_000
    assert len(json.dumps(payload).encode("utf-8")) < 5_000
    assert tuple(payload) == ESP32_PROFILE_FIELDS
    assert payload["stages"][0]["limits"][0]["value"] == 4
    assert payload["stages"][1]["dynamics"]["points"][1][0] == 4
    assert payload["stages"][2]["dynamics"]["points"][1][1] == 8.6
    assert payload["stages"][3]["dynamics"]["points"][0][1] == 4
    assert "image" not in payload
    assert "id" not in payload
    assert "display" not in payload
    assert "author" not in payload
    assert "community" not in payload
    assert "unknown_future_metadata" not in payload


def test_node_profile_keeps_complete_runtime_graph_but_not_metadata():
    profile = {
        "id": "11111111-2222-4333-8444-555555555555",
        "name": "Diagnostic graph",
        "stages": [
            {
                "name": "initialize",
                "nodes": [
                    {
                        "id": -1,
                        "controllers": [
                            {"kind": "time_reference", "id": 1},
                            {"kind": "log_controller", "message": "loaded"},
                        ],
                        "triggers": [
                            {
                                "kind": "timer_trigger",
                                "timer_reference_id": 1,
                                "operator": ">=",
                                "value": 1,
                                "next_node_id": -2,
                            }
                        ],
                    }
                ],
            }
        ],
        "display": {"image": "data:image/png;base64,unused"},
        "author": "UI metadata",
    }

    payload = ProfileManager._profile_for_esp32(profile)

    assert payload == {
        "name": profile["name"],
        "stages": profile["stages"],
    }


def test_send_preserves_alarm_guard_profile_state_and_load_event(monkeypatch):
    profile = adaptive_profile()
    sent_payloads = []
    saved_profiles = []
    emitted_events = []

    monkeypatch.setattr(ProfileManager, "handle_image", lambda _profile: None)
    monkeypatch.setattr(ProfileManager, "validate_profile", lambda _profile: None)
    monkeypatch.setattr(
        "profiles.AlarmManager.is_alarm_set",
        lambda alarm: None if alarm == AlarmType.MOTOR_STRESSED else None,
    )
    monkeypatch.setattr(Machine, "send_json_with_hash", sent_payloads.append)
    monkeypatch.setattr(ProfileManager, "_set_last_profile", saved_profiles.append)
    monkeypatch.setattr(
        ProfileManager,
        "_emit_profile_event",
        lambda event, profile_id: emitted_events.append((event, profile_id)),
    )
    monkeypatch.setattr(ProfileManager, "_async_emit_profile_hover", lambda: None)
    monkeypatch.setattr(asyncio, "run_coroutine_threadsafe", lambda _coro, _loop: None)

    result = ProfileManager.send_profile_to_esp32(profile)

    assert result is profile
    assert saved_profiles == [profile]
    assert emitted_events == [(PROFILE_EVENT.LOAD, profile["id"])]
    assert len(sent_payloads) == 1
    assert tuple(sent_payloads[0]) == ESP32_PROFILE_FIELDS
    assert len(json.dumps(sent_payloads[0]).encode("utf-8")) < 5_000

    monkeypatch.setattr(
        "profiles.AlarmManager.is_alarm_set",
        lambda alarm: float("inf") if alarm == AlarmType.MOTOR_STRESSED else None,
    )
    monkeypatch.setattr("profiles.AlarmManager._notify_user", lambda **_kwargs: None)

    assert ProfileManager.send_profile_to_esp32(profile) is False
    assert len(sent_payloads) == 1
