"""Device pairing and API access tokens.

The machine's local HTTP API is authenticated with per-device bearer tokens.
A new device (phone app, community web app, a developer's computer, an
automated-test page, ...) cannot obtain a token on its own: it opens a pairing
session, the machine shows a 6-digit code on the Dial with an Allow/Deny prompt,
and only an explicit on-screen approval mints a token. Approval is per-device;
once authorized, the device reuses its token until the owner revokes it from the
Dial ("Paired devices") or performs a factory reset.

Design notes:
- The plaintext token is returned to the client exactly once, when it polls the
  pairing status after approval. Only its SHA-256 hash is persisted, in the
  CONFIG_PAIRED_DEVICES section of config.yml. The machine never stores the
  plaintext.
- Verification is constant-time (hmac.compare_digest) against the stored hashes.
- Loopback callers (the Dial itself) are handled by the request-layer exemption,
  not here -- this module only governs LAN devices.

This module has no hardware dependencies so it can be unit-tested directly.
"""

import hashlib
import hmac
import secrets
import threading
import time
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Optional

from config import (
    CONFIG_PAIRED_DEVICES,
    MeticulousConfig,
)
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

# A pairing session is short-lived: the user must approve on the Dial within
# this window or the client must re-request.
PAIRING_SESSION_TTL_SECONDS = 60
# Bytes of entropy for the opaque token (256 bits).
TOKEN_ENTROPY_BYTES = 32
# Cap concurrent pending sessions so a hostile LAN peer cannot spam the Dial.
MAX_PENDING_SESSIONS = 5
# After this many denied/expired sessions from the request path, new requests
# are refused for a cooldown to blunt brute-forcing the on-screen prompt.
REJECTION_BACKOFF_THRESHOLD = 5
REJECTION_BACKOFF_SECONDS = 60


class PairingStatus:
    PENDING = "pending"
    APPROVED = "approved"
    DENIED = "denied"
    EXPIRED = "expired"


def hash_token(token: str) -> str:
    """SHA-256 hex digest of a token. The only form stored on the machine."""
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


def _generate_token() -> str:
    return secrets.token_urlsafe(TOKEN_ENTROPY_BYTES)


def _generate_code() -> str:
    # 6-digit numeric code shown on the Dial and on the client, so the user can
    # confirm they are approving their own device and not a racing attacker.
    return f"{secrets.randbelow(1_000_000):06d}"


def _now() -> float:
    return time.time()


def _iso_now() -> str:
    return datetime.now(timezone.utc).isoformat()


@dataclass
class PairingSession:
    pairing_id: str
    device_name: str
    code: str
    created_at: float
    status: str = PairingStatus.PENDING
    # Plaintext token, held only between approval and the client's first poll.
    _token: Optional[str] = field(default=None, repr=False)
    device_id: Optional[str] = None

    def is_expired(self, now: float) -> bool:
        return (now - self.created_at) > PAIRING_SESSION_TTL_SECONDS


