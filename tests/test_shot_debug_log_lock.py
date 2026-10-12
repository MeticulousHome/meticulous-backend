"""Logging into the debug shot must never wait for the debug shot's data lock.

From the second shot on, ShotLogHandler copies every log record into the
current debug shot. ShotDebugManager holds clear_current_data_lock while it
builds a DebugShot or ingests data, and those paths log (a DebugShot logs a
warning when the image has no build date file). If the handler also took that
lock, a thread logging under it would wait for the logging handler's lock,
held by another thread that is itself waiting in the handler for the data lock:
the backend froze at the start of every second shot (found by the Dial
journeys in meticulous-integration, with the real firmware).
"""

import logging
import threading
import time
from types import SimpleNamespace

import pytest

from shot_debug_manager import ShotDebugManager, ShotLogHandler


@pytest.fixture
def debug_shot(monkeypatch):
    shot = SimpleNamespace(startTime=time.time(), logs=[])
    monkeypatch.setattr(ShotDebugManager, "_current_data", shot)
    handler = ShotLogHandler()
    logger = logging.getLogger("test_shot_debug_log_lock")
    logger.setLevel(logging.INFO)
    logger.propagate = False
    logger.addHandler(handler)
    yield SimpleNamespace(shot=shot, logger=logger)
    logger.removeHandler(handler)


def test_a_log_record_reaches_the_debug_shot(debug_shot):
    debug_shot.logger.warning("pressure sensor slow")

    assert len(debug_shot.shot.logs) == 1
    entry = debug_shot.shot.logs[0]
    assert entry["loglevel"] == "WARNING"
    assert entry["caller"] == "test_shot_debug_log_lock"
    assert entry["log_message"].endswith("pressure sensor slow")
    assert entry["profile_ms"] >= 0


def test_logging_does_not_wait_while_another_thread_holds_the_data_lock(debug_shot):
    logged = threading.Event()

    def log_from_another_thread():
        debug_shot.logger.info("GET /api/v1/settings")
        logged.set()

    with ShotDebugManager.clear_current_data_lock:
        worker = threading.Thread(target=log_from_another_thread, daemon=True)
        worker.start()
        finished = logged.wait(timeout=2)

    worker.join(timeout=2)
    assert (
        finished
    ), "logging blocked on clear_current_data_lock: the shot log handler can deadlock"
    assert [e["log_message"] for e in debug_shot.shot.logs] == ["GET /api/v1/settings"]


def test_two_threads_logging_while_one_holds_the_data_lock_do_not_deadlock(debug_shot):
    """The shape of the freeze: one thread logs under the data lock, another logs at the same time."""
    done = []

    def holds_the_data_lock_and_logs():
        with ShotDebugManager.clear_current_data_lock:
            time.sleep(0.05)  # the other thread is inside the handler by now
            debug_shot.logger.warning("/etc/build-date file not found")
        done.append("holder")

    def logs_a_request():
        for _ in range(50):
            debug_shot.logger.info("200 GET /api/v1/settings")
        done.append("request")

    threads = [
        threading.Thread(target=holds_the_data_lock_and_logs, daemon=True),
        threading.Thread(target=logs_a_request, daemon=True),
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=5)

    assert sorted(done) == ["holder", "request"], "the two threads deadlocked"
    assert len(debug_shot.shot.logs) == 51


def test_a_record_logged_with_no_shot_running_goes_nowhere(debug_shot, monkeypatch):
    monkeypatch.setattr(ShotDebugManager, "_current_data", None)

    debug_shot.logger.info("between shots")

    assert debug_shot.shot.logs == []


def test_the_saved_copy_keeps_the_logs_and_later_records_do_not_change_it(debug_shot):
    """_copy_current_data snapshots the log list under the same lock the handler appends with."""
    debug_shot.logger.warning("before the copy")

    copy = ShotDebugManager._copy_current_data()
    debug_shot.logger.warning("after the copy")

    assert [entry["log_message"] for entry in copy.logs] == ["before the copy"]
    assert len(debug_shot.shot.logs) == 2
