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
    }
)
_PUBLIC_PREFIXES = ("/api/v1/pair/status/",)


def is_public_path(path: str) -> bool:
    if path in _PUBLIC_EXACT:
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


def is_authorized(
    method: str,
    path: str,
    remote_ip: Optional[str],
    x_real_ip: Optional[str],
    authorization: Optional[str],
) -> bool:
    """The single authorization decision for an incoming request."""
    if method == "OPTIONS":
        return True
    if is_public_path(path):
        return True
    if client_is_local(remote_ip, x_real_ip):
        return True
    token = parse_bearer_token(authorization)
    return PairingManagerInstance.verify_token(token) is not None


def request_is_local(handler) -> bool:
    """Convenience for handlers that must stay loopback-only (e.g. exposing the
    AP password): True only for a genuine loopback/Dial caller."""
    return client_is_local(
        handler.request.remote_ip,
        handler.request.headers.get("X-Real-IP"),
    )


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
        ):
            self.set_status(401)
            self.set_header("Content-type", "application/json")
            self.finish(json.dumps({"error": "Unauthorized"}))
            return None
        # Preserve the wrapped handler's own prepare() (may be a coroutine, e.g.
        # StaticFileHandler); Tornado awaits whatever prepare returns.
        return super().prepare()


def with_auth(handler):
    """Return a subclass of `handler` with AuthMixin prepended."""
    return type(f"Authed{handler.__name__}", (AuthMixin, handler), {})
