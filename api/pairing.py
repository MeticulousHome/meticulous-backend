"""HTTP endpoints for device pairing (M3).

Flow:
  GET  /pair                  -> the standalone pairing page (a browser fallback)
  POST /pair/request          -> opens a session, shows the code on the Dial,
                                 returns {pairing_id} (the code is NOT returned;
                                 it is shown only on the Dial)
  POST /pair/verify           -> {pairing_id, code}; if the typed code matches
                                 the one on the Dial, returns {token}
  GET  /pair/status/{id}      -> {status} for a session (polling)
  GET  /pair/devices          -> list of paired devices (management; auth/loopback)
  POST /pair/devices/{id}/revoke -> revoke a device (management; auth/loopback)

There is exactly ONE way to approve a device: type the code shown on the Dial
(POST /pair/verify). The Dial only displays the code (plus a Cancel that rejects
the pending request); it does not offer a second "Allow" that would authorize by
itself. A paired device is later managed from Settings -> Paired Devices.

/pair (GET), /pair/request, /pair/verify and /pair/status are reachable without
a token (an unpaired device has none yet); the request-layer enforcement
allowlists them. The management endpoints are not allowlisted, so they require a
token or loopback (the Dial's "Paired devices" screen calls them over loopback).
"""

import json

import socket_registry
from notifications import Notification, NotificationManager
from pairing import PairingError, PairingManagerInstance
from log import MeticulousLogger

from .auth import extract_token
from .base_handler import BaseHandler, LocalAccessHandler
from .api import API, APIVersion
import identity
from identity import IdentityManagerInstance
from config import MeticulousConfig, CONFIG_SYSTEM, MACHINE_SERIAL_NUMBER

logger = MeticulousLogger.getLogger(__name__)


def _clean_device_name(raw) -> str:
    """One printable line, bounded. The name is interpolated into the Dial
    prompt, so newlines/control characters would let a requester forge extra
    prompt text (e.g. a fake instruction line)."""
    name = "".join(ch for ch in str(raw or "") if ch.isprintable())
    name = " ".join(name.split())[:64]
    return name or "Unknown device"


def _client_source(handler) -> str:
    """Where a pairing request came from, for per-source rate limiting: the
    proxy-set client address, else the socket peer."""
    return handler.request.headers.get("X-Real-IP") or handler.request.remote_ip or "?"


def _format_code(code: str) -> str:
    # "482913" -> "482 913" for legibility on the Dial prompt.
    return f"{code[:3]} {code[3:]}" if len(code) == 6 else code


# Open pairing prompts, keyed by pairing_id, so we can clear the Dial prompt
# once the device is authorized (or denied) instead of leaving a stale code.
_prompts: dict = {}


def _push_approval_prompt(pairing_id: str, device_name: str, code: str) -> None:
    """Display the pairing code on the Dial.

    There is exactly one way to approve a device: type this code on it
    (POST /pair/verify). The Dial therefore does NOT offer an "Allow" that would
    authorize by itself -- that would be a second, contradictory approval path.
    It shows the code plus a single "Cancel" that rejects the pending request
    (only meaningful before the code is typed; once typed, the device is paired
    and is managed from Settings -> Paired Devices).

    English only.
    """
    message = (
        f"A device wants to connect:\n{device_name}\n\n"
        f"To allow it, type this code on the device:\n{_format_code(code)}"
    )
    # sensitive: the body carries the pairing code and answering it grants
    # access, so it is Dial-only (never listed/broadcast/acknowledged remotely).
    notification = Notification(message, responses=["Cancel"], sensitive=True)

    def on_answer():
        # The only button cancels the pending request. Idempotent and a no-op
        # once the session is no longer pending (already approved/expired).
        PairingManagerInstance.deny(pairing_id)
        _prompts.pop(pairing_id, None)

    notification.callback = on_answer
    _prompts[pairing_id] = notification
    NotificationManager.add_notification(notification)


