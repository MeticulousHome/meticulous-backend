"""Privacy controls shared by automatic backend and ESP Sentry events."""

from collections.abc import Mapping
from copy import deepcopy
import re
from typing import Any

from reportable_config import get_reportable_config


ESP_DATA_KEYS = {
    "temperature",
    "status",
    "value_in_curve",
    "ref_id",
    "time_series_index",
    "context",
    "control_type",
    "reason",
    "requested",
    "applied",
    "stage_name",
    "weight",
    "threshold",
    "retract_name",
    "requested_delta_mm",
    "start_position",
    "target_position",
    "current_position",
    "piston_speed",
    "stall_timeout_ms",
}

_SENSITIVE_KEY_PARTS = {
    "password",
    "passwd",
    "secret",
    "token",
    "credential",
    "authorization",
    "cookie",
    "api_key",
    "auth_key",
    "root_password",
    "ssid",
    "apname",
    "knownwifis",
    "ip",
    "ip_address",
    "ipaddress",
    "client_ip",
    "remote_addr",
    "hostname",
    "machine_name",
    "email",
    "username",
}
_TECHNICAL_VALUE = re.compile(r"^[A-Za-z0-9_.:+\- ]{1,128}$")
_CONTROL_CHARACTER = re.compile(r"[\x00-\x1f\x7f]")


def drop_breadcrumb(crumb: dict[str, Any], hint: dict[str, Any]) -> None:
    """Disable every automatic breadcrumb."""

    return None


def _normalized_key(key: Any) -> str:
    return str(key).strip().lower().replace("-", "_").replace(" ", "_")


def _is_sensitive_key(key: Any) -> bool:
    normalized = _normalized_key(key)
    return normalized in _SENSITIVE_KEY_PARTS or any(
        normalized.startswith(f"{part}_")
        or normalized.endswith(f"_{part}")
        or f"_{part}_" in normalized
        for part in _SENSITIVE_KEY_PARTS
    )


def _scrub_structured(value: Any) -> Any:
    if isinstance(value, Mapping):
        return {
            key: _scrub_structured(item)
            for key, item in value.items()
            if not _is_sensitive_key(key)
        }
    if isinstance(value, list):
        return [_scrub_structured(item) for item in value]
    if isinstance(value, tuple):
        return tuple(_scrub_structured(item) for item in value)
    return value


def _bounded_stage_name(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    sanitized = _CONTROL_CHARACTER.sub("", value).strip()
    if not sanitized:
        return None
    return sanitized[:128]


def sanitize_esp_data(data: Any) -> dict[str, str]:
    """Keep only reviewed UART diagnostic fields and safe scalar values."""

    if not isinstance(data, Mapping):
        return {}

    output: dict[str, str] = {}
    for key, value in data.items():
        if key not in ESP_DATA_KEYS or not isinstance(value, str):
            continue
        if key == "stage_name":
            stage_name = _bounded_stage_name(value)
            if stage_name is not None:
                output[key] = stage_name
            continue
        if _TECHNICAL_VALUE.fullmatch(value):
            output[key] = value
    return output


def _remove_frame_variables(event: dict[str, Any]) -> None:
    exception = event.get("exception")
    if not isinstance(exception, Mapping):
        return
    values = exception.get("values")
    if not isinstance(values, list):
        return
    for exception_value in values:
        if not isinstance(exception_value, dict):
            continue
        stacktrace = exception_value.get("stacktrace")
        if not isinstance(stacktrace, Mapping):
            continue
        frames = stacktrace.get("frames")
        if not isinstance(frames, list):
            continue
        for frame in frames:
            if isinstance(frame, dict):
                frame.pop("vars", None)


def sanitize_sentry_event(event: dict[str, Any], hint: Any = None) -> dict[str, Any]:
    """Return a sanitized copy of an automatic Sentry event."""

    sanitized = deepcopy(event)
    sanitized.pop("breadcrumbs", None)
    sanitized.pop("request", None)
    sanitized.pop("user", None)
    sanitized.pop("server_name", None)

    tags = sanitized.get("tags")
    if isinstance(tags, Mapping):
        sanitized["tags"] = {
            key: value
            for key, value in tags.items()
            if _normalized_key(key) not in {"machine", "machine_name", "hostname"}
            and not _is_sensitive_key(key)
        }

    contexts = sanitized.get("contexts")
    if isinstance(contexts, Mapping):
        clean_contexts = _scrub_structured(contexts)
        raw_config = contexts.get("config")
        if isinstance(raw_config, Mapping):
            clean_contexts["config"] = get_reportable_config(raw_config)
        raw_esp_data = contexts.get("esp-data")
        if raw_esp_data is not None:
            clean_contexts["esp-data"] = sanitize_esp_data(raw_esp_data)
        sanitized["contexts"] = clean_contexts

    if "extra" in sanitized:
        sanitized["extra"] = _scrub_structured(sanitized["extra"])

    _remove_frame_variables(sanitized)
    return sanitized
