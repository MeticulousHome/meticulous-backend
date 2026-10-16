import copy

import yaml

from config import (
    CONFIG_USER,
    DefaultConfiguration_V1,
    MeticulousConfigDict,
    PROFILE_PARTIAL_RETRACTION,
    PROFILE_PARTIAL_RETRACTION_DEFAULT,
    PROFILE_PARTIAL_RETRACTION_MIN,
    PROFILE_TARE_BEHAVIOR,
    PROFILE_TARE_BEHAVIOR_DEFAULT,
)


def test_load_removes_retired_telemetry_settings(tmp_path):
    config_path = tmp_path / "config.yml"
    disk_config = copy.deepcopy(DefaultConfiguration_V1)
    disk_config[CONFIG_USER]["telemetry_service_enabled"] = True
    disk_config[CONFIG_USER]["allow_debug_sending"] = True
    config_path.write_text(yaml.safe_dump(disk_config), encoding="utf-8")

    config = MeticulousConfigDict(config_path, copy.deepcopy(DefaultConfiguration_V1))

    assert "telemetry_service_enabled" not in config[CONFIG_USER]
    assert "allow_debug_sending" not in config[CONFIG_USER]
    persisted_config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    assert "telemetry_service_enabled" not in persisted_config[CONFIG_USER]
    assert "allow_debug_sending" not in persisted_config[CONFIG_USER]


def _load_config(tmp_path, user_updates):
    config_path = tmp_path / "config.yml"
    disk_config = copy.deepcopy(DefaultConfiguration_V1)
    disk_config[CONFIG_USER].update(user_updates)
    config_path.write_text(yaml.safe_dump(disk_config), encoding="utf-8")
    return MeticulousConfigDict(config_path, copy.deepcopy(DefaultConfiguration_V1))


def test_load_migrates_legacy_partial_retraction_default(tmp_path):
    config = _load_config(tmp_path, {PROFILE_PARTIAL_RETRACTION: 45.0})

    assert config[CONFIG_USER][PROFILE_PARTIAL_RETRACTION] == (
        PROFILE_PARTIAL_RETRACTION_DEFAULT
    )


def test_load_preserves_supported_custom_partial_retraction(tmp_path):
    config = _load_config(tmp_path, {PROFILE_PARTIAL_RETRACTION: 40.25})

    assert config[CONFIG_USER][PROFILE_PARTIAL_RETRACTION] == 40.25


def test_load_clamps_legacy_partial_retraction_below_supported_range(tmp_path):
    config = _load_config(tmp_path, {PROFILE_PARTIAL_RETRACTION: 20.0})

    assert config[CONFIG_USER][PROFILE_PARTIAL_RETRACTION] == (PROFILE_PARTIAL_RETRACTION_MIN)


def test_load_replaces_unknown_tare_behavior(tmp_path):
    config = _load_config(tmp_path, {PROFILE_TARE_BEHAVIOR: "unexpected"})

    assert config[CONFIG_USER][PROFILE_TARE_BEHAVIOR] == PROFILE_TARE_BEHAVIOR_DEFAULT


def test_load_replaces_non_string_tare_behavior(tmp_path):
    config = _load_config(tmp_path, {PROFILE_TARE_BEHAVIOR: ["before_retraction"]})

    assert config[CONFIG_USER][PROFILE_TARE_BEHAVIOR] == PROFILE_TARE_BEHAVIOR_DEFAULT


def test_load_adds_default_tare_behavior_to_older_config(tmp_path):
    config_path = tmp_path / "config.yml"
    disk_config = copy.deepcopy(DefaultConfiguration_V1)
    disk_config[CONFIG_USER].pop(PROFILE_TARE_BEHAVIOR)
    config_path.write_text(yaml.safe_dump(disk_config), encoding="utf-8")

    config = MeticulousConfigDict(config_path, copy.deepcopy(DefaultConfiguration_V1))

    assert config[CONFIG_USER][PROFILE_TARE_BEHAVIOR] == PROFILE_TARE_BEHAVIOR_DEFAULT
