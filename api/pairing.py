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

from notifications import Notification, NotificationManager
from pairing import PairingError, PairingManagerInstance
from log import MeticulousLogger

from .base_handler import BaseHandler
from .api import API, APIVersion

logger = MeticulousLogger.getLogger(__name__)


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
    notification = Notification(message, responses=["Cancel"])

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
    revoke = Notification("", responses=[])
    revoke.id = notification.id
    NotificationManager.add_notification(revoke)


class PairRequestHandler(BaseHandler):
    def post(self):
        try:
            data = json.loads(self.request.body or "{}")
        except json.JSONDecodeError:
            self.report_error(400, "Invalid JSON body")
            return

        device_name = data.get("device_name") or "Unknown device"
        try:
            session = PairingManagerInstance.request_pairing(device_name)
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
        token = PairingManagerInstance.verify_code(pairing_id, code)
        if token is None:
            # Unknown/expired session or wrong code.
            self.report_error(401, "Invalid or expired code")
            return
        # Approved: clear the code prompt from the Dial so it does not linger.
        _dismiss_prompt(pairing_id)
        self.write({"status": "approved", "token": token})


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


class PairedDevicesHandler(BaseHandler):
    def get(self):
        self.write({"devices": PairingManagerInstance.list_devices()})


class PairedDeviceRevokeHandler(BaseHandler):
    def post(self, device_id):
        if PairingManagerInstance.revoke(device_id):
            self.write({"status": "success"})
        else:
            self.report_error(404, "Device not found")


API.register_handler(APIVersion.V1, r"/pair", PairPageHandler)
API.register_handler(APIVersion.V1, r"/pair/request", PairRequestHandler)
API.register_handler(APIVersion.V1, r"/pair/verify", PairVerifyHandler)
API.register_handler(APIVersion.V1, r"/pair/status/([^/]+)", PairStatusHandler)
API.register_handler(APIVersion.V1, r"/pair/devices", PairedDevicesHandler)
API.register_handler(APIVersion.V1, r"/pair/devices/([^/]+)/revoke", PairedDeviceRevokeHandler)


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
  </div>

  <div id="status" class="status" role="status"></div>
</div>

<script>
(function () {
  var TOKEN_KEY = "meticulous.deviceToken";
  var pairingId = null, expiryTimer = null;
  var intro = document.getElementById("intro");
  var codeStep = document.getElementById("codeStep");
  var done = document.getElementById("done");
  var status = document.getElementById("status");
  var startBtn = document.getElementById("startBtn");
  var verifyBtn = document.getElementById("verifyBtn");
  var codeInput = document.getElementById("codeInput");

  function say(msg, cls) { status.textContent = msg || ""; status.className = "status " + (cls || ""); }

  // Already authorized on this origin?
  try {
    if (localStorage.getItem(TOKEN_KEY)) {
      intro.classList.add("hidden");
      done.classList.remove("hidden");
      say("This device was already authorized.", "ok");
    }
  } catch (e) {}

  startBtn.addEventListener("click", function () {
    startBtn.disabled = true;
    say("Asking the machine...");
    fetch("/api/v1/pair/request", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ device_name: navigator.userAgent.slice(0, 60) || "Web browser" })
    }).then(function (r) {
      if (r.status === 429) throw new Error("Too many attempts. Wait a moment and try again.");
      if (!r.ok) throw new Error("Could not reach the machine.");
      return r.json();
    }).then(function (d) {
      pairingId = d.pairing_id;
      intro.classList.add("hidden");
      codeStep.classList.remove("hidden");
      codeInput.focus();
      say("Enter the code shown on your machine's screen.");
      var secs = d.expires_in || 60;
      expiryTimer = setTimeout(function () {
        codeStep.classList.add("hidden");
        intro.classList.remove("hidden");
        startBtn.disabled = false;
        say("That code expired. Please start again.", "err");
      }, secs * 1000);
    }).catch(function (e) {
      startBtn.disabled = false;
      say(e.message, "err");
    });
  });

  function verify() {
    var code = (codeInput.value || "").replace(/[^0-9]/g, "");
    if (code.length !== 6) { say("Enter the 6-digit code.", "err"); return; }
    verifyBtn.disabled = true;
    say("Checking...");
    fetch("/api/v1/pair/verify", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ pairing_id: pairingId, code: code })
    }).then(function (r) {
      if (r.status === 401) throw new Error("That code is wrong or expired.");
      if (!r.ok) throw new Error("Could not verify the code.");
      return r.json();
    }).then(function (d) {
      if (expiryTimer) clearTimeout(expiryTimer);
      try { localStorage.setItem(TOKEN_KEY, d.token); } catch (e) {}
      // SameSite=Strict cookie so this browser can hit any endpoint straight
      // from the address bar; other sites can never make the browser send it.
      document.cookie = "met_device_token=" + d.token +
        "; Path=/; Max-Age=31536000; SameSite=Strict";
      codeStep.classList.add("hidden");
      done.classList.remove("hidden");
      say("This device is now authorized.", "ok");
    }).catch(function (e) {
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
