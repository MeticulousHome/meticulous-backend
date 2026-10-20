"""Offline-only owner of a versioned energy session on an already-open port.

No Machine import, endpoint, reset, suspend syscall or automatic activation.
The firmware experiment owns the wire contract. All callers, including command
transactions, must use this owner; a second serial user invalidates its proof.
"""

import json
import math
import os
import re
import secrets
import tempfile
import threading
from collections.abc import Callable, Iterable
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Protocol

from esp_serial.energy_writer import EnergyCommandWriter, SerialWriter

MAX_COUNTER = (1 << 31) - 1
MAX_FRAME = 240
TOKEN = re.compile(r"[0-9a-f]{32}\Z")
PRESSURE = re.compile(r"-?(?:0|[1-9][0-9]*)\.[0-9]{6}\Z")
NUMBER = re.compile(r"0|[1-9][0-9]*\Z")


class EnergyError(RuntimeError):
    """The command gate stays closed; the caller must reconcile or investigate."""


class EnergyPort(SerialWriter, Protocol):
    timeout: float | None

    def read(self, size: int) -> bytes:
        """Read up to size bytes within timeout."""
        ...


def token(value: str) -> str:
    if not TOKEN.fullmatch(value):
        raise EnergyError("Invalid session identifier")
    return value


def counter(value: str, *, zero: bool = False) -> int:
    if not NUMBER.fullmatch(value):
        raise EnergyError("Invalid counter")
    result = int(value)
    if not (0 if zero else 1) <= result <= MAX_COUNTER:
        raise EnergyError("Counter outside the experimental contract")
    return result


@dataclass(frozen=True)
class Intent:
    version: int
    session: str
    sequence: int
    phase: str

    def validate(self) -> "Intent":
        token(self.session)
        if (
            type(self.version) is not int
            or self.version != 1
            or type(self.sequence) is not int
            or not 0 <= self.sequence <= MAX_COUNTER
            or self.phase not in ("pending", "complete", "writing")
        ):
            raise EnergyError("Invalid durable intent")
        return self


