"""Request authorization for the local HTTP API (M3).

Every request is either:
  * a CORS preflight (OPTIONS) -> allowed, the browser attaches no auth to these;
  * a public pairing endpoint  -> allowed, an unpaired device has no token yet;
  * a local/Dial call          -> allowed (the Dial talks to the backend on
                                   loopback, not through nginx);
  * a LAN device with a valid bearer token -> allowed;
  * anything else -> 401.

Reverse-proxy trust boundary
----------------------------
The backend binds to 127.0.0.1 only; the sole way a LAN client reaches it is
through the machine's nginx, which sets `X-Real-IP` to the real client address.
So a request whose `X-Real-IP` is absent or itself loopback originates from a
genuine loopback caller (the Dial); a request with a non-loopback `X-Real-IP` is
a remote client and must present a token.

SECURITY: this rests on nginx unconditionally overwriting `X-Real-IP` (so a LAN
client cannot forge it). That must be verified in the deployed image before this
is relied on (meticulous-machine; "Ticket 7" in the design). If nginx does not
overwrite it, a LAN client could spoof `X-Real-IP: 127.0.0.1` and bypass auth.

The decision logic here is pure and unit-tested; the Tornado glue that calls it
lives in `enforce_authorization` / `AuthMixin`.
"""

import json
from typing import Optional

from pairing import PairingManagerInstance

_LOOPBACK = ("127.0.0.1", "::1", "localhost")

# Endpoints reachable without a token. An unpaired device must be able to start
# and poll a pairing session before it can hold a token.
_PUBLIC_EXACT = frozenset(
    {
        "/api/v1/pair/request",
        "/api/v1/pair/verify",
        "/api/v1/identity/challenge",
    }
)
_PUBLIC_PREFIXES = ("/api/v1/pair/status/",)
# Read-only identity, public by design: the app's discovery flow (zeroconf/BLE)
# calls GET /machine on every candidate to render the machine list *before* the
# user can pick one and pair with it. Everything it returns (name, hostname,
# serial, color) is already broadcast to the whole LAN via the mDNS
# announcement, so this does not widen what an unpaired peer can learn.
_PUBLIC_GET_EXACT = frozenset(
    {
        "/api/v1/machine",
        # The standalone pairing page: an unpaired browser must be able to load
        # it to start pairing. It only calls the public /pair/* endpoints.
        "/api/v1/pair",
    }
)


def is_public_path(method: str, path: str) -> bool:
    if path in _PUBLIC_EXACT:
        return True
    if method == "GET" and path in _PUBLIC_GET_EXACT:
        return True
    return any(path.startswith(prefix) for prefix in _PUBLIC_PREFIXES)


def _is_loopback(value: Optional[str]) -> bool:
    if not value:
        return False
    return value in _LOOPBACK or value.startswith("127.")


def client_is_local(remote_ip: Optional[str], x_real_ip: Optional[str]) -> bool:
    """True for a genuine loopback caller (the Dial), False for a LAN client.

    A non-loopback X-Real-IP means the request came through nginx from a real
    client. Absent/loopback X-Real-IP with a loopback socket peer means the Dial.
    """
    if x_real_ip and not _is_loopback(x_real_ip):
        return False
    return _is_loopback(remote_ip)


def parse_bearer_token(authorization: Optional[str]) -> Optional[str]:
    if not authorization:
        return None
    parts = authorization.split(None, 1)
    if len(parts) == 2 and parts[0].lower() == "bearer":
        return parts[1].strip()
    return None


# Cookie fallback for browsers. The pairing pages store the token in a
# SameSite=Strict cookie as well as localStorage, so a browser that authorized
# once can hit any endpoint straight from the address bar (a plain navigation
# carries no Authorization header). SameSite=Strict means other sites can never
# make the browser attach it (CSRF), and CORS never allows credentials, so no
# foreign origin can ride it.
def extract_token(authorization: Optional[str], cookie: Optional[str] = None) -> Optional[str]:
    """Bearer header only. The met_device_token cookie was removed (ADV-020 /
    identity D10): a cookie is attached by the browser on any plain top-level
    navigation, which never passes the identity check, so after a DHCP-reused
    address a browser would hand the token to an impostor. `cookie` is accepted
    and ignored so existing call sites keep compiling."""
    return parse_bearer_token(authorization)


def is_authorized(
    method: str,
    path: str,
    remote_ip: Optional[str],
    x_real_ip: Optional[str],
    authorization: Optional[str],
    cookie: Optional[str] = None,
) -> bool:
    """The single authorization decision for an incoming request."""
    if method == "OPTIONS":
        return True
    if is_public_path(method, path):
        return True
    if client_is_local(remote_ip, x_real_ip):
        return True
    token = extract_token(authorization, cookie)
    return PairingManagerInstance.verify_token(token) is not None


