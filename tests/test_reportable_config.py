from reportable_config import get_reportable_config


def _leaf_paths(value, prefix=()):
    if not isinstance(value, dict):
        return {prefix}
    return {path for key, item in value.items() for path in _leaf_paths(item, (*prefix, key))}


def test_reportable_config_is_allowlist_only_and_does_not_mutate_input():
    config = {
        "version": 1,
        "logging": {"log_all_sensor_messages": True, "unknown": "drop"},
        "system": {
            "notifications_ttl": 3600,
            "sounds_theme": "my-custom-theme",
            "serial": "M123",
            "batch_number": "B1",
            "build_date": "2026-01-02",
            "color": "silver",
            "last_system_versions": ["1.0", "0.9"],
            "machine_name": ["Private", "Name"],
            "root_password": "secret",
            "auth_key": "secret",
        },
        "user": {
            "enable_sounds": True,
            "disallow_firmware_flashing": False,
            "debug_shot_data_retention_days": 31,
            "auto_start_shot": False,
            "auto_purge_after_shot": True,
            "partial_retraction": 45.0,
            "heat_on_boot": True,
            "heating_timeout": 10,
            "update_channel": "stable",
            "idle_screen": "default",
            "reverse_scrolling": {
                "home": True,
                "keyboard": False,
                "menus": True,
                "future": True,
            },
            "clock_format_24_hour": True,
            "allow_legacy_json": False,
            "allow_stage_skipping": False,
            "usb_mode": "host",
            "timezone_sync": "automatic",
            "ssh_enabled": False,
            "disable_ui_features": True,
            "time_zone": "America/Mexico_City",
        },
        "wifi": {
            "mode": "CLIENT",
            "APName": "Private SSID",
            "APPassword": "secret",
            "KnownWifis": {"Private SSID": {"password": "secret"}},
        },
        "manufacturing": {
            "enabled": False,
            "last_boot_mode": "normal",
            "skip_stage": False,
        },
        "future": {"secret": "drop"},
    }

    reportable = get_reportable_config(config)

    assert reportable["system"]["sounds_theme"] == "custom"
    assert reportable["wifi"] == {"mode": "CLIENT"}
    assert reportable["manufacturing"] == {
        "enabled": False,
        "last_boot_mode": "normal",
        "skip_stage": False,
    }
    assert "machine_name" not in reportable["system"]
    assert "root_password" not in reportable["system"]
    assert "disable_ui_features" not in reportable["user"]
    assert "time_zone" not in reportable["user"]
    assert "future" not in reportable
    assert _leaf_paths(reportable) == {
        ("version",),
        ("logging", "log_all_sensor_messages"),
        ("system", "notifications_ttl"),
        ("system", "sounds_theme"),
        ("system", "serial"),
        ("system", "batch_number"),
        ("system", "build_date"),
        ("system", "color"),
        ("system", "last_system_versions"),
        ("user", "enable_sounds"),
        ("user", "disallow_firmware_flashing"),
        ("user", "debug_shot_data_retention_days"),
        ("user", "auto_start_shot"),
        ("user", "auto_purge_after_shot"),
        ("user", "partial_retraction"),
        ("user", "heat_on_boot"),
        ("user", "heating_timeout"),
        ("user", "update_channel"),
        ("user", "idle_screen"),
        ("user", "reverse_scrolling", "home"),
        ("user", "reverse_scrolling", "keyboard"),
        ("user", "reverse_scrolling", "menus"),
        ("user", "clock_format_24_hour"),
        ("user", "allow_legacy_json"),
        ("user", "allow_stage_skipping"),
        ("user", "usb_mode"),
        ("user", "timezone_sync"),
        ("user", "ssh_enabled"),
        ("wifi", "mode"),
        ("manufacturing", "enabled"),
        ("manufacturing", "last_boot_mode"),
        ("manufacturing", "skip_stage"),
    }

    reportable["system"]["last_system_versions"].append("changed")
    assert config["system"]["last_system_versions"] == ["1.0", "0.9"]


def test_reportable_config_normalizes_default_theme():
    assert get_reportable_config({"system": {"sounds_theme": "default"}}) == {
        "system": {"sounds_theme": "default"}
    }


def test_reportable_config_omits_invalid_types_and_non_mapping_input():
    assert (
        get_reportable_config(
            {
                "version": True,
                "system": {"notifications_ttl": "3600", "sounds_theme": None},
                "user": {"enable_sounds": 1, "partial_retraction": True},
            }
        )
        == {}
    )
    assert get_reportable_config(None) == {}
