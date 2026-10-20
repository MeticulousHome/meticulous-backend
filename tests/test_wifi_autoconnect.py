"""Regression tests for the Wi-Fi auto-connect background loop.

It is the only thing that rejoins a known network after the machine drops off
Wi-Fi, so it must outlive any single NetworkManager or D-Bus error.
"""

from itertools import chain, repeat
from unittest.mock import MagicMock, patch

import pytest

from tests.test_wifi_repair import _load_wifi_module, _wifi_config


class _StopLoop(BaseException):
    pass


def _run_autoconnect(module, seconds):
    """Run the auto-connect loop on a fake clock until `seconds` have passed."""
    clock = {"now": 1_000_000.0}
    deadline = clock["now"] + seconds

    def sleep(duration):
        clock["now"] += duration
        if clock["now"] > deadline:
            raise _StopLoop()

    fake_time = MagicMock()
    fake_time.time.side_effect = lambda: clock["now"]
    fake_time.sleep.side_effect = sleep
    with (
        patch.object(module, "time", fake_time),
        patch.object(module, "asyncio"),
        pytest.raises(_StopLoop),
    ):
        module.WifiManager.tryAutoConnect()


def test_autoconnect_keeps_running_after_a_networkmanager_error(monkeypatch):
    """One failed nmcli/D-Bus call ended the auto-connect thread for good, so a
    machine that later dropped off Wi-Fi never rejoined until a reboot. The loop
    must try again on the next tick and go on looking for known networks."""
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    module.Machine.enable_manufacturing = False
    disconnected = _wifi_config(module, connected=False, connection_name=None)
    configs = chain([RuntimeError("D-Bus timeout")], repeat(disconnected, 10), [_StopLoop()])

    with (
        patch.object(manager, "getCurrentConfig", side_effect=configs),
        patch.object(manager, "scanForNetworks", return_value=[]) as scan,
    ):
        _run_autoconnect(module, seconds=30)

    scan.assert_called()


def test_persistent_error_is_retried_once_per_tick_and_logged_once_per_streak(monkeypatch):
    """A NetworkManager that keeps failing must neither spin the loop nor log a
    traceback every tick. Expected values come from the 10 s tick sleep in
    tryAutoConnect: six ticks in 60 s, and an ERROR for each of the two streaks."""
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    module.Machine.enable_manufacturing = False
    disconnected = _wifi_config(module, connected=False, connection_name=None)
    error = RuntimeError("D-Bus timeout")
    # The trailing stop turns a loop that never sleeps into a failure instead of a hang.
    script = [error, error, error, disconnected, error]
    configs = chain(script, repeat(disconnected, 10), [_StopLoop()])

    with (
        patch.object(manager, "getCurrentConfig", side_effect=configs) as get_config,
        patch.object(manager, "scanForNetworks", return_value=[]),
    ):
        _run_autoconnect(module, seconds=60)

    assert get_config.call_count == 6
    assert module.logger.error.call_count == 2