def _dismiss_prompt(pairing_id: str) -> None:
    """Clear the Dial pairing prompt for a session (after it is authorized).

    Revokes the on-screen notification by re-emitting it with an empty body,
    which the Dial treats as a dismissal; harmless if the Dial ignores it, as
    the pairing session expires on its own shortly after.
    """
    notification = _prompts.pop(pairing_id, None)
    if notification is None:
        return
    # Dial-only like the prompt it clears; the empty body is a removal.
    revoke = Notification("", responses=[], sensitive=True)
    revoke.id = notification.id
    NotificationManager.add_notification(revoke)


class PairRequestHandler(BaseHandler):
    def post(self):
        try:
            data = json.loads(self.request.body or "{}")
        except json.JSONDecodeError:
            self.report_error(400, "Invalid JSON body")
            return

        device_name = _clean_device_name(data.get("device_name"))
        try:
            session = PairingManagerInstance.request_pairing(
                device_name, source=_client_source(self)
            )
        except PairingError as e:
            # Too many attempts / pending sessions -> rate limited.
            self.report_error(429, str(e))
            return

        _push_approval_prompt(session["pairing_id"], device_name, session["code"])
        # The code is deliberately NOT returned: it is shown only on the Dial, so
        # a client proves physical sight of the machine by typing it back to
        # /pair/verify. Typing the code is the ONLY way to approve.
        self.write(
            {
                "pairing_id": session["pairing_id"],
                "expires_in": session["expires_in"],
            }
        )


class PairVerifyHandler(BaseHandler):
    def post(self):
        try:
            data = json.loads(self.request.body or "{}")
        except json.JSONDecodeError:
            self.report_error(400, "Invalid JSON body")
            return
        pairing_id = data.get("pairing_id")
        code = data.get("code")
        if not pairing_id or not code:
            self.report_error(400, "pairing_id and code are required")
            return
        # Never cache a response that carries the token.
        self.set_header("Cache-Control", "no-store")

        # Optional: reserve a client public key for phase 2 (no proof of
        # possession is checked now). An invalid key is rejected BEFORE the code
        # is verified, so it never burns the pairing session.
        client_public_key = data.get("client_public_key")
        client_key_fpr = None
        if client_public_key is not None:
            if data.get("client_key_alg") not in (None, "ES256"):
                self.report_error(400, "invalid_client_public_key")
                return
            try:
                client_key_fpr = identity.client_key_fingerprint(client_public_key)
            except Exception:
                self.report_error(400, "invalid_client_public_key")
                return

        token = PairingManagerInstance.verify_code(
            pairing_id,
            code,
            client_public_key=client_public_key,
            client_key_fingerprint=client_key_fpr,
        )
        if token is None:
            # Unknown/expired session or wrong code. Deliberately NOT 401: for
            # clients 401 means "your credential was revoked", and a mistyped
            # code must not make them discard a valid token.
            self.set_status(400)
            self.write({"error": "invalid_code", "message": "Invalid or expired code"})
            return
        # Approved: clear the code prompt from the Dial so it does not linger.
        _dismiss_prompt(pairing_id)
        response = {
            "status": "approved",
            "token": token,
            "device_id": PairingManagerInstance.approved_device_id(pairing_id),
            "serial": MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] or "",
        }
        if IdentityManagerInstance.is_ready():
            # The client pins this fingerprint and, before ever sending the
            # token, requires the origin to sign a fresh nonce under this key.
            response["identity"] = IdentityManagerInstance.identity_dict()
        self.write(response)


class PairStatusHandler(BaseHandler):
    def get(self, pairing_id):
        self.write(PairingManagerInstance.poll(pairing_id))


class PairPageHandler(BaseHandler):
    """Self-contained standalone pairing page.

    Served by the machine at its own origin, this is the fallback for any client
    that cannot pair on its own (a browser, an app that predates pairing): open
    the machine's address, request pairing, read the code shown on the Dial,
    type it here. The token is kept in this origin's localStorage so first-party
    web tools reuse it. No build step and no external assets.
    """

    def get(self):
        self.set_header("Content-type", "text/html; charset=utf-8")
        self.write(_PAIR_PAGE_HTML)


