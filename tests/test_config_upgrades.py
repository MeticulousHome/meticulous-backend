"""The config file a machine already has, loaded by this release.

An OTA update keeps /meticulous-user/config/config.yml, so a release must load
what older releases wrote (missing keys), what newer ones wrote (after a
rollback: unknown keys, a higher version) and a file damaged by a power cut,
without losing the user's settings.
"""

import copy

import yaml

from config import (
    CONFIG_SYSTEM,
    CONFIG_USER,
    CONFIG_WIFI,
    MACHINE_HEAT_ON_BOOT,
    SOUNDS_ENABLED,
    WIFI_MODE,
    WIFI_MODE_CLIENT,
    DefaultConfiguration_V1,
    MeticulousConfigDict,
)


def _load(path):
    return MeticulousConfigDict(path, copy.deepcopy(DefaultConfiguration_V1))


def test_an_older_config_gains_every_new_default_and_keeps_the_users_values(tmp_path):
    path = tmp_path / "config.yml"
    # Values that differ from the defaults, so keeping them is observable.
    old_user = {
        SOUNDS_ENABLED: not DefaultConfiguration_V1[CONFIG_USER][SOUNDS_ENABLED],
        MACHINE_HEAT_ON_BOOT: not DefaultConfiguration_V1[CONFIG_USER][MACHINE_HEAT_ON_BOOT],
    }
    path.write_text(
        yaml.safe_dump(
            {"version": 1, CONFIG_USER: old_user, CONFIG_WIFI: {WIFI_MODE: WIFI_MODE_CLIENT}}
        )
    )

    config = _load(path)

    assert not config.hasError()
    for section, defaults in DefaultConfiguration_V1.items():
        if isinstance(defaults, dict):
            assert set(defaults) <= set(config[section]), section
    for key, value in old_user.items():
        assert config[CONFIG_USER][key] == value
    assert config[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_CLIENT
    assert (
        yaml.safe_load(path.read_text())[CONFIG_USER].keys()
        >= DefaultConfiguration_V1[CONFIG_USER].keys()
    )


def test_a_config_from_a_newer_release_keeps_its_unknown_settings(tmp_path):
    path = tmp_path / "config.yml"
    newer = copy.deepcopy(DefaultConfiguration_V1)
    newer["version"] = DefaultConfiguration_V1["version"] + 1
    newer[CONFIG_USER]["setting_from_the_future"] = "kept"
    path.write_text(yaml.safe_dump(newer))

    config = _load(path)

    assert not config.hasError()
    assert config[CONFIG_USER]["setting_from_the_future"] == "kept"
    assert yaml.safe_load(path.read_text())[CONFIG_USER]["setting_from_the_future"] == "kept"


def test_a_damaged_config_is_set_aside_and_replaced_by_defaults(tmp_path):
    path = tmp_path / "config.yml"
    damaged = "user: {enable_sounds: [unterminated\n"
    path.write_text(damaged)

    config = _load(path)

    assert config.hasError()
    backups = list(tmp_path.glob("config_broken_*.yml"))
    assert len(backups) == 1
    assert backups[0].read_text() == damaged
    assert config[CONFIG_USER] == DefaultConfiguration_V1[CONFIG_USER]
    assert yaml.safe_load(path.read_text())[CONFIG_SYSTEM].keys() == (
        DefaultConfiguration_V1[CONFIG_SYSTEM].keys()
    )


def test_an_empty_config_file_is_treated_as_damaged(tmp_path):
    # What a power cut during a write can leave behind.
    path = tmp_path / "config.yml"
    path.write_text("")

    config = _load(path)

    assert config.hasError()
    assert len(list(tmp_path.glob("config_broken_*.yml"))) == 1
    assert config[CONFIG_USER] == DefaultConfiguration_V1[CONFIG_USER]
