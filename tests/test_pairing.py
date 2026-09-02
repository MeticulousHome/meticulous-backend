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
    # Typing the code shown on the Dial is the ONLY approval path.
    req = manager.request_pairing(device_name)
    code = manager.get_pending_prompt(req["pairing_id"])["code"]
    token = manager.verify_code(req["pairing_id"], code)
    assert token
    assert manager.poll(req["pairing_id"])["status"] == PairingStatus.APPROVED
    return req, token


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


def test_expired_session_cannot_be_verified(manager, monkeypatch):
    req = manager.request_pairing("Slow Device")
    session = manager._sessions[req["pairing_id"]]
    # Move the session's creation past the TTL.
    session.created_at -= pairing.PAIRING_SESSION_TTL_SECONDS + 1
    code = session.code
    assert manager.verify_code(req["pairing_id"], code) is None
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


def test_verify_code_mints_token_and_authenticates(manager):
    req = manager.request_pairing("Browser")
    code = manager.get_pending_prompt(req["pairing_id"])["code"]
    token = manager.verify_code(req["pairing_id"], code)
    assert token
    # The token authenticates like any other device token.
    assert manager.verify_token(token) is not None
    # The device is now listed.
    assert len(manager.list_devices()) == 1


def test_verify_code_rejects_wrong_code(manager):
    req = manager.request_pairing("Browser")
    good = manager.get_pending_prompt(req["pairing_id"])["code"]
    wrong = "000000" if good != "000000" else "111111"
    assert manager.verify_code(req["pairing_id"], wrong) is None
    # No device was created.
    assert manager.list_devices() == []
    # A single mistype is forgiven: the correct code still works.
    assert manager.verify_code(req["pairing_id"], good)


def test_verify_code_burns_session_after_repeated_wrong_codes(manager):
    # ADV-004: a ~20-bit code is brute-forcible if a session accepts unlimited
    # guesses within its lifetime. After the cap the session is dead even for
    # the CORRECT code.
    req = manager.request_pairing("Attacker")
    good = manager.get_pending_prompt(req["pairing_id"])["code"]
    wrong = "000000" if good != "000000" else "111111"
    for _ in range(pairing.MAX_VERIFY_ATTEMPTS):
        assert manager.verify_code(req["pairing_id"], wrong) is None
    assert manager.verify_code(req["pairing_id"], good) is None
    assert manager.list_devices() == []


def test_status_never_returns_the_token(manager):
    # ADV-011: /pair/status is public; the secret must be delivered exactly once
    # by verify_code and never be retrievable again.
    req = manager.request_pairing("Browser")
    code = manager.get_pending_prompt(req["pairing_id"])["code"]
    token = manager.verify_code(req["pairing_id"], code)
    assert token
    status = manager.poll(req["pairing_id"])
    assert status == {"status": PairingStatus.APPROVED}
    assert "token" not in status


def test_pairing_code_is_never_logged(manager, caplog):
    # ADV-012: logs reach bug reports/archives/Sentry, so the enrollment secret
    # must never appear in them.
    import logging

    with caplog.at_level(logging.DEBUG):
        req = manager.request_pairing("Browser")
        code = manager.get_pending_prompt(req["pairing_id"])["code"]
        manager.verify_code(req["pairing_id"], code)
    assert code not in caplog.text


def test_verify_code_unknown_session(manager):
    assert manager.verify_code("does-not-exist", "123456") is None


def test_verify_code_expired(manager, monkeypatch):
    req = manager.request_pairing("Browser")
    code = manager.get_pending_prompt(req["pairing_id"])["code"]
    now = pairing._now() + pairing.PAIRING_SESSION_TTL_SECONDS + 1
    monkeypatch.setattr(pairing, "_now", lambda: now)
    assert manager.verify_code(req["pairing_id"], code) is None


def test_verify_code_wrong_guesses_trigger_backoff(manager):
    req = manager.request_pairing("Browser")
    for _ in range(pairing.REJECTION_BACKOFF_THRESHOLD):
        manager.verify_code(req["pairing_id"], "999999")
    # After enough wrong guesses, new pairing requests are refused.
    with pytest.raises(PairingError):
        manager.request_pairing("Another")


def test_revoke_all_clears_every_device(manager):
    for i in range(3):
        req = manager.request_pairing(f"Device {i}")
        code = manager.get_pending_prompt(req["pairing_id"])["code"]
        manager.verify_code(req["pairing_id"], code)
    assert len(manager.list_devices()) == 3
    assert manager.revoke_all() == 3
    assert manager.list_devices() == []
    # Revoking again reports zero, no error.
    assert manager.revoke_all() == 0
