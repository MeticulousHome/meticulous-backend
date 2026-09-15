"""Machine identity (phase 1): key lifecycle, canonicalization, signing.

The pure-logic units are complemented by direct handler tests for origin
ownership and served-origin enforcement. Rate limiting is also verified live.
"""

import base64
import hashlib
import json
import os
import stat
import sys
from types import ModuleType, SimpleNamespace

import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ec

import identity
from identity import IdentityManager, build_message, canonical_origin, OriginError


@pytest.fixture
def mgr(tmp_path, monkeypatch):
    d = tmp_path / "identity"
    monkeypatch.setattr(identity, "IDENTITY_PATH", str(d))
    monkeypatch.setenv("IDENTITY_PATH", str(d))  # so _require_root() -> False
    return IdentityManager()


def _verify(spki_b64, message, sig_b64):
    der = base64.b64decode(spki_b64)
    pub = serialization.load_der_public_key(der)
    r = int.from_bytes(base64.b64decode(sig_b64)[:32], "big")
    s = int.from_bytes(base64.b64decode(sig_b64)[32:], "big")
    from cryptography.hazmat.primitives.asymmetric.utils import encode_dss_signature
    from cryptography.hazmat.primitives import hashes

    pub.verify(encode_dss_signature(r, s), message, ec.ECDSA(hashes.SHA256()))


# --- key lifecycle -----------------------------------------------------------


def test_keygen_creates_file_outside_config_with_mirror(mgr):
    mgr.load_or_create()
    key_path, mirror_path = mgr._paths()
    assert os.path.exists(key_path) and os.path.exists(mirror_path)
    assert not os.path.basename(key_path).startswith(".")
    assert mgr.is_ready() and len(mgr.fingerprint_hex()) == 64
    if os.name != "nt":
        assert stat.S_IMODE(os.stat(key_path).st_mode) == 0o600


def test_load_is_idempotent_and_stable(mgr):
    mgr.load_or_create()
    fp1, gen1 = mgr.fingerprint_hex(), mgr.generation()
    mgr2 = IdentityManager()
    mgr2.load_or_create()
    assert mgr2.fingerprint_hex() == fp1
    assert mgr2.generation() == gen1


def test_absent_key_generated_quietly_without_revoke(mgr, monkeypatch):
    called = {"revoke": 0}
    import pairing

    monkeypatch.setattr(
        pairing.PairingManagerInstance,
        "revoke_all",
        lambda: called.__setitem__("revoke", called["revoke"] + 1),
    )
    mgr.load_or_create()
    assert called["revoke"] == 0  # first boot must not revoke


def test_corrupt_primary_recovers_from_mirror_without_revoke(mgr, monkeypatch):
    mgr.load_or_create()
    fp = mgr.fingerprint_hex()
    key_path, _ = mgr._paths()
    with open(key_path, "wb") as f:
        f.write(b"garbage not a key")
    called = {"revoke": 0}
    import pairing

    monkeypatch.setattr(
        pairing.PairingManagerInstance,
        "revoke_all",
        lambda: called.__setitem__("revoke", called["revoke"] + 1),
    )
    mgr2 = IdentityManager()
    mgr2.load_or_create()
    assert mgr2.fingerprint_hex() == fp  # recovered from mirror
    assert called["revoke"] == 0  # recoverable -> no revoke


def test_unrecoverable_corruption_regenerates_and_revokes(mgr, monkeypatch):
    mgr.load_or_create()
    old_fp = mgr.fingerprint_hex()
    key_path, mirror_path = mgr._paths()
    for p in (key_path, mirror_path):
        with open(p, "wb") as f:
            f.write(b"garbage")
    called = {"revoke": 0, "disconnect": 0}
    import pairing
    import socket_registry

    monkeypatch.setattr(
        pairing.PairingManagerInstance,
        "revoke_all",
        lambda: called.__setitem__("revoke", called["revoke"] + 1),
    )
    monkeypatch.setattr(
        socket_registry,
        "disconnect_all_devices",
        lambda: called.__setitem__("disconnect", called["disconnect"] + 1),
    )
    mgr2 = IdentityManager()
    mgr2.load_or_create()
    assert mgr2.fingerprint_hex() != old_fp
    assert mgr2.generation() >= 2
    assert called["revoke"] == 1 and called["disconnect"] == 1
    # broken copies renamed, not deleted
    broken = [f for f in os.listdir(os.path.dirname(key_path)) if ".broken-" in f]
    assert broken


