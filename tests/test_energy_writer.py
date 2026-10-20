"""Prevent normal commands escaping a closed host command barrier.

The contract is the coordinated-suspend proposal's normal-command ownership:
closing stays latched, waits for an in-flight writer and never replays a refused
command. Short writes/timeouts cannot report success. This is an unused adapter,
not a suspend service. The old suite never closes it; removing the closed check
inside the writer lock lets a queued command escape and must fail this test.
OS pseudo-terminals exercise pyserial without opening any machine device.
"""

import os
import threading
import time
from concurrent.futures import ThreadPoolExecutor

import pytest
import serial

from esp_serial.energy_writer import EnergyCommandWriter


class Port:
    write_timeout = None

    def __init__(self, result=None):
        self.writes = []
        self.result = result

    def write(self, data):
        assert self.write_timeout is not None and self.write_timeout > 0
        self.writes.append(data)
        if isinstance(self.result, BaseException):
            raise self.result
        return len(data) if self.result is None else self.result


def test_closed_barrier_outlasts_an_inflight_writer_and_does_not_replay():
    entered, release, queued = threading.Event(), threading.Event(), threading.Event()

    class BlockingPort(Port):
        def write(self, data):
            entered.set()
            # Test harness watchdog only, not a product deadline or timing limit.
            assert release.wait(5)
            return super().write(data)

    port = BlockingPort()
    writer = EnergyCommandWriter(port, clock=lambda: 100.0)
    actual_lock = writer._writer

    class ObservedLock:
        # Observe the real lock boundary so the second command is definitely
        # queued before close. No sleep is used to guess thread scheduling.
        attempts = 0

        def acquire(self, *, timeout):
            self.attempts += 1
            if self.attempts == 2:
                queued.set()
            return actual_lock.acquire(timeout=timeout)

        def release(self):
            actual_lock.release()

    writer._writer = ObservedLock()
    with ThreadPoolExecutor(max_workers=2) as pool:
        first = pool.submit(writer.write_command, b"first\x03", deadline=105)
        try:
            assert entered.wait(5)
            second = pool.submit(writer.write_command, b"queued\x03", deadline=105)
            assert queued.wait(5)
            assert not writer.close_commands(deadline=100)
            assert not writer.write_command(b"late\x03", deadline=105)
        finally:
            release.set()
        assert first.result(timeout=5)
        assert not second.result(timeout=5)
    assert writer.close_commands(deadline=105)
    assert writer.close_commands(deadline=105)
    assert not writer.write_command(b"later\x03", deadline=105)
    assert port.writes == [b"first\x03"]
    assert port.write_timeout is None


@pytest.mark.parametrize(
    "result", [1, serial.SerialTimeoutException("test timeout"), KeyboardInterrupt()]
)
def test_uncertain_partial_write_permanently_blocks_later_commands(result):
    port = Port(result)
    writer = EnergyCommandWriter(port, clock=lambda: 100.0)
    if isinstance(result, BaseException):
        with pytest.raises(type(result)):
            writer.write_command(b"command\x03", deadline=105)
    else:
        assert not writer.write_command(b"command\x03", deadline=105)
    port.result = None  # A recovered OS port cannot clear protocol uncertainty.
    assert not writer.write_command(b"late\x03", deadline=105)
    assert port.writes == [b"command\x03"]
    assert port.write_timeout is None


@pytest.mark.parametrize("deadline", [float("inf"), float("nan")])
def test_unbounded_deadline_cannot_start_io(deadline):
    port = Port()
    writer = EnergyCommandWriter(port, clock=lambda: 100.0)
    with pytest.raises(ValueError):
        writer.write_command(b"command\x03", deadline=deadline)
    assert port.writes == []


def test_real_pyserial_pty_writes_complete_frame_and_closed_writer_sends_nothing():
    master, slave = os.openpty()
    try:
        with serial.Serial(os.ttyname(slave), baudrate=115200, timeout=0) as port:
            writer = EnergyCommandWriter(port, clock=time.monotonic)
            assert writer.write_command(b"command\x03", deadline=time.monotonic() + 5)
            assert os.read(master, 64) == b"command\x03"
            assert writer.close_commands(deadline=time.monotonic() + 5)
            assert not writer.write_command(b"late\x03", deadline=time.monotonic() + 5)
            os.set_blocking(master, False)
            with pytest.raises(BlockingIOError):
                os.read(master, 64)
            assert port.write_timeout is None
    finally:
        os.close(master)
        os.close(slave)
