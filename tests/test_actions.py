"""Routing tests for the machine action allow lists.

The dial is the only decider for an encoder double click during a shot and sends
the result through the normal action path: ``finish`` is forwarded to the ESP,
``abort`` ends the profile in the backend. These tests pin that routing.

The socket handler in ``backend.py`` consumes the same two allow lists, but it
cannot be imported from a unit test: ``backend`` pulls in ``imager``, which needs
the machine-only ``parted`` package, and importing it also starts background
threads that write to machine-only paths. The HTTP handler in ``api/action.py``
applies the same allow lists, so it is exercised here instead. Both machine entry
points are replaced, so no serial connection is involved.
"""

import pytest

from api.action import ExecuteActionHandler
from machine import Machine


class RecordingHandler:
    """Minimal stand-in for the Tornado handler ``post`` runs against."""

    def __init__(self):
        self.written = []
        self.status = 200

    def write(self, chunk):
        self.written.append(chunk)

    def set_status(self, code):
        self.status = code


@pytest.fixture
def calls(monkeypatch):
    """Record what the handler dispatches instead of touching the machine."""
    recorded = {"action": [], "end_profile": 0}

    def fake_action(action_event) -> bool:
        recorded["action"].append(action_event)
        return True

    def fake_end_profile():
        recorded["end_profile"] += 1

    monkeypatch.setattr(Machine, "action", fake_action)
    monkeypatch.setattr(Machine, "end_profile", fake_end_profile)
    return recorded


def test_finish_is_an_allowed_esp_action():
    assert "finish" in Machine.ALLOWED_ESP_ACTIONS


def test_abort_is_an_allowed_backend_action():
    assert "abort" in Machine.ALLOWED_BACKEND_ACTIONS
    # The handler checks the ESP list first, so abort must stay out of it.
    assert "abort" not in Machine.ALLOWED_ESP_ACTIONS


def test_finish_is_forwarded_to_the_esp(calls):
    handler = RecordingHandler()

    ExecuteActionHandler.post(handler, "finish")

    assert calls["action"] == ["finish"]
    assert calls["end_profile"] == 0
    assert handler.status == 200


def test_abort_ends_the_profile(calls):
    handler = RecordingHandler()

    ExecuteActionHandler.post(handler, "abort")

    assert calls["end_profile"] == 1
    assert calls["action"] == []
