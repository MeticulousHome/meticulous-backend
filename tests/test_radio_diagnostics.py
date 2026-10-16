import json
from unittest.mock import MagicMock

from radio_diagnostics import emit_radio_recovery_event, safe_code


def test_safe_code_rejects_network_names_and_credentials():
    assert safe_code("gateway_unreachable") == "gateway_unreachable"
    assert safe_code("hotspot_start_failed: details") == "hotspot_start_failed"
    assert safe_code("Private SSID / secret-password") == "other"


def test_radio_event_contains_only_bounded_structured_metadata():
    logger = MagicMock()
    secret = "SENTINEL-PRIVATE-NETWORK-PASSWORD"

    emit_radio_recovery_event(
        logger,
        "step_exception",
        operation_id="abc123",
        elapsed_ms=1250,
        reason=secret,
        mode="client",
        connected=True,
        has_ipv4=False,
        action="restart_wifi_radio",
        error=RuntimeError(secret),
    )

    message = logger.info.call_args.args[0]
    assert secret not in message
    prefix, encoded = message.split(" ", 1)
    payload = json.loads(encoded)
    assert prefix == "RADIO_RECOVERY"
    assert payload == {
        "action": "restart_wifi_radio",
        "connected": True,
        "elapsed_ms": 1250,
        "error_type": "RuntimeError",
        "event": "step_exception",
        "has_ipv4": False,
        "mode": "client",
        "operation_id": "abc123",
        "reason": "other",
        "schema_version": 1,
    }
