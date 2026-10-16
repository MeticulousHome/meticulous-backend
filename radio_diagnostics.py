"""Structured, credential-free diagnostics for shared radio recovery."""

import json

_SAFE_CODES = {
    "already_in_progress",
    "ap",
    "apply_ap_mode",
    "client",
    "completed",
    "dns_unreachable",
    "driver_in_band_reset",
    "failed",
    "gateway_unreachable",
    "health_assessed",
    "healthy",
    "hotspot_not_active",
    "hotspot_start_failed",
    "hotspot_stop_failed",
    "internet_unreachable",
    "manual",
    "missing_ipv4",
    "networking_unavailable",
    "not_needed",
    "not_recoverable",
    "recovered",
    "rejected",
    "restart_connection",
    "restart_wifi_radio",
    "restart_wifi_service",
    "started",
    "start_hotspot",
    "step_assessed",
    "step_exception",
    "step_started",
    "wifi_device_unavailable",
    "wifi_not_connected",
}


def safe_code(value) -> str:
    candidate = str(value or "unknown").split(":", 1)[0].strip().lower()
    return candidate if candidate in _SAFE_CODES else "other"


def emit_radio_recovery_event(
    logger,
    event: str,
    *,
    operation_id: str,
    elapsed_ms: int,
    reason=None,
    mode=None,
    connected=None,
    has_ipv4=None,
    action=None,
    result=None,
    error=None,
):
    payload = {
        "schema_version": 1,
        "event": safe_code(event),
        "operation_id": operation_id,
        "elapsed_ms": max(0, int(elapsed_ms)),
    }
    optional = {
        "reason": safe_code(reason) if reason is not None else None,
        "mode": safe_code(mode) if mode is not None else None,
        "connected": bool(connected) if connected is not None else None,
        "has_ipv4": bool(has_ipv4) if has_ipv4 is not None else None,
        "action": safe_code(action) if action is not None else None,
        "result": safe_code(result) if result is not None else None,
        "error_type": type(error).__name__ if error is not None else None,
    }
    payload.update({key: value for key, value in optional.items() if value is not None})
    logger.info(f"RADIO_RECOVERY {json.dumps(payload, sort_keys=True)}")
