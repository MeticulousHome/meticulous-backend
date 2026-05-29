import copy
from datetime import datetime, timezone

import pytest

from config import (
    CONFIG_SYSTEM,
    CONFIG_USER,
    LAST_SYSTEM_VERSIONS,
    MeticulousConfig,
    UPDATE_CHANNEL,
)
from ota import UpdateManager


@pytest.fixture(autouse=True)
def restore_config_and_update_manager():
    original_config = copy.deepcopy(dict(MeticulousConfig))
    original_state = {
        "ROOTFS_BUILD_DATE": UpdateManager.ROOTFS_BUILD_DATE,
        "CHANNEL": UpdateManager.CHANNEL,
        "REPO_INFO": UpdateManager.REPO_INFO,
        "VERSION": UpdateManager.VERSION,
        "is_changed": UpdateManager.is_changed,
    }

    yield

    MeticulousConfig.clear()
    MeticulousConfig.update(original_config)
    for attr, value in original_state.items():
        setattr(UpdateManager, attr, value)


def test_init_updates_channel_to_new_image_channel(monkeypatch):
    set_channel_calls = []
    save_calls = []

    MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] = "stable"
    MeticulousConfig[CONFIG_SYSTEM][LAST_SYSTEM_VERSIONS] = ["stable-20260528_120000"]

    monkeypatch.setattr(UpdateManager, "getImageChannel", lambda: "iw612")
    monkeypatch.setattr(
        UpdateManager,
        "getBuildTimestamp",
        lambda: datetime(2026, 5, 29, 12, 0, 0, tzinfo=timezone.utc),
    )
    monkeypatch.setattr(
        UpdateManager, "setChannel", lambda channel: set_channel_calls.append(channel)
    )
    monkeypatch.setattr(MeticulousConfig, "save", lambda: save_calls.append(True))

    UpdateManager.init()

    assert MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] == "iw612"
    assert MeticulousConfig[CONFIG_SYSTEM][LAST_SYSTEM_VERSIONS][-1] == "iw612-20260529_120000"
    assert set_channel_calls == ["iw612"]
    assert UpdateManager.is_changed is True
    assert len(save_calls) == 2


def test_init_keeps_existing_channel_when_image_is_unchanged(monkeypatch):
    set_channel_calls = []
    save_calls = []

    MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] = "stable"
    MeticulousConfig[CONFIG_SYSTEM][LAST_SYSTEM_VERSIONS] = ["stable-20260529_120000"]

    monkeypatch.setattr(UpdateManager, "getImageChannel", lambda: "stable")
    monkeypatch.setattr(
        UpdateManager,
        "getBuildTimestamp",
        lambda: datetime(2026, 5, 29, 12, 0, 0, tzinfo=timezone.utc),
    )
    monkeypatch.setattr(
        UpdateManager, "setChannel", lambda channel: set_channel_calls.append(channel)
    )
    monkeypatch.setattr(MeticulousConfig, "save", lambda: save_calls.append(True))

    UpdateManager.init()

    assert MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] == "stable"
    assert MeticulousConfig[CONFIG_SYSTEM][LAST_SYSTEM_VERSIONS] == ["stable-20260529_120000"]
    assert set_channel_calls == ["stable"]
    assert UpdateManager.is_changed is False
    assert save_calls == []
