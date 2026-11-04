import asyncio
import importlib
import sys
from types import ModuleType, SimpleNamespace

import pytest


class StubNotification:
    def __init__(self, message, responses=None, image=None):
        self.message = message
        self.respone_options = responses or []
        self.image = image


@pytest.fixture
def dbus_monitor(monkeypatch):
    notifications = ModuleType("notifications")
    notifications.Notification = StubNotification
    notifications.NotificationManager = SimpleNamespace(add_notification=lambda _value: None)
    notifications.NotificationResponse = SimpleNamespace(OK="Ok")
    monkeypatch.setitem(sys.modules, "notifications", notifications)

    dbus_client = ModuleType("dbus_client")
    dbus_client.AsyncDBUSClient = lambda: SimpleNamespace()
    monkeypatch.setitem(sys.modules, "dbus_client", dbus_client)

    api_machine = ModuleType("api.machine")
    api_machine.OSStatus = SimpleNamespace(
        DOWNLOADING="downloading",
        INSTALLING="installing",
        COMPLETE="complete",
        FAILED="failed",
    )
    api_machine.UpdateOSStatus = SimpleNamespace(
        sendStatus=lambda *_args, **_kwargs: None,
        isRecoveryUpdate=lambda: False,
        markAsRecoveryUpdate=lambda _value: None,
    )
    monkeypatch.setitem(sys.modules, "api.machine", api_machine)

    images = ModuleType("images.notificationImages.base64")
    images.FLASHING_NOTIFICATION_IMAGE = "flashing-image"
    images.USB_DEVICE_NOTIFICATION_IMAGE = "usb-image"
    monkeypatch.setitem(sys.modules, "images.notificationImages.base64", images)

    sys.modules.pop("dbus_monitor", None)
    module = importlib.import_module("dbus_monitor")
    yield module
    sys.modules.pop("dbus_monitor", None)


def test_repeated_deployment_space_error_is_reported_hourly(dbus_monitor, monkeypatch):
    error_messages = []
    monkeypatch.setattr(
        dbus_monitor,
        "logger",
        SimpleNamespace(error=error_messages.append),
    )
    timestamps = iter([100.0, 100.1, 3700.0])
    monkeypatch.setattr(
        dbus_monitor,
        "time",
        SimpleNamespace(monotonic=lambda: next(timestamps)),
    )

    parameters = (
        dbus_monitor.HAWKBIT_DEPLOYMENT_PROCESS,
        dbus_monitor.HAWKBIT_DEPLOYMENT_SPACE_ERROR,
    )
    for _ in range(3):
        asyncio.run(
            dbus_monitor.DBusMonitor.report_hawkbit_error(
                None, None, None, None, None, parameters
            )
        )

    assert error_messages == [
        "Error in processing deployment process: File size exceeds available space",
        "Error in processing deployment process: File size exceeds available space",
    ]


def test_other_hawkbit_errors_are_never_rate_limited(dbus_monitor, monkeypatch):
    error_messages = []
    monkeypatch.setattr(
        dbus_monitor,
        "logger",
        SimpleNamespace(error=error_messages.append),
    )

    parameters = (dbus_monitor.HAWKBIT_DEPLOYMENT_PROCESS, "Signature verification failed")
    for _ in range(2):
        asyncio.run(
            dbus_monitor.DBusMonitor.report_hawkbit_error(
                None, None, None, None, None, parameters
            )
        )

    assert error_messages == [
        "Error in processing deployment process: Signature verification failed",
        "Error in processing deployment process: Signature verification failed",
    ]


def test_download_space_error_is_not_shared_with_deployment_window(dbus_monitor, monkeypatch):
    error_messages = []
    monkeypatch.setattr(
        dbus_monitor,
        "logger",
        SimpleNamespace(error=error_messages.append),
    )
    monkeypatch.setattr(
        dbus_monitor,
        "time",
        SimpleNamespace(monotonic=lambda: 100.0),
    )

    asyncio.run(
        dbus_monitor.DBusMonitor.report_hawkbit_error(
            None,
            None,
            None,
            None,
            None,
            (
                dbus_monitor.HAWKBIT_DEPLOYMENT_PROCESS,
                dbus_monitor.HAWKBIT_DEPLOYMENT_SPACE_ERROR,
            ),
        )
    )
    asyncio.run(
        dbus_monitor.DBusMonitor.report_hawkbit_error(
            None,
            None,
            None,
            None,
            None,
            ("EDOWNLOAD", dbus_monitor.HAWKBIT_DEPLOYMENT_SPACE_ERROR),
        )
    )

    assert error_messages == [
        "Error in processing deployment process: File size exceeds available space",
        "Error in downloading process: File size exceeds available space",
    ]
