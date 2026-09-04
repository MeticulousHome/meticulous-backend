"""Notification security-boundary properties (ADV-001 residuals)."""

import json

from notifications import Notification, NotificationManager


def test_to_json_marks_sensitive_so_consumers_never_log_it():
    # The Dial forwards console output to the journal; a consumer must be able
    # to see a body is sensitive and refuse to log/forward it.
    plain = json.loads(Notification("hello").to_json())
    assert plain["sensitive"] is False

    secret = json.loads(Notification("code 482 913", sensitive=True).to_json())
    assert secret["sensitive"] is True


def test_dismissal_is_not_retained_in_the_listing():
    # An empty-body notification clears a prompt; it must not linger,
    # unacknowledged, in every client's GET /notifications forever (ADV-001 R4).
    NotificationManager._notifications = []
    NotificationManager._queue = type(NotificationManager._queue)()

    live = Notification("A device wants to connect", sensitive=True)
    NotificationManager.add_notification(live)
    assert live in NotificationManager._notifications

    dismiss = Notification("", responses=[], sensitive=True)
    dismiss.id = live.id
    NotificationManager.add_notification(dismiss)
    # The dismissal was queued for the Dial but not stored, and it did not
    # resurrect the original as an unacknowledged entry.
    assert dismiss not in NotificationManager._notifications


def test_acknowledge_runs_callback_exactly_once():
    NotificationManager._notifications = []
    NotificationManager._queue = type(NotificationManager._queue)()
    calls = {"n": 0}

    def cb():
        calls["n"] += 1

    n = Notification("do it?", responses=["Yes"], callback=cb)
    NotificationManager.add_notification(n)
    assert NotificationManager.acknowledge_notification(n.id, "Yes", is_local=True) is True
    assert calls["n"] == 1


def test_remote_cannot_acknowledge_a_sensitive_prompt():
    NotificationManager._notifications = []
    NotificationManager._queue = type(NotificationManager._queue)()
    fired = {"n": 0}

    def cb():
        fired["n"] += 1

    n = Notification("secret prompt", responses=["Yes"], callback=cb, sensitive=True)
    NotificationManager.add_notification(n)
    # A LAN caller (is_local=False) must not be able to answer it.
    assert NotificationManager.acknowledge_notification(n.id, "Yes", is_local=False) is False
    assert fired["n"] == 0
    # The Dial (is_local=True) still can.
    assert NotificationManager.acknowledge_notification(n.id, "Yes", is_local=True) is True
    assert fired["n"] == 1