class IntentStore:
    """Atomic file replacement plus file and parent-directory fsync.

    An existing private directory and exclusive process ownership are required.
    A failed/uncertain save is never treated as completion. No recovery deletes
    a corrupt or missing record. Physical media guarantees remain platform work.
    """

    def __init__(self, path: Path):
        self.path = path

    def load(self) -> Intent | None:
        try:
            with self.path.open("rb") as source:
                data = source.read(1025)
        except FileNotFoundError:
            return None
        try:
            if len(data) > 1024:
                raise ValueError("oversize")
            raw = json.loads(data)
            if not isinstance(raw, dict) or set(raw) != {
                "version",
                "session",
                "sequence",
                "phase",
            }:
                raise ValueError("shape")
            return Intent(**raw).validate()
        except (ValueError, TypeError) as exc:
            raise EnergyError("Unreadable durable intent; commands remain closed") from exc

    def save(self, intent: Intent) -> None:
        data = json.dumps(asdict(intent.validate()), sort_keys=True).encode() + b"\n"
        fd, temporary = tempfile.mkstemp(prefix=".energy-", dir=self.path.parent)
        try:
            with os.fdopen(fd, "wb") as target:
                target.write(data)
                target.flush()
                os.fsync(target.fileno())
            os.replace(temporary, self.path)
            directory = os.open(self.path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
            if self.load() != intent:
                raise EnergyError("Durable intent readback mismatch")
        finally:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass


class EnergySession:
    def __init__(
        self,
        port: EnergyPort,
        store: IntentStore,
        *,
        clock: Callable[[], float],
        nonce: Callable[[], str] = lambda: secrets.token_hex(16),
    ):
        self._port = port
        self._store = store
        self._clock = clock  # Linux caller must supply CLOCK_BOOTTIME.
        self._nonce = nonce
        self._lock = threading.Lock()
        self._writer: EnergyCommandWriter | None = None
        self._poisoned = False
        self._accepting = threading.Event()
        self._line = bytearray()
        self._discard = False
        self._intent = store.load()  # Corruption fails construction, never opens commands.
        self._poisoned = self._intent is not None and self._intent.phase == "writing"
        self._boot: str | None = None

    def _remaining(self, deadline: float) -> float:
        if not math.isfinite(deadline):
            raise ValueError("A finite absolute deadline is required")
        remaining = deadline - self._clock()
        if remaining <= 0:
            raise EnergyError("Energy session deadline expired")
        if remaining > threading.TIMEOUT_MAX:
            raise ValueError("Unsupported deadline")
        return remaining

    def _enter(self, deadline: float) -> None:
        if not self._lock.acquire(timeout=self._remaining(deadline)):
            raise EnergyError("Energy owner is busy")

    def _close(self, deadline: float) -> None:
        if self._writer is not None:
            writer, self._writer = self._writer, None
            if not writer.close_commands(deadline=deadline) or writer.uncertain:
                self._poisoned = True
        if self._poisoned:
            raise EnergyError(
                "Partial transport is uncertain; fresh transport recovery required"
            )

    def _save(self, intent: Intent) -> None:
        try:
            self._store.save(intent)
        except BaseException:
            # rename/fsync failures may have persisted either record. Only a
            # fresh owner reloading the journal can resolve that uncertainty.
            self._poisoned = True
            self._accepting.clear()
            raise
        self._intent = intent

    def _send(self, *fields: str, deadline: float) -> None:
        frame = ("Energy,1," + ",".join(fields)).encode("ascii") + b"\x03"
        if len(frame) > MAX_FRAME:
            raise EnergyError("Oversize control frame")
        writer = EnergyCommandWriter(self._port, clock=self._clock)
        try:
            if not writer.write_command(frame, deadline=deadline):
                raise EnergyError("Control write failed")
        except BaseException:
            # Never append a recovery frame to an unknown partial legacy frame.
            self._poisoned = True
            raise

    def _receive(self, deadline: float) -> list[str]:
        while True:
            previous = self._port.timeout
            try:
                self._port.timeout = self._remaining(deadline)
                data = self._port.read(1)
            finally:
                self._port.timeout = previous
            self._remaining(deadline)  # Late complete replies are not acknowledgements.
            if not data:
                continue
            if data == b"\n":
                raw, discard = bytes(self._line), self._discard
                self._line.clear()
                self._discard = False
                if discard:
                    continue
                try:
                    fields = raw.decode("ascii").split(",")
                except UnicodeDecodeError:
                    continue
                if fields[:2] == ["Energy", "1"]:
                    return fields[2:]
            elif len(self._line) >= MAX_FRAME - 1:
                self._line.clear()
                self._discard = True
            elif not self._discard:
                self._line.extend(data)

    def _expect(self, prefix: list[str], length: int, deadline: float) -> list[str]:
        while True:
            fields = self._receive(deadline)
            if fields[: len(prefix)] == prefix and len(fields) == length:
                return fields

    def _hello(self, io_ticks: int, deadline: float) -> tuple[str, str, int]:
        probe = token(self._nonce())
        self._send("HELLO", probe, str(io_ticks), deadline=deadline)
        fields = self._expect(["HELLO", probe], 6, deadline)
        self._boot = token(fields[2])
        state, session, sequence = fields[3], fields[4], counter(fields[5], zero=True)
        if state not in ("empty", "armed", "released"):
            raise EnergyError("Peer journal is unavailable")
        token(session)
        return state, session, sequence

    def prepare(self, *, lease_ms: int, io_ticks: int, deadline: float) -> None:
        """Close normal commands and persist intent before requesting READY.

        This is not permission to suspend. The unintegrated OS/preflight/wake
        owner must still meet the design's independent gates.
        """
        lease = counter(str(lease_ms))
        io = counter(str(io_ticks))
        self._accepting.clear()
        self._enter(deadline)
        try:
            self._close(deadline)
            if self._intent is not None and self._intent.phase == "pending":
                raise EnergyError("Recover the outstanding intent first")
            state, _, _ = self._hello(io, deadline)
            if state == "armed":
                raise EnergyError("Peer has an unowned outstanding intent")
            intent = Intent(1, token(self._nonce()), 0, "pending")
            self._save(intent)
            assert self._boot is not None
            self._send(
                "PREPARE", self._boot, intent.session, str(lease), str(io), deadline=deadline
            )
            self._expect(["READY", self._boot, intent.session], 3, deadline)
        finally:
            self._lock.release()

    def recover(self, *, lease_ms: int, io_ticks: int, deadline: float) -> None:
        """Reconcile persisted intent, prove fresh acquisition, release at zero.

        On success a new one-way command lease is published. Old leases stay
        closed forever; no queued command or interrupted profile is replayed.
        """
        lease = counter(str(lease_ms))
        io = counter(str(io_ticks))
        self._accepting.clear()
        self._enter(deadline)
        try:
            self._close(deadline)
            intent = self._intent
            if intent is None or intent.phase != "pending":
                raise EnergyError("No pending durable session to recover")
            state, peer_session, sequence = self._hello(io, deadline)
            if state == "empty" or peer_session != intent.session:
                raise EnergyError("Peer/host durable sessions disagree")
            sequence = max(sequence, intent.sequence) + 1
            counter(str(sequence))
            challenge = token(self._nonce())
            intent = Intent(1, intent.session, sequence, "pending")
            self._save(intent)
            assert self._boot is not None
            identity = [self._boot, intent.session, str(sequence), challenge]
            self._send("RESUME", *identity, str(lease), str(io), deadline=deadline)
            awake = self._expect(["AWAKE", *identity], 6, deadline)
            baseline = counter(awake[5], zero=True)
            while True:
                sample = self._expect(["SAMPLE", *identity], 7, deadline)
                acquired = counter(sample[5], zero=True)
                if not PRESSURE.fullmatch(sample[6]):
                    raise EnergyError("Invalid recovery pressure sample")
                try:
                    pressure = float(sample[6])
                except ValueError as exc:
                    raise EnergyError("Invalid recovery pressure sample") from exc
                if not math.isfinite(pressure):
                    raise EnergyError("Nonfinite recovery pressure sample")
                if acquired > baseline:
                    break
            self._send("RELEASE", *identity, str(acquired), str(io), deadline=deadline)
            self._expect(["RELEASED", *identity], 5, deadline)
            complete = Intent(1, intent.session, sequence, "complete")
            self._save(complete)
            self._remaining(deadline)
            self._writer = EnergyCommandWriter(self._port, clock=self._clock)
            self._accepting.set()
        finally:
            self._lock.release()

    def write_transaction(self, parts: Iterable[bytes], *, deadline: float) -> bool:
        """One caller owns the whole multipart legacy command, including its hash."""
        if not self._accepting.is_set():
            return False
        command = b"".join(parts)
        self._enter(deadline)
        try:
            if not self._accepting.is_set() or self._writer is None:
                return False
            assert self._intent is not None
            # A process crash must not erase an uncertain partial legacy frame.
            # This offline candidate fsyncs around each complete transaction;
            # performance/wear and external-reset reconciliation are not activated.
            self._save(Intent(1, self._intent.session, self._intent.sequence, "writing"))
            try:
                result = self._writer.write_command(command, deadline=deadline)
            except BaseException:
                self._poisoned = True
                self._accepting.clear()
                self._writer = None
                raise
            if self._writer.uncertain:
                self._poisoned = True
                self._accepting.clear()
                self._writer = None
            if result:
                self._save(Intent(1, self._intent.session, self._intent.sequence, "complete"))
                self._remaining(deadline)
            else:
                self._accepting.clear()
            return result
        finally:
            self._lock.release()
