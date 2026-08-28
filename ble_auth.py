"""Authorization window for BLE Wi-Fi provisioning (M4).

BLE provisioning must be explicitly approved on the Dial: the backend pushes an
"Allow Wi-Fi setup via Bluetooth?" prompt and a Yes opens a fixed-length
authorization window. While the window is open the Improv server reports
AUTHORIZED and credential writes are accepted; once it expires (or the user
answers No) the machine returns to AWAITING_AUTHORIZATION.

This module holds only the pure time/state logic so it can be unit-tested off
the machine (ble_gatt cannot even be imported on non-Linux platforms). The
GATT server owns the Improv state transitions and the Dial prompt plumbing.
"""

import time
from typing import Optional

# How long a Dial approval keeps provisioning open. Mirrors the 180 s window of
# the original (never-enabled) implementation and the on-screen copy.
AUTH_WINDOW_SECONDS = 180
# Minimum spacing between prompts pushed to the Dial, so a hostile BLE peer in
# range cannot make the machine chime continuously by re-reading the status
# characteristic after every denial/expiry.
PROMPT_COOLDOWN_SECONDS = 30


class BleAuthorization:
    """Tracks whether BLE provisioning is currently user-authorized."""

    def __init__(self):
        self._granted_at: Optional[float] = None
        self._last_prompt_at: Optional[float] = None

    # --- window ---------------------------------------------------------------

    def grant(self, at: Optional[float] = None) -> None:
        """Open the authorization window (user answered Yes on the Dial).

        `at` is the acknowledgement timestamp; the window counts from the moment
        the user answered, not from when the callback happens to run. Idempotent:
        the notification layer may deliver the acknowledgement callback twice.
        """
        self._granted_at = time.time() if at is None else at

    def revoke(self) -> None:
        """Close the window (user answered No, or an explicit shutdown)."""
        self._granted_at = None

    def active(self, now: Optional[float] = None) -> bool:
        if self._granted_at is None:
            return False
        now = time.time() if now is None else now
        if (now - self._granted_at) < AUTH_WINDOW_SECONDS:
            return True
        # Expired: forget the grant so a later prompt starts clean.
        self._granted_at = None
        return False

    def remaining(self, now: Optional[float] = None) -> float:
        if self._granted_at is None:
            return 0.0
        now = time.time() if now is None else now
        return max(0.0, AUTH_WINDOW_SECONDS - (now - self._granted_at))

    # --- prompt throttling ----------------------------------------------------

    def should_prompt(self, prompt_pending: bool, now: Optional[float] = None) -> bool:
        """Whether a new approval prompt should be pushed to the Dial.

        Not while one is already on screen (`prompt_pending`), not while the
        window is active, and not within the cooldown after the previous prompt.
        """
        now = time.time() if now is None else now
        if prompt_pending:
            return False
        if self.active(now):
            return False
        if self._last_prompt_at is not None and (now - self._last_prompt_at) < PROMPT_COOLDOWN_SECONDS:
            return False
        return True

    def note_prompt(self, now: Optional[float] = None) -> None:
        self._last_prompt_at = time.time() if now is None else now
