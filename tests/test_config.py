import copy

import yaml

from config import CONFIG_USER, DefaultConfiguration_V1, MeticulousConfigDict


def test_load_removes_retired_telemetry_setting(tmp_path):
    config_path = tmp_path / "config.yml"
    disk_config = copy.deepcopy(DefaultConfiguration_V1)
    disk_config[CONFIG_USER]["telemetry_service_enabled"] = True
    config_path.write_text(yaml.safe_dump(disk_config), encoding="utf-8")

    config = MeticulousConfigDict(config_path, copy.deepcopy(DefaultConfiguration_V1))

    assert "telemetry_service_enabled" not in config[CONFIG_USER]
    persisted_config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    assert "telemetry_service_enabled" not in persisted_config[CONFIG_USER]
