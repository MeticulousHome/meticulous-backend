"""Guard durable recovery and prevent stale commands crossing a power transition.

Limits/bytes come from firmware's experimental energy-session-v1 contract, not
measured product deadlines. Serial transcripts replace only the hardware port;
no firmware is copied, built or run here. Removing the strictly-newer sample
check permits RELEASE on cached data; deleting the journal-before-send ordering
permits READY without recoverable host intent. Existing writer tests cover
neither regression. Failure injections include uncertain fsync and host restart.
"""

import json
import os
import threading
from collections import deque
from concurrent.futures import ThreadPoolExecutor

import pytest

from esp_serial.energy_session import EnergyError, EnergySession, Intent, IntentStore

BOOT, SESSION, CHALLENGE, PROBE = (c * 32 for c in "abcd")
EMPTY = "0" * 32
NEGOTIATION = "f" * 32
# Synthetic budgets exercise ordering only; no machine acceptance threshold.
LEASE, IO, DEADLINE = 10000, 2000, 20


def tx(body):
    return f"Energy,1,{body}\x03".encode()


def rx(body):
    return f"Energy,1,{body}\n".encode()


class Clock:
    now = 0.0

    def __call__(self):
        return self.now


class TranscriptPort:
    timeout = None
    write_timeout = None

    def __init__(self, clock, transcript):
        self.clock = clock
        self.transcript = deque(transcript)
        self.received = bytearray()
        self.writes = []

    def write(self, data):
        assert self.write_timeout is not None and self.write_timeout > 0
        self.writes.append(data)
        expected, result = self.transcript.popleft()
        assert data == expected
        if callable(result):
            result = result()
        if isinstance(result, BaseException):
            raise result
        if isinstance(result, int):
            return result
        self.received.extend(result)
        return len(data)

    def read(self, size):
        assert size == 1 and self.timeout is not None and self.timeout > 0
        if self.received:
            self.clock.now += 0.001
            result = bytes(self.received[:1])
            del self.received[:1]
            return result
        self.clock.now += self.timeout
        return b""


def prepare_transcript(ready=None):
    return [
        (tx(f"HELLO,{PROBE},{IO}"), rx(f"HELLO,{PROBE},{BOOT},empty,{EMPTY},0,{NEGOTIATION}")),
        (
            tx(f"PREPARE,{BOOT},{SESSION},{LEASE},{IO},{NEGOTIATION}"),
            rx(f"READY,{BOOT},{SESSION}") if ready is None else ready,
        ),
    ]


def recovery_transcript(*, boot=BOOT, sequence=1, samples=None, released=None):
    identity = f"{boot},{SESSION},{sequence},{CHALLENGE}"
    return [
        (
            tx(f"HELLO,{PROBE},{IO}"),
            rx(f"HELLO,{PROBE},{boot},armed,{SESSION},{sequence - 1},{NEGOTIATION}"),
        ),
        (
            tx(f"RESUME,{identity},{LEASE},{IO}"),
            rx(f"AWAKE,{identity},7")
            + (rx(f"SAMPLE,{identity},8,0.000000") if samples is None else samples),
        ),
        (
            tx(f"RELEASE,{identity},8,{IO}"),
            rx(f"RELEASED,{identity}") if released is None else released,
        ),
    ]


def owner(tmp_path, transcript, intent=None):
    clock = Clock()
    store = IntentStore(tmp_path / "intent.json")
    if intent:
        store.save(intent)
    port = TranscriptPort(clock, transcript)
    nonces = (
        iter([PROBE, SESSION, PROBE, CHALLENGE]) if intent is None else iter([PROBE, CHALLENGE])
    )
    session = EnergySession(port, store, clock=clock, nonce=lambda: next(nonces))
    return session, port, store, clock


def prepare(session):
    session.prepare(lease_ms=LEASE, io_ticks=IO, deadline=DEADLINE)


