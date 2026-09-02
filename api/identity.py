"""HTTP endpoints for machine identity (phase 1).

POST /api/v1/identity/challenge  (public) -> sign a client nonce bound to the
                                             machine serial and the client's origin
POST /api/v1/identity/rotate     (loopback) -> reset the identity, revoke devices

A client pins the fingerprint from GET /machine (or /pair/verify) and then, before
attaching any credential to an origin, sends a fresh 32-byte nonce here and checks
the signature under the pinned public key. A substitute server at the machine's
old address cannot produce that signature, so the client withholds the token.
"""

import base64
import json
import os
import time

import identity
from identity import IdentityManagerInstance, OriginError
from config import MeticulousConfig, CONFIG_SYSTEM, MACHINE_SERIAL_NUMBER
from machine import Machine
from log import MeticulousLogger

from .base_handler import BaseHandler, LocalAccessHandler
from .api import API, APIVersion

logger = MeticulousLogger.getLogger(__name__)

# The backend's own listener port (behind nginx). An origin on this port is
# served directly by the backend during development; the public origin is the
# nginx port (scheme default).
BACKEND_PORT = int(os.getenv("PORT", "8080"))

# Token-bucket rate limiting. Signing costs ~1 ms; the cap only stops the
# endpoint being used as a CPU sink. Each bucket refills at `rate` tokens/second
# up to `burst`, so a source may burst to _PER_SOURCE_BURST but sustains only
# _PER_SOURCE_RPS. (The backend event loop is single-threaded, so the plain
# dicts need no lock.)
_PER_SOURCE_RPS = 10
_PER_SOURCE_BURST = 30
_GLOBAL_RPS = 50
_GLOBAL_BURST = 50
# bucket key -> (tokens, last_monotonic)
_source_buckets: dict = {}
_global_bucket: dict = {}
_IDLE_EVICT_S = 30.0


def _refill(buckets: dict, key: str, rate: float, burst: float, now: float) -> float:
    tokens, last = buckets.get(key, (burst, now))
    return min(burst, tokens + (now - last) * rate)


def _rate_limited(source: str) -> bool:
    now = time.monotonic()
    # Evict idle sources so the per-source dict cannot grow without bound.
    if len(_source_buckets) > 2048:
        for k, (_tok, last) in list(_source_buckets.items()):
            if now - last > _IDLE_EVICT_S:
                _source_buckets.pop(k, None)
    g = _refill(_global_bucket, "g", _GLOBAL_RPS, _GLOBAL_BURST, now)
    s = _refill(_source_buckets, source, _PER_SOURCE_RPS, _PER_SOURCE_BURST, now)
    if g < 1 or s < 1:
        # Record the refilled level even on denial so `last` advances.
        _global_bucket["g"] = (g, now)
        _source_buckets[source] = (s, now)
        return True
    _global_bucket["g"] = (g - 1, now)
    _source_buckets[source] = (s - 1, now)
    return False


class IdentityChallengeHandler(BaseHandler):
    def post(self):
        self.set_header("Cache-Control", "no-store")
        if not IdentityManagerInstance.is_ready():
            self.set_status(503)
            self.write({"error": "identity_unavailable"})
            return

        source = self.request.headers.get("X-Real-IP") or self.request.remote_ip or "?"
        if _rate_limited(source):
            self.set_status(429)
            self.write({"error": "rate_limited"})
            return

        try:
            data = json.loads(self.request.body or "{}")
        except json.JSONDecodeError:
            self.set_status(400)
            self.write({"error": "invalid_json"})
            return

        nonce_b64 = data.get("nonce")
        origin_raw = data.get("origin")
        if not isinstance(nonce_b64, str) or not isinstance(origin_raw, str):
            self.set_status(400)
            self.write({"error": "invalid_request"})
            return
        # An over-long origin is rejected before any parsing/signing.
        if len(origin_raw.encode("utf-8")) >= 65536 or len(origin_raw) > 2048:
            self.set_status(400)
            self.write({"error": "invalid_origin"})
            return
        try:
            nonce = base64.b64decode(nonce_b64, validate=True)
        except Exception:
            self.set_status(400)
            self.write({"error": "invalid_nonce"})
            return
        if len(nonce) != 32:
            self.set_status(400)
            self.write({"error": "invalid_nonce"})
            return

        try:
            origin = identity.canonical_origin(origin_raw)
        except OriginError:
            self.set_status(400)
            self.write({"error": "invalid_origin"})
            return

        # Ownership + served check. Skipped only under an explicit override
        # (NOT Machine.emulated), so the real host-match logic is always tested.
        if not identity.allow_any_origin():
            scheme, host, port = identity.origin_host_port(origin)
            served_default = 80  # only http on the nginx-served port in phase 1
            if scheme != "http" or port not in (served_default, BACKEND_PORT):
                # A statement about https or a port the machine does not serve
                # in phase 1 would be signing about an endpoint that does not
                # exist. Distinct error, matching the snakeoil-TLS-fail UX.
                self.set_status(400)
                self.write({"error": "origin_not_served"})
                return
            if host not in IdentityManagerInstance.own_addresses():
                self.set_status(400)
                self.write({"error": "origin_not_mine"})
                return
        elif not _warned_any_origin():
            logger.warning("IDENTITY_ALLOW_ANY_ORIGIN set: origin ownership check bypassed")

        serial = MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] or ""
        signature = IdentityManagerInstance.sign(serial, origin, nonce)
        self.write(
            {
                "alg": identity.ALG,
                "serial": serial,
                "origin": origin,
                "nonce": nonce_b64,
                "public_key": IdentityManagerInstance.public_key_spki_b64(),
                "fingerprint": IdentityManagerInstance.fingerprint_hex(),
                "signature": signature,
            }
        )


_any_origin_warned = {"done": False}


def _warned_any_origin() -> bool:
    if _any_origin_warned["done"]:
        return True
    _any_origin_warned["done"] = True
    return False


class IdentityRotateHandler(LocalAccessHandler):
    """Loopback-only. The Dial's 'Reset machine identity' calls this; it wipes
    the key, mints a new one, and revokes every paired device."""

    def get(self):
        self.set_status(405)
        self.set_header("Allow", "POST")
        self.write({"error": "Identity reset requires POST with confirm=true"})

    def post(self):
        confirm = self.get_argument("confirm", None)
        if confirm != "true":
            try:
                confirm = "true" if json.loads(self.request.body or "{}").get("confirm") else None
            except json.JSONDecodeError:
                confirm = None
        if confirm != "true":
            self.set_status(400)
            self.write({"error": "Confirmation required. Add confirm=true"})
            return
        if Machine.emulated:
            logger.warning("Identity rotation simulated in emulated mode")
        IdentityManagerInstance.rotate()
        self.write(
            {
                "status": "success",
                "fingerprint": IdentityManagerInstance.fingerprint_hex(),
                "generation": IdentityManagerInstance.generation(),
            }
        )


API.register_handler(APIVersion.V1, r"/identity/challenge", IdentityChallengeHandler)
API.register_handler(APIVersion.V1, r"/identity/rotate", IdentityRotateHandler)
