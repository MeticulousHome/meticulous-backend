"""Motor strain protection and profile streaming survive what the ESP sends.

A NaN pressure or flow sample must not kill the motor-strain check or leave a
gap that the next valid sample counts as energy, a start is refused while the
motor rests like a purge, and streaming a profile returns when the debug shot
never appears instead of freezing the API.
"""

import itertools
import threading
import time
from collections import deque
from types import SimpleNamespace

import machine as machine_module
import monitoring.motor_power_monitoring as monitoring
from esp_serial.data import SensorData, ShotData
from machine import AlarmManager, Machine
from monitoring.motor_power_monitoring import EnergyCalculator


def sample(flow, pressure):
    shot = object.__new__(ShotData)
    shot.flow = flow
    shot.pressure = pressure
    sensors = object.__new__(SensorData)
    sensors.motor_power = 100.0
    sensors.motor_current = 10.0
    return sensors, shot


def test_a_nan_sample_is_skipped_without_raising():
    calculator = object.__new__(EnergyCalculator)
    calculator.total_energy = 1.5
    calculator.window_seconds = 1000
    calculator.history = deque([(0, time.monotonic())])

    assert calculator.calculate_motor_energy(*sample("NaN", "NaN")) <= 1.5
    assert len(calculator.history) == 2


def test_the_first_sample_after_nan_does_not_count_the_gap(monkeypatch):
    clock = {"now": 100.0}
    monkeypatch.setattr(monitoring, "time", SimpleNamespace(monotonic=lambda: clock["now"]))
    calculator = object.__new__(EnergyCalculator)
    calculator.total_energy = 0.0
    calculator.window_seconds = 1000
    calculator.history = deque([(0, clock["now"])])

    clock["now"] += 50
    calculator.calculate_motor_energy(*sample("NaN", 1.0))
    clock["now"] += 0.1
    calculator.calculate_motor_energy(*sample(1.0, 1.0))

    assert calculator.total_energy < 5


def test_start_is_refused_while_the_motor_rests(monkeypatch):
    written = []
    monkeypatch.setattr(Machine, "profileReady", True)
    monkeypatch.setattr(Machine, "_stopESPcomm", False)
    monkeypatch.setattr(
        Machine, "_connection", SimpleNamespace(port=SimpleNamespace(write=written.append))
    )
    monkeypatch.setattr(AlarmManager, "is_alarm_set", lambda _alarm: time.time() + 600)
    monkeypatch.setattr(AlarmManager, "_notify_user", lambda **kwargs: None)
    monkeypatch.setattr(machine_module.sentry_sdk, "capture_message", lambda *args: None)

    assert Machine.action("start") is False
    assert written == []


def test_streaming_a_profile_returns_when_no_debug_shot_appears(monkeypatch):
    written = []
    ticks = itertools.count(step=0.5)
    monkeypatch.setattr(machine_module.ShotDebugManager, "_current_data", None)
    monkeypatch.setattr(Machine, "profileReady", False)
    monkeypatch.setattr(Machine, "write", written.append)
    monkeypatch.setattr(
        machine_module,
        "time",
        SimpleNamespace(
            time=time.time, monotonic=lambda: next(ticks), sleep=lambda seconds: None
        ),
    )

    caller = threading.Thread(
        target=Machine.send_json_with_hash, args=({"name": "x"},), daemon=True
    )
    caller.start()
    caller.join(timeout=5)
    returned = not caller.is_alive()
    # frees the caller if it is still waiting, so a failure does not leave it spinning
    machine_module.ShotDebugManager._current_data = SimpleNamespace(nodeJSON=None)
    caller.join(timeout=5)

    assert returned
    assert Machine.profileReady is True
    assert b"hash," in written[0]
