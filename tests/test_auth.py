import pytest

from api import auth


@pytest.fixture(autouse=True)
def fake_verifier(monkeypatch):
    # Only "good-token" is a valid device token for these tests.
    monkeypatch.setattr(
        auth.PairingManagerInstance,
        "verify_token",
        lambda token: "device-1" if token == "good-token" else None,
    )


def test_options_always_allowed():
    assert auth.is_authorized("OPTIONS", "/api/v1/settings", "1.2.3.4", "1.2.3.4", None)


def test_public_pairing_paths_allowed_without_token():
    assert auth.is_public_path("/api/v1/pair/request")
    assert auth.is_public_path("/api/v1/pair/status/abc-123")
    assert not auth.is_public_path("/api/v1/pair/devices")
    assert not auth.is_public_path("/api/v1/settings")
    # Reachable from a remote client with no token:
    assert auth.is_authorized("POST", "/api/v1/pair/request", "127.0.0.1", "9.9.9.9", None)
    assert auth.is_authorized("GET", "/api/v1/pair/status/x", "127.0.0.1", "9.9.9.9", None)


def test_client_is_local():
    # Genuine loopback caller (the Dial): loopback peer, no X-Real-IP.
    assert auth.client_is_local("127.0.0.1", None)
    assert auth.client_is_local("::1", None)
    # Through nginx from a LAN client: loopback peer but X-Real-IP is the client.
    assert not auth.client_is_local("127.0.0.1", "192.168.1.50")
    # X-Real-IP itself loopback -> still local.
    assert auth.client_is_local("127.0.0.1", "127.0.0.1")
    # A spoofed loopback X-Real-IP is only "local" because nginx is trusted to
    # overwrite it; documented dependency, asserted here as current behavior.
    assert auth.client_is_local("127.0.0.1", "localhost")


def test_parse_bearer_token():
    assert auth.parse_bearer_token("Bearer abc") == "abc"
    assert auth.parse_bearer_token("bearer abc") == "abc"
    assert auth.parse_bearer_token("Basic abc") is None
    assert auth.parse_bearer_token("") is None
    assert auth.parse_bearer_token(None) is None


def test_lan_client_requires_valid_token():
    # LAN client (non-loopback X-Real-IP) with a valid token -> allowed.
    assert auth.is_authorized(
        "GET", "/api/v1/settings", "127.0.0.1", "192.168.1.50", "Bearer good-token"
    )
    # Wrong token -> denied.
    assert not auth.is_authorized(
        "GET", "/api/v1/settings", "127.0.0.1", "192.168.1.50", "Bearer bad-token"
    )
    # No token -> denied.
    assert not auth.is_authorized("GET", "/api/v1/settings", "127.0.0.1", "192.168.1.50", None)


def test_dial_loopback_allowed_without_token():
    assert auth.is_authorized("GET", "/api/v1/settings", "127.0.0.1", None, None)
    assert auth.is_authorized("POST", "/api/v1/machine/factory_reset", "127.0.0.1", None, None)
