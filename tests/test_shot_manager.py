import sys
import types

try:
    import pyprctl  # noqa: F401
except ImportError:
    sys.modules["pyprctl"] = types.SimpleNamespace(set_name=lambda _name: None)

from shot_manager import PushToBrewTimer, Shot, ShotManager


def test_push_to_brew_timer_measures_click_to_start_status_in_milliseconds():
    timer = PushToBrewTimer()

    timer.observe("heating", 10.0)
    timer.observe("click to start", 12.0)
    timer.observe("click to start", 13.25)
    timer.observe("retracting", 14.5)

    assert timer.duration_ms == 2500


def test_push_to_brew_timer_reset_clears_previous_shot_duration():
    timer = PushToBrewTimer()
    timer.observe("click to start", 1.0)
    timer.observe("retracting", 2.0)

    timer.reset()
    timer.observe("heating", 3.0)

    assert timer.duration_ms == 0


def test_normal_shot_serializes_push_to_brew_time():
    shot = Shot(push_to_brew_time=987)

    assert shot.to_json()["push_to_brew_time"] == 987


def test_normal_shot_without_push_to_brew_stage_serializes_zero():
    assert Shot().to_json()["push_to_brew_time"] == 0


def test_shot_manager_starts_normal_shot_with_push_to_brew_time():
    ShotManager.start(push_to_brew_time=654)

    try:
        assert ShotManager.getCurrentShot()["push_to_brew_time"] == 654
    finally:
        ShotManager._current_shot = None