def test_private_key_never_logged(mgr, caplog):
    import logging

    with caplog.at_level(logging.DEBUG):
        mgr.load_or_create()
        serial, origin = "SER123", "http://10.10.0.42"
        for _ in range(20):
            mgr.sign(serial, origin, os.urandom(32))
    assert "BEGIN PRIVATE KEY" not in caplog.text
    pem = mgr._private_key.private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    ).decode()
    body = pem.splitlines()[1]
    assert body not in caplog.text


# --- signing -----------------------------------------------------------------


def test_sign_roundtrips_and_binds_all_fields(mgr):
    mgr.load_or_create()
    serial, origin = "SERIAL-1", "http://10.10.0.42"
    nonce = os.urandom(32)
    sig = mgr.sign(serial, origin, nonce)
    assert len(base64.b64decode(sig)) == 64  # P1363 r||s, not DER
    _verify(mgr.public_key_spki_b64(), build_message(serial, origin, nonce), sig)
    # Changing any field breaks verification.
    for bad in (
        build_message("OTHER", origin, nonce),
        build_message(serial, "http://10.10.0.99", nonce),
        build_message(serial, origin, os.urandom(32)),
    ):
        with pytest.raises(Exception):
            _verify(mgr.public_key_spki_b64(), bad, sig)


def test_fingerprint_is_sha256_of_spki(mgr):
    mgr.load_or_create()
    der = base64.b64decode(mgr.public_key_spki_b64())
    assert mgr.fingerprint_hex() == hashlib.sha256(der).hexdigest()


# --- length prefix -----------------------------------------------------------


def test_lp_rejects_len_ge_65536():
    identity._lp(b"x" * 65535)  # ok
    with pytest.raises(ValueError):
        identity._lp(b"x" * 65536)


# --- canonical origin KATs (shared with the TS client) -----------------------


def test_canonical_origin_kats():
    cases = {
        "http://10.10.0.42": "http://10.10.0.42",
        "http://10.10.0.42:80": "http://10.10.0.42",
        "http://10.10.0.42:8080": "http://10.10.0.42:8080",
        "https://10.10.0.42:443": "https://10.10.0.42",
        "HTTP://10.10.0.42": "http://10.10.0.42",
        "http://Espresso.Local": "http://espresso.local",
        "http://espresso.local.": "http://espresso.local",
        "http://[2001:DB8::1]:8080": "http://[2001:db8::1]:8080",
        "http://[2001:db8:0:0:0:0:0:1]": "http://[2001:db8::1]",
    }
    for raw, expect in cases.items():
        assert canonical_origin(raw) == expect, raw


def test_canonical_origin_rejects_bad():
    for bad in (
        "ftp://10.10.0.42",
        "http://user:pw@10.10.0.42",
        "http://10.10.0.42/path",
        "http://10.10.0.42?q=1",
        "http://[fe80::1%eth0]",
        "",
    ):
        with pytest.raises(OriginError):
            canonical_origin(bad)


def test_own_addresses_override_via_env(mgr, monkeypatch):
    monkeypatch.setenv("IDENTITY_OWN_ADDRESSES", "10.10.0.42, espresso.local")
    assert mgr.own_addresses() == {"10.10.0.42", "espresso.local"}


# --- challenge handler origin boundary ---------------------------------------


class _ChallengeRecorder:
    def __init__(self, origin):
        self.request = SimpleNamespace(
            headers={},
            remote_ip="192.0.2.55",
            body=json.dumps(
                {
                    "nonce": base64.b64encode(bytes(range(32))).decode(),
                    "origin": origin,
                }
            ).encode(),
        )
        self.status = 200
        self.headers = {}
        self.body = None

    def set_status(self, status):
        self.status = status

    def set_header(self, name, value):
        self.headers[name] = value

    def write(self, body):
        self.body = body


@pytest.fixture
def challenge_endpoint(monkeypatch):
    # The endpoint only reads Machine.emulated in the unrelated rotate handler.
    # Avoid importing the hardware-heavy machine module in this handler unit.
    machine_stub = ModuleType("machine")
    machine_stub.Machine = SimpleNamespace(emulated=False)
    monkeypatch.setitem(sys.modules, "machine", machine_stub)
    import api.identity as endpoint

    signed = []
    monkeypatch.setattr(endpoint.IdentityManagerInstance, "is_ready", lambda: True)
    monkeypatch.setattr(
        endpoint.IdentityManagerInstance,
        "own_addresses",
        lambda: {"10.10.0.42", "espresso.local"},
    )
    monkeypatch.setattr(
        endpoint.IdentityManagerInstance,
        "sign",
        lambda serial, origin, nonce: signed.append((serial, origin, nonce)) or "sig",
    )
    monkeypatch.setattr(endpoint.IdentityManagerInstance, "public_key_spki_b64", lambda: "spki")
    monkeypatch.setattr(endpoint.IdentityManagerInstance, "fingerprint_hex", lambda: "fp")
    monkeypatch.setattr(endpoint, "_rate_limited", lambda _source: False)
    monkeypatch.setattr(endpoint.identity, "allow_any_origin", lambda: False)
    monkeypatch.setattr(
        endpoint,
        "MeticulousConfig",
        {endpoint.CONFIG_SYSTEM: {endpoint.MACHINE_SERIAL_NUMBER: "MET-HANDLER-1"}},
    )
    return endpoint, signed