def request_is_local(handler) -> bool:
    """Convenience for handlers that must stay loopback-only (e.g. exposing the
    AP password): True only for a genuine loopback/Dial caller."""
    return client_is_local(
        handler.request.remote_ip,
        handler.request.headers.get("X-Real-IP"),
    )


def request_has_access(handler) -> bool:
    """True if the request is loopback/Dial or carries a valid device token.

    For endpoints that are public but must return LESS to an unpaired caller
    (e.g. /machine returns only a minimal identity for discovery, full build and
    repository detail only once authorized)."""
    if client_is_local(
        handler.request.remote_ip, handler.request.headers.get("X-Real-IP")
    ):
        return True
    token = extract_token(
        handler.request.headers.get("Authorization"),
        handler.request.headers.get("Cookie"),
    )
    return PairingManagerInstance.verify_token(token) is not None


def socket_token(auth, environ) -> Optional[str]:
    """Pull the token from a Socket.IO handshake.

    Clients pass it in the connection `auth` payload (io(url, {auth:{token}}));
    we also accept an Authorization header (non-browser clients). The cookie
    fallback was removed (ADV-020 / identity D10): a cookie rides plain
    navigations with no identity check.
    """
    if isinstance(auth, dict):
        token = auth.get("token")
        if token:
            return token
    if isinstance(environ, dict):
        return parse_bearer_token(environ.get("HTTP_AUTHORIZATION"))
    return None


def socket_remote_ip(environ):
    """The REAL TCP peer of a Socket.IO connection.

    python-engineio's Tornado driver hardcodes environ['REMOTE_ADDR'] to
    '127.0.0.1' for every connection, so trusting it would decide locality from
    X-Real-IP alone. The Tornado handler it stashes in the environ knows the
    genuine peer (the app runs without xheaders, so remote_ip is the socket
    peer, never a header)."""
    environ = environ or {}
    handler = environ.get("tornado.handler")
    if handler is not None:
        try:
            return handler.request.remote_ip
        except AttributeError:
            pass
    return environ.get("REMOTE_ADDR")


def socket_is_local(environ) -> bool:
    """True only for a genuine loopback (Dial) socket: real peer loopback AND
    no non-loopback X-Real-IP from nginx."""
    environ = environ or {}
    return client_is_local(socket_remote_ip(environ), environ.get("HTTP_X_REAL_IP"))


def socket_device_id(environ, auth):
    """The paired device_id that authorizes a Socket.IO connection, or None for
    a loopback/Dial caller (which holds no token). Used to tie a live socket to
    the device that authorized it, so revoking that device can drop the socket."""
    if socket_is_local(environ):
        return None
    return PairingManagerInstance.verify_token(socket_token(auth, environ or {}))


def is_socket_authorized(environ, auth) -> bool:
    """Authorization decision for a Socket.IO connection.

    Mirrors the HTTP rule (api auth): the Dial connects over loopback and is
    exempt; a LAN client must present a valid device token in the handshake.
    This gates the sensor/status stream, the `action` control channel, and the
    `notification` acknowledgement path (which is how pairing/BLE approvals come
    back) -- all of which live on this one socket. Without it, an unpaired LAN
    peer could read telemetry, drive the machine, and forge an on-screen
    approval by replaying a broadcast notification id.
    """
    environ = environ or {}
    if socket_is_local(environ):
        return True
    return PairingManagerInstance.verify_token(socket_token(auth, environ)) is not None


class AuthMixin:
    """Prepended to every registered handler in API.get_routes() so the
    authorization decision runs before any handler logic, including for handlers
    that do not derive from BaseHandler (StaticFileHandler, RedirectHandler,
    web UI). Public pairing endpoints and loopback/Dial calls pass through; LAN
    clients need a valid bearer token.
    """

    def prepare(self):
        if not is_authorized(
            self.request.method,
            self.request.path,
            self.request.remote_ip,
            self.request.headers.get("X-Real-IP"),
            self.request.headers.get("Authorization"),
            self.request.headers.get("Cookie"),
        ):
            self.set_status(401)
            self.set_header("Content-type", "application/json")
            self.finish(json.dumps({"error": "Unauthorized"}))
            return None
        # Preserve the wrapped handler's own prepare() (may be a coroutine, e.g.
        # StaticFileHandler); Tornado awaits whatever prepare returns.
        return super().prepare()

    def options(self, *args, **kwargs):
        # Answer CORS preflights uniformly. Handlers that do not derive from
        # BaseHandler (e.g. the history StaticFileHandler) have no options()
        # of their own and would answer 405, which makes browsers fail any
        # cross-origin request that carries the Authorization header.
        self.set_status(204)
        self.finish()


def with_auth(handler):
    """Return a subclass of `handler` with AuthMixin prepended."""
    return type(f"Authed{handler.__name__}", (AuthMixin, handler), {})