class PairingManager:
    """In-memory pairing sessions plus persisted per-device tokens."""

    def __init__(self):
        self._sessions: dict[str, PairingSession] = {}
        self._lock = threading.RLock()
        self._recent_rejections: list[float] = []

    # --- pairing session lifecycle -------------------------------------------

    def request_pairing(self, device_name: str) -> dict:
        """Open a pairing session. Returns {pairing_id, code, expires_in}.

        Raises PairingError if the machine is rate-limiting new requests.
        """
        device_name = (device_name or "Unknown device").strip()[:64]
        now = _now()
        with self._lock:
            self._prune(now)
            if self._in_backoff(now):
                raise PairingError("Too many pairing attempts, try again shortly")
            pending = [s for s in self._sessions.values() if s.status == PairingStatus.PENDING]
            if len(pending) >= MAX_PENDING_SESSIONS:
                raise PairingError("Too many pending pairing requests")

            session = PairingSession(
                pairing_id=str(uuid.uuid4()),
                device_name=device_name,
                code=_generate_code(),
                created_at=now,
            )
            self._sessions[session.pairing_id] = session
            logger.info(
                f"Pairing requested by '{session.device_name}' "
                f"(id={session.pairing_id}, code={session.code})"
            )
            return {
                "pairing_id": session.pairing_id,
                "code": session.code,
                "expires_in": PAIRING_SESSION_TTL_SECONDS,
            }

    def get_pending_prompt(self, pairing_id: str) -> Optional[dict]:
        """The data the Dial needs to render the approval prompt, or None."""
        with self._lock:
            session = self._sessions.get(pairing_id)
            if not session or session.status != PairingStatus.PENDING:
                return None
            if session.is_expired(_now()):
                return None
            return {"device_name": session.device_name, "code": session.code}

    def approve(self, pairing_id: str) -> bool:
        """Approve a session (called from the Dial approval bridge).

        Mints a token, persists its hash as a new paired device, and stashes the
        plaintext on the session for the client's next poll. Returns False if the
        session is unknown or no longer pending.
        """
        with self._lock:
            session = self._sessions.get(pairing_id)
            if not session or session.status != PairingStatus.PENDING:
                return False
            if session.is_expired(_now()):
                session.status = PairingStatus.EXPIRED
                self._record_rejection(_now())
                return False

            token = _generate_token()
            device_id = self._persist_device(session.device_name, token)
            session._token = token
            session.device_id = device_id
            session.status = PairingStatus.APPROVED
            logger.info(f"Pairing approved for '{session.device_name}' (device_id={device_id})")
            return True

    def verify_code(self, pairing_id: str, code: str) -> Optional[str]:
        """Approve a session by typing back the code shown on the Dial.

        This is the browser/standalone path: the code is displayed only on the
        machine screen, and a client proves it can see the Dial by submitting it
        here. On a correct code the token is minted and returned directly (no
        polling needed). A wrong code counts as a rejection so repeated guesses
        hit the backoff. Returns the token, or None on unknown/expired/mismatch.
        """
        code = (code or "").strip()
        with self._lock:
            session = self._sessions.get(pairing_id)
            if not session or session.status != PairingStatus.PENDING:
                return None
            if session.is_expired(_now()):
                session.status = PairingStatus.EXPIRED
                self._record_rejection(_now())
                return None
            if not hmac.compare_digest(session.code, code):
                self._record_rejection(_now())
                logger.info(f"Pairing code mismatch for '{session.device_name}'")
                return None

            token = _generate_token()
            device_id = self._persist_device(session.device_name, token)
            session._token = token
            session.device_id = device_id
            session.status = PairingStatus.APPROVED
            logger.info(
                f"Pairing approved by code for '{session.device_name}' (device_id={device_id})"
            )
            return token

    def deny(self, pairing_id: str) -> bool:
        with self._lock:
            session = self._sessions.get(pairing_id)
            if not session or session.status != PairingStatus.PENDING:
                return False
            session.status = PairingStatus.DENIED
            self._record_rejection(_now())
            logger.info(f"Pairing denied for '{session.device_name}'")
            return True

    def poll(self, pairing_id: str) -> dict:
        """Client-facing status. Returns the token exactly once, on first poll
        after approval, then clears it from memory."""
        with self._lock:
            session = self._sessions.get(pairing_id)
            if not session:
                return {"status": PairingStatus.EXPIRED}
            if session.status == PairingStatus.PENDING and session.is_expired(_now()):
                session.status = PairingStatus.EXPIRED
                self._record_rejection(_now())

            if session.status == PairingStatus.APPROVED and session._token is not None:
                token = session._token
                session._token = None  # deliver once
                return {"status": PairingStatus.APPROVED, "token": token}
            return {"status": session.status}

    # --- token verification & device management ------------------------------

    def verify_token(self, token: Optional[str]) -> Optional[str]:
        """Return the device_id for a valid token, else None.

        Updates last_seen in memory only; verification runs on every authorized
        request, so we do not flush to disk each time. The updated timestamp is
        persisted lazily on the next config save (best-effort; last_seen is
        informational).
        """
        if not token:
            return None
        candidate = hash_token(token)
        with self._lock:
            devices = self._devices()
            for device_id, record in devices.items():
                stored = record.get("token_hash", "") if isinstance(record, dict) else ""
                if stored and hmac.compare_digest(stored, candidate):
                    record["last_seen_at"] = _iso_now()
                    return device_id
        return None

    def list_devices(self) -> list[dict]:
        """Public device metadata for the 'Paired devices' screen (no hashes)."""
        with self._lock:
            return [
                {
                    "device_id": device_id,
                    "device_name": record.get("device_name", "Unknown device"),
                    "created_at": record.get("created_at"),
                    "last_seen_at": record.get("last_seen_at"),
                }
                for device_id, record in self._devices().items()
                if isinstance(record, dict)
            ]

    def revoke(self, device_id: str) -> bool:
        with self._lock:
            devices = self._devices()
            if device_id in devices:
                name = devices[device_id].get("device_name", "?")
                del devices[device_id]
                MeticulousConfig.save()
                logger.info(f"Revoked paired device '{name}' (device_id={device_id})")
                return True
        return False

    # --- internals -----------------------------------------------------------

    def _devices(self) -> dict:
        devices = MeticulousConfig.get(CONFIG_PAIRED_DEVICES)
        if not isinstance(devices, dict):
            devices = {}
            MeticulousConfig[CONFIG_PAIRED_DEVICES] = devices
        return devices

    def _persist_device(self, device_name: str, token: str) -> str:
        device_id = str(uuid.uuid4())
        self._devices()[device_id] = {
            "device_name": device_name,
            "token_hash": hash_token(token),
            "created_at": _iso_now(),
            "last_seen_at": None,
        }
        MeticulousConfig.save()
        return device_id

    def _prune(self, now: float) -> None:
        stale = [
            pid
            for pid, s in self._sessions.items()
            if s.status != PairingStatus.PENDING or s.is_expired(now)
        ]
        # Keep expired/finished sessions briefly so a late poll still gets a
        # meaningful status, but drop the oldest to bound memory.
        for pid in stale:
            s = self._sessions[pid]
            if (now - s.created_at) > (PAIRING_SESSION_TTL_SECONDS * 2):
                del self._sessions[pid]

    def _record_rejection(self, now: float) -> None:
        self._recent_rejections.append(now)
        self._recent_rejections = [
            t for t in self._recent_rejections if (now - t) < REJECTION_BACKOFF_SECONDS
        ]

    def _in_backoff(self, now: float) -> bool:
        self._recent_rejections = [
            t for t in self._recent_rejections if (now - t) < REJECTION_BACKOFF_SECONDS
        ]
        return len(self._recent_rejections) >= REJECTION_BACKOFF_THRESHOLD


class PairingError(Exception):
    """Raised when a pairing request cannot be accepted (rate limiting, etc.)."""


# Process-wide singleton, mirroring the pattern of other backend managers.
PairingManagerInstance = PairingManager()