def recover(session, deadline=DEADLINE):
    session.recover(lease_ms=LEASE, io_ticks=IO, deadline=deadline)


def two_cycle_owner(tmp_path):
    """Two complete handshakes with fresh identities and matching tombstones."""
    transcript, nonces, sessions = [], [], []
    previous = EMPTY
    for start in (1, 7):
        probe, current, resume_probe, challenge, negotiation, resume_negotiation = (
            f"{n:032x}" for n in range(start, start + 6)
        )
        nonces.extend((probe, current, resume_probe, challenge))
        sessions.append(current)
        state, sequence = ("empty", 0) if previous == EMPTY else ("released", 1)
        identity = f"{BOOT},{current},1,{challenge}"
        transcript.extend(
            [
                (
                    tx(f"HELLO,{probe},{IO}"),
                    rx(f"HELLO,{probe},{BOOT},{state},{previous},{sequence},{negotiation}"),
                ),
                (
                    tx(f"PREPARE,{BOOT},{current},{LEASE},{IO},{negotiation}"),
                    rx(f"READY,{BOOT},{current}"),
                ),
                (
                    tx(f"HELLO,{resume_probe},{IO}"),
                    rx(f"HELLO,{resume_probe},{BOOT},armed,{current},0,{resume_negotiation}"),
                ),
                (
                    tx(f"RESUME,{identity},{LEASE},{IO}"),
                    rx(f"AWAKE,{identity},7") + rx(f"SAMPLE,{identity},8,0.000000"),
                ),
                (tx(f"RELEASE,{identity},8,{IO}"), rx(f"RELEASED,{identity}")),
            ]
        )
        previous = current
    clock = Clock()
    port = TranscriptPort(clock, transcript)
    store = IntentStore(tmp_path / "intent.json")
    values = iter(nonces)
    session = EnergySession(port, store, clock=clock, nonce=lambda: next(values))
    return session, port, store, sessions


class DelayedLock:
    """Schedule one caller behind later callers at the OS lock boundary.

    A real lock still serializes all owner work. Events deterministically model
    an unfavorably scheduled waiter; sleeps or assumptions about lock fairness
    would not prove that a request can cross a complete close/reopen cycle.
    """

    def __init__(self):
        self.lock = threading.Lock()
        self.waiting = threading.Event()
        self.proceed = threading.Event()

    def acquire(self, *, timeout):
        if threading.current_thread().name.startswith("delayed"):
            self.waiting.set()
            assert self.proceed.wait(5)  # Harness deadlock watchdog only.
        return self.lock.acquire(timeout=timeout)

    def release(self):
        self.lock.release()


@pytest.mark.parametrize("stalled_at", ["parts", "owner_lock", "admission_lock"])
def test_old_transaction_cannot_cross_a_successful_recovery(tmp_path, stalled_at):
    """No old hash/profile or actuator request may enter a newly released session.

    The one-way command lease contract forbids replay across recovery. The old
    boolean gate passes all previous tests but accepts this stale transaction,
    whether delayed while building its parts or waiting for either lock.
    A genuinely new command must still work after the second complete handshake.
    """
    session, port, store, sessions = two_cycle_owner(tmp_path)
    prepare(session)
    recover(session)
    delayed_lock = DelayedLock()
    entered, proceed = threading.Event(), threading.Event()
    if stalled_at == "owner_lock":
        session._lock = delayed_lock
        entered, proceed = delayed_lock.waiting, delayed_lock.proceed
    elif stalled_at == "admission_lock":
        session._admission_lock = delayed_lock
        entered, proceed = delayed_lock.waiting, delayed_lock.proceed
    stale_parts = [b"hash,old\x03", b"json\n{}\x03"]
    stale = b"".join(stale_parts)

    def parts():
        if stalled_at == "parts":
            entered.set()
            assert proceed.wait(5)
        yield from stale_parts

    with ThreadPoolExecutor(max_workers=1, thread_name_prefix="delayed") as pool:
        old = pool.submit(session.write_transaction, parts(), deadline=DEADLINE)
        try:
            assert entered.wait(5)
            prepare(session)
            recover(session)
            complete = Intent(1, sessions[1], 1, "complete")
            assert store.load() == complete
            # Permit a buggy send to return so the assertion identifies the
            # stale acceptance, rather than an unrelated transcript mismatch.
            port.transcript.append((stale, b""))
            before = list(port.writes)
        finally:
            proceed.set()
        assert not old.result(timeout=5)
    assert port.writes == before and store.load() == complete
    assert port.transcript.popleft() == (stale, b"")
    fresh = b"action,preheat\x03"
    port.transcript.append((fresh, b""))
    assert session.write_transaction([fresh], deadline=DEADLINE)
    assert port.writes[-1] == fresh and not port.transcript
    assert store.load() == complete


