import sys
import types

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

import pytest

from api import tests as hardware_tests
from sounds import SoundPlayer


class FakeSound:
    """Stands in for a playsound3 Sound that stays alive for a number of polls."""

    def __init__(self, alive_polls):
        self.alive_polls = alive_polls

    def is_alive(self):
        if self.alive_polls <= 0:
            return False
        self.alive_polls -= 1
        return True

    def stop(self):
        self.alive_polls = 0


@pytest.fixture(autouse=True)
def no_current_sound(monkeypatch):
    monkeypatch.setattr(SoundPlayer, "_current_sound", None)


def test_is_playing_follows_the_current_sound(monkeypatch):
    assert SoundPlayer.is_playing() is False

    sound = FakeSound(alive_polls=1)
    monkeypatch.setattr(SoundPlayer, "_current_sound", sound)

    assert SoundPlayer.is_playing() is True
    assert SoundPlayer.is_playing() is False


def test_speaker_test_outlives_the_sound_and_restores_volume(monkeypatch):
    sound = FakeSound(alive_polls=20)
    played = []

    def play_sound(name):
        played.append(name)
        SoundPlayer._current_sound = sound
        return True

    clock = [0.0]

    def sleep(seconds):
        clock[0] += seconds

    volumes = []
    monkeypatch.setattr(SoundPlayer, "play_sound", play_sound)
    monkeypatch.setattr(
        hardware_tests, "time", types.SimpleNamespace(monotonic=lambda: clock[0], sleep=sleep)
    )
    monkeypatch.setattr(
        hardware_tests.subprocess, "run", lambda cmd, check: volumes.append(cmd[-1])
    )

    hardware_tests.TestsHandler.test_speaker(types.SimpleNamespace())

    assert played == ["speaker_test"]
    # The test thread must not return while gst-play is still running: playsound3
    # kills the player as soon as the thread that started it exits.
    assert sound.alive_polls == 0
    assert volumes == ["50%", "70%"]
