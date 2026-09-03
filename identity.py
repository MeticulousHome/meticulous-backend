"""Per-machine cryptographic identity (phase 1).

Every machine generates one ECDSA P-256 key at first boot and keeps it in a
root-only file outside config.yml. The public key and its SHA-256 fingerprint
are published on the already-public GET /api/v1/machine, and POST
/api/v1/identity/challenge signs a client nonce bound to the machine serial and
to the origin the client is talking to. A client pins the fingerprint when it
receives a token and, from then on, never sends that token to an origin that has
not just proven possession of the pinned key.

This closes ADV-006 (substitute server at the machine's address after DHCP
reuse / mDNS spoof) and the substitute-server half of ADV-002. It does NOT close
the on-path half of ADV-002 (needs TLS, phase 2) nor same-origin browser pages.

Design: DESIGN-MACHINE-IDENTITY-P256.md, with the panel review corrections
(60 s TTL is a client concern; own-address bypass decoupled from Machine.emulated
onto IDENTITY_ALLOW_ANY_ORIGIN/IDENTITY_OWN_ADDRESSES; absence vs corruption on
boot with a mirror copy; length-prefix bound; served scheme/port only).

No hardware dependencies, so it is unit-testable directly.
"""

import base64
import hashlib
import ipaddress
import os
import time
from typing import Optional
from urllib.parse import urlsplit

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

# Domain separator for the signed message. Versioned so a future message layout
# cannot be confused with this one.
DOMAIN = b"meticulous-machine-identity/v1"
ALG = "ES256"

# secp256r1 group order, for low-S normalization (see sign()).
_P256_ORDER = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551


class IdentityIOError(Exception):
    """A transient failure reading a key file that DOES exist (EIO, EBUSY,
    permission). Distinct from corruption: we must fail closed and let startup
    retry, never regenerate the key and revoke every device over a flaky read."""


# Where the key lives. Must NOT start with a dot: factory reset is
# `rm -rf /meticulous-user/*`, which skips dotfiles, and we WANT the key wiped
# on factory reset (together with paired devices).
IDENTITY_PATH = os.environ.get("IDENTITY_PATH", "/meticulous-user/identity/")
_KEY_NAME = "machine_key.pem"
_MIRROR_NAME = "machine_key.mirror.pem"

# Own-address set caching.
_OWN_ADDR_TTL_S = 5.0


def _now() -> float:
    return time.monotonic()


def _lp(b: bytes) -> bytes:
    """2-byte big-endian length prefix. An over-long field is a hard error: it
    must never silently wrap the 2-byte length and forge a different message."""
    if len(b) >= 65536:
        raise ValueError(f"length-prefixed field too long: {len(b)} bytes")
    return len(b).to_bytes(2, "big") + b


def build_message(serial: str, origin: str, nonce_bytes: bytes) -> bytes:
    """The exact bytes signed by /identity/challenge. Every field is length
    prefixed so no boundary is ambiguous. serial/origin are UTF-8; nonce is the
    raw 32 bytes, not its base64."""
    return (
        _lp(DOMAIN)
        + _lp(serial.encode("utf-8"))
        + _lp(origin.encode("utf-8"))
        + _lp(nonce_bytes)
    )


class OriginError(ValueError):
    """Raised when an origin string is not a valid, canonicalizable origin."""


def canonical_origin(url_or_origin: str) -> str:
    """The single byte-exact origin canonicalization, shared with the TS client.

    Rules (must match canonicalOrigin() in meticulous-typescript-api exactly):
    - scheme lowercased, must be http or https
    - host lowercased, trailing dot stripped
    - IPv6 literal lowercased and compressed, re-bracketed; zone-ids rejected
    - default port omitted (80 for http, 443 for https), otherwise kept
    - userinfo, path, query, fragment are rejected (an origin has none)
    """
    if not url_or_origin or len(url_or_origin.encode("utf-8")) >= 65536:
        raise OriginError("empty or over-long origin")
    parts = urlsplit(url_or_origin.strip())
    scheme = parts.scheme.lower()
    if scheme not in ("http", "https"):
        raise OriginError(f"unsupported scheme: {scheme!r}")
    if parts.username or parts.password:
        raise OriginError("origin must not contain userinfo")
    if parts.path not in ("", "/") or parts.query or parts.fragment:
        raise OriginError("origin must not contain path, query or fragment")
    host = parts.hostname  # already lowercased by urlsplit
    if not host:
        raise OriginError("origin has no host")
    if "%" in host:
        raise OriginError("IPv6 zone-ids are not allowed")
    host = host.rstrip(".")  # canonicalize trailing-dot FQDN
    if not host:
        raise OriginError("origin has no host")
    bracketed = host
    # IPv6 literal? urlsplit strips the brackets in .hostname.
    try:
        ip = ipaddress.ip_address(host)
        if ip.version == 6:
            bracketed = f"[{ip.compressed}]"
        else:
            bracketed = ip.compressed
    except ValueError:
        pass  # a DNS name, not an IP literal
    try:
        port = parts.port
    except ValueError:
        raise OriginError("invalid port")
    default = 80 if scheme == "http" else 443
    if port is None or port == default:
        return f"{scheme}://{bracketed}"
    return f"{scheme}://{bracketed}:{port}"