# Credential administration (list / revoke others / revoke all) is a
# machine-owner power, exercised at the Dial over loopback. An ordinary paired
# client must not be able to inventory or disconnect other devices, so these
# are loopback-only (LocalAccessHandler decides from the real peer).
class PairedDevicesHandler(LocalAccessHandler):
    def get(self):
        self.write({"devices": PairingManagerInstance.list_devices()})


class PairedDeviceSelfRevokeHandler(BaseHandler):
    """Let a paired client revoke ITS OWN token ("forget this machine").
    Ordinary token holders may do this and nothing else administrative."""

    def post(self):
        token = extract_token(
            self.request.headers.get("Authorization"), self.request.headers.get("Cookie")
        )
        device_id = PairingManagerInstance.verify_token(token)
        if device_id is None:
            self.report_error(401, "No paired device for this credential")
            return
        PairingManagerInstance.revoke(device_id)
        socket_registry.disconnect_device(device_id)
        self.write({"status": "success", "device_id": device_id})


class PairedDeviceRevokeHandler(LocalAccessHandler):
    def post(self, device_id):
        if PairingManagerInstance.revoke(device_id):
            # End any live sockets that device authorized, so revoking actually
            # stops the telemetry stream (Socket.IO only checks the token at the
            # handshake).
            socket_registry.disconnect_device(device_id)
            self.write({"status": "success"})
        else:
            self.report_error(404, "Device not found")


class PairedDevicesRevokeAllHandler(LocalAccessHandler):
    def post(self):
        count = PairingManagerInstance.revoke_all()
        socket_registry.disconnect_all_devices()
        self.write({"status": "success", "revoked": count})


API.register_handler(APIVersion.V1, r"/pair", PairPageHandler)
API.register_handler(APIVersion.V1, r"/pair/request", PairRequestHandler)
API.register_handler(APIVersion.V1, r"/pair/verify", PairVerifyHandler)
API.register_handler(APIVersion.V1, r"/pair/status/([^/]+)", PairStatusHandler)
API.register_handler(APIVersion.V1, r"/pair/devices", PairedDevicesHandler)
API.register_handler(APIVersion.V1, r"/pair/devices/revoke-all", PairedDevicesRevokeAllHandler)
API.register_handler(APIVersion.V1, r"/pair/devices/self/revoke", PairedDeviceSelfRevokeHandler)
API.register_handler(
    APIVersion.V1, r"/pair/devices/(?!self/)([^/]+)/revoke", PairedDeviceRevokeHandler
)


