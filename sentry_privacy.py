"""Privacy controls shared by automatic backend and ESP Sentry events."""

from collections.abc import Mapping
from copy import deepcopy
from typing import Any

from reportable_config import get_reportable_config

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
            # ESP diagnostics are firmware-controlled hardware data. Preserve
            # every field so new diagnostics do not require a backend update.
            clean_contexts["esp-data"] = deepcopy(raw_esp_data)
        sanitized["contexts"] = clean_contexts

    if "extra" in sanitized:
        sanitized["extra"] = _scrub_structured(sanitized["extra"])

    _remove_frame_variables(sanitized)
    return sanitized
