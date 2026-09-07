"""Build privacy-reviewed configuration payloads for diagnostic reporting."""

from collections.abc import Mapping
from copy import deepcopy
from typing import Any, Callable

Validator = Callable[[Any], bool]


def _is_bool(value: Any) -> bool:
    return isinstance(value, bool)


def _is_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _is_string(value: Any) -> bool:
    return isinstance(value, str)


def _is_list(value: Any) -> bool:
    return isinstance(value, list)


_APPROVED_PATHS: tuple[tuple[tuple[str, ...], Validator], ...] = (
    (("version",), _is_int),
    (("logging", "log_all_sensor_messages"), _is_bool),
    (("system", "notifications_ttl"), _is_int),
    (("system", "serial"), _is_string),
    (("system", "batch_number"), _is_string),
    (("system", "build_date"), _is_string),
    (("system", "color"), _is_string),
    (("system", "last_system_versions"), _is_list),
    (("user", "enable_sounds"), _is_bool),
    (("user", "disallow_firmware_flashing"), _is_bool),
    (("user", "debug_shot_data_retention_days"), _is_int),
    (("user", "auto_start_shot"), _is_bool),
    (("user", "auto_purge_after_shot"), _is_bool),
    (("user", "tare_behavior"), _is_string),
    (("user", "partial_retraction"), _is_number),
    (("user", "heat_on_boot"), _is_bool),
    (("user", "heating_timeout"), _is_number),
    (("user", "update_channel"), _is_string),
    (("user", "idle_screen"), _is_string),
    (("user", "reverse_scrolling", "home"), _is_bool),
    (("user", "reverse_scrolling", "keyboard"), _is_bool),
    (("user", "reverse_scrolling", "menus"), _is_bool),
    (("user", "clock_format_24_hour"), _is_bool),
    (("user", "allow_legacy_json"), _is_bool),
    (("user", "allow_stage_skipping"), _is_bool),
    (("user", "usb_mode"), _is_string),
    (("user", "timezone_sync"), _is_string),
    (("user", "ssh_enabled"), _is_bool),
    (("wifi", "mode"), _is_string),
    (("manufacturing", "enabled"), _is_bool),
    (("manufacturing", "last_boot_mode"), _is_string),
    (("manufacturing", "skip_stage"), _is_bool),
)


def _read_path(config: Mapping[str, Any], path: tuple[str, ...]) -> tuple[bool, Any]:
    current: Any = config
    for part in path:
        if not isinstance(current, Mapping) or part not in current:
            return False, None
        current = current[part]
    return True, current


def _write_path(output: dict[str, Any], path: tuple[str, ...], value: Any) -> None:
    current = output
    for part in path[:-1]:
        current = current.setdefault(part, {})
    current[path[-1]] = deepcopy(value)


def get_reportable_config(config: Mapping[str, Any]) -> dict[str, Any]:
    """Return a new mapping containing only approved, type-valid config values."""

    if not isinstance(config, Mapping):
        return {}

    output: dict[str, Any] = {}
    for path, validator in _APPROVED_PATHS:
        found, value = _read_path(config, path)
        if found and validator(value):
            _write_path(output, path, value)

    found, sounds_theme = _read_path(config, ("system", "sounds_theme"))
    if found and isinstance(sounds_theme, str):
        _write_path(
            output,
            ("system", "sounds_theme"),
            "default" if sounds_theme == "default" else "custom",
        )

    return output
