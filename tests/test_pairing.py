import re

import pytest

import pairing
from config import CONFIG_PAIRED_DEVICES, MeticulousConfig
from pairing import PairingError, PairingManager, PairingStatus, hash_token


@pytest.fixture
def manager(monkeypatch):
    # Isolate from disk: pairing persists via MeticulousConfig.save(); make it a
    # no-op and start from an empty paired-devices section every test.
    monkeypatch.setattr(MeticulousConfig, "save", lambda: None)
    MeticulousConfig[CONFIG_PAIRED_DEVICES] = {}
    return PairingManager()


def _approve_and_get_token(manager, device_name="Alu's iPhone"):
    req = manager.request_pairing(device_name)
    assert manager.approve(req["pairing_id"]) is True
    result = manager.poll(req["pairing_id"])
    assert result["status"] == PairingStatus.APPROVED
    return req, result["token"]


def test_request_pairing_returns_id_and_six_digit_code(manager):
    req = manager.request_pairing("Test Device")
    assert req["pairing_id"]
    assert re.fullmatch(r"\d{6}", req["code"])
    assert req["expires_in"] == pairing.PAIRING_SESSION_TTL_SECONDS
    # The prompt the Dial renders matches the session.
    prompt = manager.get_pending_prompt(req["pairing_id"])
    assert prompt == {"device_name": "Test Device", "code": req["code"]}


def test_approved_token_is_delivered_once_then_verifies(manager):
    req, token = _approve_and_get_token(manager)
    # Token is delivered exactly once.
    assert manager.poll(req["pairing_id"]) == {"status": PairingStatus.APPROVED}
    # And it authenticates.
    device_id = manager.verify_token(token)
    assert device_id is not None
    # A wrong token does not.
    assert manager.verify_token(token + "x") is None
    assert manager.verify_token("") is None
    assert manager.verify_token(None) is None


def test_only_hash_is_stored_never_plaintext(manager):
    _req, token = _approve_and_get_token(manager)
    devices = MeticulousConfig[CONFIG_PAIRED_DEVICES]
    (record,) = devices.values()
    assert record["token_hash"] == hash_token(token)
    assert token not in str(devices)


def test_deny_yields_no_token(manager):
    req = manager.request_pairing("Bad Device")
    assert manager.deny(req["pairing_id"]) is True
    assert manager.poll(req["pairing_id"]) == {"status": PairingStatus.DENIED}
    assert MeticulousConfig[CONFIG_PAIRED_DEVICES] == {}


def test_expired_session_cannot_be_approved(manager, monkeypatch):
    req = manager.request_pairing("Slow Device")
    session = manager._sessions[req["pairing_id"]]
    # Move the session's creation past the TTL.
    session.created_at -= pairing.PAIRING_SESSION_TTL_SECONDS + 1
    assert manager.approve(req["pairing_id"]) is False
    assert manager.poll(req["pairing_id"]) == {"status": PairingStatus.EXPIRED}
    assert manager.get_pending_prompt(req["pairing_id"]) is None


def test_list_devices_excludes_hash_and_revoke_removes(manager):
    _req, token = _approve_and_get_token(manager, "Revoke Me")
    devices = manager.list_devices()
    assert len(devices) == 1
    entry = devices[0]
    assert entry["device_name"] == "Revoke Me"
    assert "token_hash" not in entry
    device_id = entry["device_id"]

    assert manager.revoke(device_id) is True
    assert manager.list_devices() == []
    # Revoked token no longer authenticates.
    assert manager.verify_token(token) is None
    assert manager.revoke(device_id) is False


def test_last_seen_updates_on_verify(manager):
    _req, token = _approve_and_get_token(manager)
    assert manager.list_devices()[0]["last_seen_at"] is None
    manager.verify_token(token)
    assert manager.list_devices()[0]["last_seen_at"] is not None


def test_max_pending_sessions_enforced(manager):
    for _ in range(pairing.MAX_PENDING_SESSIONS):
        manager.request_pairing("Device")
    with pytest.raises(PairingError):
        manager.request_pairing("One Too Many")


def test_rejection_backoff(manager):
    # Enough denials trip the cooldown on new requests.
    for _ in range(pairing.REJECTION_BACKOFF_THRESHOLD):
        req = manager.request_pairing("Device")
        manager.deny(req["pairing_id"])
    with pytest.raises(PairingError):
        manager.request_pairing("Blocked")


def test_hash_token_is_stable_and_hex(manager):
    h = hash_token("abc")
    assert h == hash_token("abc")
    assert re.fullmatch(r"[0-9a-f]{64}", h)