def _post_challenge(endpoint, origin):
    recorder = _ChallengeRecorder(origin)
    endpoint.IdentityChallengeHandler.post(recorder)
    return recorder


def test_challenge_refuses_foreign_origin_without_signing(challenge_endpoint):
    endpoint, signed = challenge_endpoint
    response = _post_challenge(endpoint, "http://evil.example")

    assert response.status == 400
    assert response.body == {"error": "origin_not_mine"}
    assert response.headers["Cache-Control"] == "no-store"
    assert signed == []


@pytest.mark.parametrize(
    "origin",
    ["https://10.10.0.42", "http://10.10.0.42:8081"],
)
def test_challenge_refuses_unserved_scheme_or_port_without_signing(challenge_endpoint, origin):
    endpoint, signed = challenge_endpoint
    response = _post_challenge(endpoint, origin)

    assert response.status == 400
    assert response.body == {"error": "origin_not_served"}
    assert signed == []


def test_challenge_signs_canonical_origin_owned_by_machine(challenge_endpoint):
    endpoint, signed = challenge_endpoint
    response = _post_challenge(endpoint, "HTTP://10.10.0.42:80")

    assert response.status == 200
    assert response.headers["Cache-Control"] == "no-store"
    assert response.body["origin"] == "http://10.10.0.42"
    assert response.body["serial"] == "MET-HANDLER-1"
    assert response.body["signature"] == "sig"
    assert signed == [("MET-HANDLER-1", "http://10.10.0.42", bytes(range(32)))]


# --- cross-language vector ---------------------------------------------------


def test_matches_committed_vector():
    path = os.path.join(os.path.dirname(__file__), "vectors", "identity_v1.json")
    v = json.load(open(path))
    msg = build_message(v["serial"], v["origin"], base64.b64decode(v["nonce"]))
    assert msg.hex() == v["message_hex"]
    _verify(v["public_key"], msg, v["signature"])  # raises if invalid


# --- low-S normalization (audit blocker regression) --------------------------


def _is_low_s(sig_b64):
    from identity import _P256_ORDER

    s = int.from_bytes(base64.b64decode(sig_b64)[32:], "big")
    return s <= _P256_ORDER // 2


def test_signatures_are_always_low_s(mgr):
    # @noble/curves p256.verify rejects high-S by default (the insecure-context
    # browser and mobile clients). Every signature we emit MUST be low-S, or ~50%
    # of genuine verifications would fail.
    mgr.load_or_create()
    for i in range(200):
        sig = mgr.sign("SER", "http://10.10.0.42", os.urandom(32))
        assert _is_low_s(sig), f"high-S signature at iteration {i}"


def test_committed_vector_is_low_s():
    v = json.load(open(os.path.join(os.path.dirname(__file__), "vectors", "identity_v1.json")))
    assert _is_low_s(v["signature"])


def test_transient_read_error_fails_closed_not_regenerate(mgr, monkeypatch):
    # A transient IO error on an existing key must raise, NOT be treated as
    # corruption (which would regenerate and revoke every device).
    import identity as idmod

    mgr.load_or_create()
    fp = mgr.fingerprint_hex()

    real_open = open

    def flaky_open(path, *a, **k):
        if str(path).endswith(".pem"):
            raise OSError(5, "EIO simulated")
        return real_open(path, *a, **k)

    monkeypatch.setattr("builtins.open", flaky_open)
    called = {"revoke": 0}
    import pairing

    monkeypatch.setattr(
        pairing.PairingManagerInstance,
        "revoke_all",
        lambda: called.__setitem__("revoke", called["revoke"] + 1),
    )
    mgr2 = idmod.IdentityManager()
    with pytest.raises(idmod.IdentityIOError):
        mgr2.load_or_create()
    assert called["revoke"] == 0  # never revoked over a transient read error
    # Restore ONLY open (not the fixture's IDENTITY_PATH patch): the good key on
    # disk is untouched and loads cleanly once the transient fault clears.
    monkeypatch.setattr("builtins.open", real_open)
    mgr3 = idmod.IdentityManager()
    mgr3.load_or_create()
    assert mgr3.fingerprint_hex() == fp