# Self-contained standalone pairing page (served by PairPageHandler). Kept as a
# module constant so there is no extra file to ship. All CSS/JS inline; the only
# network calls are same-origin to /api/v1/pair/*.
_PAIR_PAGE_HTML = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Authorize this device - Meticulous</title>
<style>
  :root { color-scheme: light dark; }
  * { box-sizing: border-box; }
  body {
    margin: 0; min-height: 100vh; display: flex; align-items: center;
    justify-content: center; padding: 24px;
    font: 16px/1.5 system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
    background: #f5f5f4; color: #1c1c1a;
  }
  @media (prefers-color-scheme: dark) {
    body { background: #14140f; color: #f0efe9; }
    .card { background: #201f1a !important; border-color: #35342c !important; }
    input { background: #14140f !important; color: inherit !important; border-color: #45443a !important; }
  }
  .card {
    width: 100%; max-width: 400px; background: #fff; border: 1px solid #e3e2dc;
    border-radius: 16px; padding: 28px; box-shadow: 0 8px 30px rgba(0,0,0,.06);
  }
  h1 { font-size: 1.25rem; margin: 0 0 6px; }
  p { margin: 0 0 18px; color: #6b6a62; }
  @media (prefers-color-scheme: dark) { p { color: #a4a399; } }
  button {
    width: 100%; padding: 13px 16px; border: 0; border-radius: 10px;
    background: #b5361f; color: #fff; font-size: 1rem; font-weight: 600;
    cursor: pointer;
  }
  button:disabled { opacity: .5; cursor: default; }
  button.secondary {
    margin-top: 10px; background: transparent; color: inherit; border: 1px solid #a7a69f;
  }
  input {
    width: 100%; padding: 13px 16px; border: 1px solid #d8d7d0; border-radius: 10px;
    font-size: 1.5rem; letter-spacing: .35em; text-align: center; margin-bottom: 14px;
    font-variant-numeric: tabular-nums;
  }
  .hidden { display: none; }
  .status { margin-top: 16px; font-size: .95rem; min-height: 1.4em; }
  .ok { color: #1a7f45; } .err { color: #c0392b; }
  @media (prefers-color-scheme: dark) { .ok { color: #4ec27f; } .err { color: #ff6b5b; } }
  .steps { font-size: .95rem; color: #6b6a62; margin: 0 0 18px; }
</style>
</head>
<body>
<div class="card">
  <h1>Authorize this device</h1>

  <div id="intro">
    <p>To let this device control your Meticulous machine, approve it on the machine's screen.</p>
    <button id="startBtn">Authorize this device</button>
  </div>

  <div id="codeStep" class="hidden">
    <p class="steps">Your machine is showing a 6-digit code. Enter it below to finish.</p>
    <input id="codeInput" inputmode="numeric" maxlength="6" autocomplete="one-time-code"
           placeholder="000000" aria-label="Code from the machine screen">
    <button id="verifyBtn">Authorize</button>
  </div>

  <div id="done" class="hidden">
    <p class="ok">This device is now authorized. You can use the Meticulous web tools.</p>
    <button id="restartBtn" class="secondary">Authorize again</button>
  </div>

  <div id="status" class="status" role="status"></div>
</div>

<script>
(function () {
  var CRED_KEY = "meticulous.machineCredential";
  var STORAGE_TEST_KEY = CRED_KEY + ".storage-test";
  var pairingId = null, expiryTimer = null, pairingRevision = 0, verifying = false;
  var invalidStoredCredential = false;
  var intro = document.getElementById("intro");
  var codeStep = document.getElementById("codeStep");
  var done = document.getElementById("done");
  var status = document.getElementById("status");
  var startBtn = document.getElementById("startBtn");
  var restartBtn = document.getElementById("restartBtn");
  var verifyBtn = document.getElementById("verifyBtn");
  var codeInput = document.getElementById("codeInput");

  function say(msg, cls) { status.textContent = msg || ""; status.className = "status " + (cls || ""); }

  function validCredential(value) {
    return value && typeof value === "object" &&
      typeof value.token === "string" && value.token.length > 0 &&
      typeof value.serial === "string" && value.serial.length > 0 &&
      typeof value.fingerprint === "string" && /^[0-9a-f]{64}$/i.test(value.fingerprint) &&
      typeof (value.public_key || value.publicKey) === "string" &&
      (value.public_key || value.publicKey).length > 0;
  }

  function storageIsWritable() {
    try {
      localStorage.setItem(STORAGE_TEST_KEY, "1");
      localStorage.removeItem(STORAGE_TEST_KEY);
      return true;
    } catch (e) {
      return false;
    }
  }

  function readCredential() {
    try {
      var raw = localStorage.getItem(CRED_KEY);
      if (!raw) return null;
      var credential = JSON.parse(raw);
      if (!validCredential(credential)) {
        invalidStoredCredential = true;
        localStorage.removeItem(CRED_KEY);
        return null;
      }
      return credential;
    } catch (e) {
      invalidStoredCredential = true;
      try { localStorage.removeItem(CRED_KEY); } catch (ignored) {}
      return null;
    }
  }

  function saveCredential(data) {
    var credential = {
      token: data && data.token,
      serial: data && data.serial,
      fingerprint: data && data.identity && data.identity.fingerprint,
      public_key: data && data.identity && data.identity.public_key
    };
    if (!validCredential(credential)) {
      throw new Error("The machine returned incomplete authorization. Please start again.");
    }
    try {
      // No cookie: a cookie rides plain navigations with no identity check.
      localStorage.setItem(CRED_KEY, JSON.stringify(credential));
    } catch (e) {
      throw new Error("This browser could not save authorization. Enable site storage and try again.");
    }
  }

  function showIntro(message, cls) {
    if (expiryTimer) clearTimeout(expiryTimer);
    expiryTimer = null;
    pairingId = null;
    verifying = false;
    codeInput.value = "";
    verifyBtn.disabled = false;
    startBtn.disabled = false;
    restartBtn.disabled = false;
    codeStep.classList.add("hidden");
    done.classList.add("hidden");
    intro.classList.remove("hidden");
    say(message, cls);
  }

  // Already authorized on this origin?
  if (!storageIsWritable()) {
    startBtn.disabled = true;
    say("This browser cannot save authorization. Enable site storage and reload this page.", "err");
  } else if (readCredential()) {
    intro.classList.add("hidden");
    done.classList.remove("hidden");
    say("This device was already authorized.", "ok");
  } else if (invalidStoredCredential) {
    say("The saved authorization was invalid. Authorize this device again.", "err");
  }

  function startPairing() {
    if (!storageIsWritable()) {
      showIntro("This browser cannot save authorization. Enable site storage and reload this page.", "err");
      startBtn.disabled = true;
      return;
    }
    pairingRevision += 1;
    var revision = pairingRevision;
    startBtn.disabled = true;
    restartBtn.disabled = true;
    done.classList.add("hidden");
    intro.classList.remove("hidden");
    say("Asking the machine...");
    fetch("/api/v1/pair/request", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ device_name: "Web browser" })
    }).then(function (r) {
      if (r.status === 429) throw new Error("Too many attempts. Wait a moment and try again.");
      if (!r.ok) throw new Error("Could not reach the machine.");
      return r.json();
    }).then(function (d) {
      if (revision !== pairingRevision) return;
      if (!d || typeof d.pairing_id !== "string" || !d.pairing_id) {
        throw new Error("The machine returned an invalid pairing session. Please try again.");
      }
      pairingId = d.pairing_id;
      intro.classList.add("hidden");
      done.classList.add("hidden");
      codeStep.classList.remove("hidden");
      codeInput.focus();
      say("Enter the code shown on your machine's screen.");
      var secs = d.expires_in || 60;
      expiryTimer = setTimeout(function () {
        if (revision !== pairingRevision) return;
        pairingRevision += 1;
        showIntro("That code expired. Please start again.", "err");
      }, secs * 1000);
    }).catch(function (e) {
      if (revision === pairingRevision) showIntro(e.message, "err");
    });
  }

  startBtn.addEventListener("click", startPairing);
  restartBtn.addEventListener("click", startPairing);

  function verify() {
    if (verifying || !pairingId) return;
    var code = (codeInput.value || "").replace(/[^0-9]/g, "");
    if (code.length !== 6) { say("Enter the 6-digit code.", "err"); return; }
    var revision = pairingRevision;
    var approved = false;
    var verifyingPairingId = pairingId;
    verifying = true;
    verifyBtn.disabled = true;
    say("Checking...");
    fetch("/api/v1/pair/verify", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ pairing_id: verifyingPairingId, code: code })
    }).then(function (r) {
      if (r.status === 400 || r.status === 401) throw new Error("That code is wrong or expired.");
      if (!r.ok) throw new Error("Could not verify the code.");
      return r.json();
    }).then(function (d) {
      if (revision !== pairingRevision) return;
      approved = true;
      saveCredential(d);
      if (expiryTimer) clearTimeout(expiryTimer);
      expiryTimer = null;
      verifying = false;
      restartBtn.disabled = false;
      codeStep.classList.add("hidden");
      intro.classList.add("hidden");
      done.classList.remove("hidden");
      say("This device is now authorized.", "ok");
    }).catch(function (e) {
      if (revision !== pairingRevision) return;
      if (approved) {
        pairingRevision += 1;
        showIntro(e.message, "err");
        return;
      }
      verifying = false;
      verifyBtn.disabled = false;
      say(e.message, "err");
    });
  }

  verifyBtn.addEventListener("click", verify);
  codeInput.addEventListener("keydown", function (e) { if (e.key === "Enter") verify(); });
})();
</script>
</body>
</html>
"""