@pytest.mark.parametrize("transition", ["prepare", "recover"])
def test_later_transition_cannot_be_undone_by_recovery_publication(tmp_path, transition):
    """Even a timed-out closer revokes an earlier recovery's command admission.

    The complete RELEASED handshake and durable tombstone do not authorize
    overriding a later close request. The previous boolean implementation
    reopens here; the next explicit prepare/recover must be able to recover.
    """
    session, port, store, sessions = two_cycle_owner(tmp_path)
    prepare(session)
    entered, proceed = threading.Event(), threading.Event()
    expected, released = port.transcript[2]

    def block_release():
        entered.set()
        assert proceed.wait(5)
        return released

    port.transcript[2] = (expected, block_release)
    with ThreadPoolExecutor(max_workers=1) as pool:
        recovering = pool.submit(recover, session)
        try:
            assert entered.wait(5)
            with pytest.raises(EnergyError, match="busy"):
                getattr(session, transition)(
                    lease_ms=LEASE, io_ticks=IO, deadline=port.clock.now + 0.001
                )
        finally:
            proceed.set()
        with pytest.raises(EnergyError, match="superseded"):
            recovering.result(timeout=5)
    assert store.load() == Intent(1, sessions[0], 1, "complete")
    assert len(port.writes) == 5
    assert not session.write_transaction([b"action,start\x03"], deadline=DEADLINE)
    prepare(session)
    recover(session)
    port.transcript.append((b"action,preheat\x03", b""))
    assert session.write_transaction([b"action,preheat\x03"], deadline=DEADLINE)
    assert store.load() == Intent(1, sessions[1], 1, "complete")
    assert not port.transcript


@pytest.mark.parametrize("transition", ["prepare", "recover"])
def test_superseded_transition_cannot_close_a_newly_released_session(tmp_path, transition):
    """An older queued transition cannot act on a newer successful recovery.

    Lock acquisition order is not request order. Generation ownership must be
    checked before control I/O as well as before publication; otherwise an old
    waiter consumes/revokes the newer command lease after its release.
    """
    session, port, store, sessions = two_cycle_owner(tmp_path)
    prepare(session)
    recover(session)
    delayed_lock = DelayedLock()
    session._lock = delayed_lock
    with ThreadPoolExecutor(max_workers=1, thread_name_prefix="delayed") as pool:
        old = pool.submit(
            getattr(session, transition), lease_ms=LEASE, io_ticks=IO, deadline=DEADLINE
        )
        try:
            assert delayed_lock.waiting.wait(5)
            prepare(session)
            recover(session)
        finally:
            delayed_lock.proceed.set()
        with pytest.raises(EnergyError, match="superseded"):
            old.result(timeout=5)
    assert len(port.writes) == 10
    port.transcript.append((b"action,preheat\x03", b""))
    assert session.write_transaction([b"action,preheat\x03"], deadline=DEADLINE)
    assert store.load() == Intent(1, sessions[1], 1, "complete")
    assert not port.transcript


