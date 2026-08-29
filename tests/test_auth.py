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
    assert auth.is_public_path("POST", "/api/v1/pair/request")
    assert auth.is_public_path("GET", "/api/v1/pair/status/abc-123")
    assert not auth.is_public_path("GET", "/api/v1/pair/devices")
    assert not auth.is_public_path("GET", "/api/v1/settings")
    # Reachable from a remote client with no token:
    assert auth.is_authorized("POST", "/api/v1/pair/request", "127.0.0.1", "9.9.9.9", None)
    assert auth.is_authorized("GET", "/api/v1/pair/status/x", "127.0.0.1", "9.9.9.9", None)


def test_machine_identity_public_for_discovery():
    # The app lists machines (zeroconf/BLE discovery) before it can pair, so
    # GET /machine is public by design; every other method stays gated.
    assert auth.is_public_path("GET", "/api/v1/machine")
    assert not auth.is_public_path("POST", "/api/v1/machine")
    assert auth.is_authorized("GET", "/api/v1/machine", "127.0.0.1", "9.9.9.9", None)
    # Sibling machine endpoints are NOT public.
    assert not auth.is_public_path("GET", "/api/v1/machine/factory_reset")
    assert not auth.is_authorized(
        "GET", "/api/v1/machine/factory_reset", "127.0.0.1", "9.9.9.9", None
    )


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


def test_socket_token_extraction():
    assert auth.socket_token({"token": "abc"}, {}) == "abc"
    assert auth.socket_token(None, {"HTTP_AUTHORIZATION": "Bearer abc"}) == "abc"
    assert auth.socket_token({}, {"HTTP_AUTHORIZATION": "Bearer xyz"}) == "xyz"
    assert auth.socket_token(None, {}) is None
    # auth payload wins over header
    assert auth.socket_token({"token": "fromauth"}, {"HTTP_AUTHORIZATION": "Bearer fromheader"}) == "fromauth"


def test_socket_dial_loopback_allowed_without_token():
    # The Dial connects to the backend on loopback, no X-Real-IP.
    assert auth.is_socket_authorized({"REMOTE_ADDR": "127.0.0.1"}, None)
    assert auth.is_socket_authorized({"REMOTE_ADDR": "::1"}, {})


def test_socket_lan_client_needs_valid_token():
    lan = {"REMOTE_ADDR": "127.0.0.1", "HTTP_X_REAL_IP": "192.168.1.50"}
    # No token -> refused.
    assert not auth.is_socket_authorized(lan, None)
    assert not auth.is_socket_authorized(lan, {"token": "bad-token"})
    # Valid token in the handshake auth payload -> allowed.
    assert auth.is_socket_authorized(lan, {"token": "good-token"})
    # Valid token via Authorization header fallback -> allowed.
    assert auth.is_socket_authorized(
        {**lan, "HTTP_AUTHORIZATION": "Bearer good-token"}, None
    )
