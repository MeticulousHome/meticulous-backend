"""Unused one-way command barrier for the coordinated-suspend investigation.

This adapter owns an already-open pyserial port; it never opens or resets one.
It is deliberately not connected to Machine.write until the whole handshake,
multi-part command ownership and recovery contract are implemented.
"""

import math
import threading
from collections.abc import Callable
from typing import Protocol


class SerialWriter(Protocol):
    write_timeout: float | None

    def write(self, data: bytes) -> int:
        """Return the number of bytes accepted by the port."""
        ...


class EnergyCommandWriter:
    def __init__(self, port: SerialWriter, *, clock: Callable[[], float]):
        # The eventual Linux owner must supply a clock that includes suspend
        # (CLOCK_BOOTTIME), not Linux CLOCK_MONOTONIC. No timeout default here.
        self._port = port
        self._clock = clock
        self._writer = threading.Lock()
        self._closed = threading.Event()

    def _remaining(self, deadline: float) -> float:
        if not math.isfinite(deadline):
            raise ValueError("A finite absolute deadline is required")
        remaining = max(0.0, deadline - self._clock())
        if remaining > threading.TIMEOUT_MAX:
            raise ValueError("Deadline exceeds the lock's supported wait")
        return remaining

    def write_command(self, command: bytes, *, deadline: float) -> bool:
        """Write one complete command, or refuse it. Never queue for recovery.

        A timeout, short write or exception leaves this adapter closed because
        the peer may have received a partial command. Only the protocol owner
        can reconcile that uncertainty; there is no reopen method.
        """
        remaining = self._remaining(deadline)
        if self._closed.is_set() or not remaining:
            return False
        if not self._writer.acquire(timeout=remaining):
            return False
        try:
            remaining = self._remaining(deadline)
            if self._closed.is_set() or not remaining:
                return False
            previous_timeout = self._port.write_timeout
            try:
                # pyserial's default write timeout is unbounded. Scope a finite
                # timeout to this writer; all users must share this ownership.
                self._port.write_timeout = remaining
                count = self._port.write(command)
                complete = count == len(command) and self._remaining(deadline) > 0
                if not complete:
                    self._closed.set()
                return complete
            finally:
                self._port.write_timeout = previous_timeout
        except Exception:
            self._closed.set()
            raise
        finally:
            self._writer.release()

    def close_commands(self, *, deadline: float) -> bool:
        """Latch closed, then wait within budget for an existing writer.

        True proves only that this adapter has no active normal writer. It does
        not prove UART FIFO drain, firmware inhibition or permission to suspend.
        pyserial.flush/tcdrain can block without a timeout, so neither is called.
        """
        remaining = self._remaining(deadline)
        self._closed.set()
        if not remaining or not self._writer.acquire(timeout=remaining):
            return False
        try:
            return self._remaining(deadline) > 0
        finally:
            self._writer.release()