def test_durable_intent_precedes_prepare_and_completion_precedes_commands(tmp_path):
    session, port, store, _ = owner(tmp_path, prepare_transcript() + recovery_transcript())

    def require_intent_before_ready():
        assert store.load() == Intent(1, SESSION, 0, "pending")
        return rx(f"READY,{BOOT},{SESSION}")

    port.transcript[1] = (port.transcript[1][0], require_intent_before_ready)
    assert not session.write_transaction([b"action,start\x03"], deadline=DEADLINE)
    prepare(session)
    assert not session.write_transaction([b"action,start\x03"], deadline=DEADLINE)
    recover(session)
    assert store.load() == Intent(1, SESSION, 1, "complete")
    # A multipart hash/profile send is one serial write, owned as a unit.
    port.transcript.append((b"hash,h\x03json\n{}\x03", b""))
    assert session.write_transaction([b"hash,h\x03", b"json\n{}\x03"], deadline=DEADLINE)
    assert not port.transcript and port.timeout is None and port.write_timeout is None


@pytest.mark.parametrize("failure_phase", ["ready", "released"])
def test_lost_ack_restarts_from_durable_intent_with_new_recovery_sequence(
    tmp_path, failure_phase
):
    transcript = prepare_transcript(ready=b"" if failure_phase == "ready" else None)
    if failure_phase == "released":
        transcript += recovery_transcript(released=b"")
    session, port, store, clock = owner(tmp_path, transcript)
    with pytest.raises(EnergyError, match="deadline"):
        prepare(session)
        recover(session)
    previous = store.load()
    assert previous.phase == "pending"
    assert not session.write_transaction([b"old\x03"], deadline=clock.now + DEADLINE)
    # Simulated process/ESP reset: new boot, same durable session, fresh probe.
    port = TranscriptPort(
        clock, recovery_transcript(boot="e" * 32, sequence=previous.sequence + 1)
    )
    nonces = iter([PROBE, CHALLENGE])
    restarted = EnergySession(port, store, clock=clock, nonce=lambda: next(nonces))
    recover(restarted, clock.now + DEADLINE)
    assert store.load().phase == "complete"
    assert store.load().sequence == previous.sequence + 1


@pytest.mark.parametrize("bad_sample", ["7", "6", "0"])
def test_cached_or_older_acquisition_never_sends_release(tmp_path, bad_sample):
    ident = f"{BOOT},{SESSION},1,{CHALLENGE}"
    transcript = recovery_transcript(samples=rx(f"SAMPLE,{ident},{bad_sample},0.000000"))[:2]
    session, port, store, _ = owner(tmp_path, transcript, Intent(1, SESSION, 0, "pending"))
    with pytest.raises(EnergyError, match="deadline"):
        recover(session)
    assert all(b",RELEASE," not in frame for frame in port.writes)
    assert store.load() == Intent(1, SESSION, 1, "pending")


def test_wrong_boot_session_sequence_challenge_and_oversize_lines_do_not_release(tmp_path):
    ident = f"{BOOT},{SESSION},1,{CHALLENGE}"
    wrong = [
        ident.replace(BOOT, "e" * 32),
        ident.replace(SESSION, "e" * 32),
        ident.replace(",1,", ",2,"),
        ident.replace(CHALLENGE, "e" * 32),
    ]
    samples = b"x" * 300 + rx(f"SAMPLE,{ident},8,0.000000")  # No newline: discard entire line.
    samples += b"\xff\n" + b"".join(rx(f"SAMPLE,{item},8,0.000000") for item in wrong)
    session, port, _, _ = owner(
        tmp_path, recovery_transcript(samples=samples)[:2], Intent(1, SESSION, 0, "pending")
    )
    with pytest.raises(EnergyError, match="deadline"):
        recover(session)
    assert len(port.writes) == 2


