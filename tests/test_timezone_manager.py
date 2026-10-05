import asyncio

import pytest

from config import CONFIG_USER, TIME_ZONE, TIMEZONE_SYNC, MeticulousConfig
from timezone_manager import TimezoneManager


class FakeEventLoop:
    def __init__(self, timezone, before_return=None):
        self.timezone = timezone
        self.before_return = before_return

    def run_until_complete(self, request):
        assert request is REQUEST_SENTINEL
        if self.before_return:
            self.before_return()
        TimezoneManager._TimezoneManager__system_synced = True
        return self.timezone


REQUEST_SENTINEL = object()


@pytest.fixture(autouse=True)
def restore_timezone_state():
    original_timezone = MeticulousConfig[CONFIG_USER][TIME_ZONE]
    original_sync_mode = MeticulousConfig[CONFIG_USER][TIMEZONE_SYNC]
    original_synced = TimezoneManager._TimezoneManager__system_synced
    original_attempts = TimezoneManager._TimezoneManager__timezone_fetch_attempts

    yield

    MeticulousConfig[CONFIG_USER][TIME_ZONE] = original_timezone
    MeticulousConfig[CONFIG_USER][TIMEZONE_SYNC] = original_sync_mode
    TimezoneManager._TimezoneManager__system_synced = original_synced
    TimezoneManager._TimezoneManager__timezone_fetch_attempts = original_attempts


def configure_background_sync(monkeypatch, timezone, before_return=None):
    MeticulousConfig[CONFIG_USER][TIMEZONE_SYNC] = "automatic"
    TimezoneManager._TimezoneManager__system_synced = False
    TimezoneManager._TimezoneManager__timezone_fetch_attempts = 0
    monkeypatch.setattr(TimezoneManager, "request_and_sync_tz", lambda: REQUEST_SENTINEL)
    monkeypatch.setattr(
        asyncio,
        "get_event_loop",
        lambda: FakeEventLoop(timezone, before_return),
    )


def test_background_sync_persists_detected_timezone(monkeypatch):
    save_calls = []
    configure_background_sync(monkeypatch, "Europe/Zurich")
    MeticulousConfig[CONFIG_USER][TIME_ZONE] = "Etc/UTC"
    monkeypatch.setattr(MeticulousConfig, "save", lambda: save_calls.append(True))

    TimezoneManager.tz_background_update()

    assert MeticulousConfig[CONFIG_USER][TIME_ZONE] == "Europe/Zurich"
    assert save_calls == [True]
    assert TimezoneManager._TimezoneManager__system_synced is True
    assert TimezoneManager._TimezoneManager__timezone_fetch_attempts == 0


def test_background_sync_retries_when_persistence_fails(monkeypatch):
    configure_background_sync(monkeypatch, "Europe/Zurich")
    MeticulousConfig[CONFIG_USER][TIME_ZONE] = "Etc/UTC"

    def fail_save():
        raise OSError("disk unavailable")

    monkeypatch.setattr(MeticulousConfig, "save", fail_save)

    TimezoneManager.tz_background_update()

    assert MeticulousConfig[CONFIG_USER][TIME_ZONE] == "Etc/UTC"
    assert TimezoneManager._TimezoneManager__system_synced is False
    assert TimezoneManager._TimezoneManager__timezone_fetch_attempts == 1


def test_background_sync_does_not_overwrite_manual_change(monkeypatch):
    restored_timezones = []

    def switch_to_manual():
        MeticulousConfig[CONFIG_USER][TIMEZONE_SYNC] = "manual"
        MeticulousConfig[CONFIG_USER][TIME_ZONE] = "Europe/London"

    configure_background_sync(monkeypatch, "Europe/Zurich", switch_to_manual)
    MeticulousConfig[CONFIG_USER][TIME_ZONE] = "Etc/UTC"
    monkeypatch.setattr(
        TimezoneManager,
        "set_system_timezone",
        lambda timezone: restored_timezones.append(timezone),
    )

    TimezoneManager.tz_background_update()

    assert MeticulousConfig[CONFIG_USER][TIME_ZONE] == "Europe/London"
    assert restored_timezones == ["Europe/London"]
    assert TimezoneManager._TimezoneManager__system_synced is False
