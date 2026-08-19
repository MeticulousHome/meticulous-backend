"""HTTP endpoints for device pairing (M3).

Flow:
  POST /pair/request          -> opens a session, shows an Allow/Deny prompt on
                                 the Dial, returns {pairing_id, code}
  GET  /pair/status/{id}      -> {status} while pending; {status, token} once the
                                 owner approves on the Dial (token delivered once)
  GET  /pair/devices          -> list of paired devices (management; auth/loopback)
  POST /pair/devices/{id}/revoke -> revoke a device (management; auth/loopback)

/pair/request and /pair/status are the only endpoints reachable without a token
(an unpaired device has none yet); the request-layer enforcement allowlists them.
The management endpoints are not allowlisted, so they require a token or loopback
(the Dial's "Paired devices" screen calls them over loopback).
"""

import json

from notifications import Notification, NotificationManager, NotificationResponse
from pairing import PairingError, PairingManagerInstance
from log import MeticulousLogger

from .base_handler import BaseHandler
from .api import API, APIVersion

logger = MeticulousLogger.getLogger(__name__)


def _format_code(code: str) -> str:
    # "482913" -> "482 913" for legibility on the Dial prompt.
    return f"{code[:3]} {code[3:]}" if len(code) == 6 else code


def _push_approval_prompt(pairing_id: str, device_name: str, code: str) -> None:
    """Show the Allow/Deny prompt on the Dial and wire the answer to the session.

    The callback fires on acknowledgement (and the notification manager may fire
    it twice); approve()/deny() are idempotent, so a double invocation is safe.
    """
    message = f"Allow '{device_name}' to connect to this machine?\nCode: {_format_code(code)}"
    notification = Notification(
        message,
        responses=[NotificationResponse.YES, NotificationResponse.NO],
    )

    def on_answer():
        if notification.response == NotificationResponse.YES:
            PairingManagerInstance.approve(pairing_id)
        else:
            PairingManagerInstance.deny(pairing_id)

    notification.callback = on_answer
    NotificationManager.add_notification(notification)


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
        self.write(
            {
                "pairing_id": session["pairing_id"],
                "code": session["code"],
                "expires_in": session["expires_in"],
            }
        )


class PairStatusHandler(BaseHandler):
    def get(self, pairing_id):
        self.write(PairingManagerInstance.poll(pairing_id))


class PairedDevicesHandler(BaseHandler):
    def get(self):
        self.write({"devices": PairingManagerInstance.list_devices()})


class PairedDeviceRevokeHandler(BaseHandler):
    def post(self, device_id):
        if PairingManagerInstance.revoke(device_id):
            self.write({"status": "success"})
        else:
            self.report_error(404, "Device not found")


API.register_handler(APIVersion.V1, r"/pair/request", PairRequestHandler)
API.register_handler(APIVersion.V1, r"/pair/status/([^/]+)", PairStatusHandler)
API.register_handler(APIVersion.V1, r"/pair/devices", PairedDevicesHandler)
API.register_handler(APIVersion.V1, r"/pair/devices/([^/]+)/revoke", PairedDeviceRevokeHandler)