@pytest.mark.parametrize(
    "state,peer", [("empty", EMPTY), ("armed", "e" * 32), ("corrupt", SESSION)]
)
def test_missing_or_foreign_peer_intent_cannot_be_adopted(tmp_path, state, peer):
    transcript = [
        (tx(f"HELLO,{PROBE},{IO}"), rx(f"HELLO,{PROBE},{BOOT},{state},{peer},0,{NEGOTIATION}"))
    ]
    session, port, store, _ = owner(tmp_path, transcript, Intent(1, SESSION, 0, "pending"))
    with pytest.raises(EnergyError):
        recover(session)
    assert store.load().sequence == 0 and len(port.writes) == 1


def test_missing_host_intent_does_not_clear_armed_peer(tmp_path):
    transcript = [
        (tx(f"HELLO,{PROBE},{IO}"), rx(f"HELLO,{PROBE},{BOOT},armed,{SESSION},0,{NEGOTIATION}"))
    ]
    session, port, store, _ = owner(tmp_path, transcript)
    with pytest.raises(EnergyError, match="unowned"):
        prepare(session)
    assert store.load() is None and len(port.writes) == 1


@pytest.mark.parametrize("stage", ["intent", "completion"])
def test_uncertain_fsync_never_publishes_commands_or_sends_next_phase(
    tmp_path, monkeypatch, stage
):
    session, port, store, _ = owner(tmp_path, prepare_transcript() + recovery_transcript())
    real_fsync = os.fsync
    calls = 0

    def fail_fsync(fd):
        nonlocal calls
        calls += 1
        # Each save fsyncs file then directory. Fail after replace, including
        # a complete tombstone that may already exist on disk.
        if calls == (2 if stage == "intent" else 6):
            raise OSError("injected directory fsync failure")
        return real_fsync(fd)

    monkeypatch.setattr(os, "fsync", fail_fsync)
    with pytest.raises(OSError, match="injected"):
        prepare(session)
        recover(session)
    assert not session.write_transaction([b"new\x03"], deadline=DEADLINE)
    count = len(port.writes)
    with pytest.raises(EnergyError, match="uncertain"):
        prepare(session)
    assert len(port.writes) == count
    assert count == (1 if stage == "intent" else 5)
    assert store.load() is not None


@pytest.mark.parametrize(
    "contents",
    [
        b"{",
        b"x" * 1025,
        b"[]",
        b'{"version":99}',
        json.dumps(
            {"version": True, "session": SESSION, "sequence": 0, "phase": "pending"}
        ).encode(),
    ],
)
def test_corrupt_journal_never_constructs_an_open_owner(tmp_path, contents):
    store = IntentStore(tmp_path / "intent.json")
    store.path.write_bytes(contents)
    with pytest.raises(EnergyError):
        EnergySession(TranscriptPort(Clock(), []), store, clock=Clock())
    assert store.path.read_bytes() == contents


@pytest.mark.parametrize("result", [1, KeyboardInterrupt()])
def test_partial_control_write_does_not_append_recovery_to_uncertain_bytes(tmp_path, result):
    session, port, _, _ = owner(tmp_path, [(tx(f"HELLO,{PROBE},{IO}"), result)])
    with pytest.raises((EnergyError, KeyboardInterrupt)):
        prepare(session)
    with pytest.raises(EnergyError, match="uncertain"):
        prepare(session)
    assert len(port.writes) == 1


@pytest.mark.parametrize("result", [1, KeyboardInterrupt()])
def test_partial_normal_transaction_remains_uncertain_after_host_restart(tmp_path, result):
    session, port, store, clock = owner(tmp_path, prepare_transcript() + recovery_transcript())
    prepare(session)
    recover(session)
    port.transcript.append((b"action,start\x03", result))
    if isinstance(result, BaseException):
        with pytest.raises(type(result)):
            session.write_transaction([b"action,start\x03"], deadline=DEADLINE)
    else:
        assert not session.write_transaction([b"action,start\x03"], deadline=DEADLINE)
    assert store.load().phase == "writing"
    empty_port = TranscriptPort(clock, [])
    restarted = EnergySession(empty_port, store, clock=clock)
    with pytest.raises(EnergyError, match="uncertain"):
        prepare(restarted)
    assert empty_port.writes == []


