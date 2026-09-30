"""Bounded, allowlisted diagnostics; never read uploader credentials or queue bodies."""

import json
import os
import time
from pathlib import Path
from uuid import UUID

NAME = "community_upload_status.json"
MAX_BYTES = 16384
CATEGORIES = frozenset(
    [
        "upload_network",
        "token_network",
        "exchange_network",
        "disconnect_network",
        "machine_history_unavailable",
        "machine_pour_over_history_unavailable",
        "machine_history_invalid",
        "machine_history_too_large",
        "machine_history_incomplete",
        "machine_pour_over_history_invalid",
        "machine_pour_over_history_too_large",
        "machine_pour_over_history_incomplete",
        "response_read_failed",
        "response_too_large",
        "state_unavailable",
        "state_persist_failed",
        "queue_write_failed",
        "queue_capacity_reached",
        "queued_body_missing",
        "queued_body_too_large",
        "shot_file_pending",
        "shot_file_too_large",
        "shot_data_invalid",
        "shot_id_invalid",
        "history_path_invalid",
        "key_missing",
        "private_key_invalid",
        "header_invalid",
        "enrollment_missing",
        "pairing_code_expired",
        "invalid_exchange_body",
        "invalid_json",
        "invalid_payload",
        "expired_request",
        "replayed_request",
        "retired_or_revoked_access",
        "idempotency_payload_mismatch",
        "community_pour_over_not_ready",
        "pour_over_replay_scheduled",
        "pour_over_samples_invalid",
        "pour_over_duration_invalid",
        "pour_over_mode_invalid",
        "pour_over_pours_invalid",
        "pour_over_schema_invalid",
        "pour_over_targets_invalid",
        "pour_over_type_invalid",
        "upload_failed",
        "token_failed",
        "exchange_failed",
        "rate_limited",
        "other_error",
    ]
)


def _number(value):
    return value if type(value) is int and 0 <= value <= 2**63 - 1 else None


def _category(value):
    if value is None:
        return None
    return value if isinstance(value, str) and value in CATEGORIES else "other_error"


def collect():
    now = int(time.time())
    result = {"schemaVersion": 1, "collectedAt": now, "available": False}
    source = (
        Path(os.getenv("CONFIG_PATH", "/meticulous-user/config"))
        / "community-upload"
        / "diagnostics.json"
    )
    try:
        with source.open("rb") as stream:
            raw = stream.read(MAX_BYTES + 1)
        if len(raw) > MAX_BYTES:
            result["reason"] = "oversized"
            return result
        data = json.loads(raw)
        if (
            not isinstance(data, dict)
            or type(data.get("schemaVersion")) is not int
            or data["schemaVersion"] != 1
        ):
            result["reason"] = "unsupported_schema"
            return result
    except FileNotFoundError:
        result["reason"] = "missing"
        return result
    except (OSError, ValueError, UnicodeError):
        result["reason"] = "unreadable"
        return result
    result["available"] = True
    for key in ("capturedAt", "pendingCount", "lastSuccessAt", "lastRetryAt"):
        result[key] = _number(data.get(key))
    captured = result["capturedAt"]
    result["stale"] = captured is None or not 0 <= now - captured <= 120
    for key in ("connected", "paused"):
        result[key] = data.get(key) if type(data.get(key)) is bool else None
    result["lastError"] = _category(data.get("lastError"))
    recovery = data.get("recovery")
    result["recovery"] = {"available": False}
    if isinstance(recovery, dict) and recovery.get("available") is True:
        selected = {"available": True}
        state = recovery.get("state")
        selected["state"] = (
            state
            if state
            in (
                "idle",
                "pending",
                "scanning",
                "uploading",
                "completed",
                "paused",
                "failed",
                "interrupted",
                "running",
            )
            else None
        )
        for key in ("added", "alreadyPresent", "preservedDeleted", "failed", "pendingCount"):
            selected[key] = _number(recovery.get(key))
        selected["lastError"] = _category(recovery.get("lastError"))
        result["recovery"] = selected
    failure = data.get("lastFailure")
    result["lastFailure"] = None
    if isinstance(failure, dict):
        status = _number(failure.get("httpStatus"))
        selected = {
            "category": _category(failure.get("category")),
            "occurredAt": _number(failure.get("occurredAt")),
            "httpStatus": status if status is not None and 100 <= status <= 599 else None,
            "retryAfterSeconds": _number(failure.get("retryAfterSeconds")),
            "requestId": None,
        }
        try:
            selected["requestId"] = str(UUID(failure.get("requestId")))
        except (ValueError, TypeError, AttributeError):
            pass
        result["lastFailure"] = selected
    return result
