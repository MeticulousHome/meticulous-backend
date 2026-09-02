"""Tracks which paired device authorized each live Socket.IO connection.

Socket.IO checks the device token once, at the handshake. Without this, a
connection stays open and keeps streaming telemetry even after its device is
revoked -- revocation would not end an active session. This registry maps each
authorized socket to the device_id that authorized it (None for loopback/Dial
callers, which hold no token), so that revoking a device can immediately
disconnect its live sockets. The client then reconnects, the handshake is
refused, and the app is sent back to the authorize flow.
"""

import tornado.ioloop

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

_sio = None
# sid -> device_id (None for loopback/Dial connections, which are exempt and
# must never be dropped when a LAN device is revoked).
_sid_to_device: dict = {}


def set_sio(sio) -> None:
    global _sio
    _sio = sio


def register(sid: str, device_id) -> None:
    _sid_to_device[sid] = device_id


def unregister(sid: str) -> None:
    _sid_to_device.pop(sid, None)


def is_dial(sid: str) -> bool:
    """True only for a loopback socket (the Dial), which holds no device token.

    Used to gate security-prompt acknowledgements: answering a pairing or BLE
    approval grants access, so only the Dial may do it. Unknown sids are not
    trusted.
    """
    return sid in _sid_to_device and _sid_to_device[sid] is None


def disconnect_device(device_id: str) -> None:
    """Disconnect every live socket that authorized as this device."""
    if not device_id or _sio is None:
        return
    sids = [sid for sid, d in _sid_to_device.items() if d == device_id]
    for sid in sids:
        _schedule_disconnect(sid)


def disconnect_all_devices() -> None:
    """Disconnect every token-authorized socket (loopback/Dial kept). Used when
    all paired devices are revoked at once."""
    if _sio is None:
        return
    sids = [sid for sid, d in _sid_to_device.items() if d is not None]
    for sid in sids:
        _schedule_disconnect(sid)


def _schedule_disconnect(sid: str) -> None:
    sio = _sio
    if sio is None:
        return

    def _run():
        import asyncio

        logger.info(f"Disconnecting socket {sid} (device revoked)")
        asyncio.ensure_future(sio.disconnect(sid))

    # Runs on the Socket.IO event loop; add_callback is thread-safe.
    tornado.ioloop.IOLoop.current().add_callback(_run)