def origin_host_port(origin: str):
    """(scheme, bare_host, effective_port) for a canonical origin. bare_host has
    IPv6 brackets removed. Used by the ownership/served check."""
    parts = urlsplit(origin)
    scheme = parts.scheme.lower()
    host = (parts.hostname or "").rstrip(".")
    default = 80 if scheme == "http" else 443
    try:
        port = parts.port or default
    except ValueError:
        port = default
    return scheme, host, port


def _env_own_addresses() -> Optional[set]:
    raw = os.environ.get("IDENTITY_OWN_ADDRESSES")
    if not raw:
        return None
    return {a.strip().lower() for a in raw.split(",") if a.strip()}


def allow_any_origin() -> bool:
    return os.environ.get("IDENTITY_ALLOW_ANY_ORIGIN", "").lower() in ("1", "true", "yes")


class IdentityManager:
    """Singleton owning the machine private key and the challenge signing."""

    def __init__(self):
        self._private_key = None
        self._spki_der = b""
        self._fingerprint = ""
        self._generation = 0
        self._own_addrs = set()
        self._own_addrs_at = 0.0

    # --- lifecycle -----------------------------------------------------------

    def _paths(self):
        return (
            os.path.join(IDENTITY_PATH, _KEY_NAME),
            os.path.join(IDENTITY_PATH, _MIRROR_NAME),
        )

    def _require_root(self) -> bool:
        """Root confinement is enforced only on the real device path. A test or
        dev run overrides IDENTITY_PATH (or sets IDENTITY_ALLOW_ANY_ORIGIN), and
        Windows has no geteuid at all."""
        if allow_any_origin():
            return False
        if os.environ.get("IDENTITY_PATH"):  # explicitly overridden -> dev/test
            return False
        return True

    def load_or_create(self) -> None:
        """Startup entry point. Three distinct outcomes per file, never conflated:
        'ok' (a valid P-256 key), 'absent' (no such file), 'corrupt' (a file that
        reads but is not a valid key). A transient read error on a file that
        exists raises IdentityIOError and fails startup closed -- it must never
        be mistaken for corruption and trigger a regenerate+revoke."""
        key_path, mirror_path = self._paths()
        pstate, pkey = self._load_key(key_path)
        if pstate == "ok":
            self._generation = self._read_generation()
            self._adopt(pkey)
            if not os.path.exists(os.path.join(IDENTITY_PATH, "identity.json")):
                self._write_sidecars()  # do not lose the generation counter
            return

        mstate, mkey = self._load_key(mirror_path)

        if pstate == "absent" and mstate == "absent":
            # True first boot: no key anywhere. Generate quietly, no revoke.
            logger.info("No machine identity yet; generating one (first boot).")
            self._generate_and_persist(revoke_devices=False)
            return

        if mstate == "ok":
            logger.warning("Primary machine identity unreadable; recovered from mirror copy.")
            self._generation = self._read_generation()
            self._adopt(mkey)
            self._restore_primary_from_current()
            self._write_sidecars()  # rewrite so the counter is not lost
            return

        # Neither copy is a valid key and at least one is corrupt (a transient
        # IO error would already have raised). Confirmed corruption: the only
        # path that regenerates and revokes, because a token must never outlive
        # the identity it was pinned to.
        logger.error(
            "Machine identity corrupt on both primary and mirror; regenerating "
            "and revoking all paired devices."
        )
        try:
            import sentry_sdk

            sentry_sdk.capture_message(
                f"machine identity corrupt (generation={self._read_generation()}), regenerated"
            )
        except Exception:
            pass
        if os.path.exists(key_path):
            self._quarantine(key_path)
        if os.path.exists(mirror_path):
            self._quarantine(mirror_path)
        self._generate_and_persist(revoke_devices=True)

    def _load_key(self, path: str):
        """Return (state, key): ('ok', key) | ('absent', None) | ('corrupt', None).
        Raises IdentityIOError on a transient read failure of a file that exists."""
        try:
            with open(path, "rb") as f:
                data = f.read()
        except FileNotFoundError:
            return ("absent", None)
        except OSError as e:
            # The file exists but could not be read: a transient fault, not
            # corruption. Fail closed rather than regenerate + revoke.
            raise IdentityIOError(
                f"transient read failure on {os.path.basename(path)}: {type(e).__name__}"
            ) from e
        try:
            key = serialization.load_pem_private_key(data, password=None)
        except Exception:
            logger.warning(f"Machine identity at {os.path.basename(path)} is not a valid key.")
            return ("corrupt", None)
        if not isinstance(key, ec.EllipticCurvePrivateKey) or not isinstance(
            key.curve, ec.SECP256R1
        ):
            logger.warning("Machine identity is not a P-256 key; treating as corrupt.")
            return ("corrupt", None)
        return ("ok", key)

    def _adopt(self, key) -> None:
        # Does NOT set self._generation: the caller owns it (load reads it from
        # disk, generate increments it before writing the sidecars).
        self._private_key = key
        self._spki_der = key.public_key().public_bytes(
            serialization.Encoding.DER,
            serialization.PublicFormat.SubjectPublicKeyInfo,
        )
        self._fingerprint = hashlib.sha256(self._spki_der).hexdigest()
        logger.info(
            f"Machine identity ready: fingerprint={self._fingerprint[:8]}... "
            f"generation={self._generation}"
        )

    def _read_generation(self) -> int:
        try:
            import json

            with open(os.path.join(IDENTITY_PATH, "identity.json")) as f:
                return int(json.load(f).get("generation", 0))
        except Exception:
            return 0

    def _generate_and_persist(self, revoke_devices: bool) -> None:
        if self._require_root():
            euid = getattr(os, "geteuid", lambda: 0)()
            if euid != 0:
                raise RuntimeError(
                    "refusing to generate the machine identity as non-root: the "
                    "0700 confinement and factory-reset assume root"
                )
        os.makedirs(IDENTITY_PATH, mode=0o700, exist_ok=True)
        try:
            os.chmod(IDENTITY_PATH, 0o700)
        except OSError:
            pass
        key = ec.generate_private_key(ec.SECP256R1())
        pem = key.private_bytes(
            serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption(),
        )
        self._generation = self._read_generation() + 1
        key_path, mirror_path = self._paths()
        self._atomic_write(key_path, pem, 0o600)
        self._atomic_write(mirror_path, pem, 0o600)
        self._adopt(key)
        self._write_sidecars()
        if revoke_devices:
            # Revoke first, in its own block: a token must never outlive the
            # identity it was pinned to, and a failure to also drop live sockets
            # must not skip the revocation.
            try:
                from pairing import PairingManagerInstance

                PairingManagerInstance.revoke_all()
            except Exception as e:
                logger.error(
                    f"Failed to revoke devices after identity change: {type(e).__name__}"
                )
            try:
                import socket_registry

                socket_registry.disconnect_all_devices()
            except Exception as e:
                logger.error(
                    f"Failed to disconnect sockets after identity change: {type(e).__name__}"
                )

    def _restore_primary_from_current(self) -> None:
        key_path, _ = self._paths()
        pem = self._private_key.private_bytes(
            serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption(),
        )
        self._atomic_write(key_path, pem, 0o600)

    def _atomic_write(self, path: str, data: bytes, mode: int) -> None:
        tmp = path + ".tmp"
        try:
            os.unlink(tmp)
        except OSError:
            pass
        fd = os.open(tmp, os.O_CREAT | os.O_EXCL | os.O_WRONLY, mode)
        try:
            os.write(fd, data)
            os.fsync(fd)
        finally:
            os.close(fd)
        os.replace(tmp, path)
        try:
            dirfd = os.open(os.path.dirname(path), os.O_RDONLY)
            try:
                os.fsync(dirfd)
            finally:
                os.close(dirfd)
        except OSError:
            pass  # directory fsync is best-effort (not supported on all platforms)

    def _write_sidecars(self) -> None:
        import json

        # Public data only. Kept inside the 0700 dir (root-readable); the public
        # key is also on GET /machine, so nothing non-root needs these files.
        self._atomic_write(
            os.path.join(IDENTITY_PATH, "machine_key.pub"),
            (base64.b64encode(self._spki_der) + b"\n"),
            0o644,
        )
        self._atomic_write(
            os.path.join(IDENTITY_PATH, "identity.json"),
            json.dumps(
                {
                    "alg": ALG,
                    "fingerprint": self._fingerprint,
                    "generation": self._generation,
                }
            ).encode("utf-8"),
            0o644,
        )

    def _quarantine(self, path: str) -> None:
        try:
            os.replace(path, f"{path}.broken-{int(time.time())}")
        except OSError as e:
            logger.error(f"Could not quarantine corrupt key: {type(e).__name__}")

    def rotate(self) -> None:
        """Operator-initiated identity reset (Dial 'Reset machine identity', or
        the recovery path). New key, generation+1, revoke every device."""
        logger.warning("Rotating machine identity (all paired devices revoked).")
        key_path, mirror_path = self._paths()
        if os.path.exists(key_path):
            self._quarantine(key_path)
        if os.path.exists(mirror_path):
            self._quarantine(mirror_path)
        self._generate_and_persist(revoke_devices=True)

    # --- public accessors ----------------------------------------------------

    def is_ready(self) -> bool:
        return self._private_key is not None

    def public_key_spki_b64(self) -> str:
        return base64.b64encode(self._spki_der).decode("ascii")

    def fingerprint_hex(self) -> str:
        return self._fingerprint

    def generation(self) -> int:
        return self._generation

    def identity_dict(self) -> dict:
        return {
            "alg": ALG,
            "public_key": self.public_key_spki_b64(),
            "fingerprint": self._fingerprint,
        }

    def sign(self, serial: str, origin: str, nonce_bytes: bytes) -> str:
        """Sign build_message() and return the signature as IEEE P1363 r||s
        (64 bytes), base64. This is the form WebCrypto subtle.verify and
        @noble/curves p256.verify expect (not DER)."""
        message = build_message(serial or "", origin, nonce_bytes)
        der = self._private_key.sign(message, ec.ECDSA(hashes.SHA256()))
        r, s = decode_dss_signature(der)
        # Low-S normalization. OpenSSL emits a high-S signature ~50% of the time;
        # @noble/curves p256.verify (the verifier the insecure-context browser and
        # mobile/Hermes clients use) REJECTS high-S by default, so an
        # un-normalized signature would fail on half of all genuine challenges.
        if s > _P256_ORDER // 2:
            s = _P256_ORDER - s
        p1363 = r.to_bytes(32, "big") + s.to_bytes(32, "big")
        return base64.b64encode(p1363).decode("ascii")

    # --- origin ownership ----------------------------------------------------

    def own_addresses(self) -> set:
        """The set of host strings this machine legitimately answers as. An
        override (env) replaces enumeration so the Docker N-suite exercises the
        real host-match logic against a synthetic set (must-fix 6)."""
        override = _env_own_addresses()
        if override is not None:
            return override
        now = _now()
        if self._own_addrs and (now - self._own_addrs_at) < _OWN_ADDR_TTL_S:
            return self._own_addrs
        addrs = {"localhost", "127.0.0.1", "::1"}
        try:
            import psutil

            for _iface, snics in psutil.net_if_addrs().items():
                for snic in snics:
                    addr = (snic.address or "").split("%")[0].strip().lower()
                    if addr:
                        try:
                            addrs.add(ipaddress.ip_address(addr).compressed)
                        except ValueError:
                            addrs.add(addr)
        except Exception as e:
            logger.warning(f"Could not enumerate interfaces: {type(e).__name__}")
        # AP-mode and USB-gadget addresses the app hardcodes.
        addrs.update({"10.42.0.1", "10.42.1.1"})
        try:
            from wifi import WifiManager

            hostname = (WifiManager.getCurrentConfig().hostname or "").rstrip(".").lower()
            if hostname:
                addrs.add(hostname)
                addrs.add(f"{hostname}.local")
        except Exception:
            pass
        try:
            import socket as _socket

            h = _socket.gethostname().rstrip(".").lower()
            if h:
                addrs.add(h)
                addrs.add(f"{h}.local")
        except Exception:
            pass
        self._own_addrs = addrs
        self._own_addrs_at = now
        return addrs


def client_key_fingerprint(spki_b64: str) -> str:
    """Validate a client-supplied P-256 public key (SPKI DER, base64) and return
    its SHA-256 fingerprint (hex). Raises ValueError on anything that is not a
    parsable P-256 SPKI. Used by /pair/verify to reserve client_public_key for
    phase 2 (no proof of possession is checked in phase 1)."""
    der = base64.b64decode(spki_b64, validate=True)
    key = serialization.load_der_public_key(der)
    if not isinstance(key, ec.EllipticCurvePublicKey) or not isinstance(
        key.curve, ec.SECP256R1
    ):
        raise ValueError("client key is not P-256")
    return hashlib.sha256(der).hexdigest()


IdentityManagerInstance = IdentityManager()
