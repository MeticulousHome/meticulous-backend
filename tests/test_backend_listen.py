"""
Regression test for invalid kwargs passed to app.listen() in backend.py.

ffebc909 introduced a typo ('aaddress' instead of 'address') that caused a
TypeError at startup. This test actually runs backend.main() — with hardware,
BLE, networking, and the event loop mocked — but lets app.listen() execute
for real. Because Application.listen() constructs HTTPServer(**kwargs) before
touching the socket layer, any invalid kwarg raises TypeError before our
bind_sockets mock is ever reached.
"""

import contextlib
import sys
from unittest.mock import MagicMock, patch


_PATCHES = [
    # Block actual socket binding — but HTTPServer(**kwargs) runs first,
    # so a bad kwarg raises TypeError before this mock is called.
    patch("tornado.netutil.bind_sockets", return_value=[]),
    # Prevent the event loop from blocking indefinitely.
    patch("tornado.ioloop.IOLoop.current"),
    # parse_command_line() would try to parse pytest's argv as Tornado flags.
    patch("backend.parse_command_line"),
    # Hardware / system dependencies
    patch("machine.Machine.init"),
    patch("machine.Machine.emulated", new=False),
    patch("ble_gatt.GATTServer.getServer", return_value=MagicMock()),
    patch("wifi.WifiManager.init"),
    patch("wifi.WifiManager.networking_available", return_value=False),
    patch("notifications.NotificationManager.init"),
    patch("profiles.ProfileManager.init"),
    patch("sounds.SoundPlayer.init"),
    patch("timezone_manager.TimezoneManager.init"),
    patch("telemetry_service.TelemetryService.init"),
    patch("config.MeticulousConfig.setSIO"),
    patch("imager.DiscImager.flash_if_required"),
    patch("dbus_monitor.DBusMonitor.init"),
    patch("dbus_monitor.DBusMonitor.enableUSBTest"),
    patch("hostname.HostnameManager.init"),
    patch("ota.UpdateManager.init"),
    patch("ota.UpdateManager.getRepositoryInfo", return_value={}),
    patch("ota.UpdateManager.getBuildTimestamp", return_value=""),
    patch("ota.UpdateManager.getImageChannel", return_value=""),
    patch("ota.UpdateManager.getImageVersion", return_value=""),
    patch("ssh_manager.SSHManager.init"),
    patch("system_services.SystemServices.init"),
    patch("usb.USBManager.init"),
    patch("api.alarms.AlarmManager.init"),
    # Prevent the stdin-reading background thread from starting.
    patch("backend.NamedThread"),
    patch("pyprctl.set_name"),
]


def test_main_starts_without_error():
    """backend.main() must reach and survive app.listen() with no TypeError.

    Any invalid kwarg (e.g. 'aaddress' instead of 'address') raises TypeError
    inside HTTPServer.__init__() before the socket mock is reached, so this
    test catches the regression by actually executing the startup path.
    """
    # Remove cached module so module-level code re-runs under our patches.
    sys.modules.pop("backend", None)

    with contextlib.ExitStack() as stack:
        for p in _PATCHES:
            stack.enter_context(p)

        import backend

        backend.main()