@pytest.mark.parametrize("pressure", ["nan", "inf", "not-a-number"])
def test_nonfinite_or_malformed_fresh_sample_cannot_release(tmp_path, pressure):
    identity = f"{BOOT},{SESSION},1,{CHALLENGE}"
    transcript = recovery_transcript(samples=rx(f"SAMPLE,{identity},8,{pressure}"))[:2]
    session, port, _, _ = owner(tmp_path, transcript, Intent(1, SESSION, 0, "pending"))
    with pytest.raises(EnergyError, match="pressure"):
        recover(session)
    assert len(port.writes) == 2


def test_matching_wake_cancels_prepare_without_waiting_for_the_deadline(tmp_path):
    session, port, store, clock = owner(
        tmp_path, prepare_transcript(ready=rx(f"WAKE,{BOOT},{SESSION}"))
    )
    with pytest.raises(EnergyError, match="Wake"):
        prepare(session)
    assert clock.now < DEADLINE
    assert store.load().phase == "pending"
    assert len(port.writes) == 2


def test_transition_closes_admission_for_a_transaction_already_waiting(tmp_path):
    """Whole hash/profile writes cannot straddle transition ownership.

    A generator signals that a second caller passed the initial admission check
    while the first owns real serial I/O. The transition times out waiting, yet
    that queued caller must still be refused. Removing the inside-owner check
    reproduces the escape; ordinary writer tests do not exercise this owner.
    """
    import threading
    from concurrent.futures import ThreadPoolExecutor

    session, port, store, _ = owner(tmp_path, prepare_transcript() + recovery_transcript())
    prepare(session)
    recover(session)
    entered, release, queued = threading.Event(), threading.Event(), threading.Event()

    def block_serial():
        entered.set()
        assert release.wait(5)  # Harness deadlock watchdog only.
        return b""

    port.transcript.extend([(b"hash,a\x03json\n{}\x03", block_serial), (b"late\x03", b"")])

    def queued_parts():
        queued.set()
        yield b"late\x03"

    with ThreadPoolExecutor(max_workers=2) as pool:
        first = pool.submit(
            session.write_transaction, [b"hash,a\x03", b"json\n{}\x03"], deadline=DEADLINE
        )
        try:
            assert entered.wait(5)
            second = pool.submit(session.write_transaction, queued_parts(), deadline=DEADLINE)
            assert queued.wait(5)
            with pytest.raises(EnergyError, match="busy"):
                session.prepare(lease_ms=LEASE, io_ticks=IO, deadline=port.clock.now + 0.001)
        finally:
            release.set()
        assert first.result(timeout=5)
        assert not second.result(timeout=5)
    assert port.writes[-1] == b"hash,a\x03json\n{}\x03"
    assert store.load().phase == "complete"


@pytest.mark.parametrize(
    "intent,state,peer,sequence",
    [
        (None, "released", SESSION, 1),
        (Intent(1, SESSION, 1, "complete"), "empty", EMPTY, 0),
        (Intent(1, SESSION, 1, "complete"), "released", "e" * 32, 1),
        (Intent(1, SESSION, 1, "complete"), "released", SESSION, 2),
    ],
)
def test_prepare_cannot_replace_a_missing_or_mismatched_completed_journal(
    tmp_path, intent, state, peer, sequence
):
    transcript = [
        (
            tx(f"HELLO,{PROBE},{IO}"),
            rx(f"HELLO,{PROBE},{BOOT},{state},{peer},{sequence},{NEGOTIATION}"),
        )
    ]
    session, port, store, _ = owner(tmp_path, transcript, intent)
    with pytest.raises(EnergyError):
        prepare(session)
    assert store.load() == intent
    assert len(port.writes) == 1
