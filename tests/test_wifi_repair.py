"""Regression tests for Wi-Fi repair safety guards.

The production module has hardware-only dependencies, so these tests load it
under an isolated module name with small system dependency stubs.
"""

import importlib.util
import sys
from pathlib import Path
from types import ModuleType
from unittest.mock import MagicMock, patch

import pytest


def _module(name: str, **attributes):
    module = ModuleType(name)
    for attribute, value in attributes.items():
        setattr(module, attribute, value)
    return module


def _load_wifi_module(monkeypatch):
    config_values = {
        "wifi": {
            "mode": "client",
            "ap_name": "Meticulous",
            "ap_password": "",
            "known_wifis": {},
        },
        "user": {"hostname_override": None},
    }
    config_module = _module(
        "config",
        CONFIG_WIFI="wifi",
        CONFIG_USER="user",
        HOSTNAME_OVERRIDE="hostname_override",
        WIFI_AP_NAME="ap_name",
        WIFI_AP_PASSWORD="ap_password",
        WIFI_KNOWN_WIFIS="known_wifis",
        WIFI_MODE="mode",
        WIFI_MODE_AP="ap",
        WIFI_MODE_CLIENT="client",
        MeticulousConfig=config_values,
    )

    api_module = _module("api")
    api_module.__path__ = []
    zeroconf_module = _module("api.zeroconf_announcement", ZeroConfAnnouncement=MagicMock)
    log_module = _module("log", MeticulousLogger=MagicMock())
    log_module.MeticulousLogger.getLogger.return_value = MagicMock()

    stub_modules = {
        "api": api_module,
        "api.zeroconf_announcement": zeroconf_module,
        "config": config_module,
        "hostname": _module("hostname", HostnameManager=MagicMock()),
        "log": log_module,
        "machine": _module("machine", Machine=MagicMock()),
        "named_thread": _module("named_thread", NamedThread=MagicMock()),
        "netaddr": _module("netaddr", IPAddress=MagicMock(), IPNetwork=MagicMock()),
        "nmcli": MagicMock(),
        "sentry_sdk": MagicMock(),
        "timezone_manager": _module("timezone_manager", TimezoneManager=MagicMock()),
    }
    for name, module in stub_modules.items():
        monkeypatch.setitem(sys.modules, name, module)

    module_name = "wifi_repair_under_test"
    module_path = Path(__file__).resolve().parents[1] / "wifi.py"
    spec = importlib.util.spec_from_file_location(module_name, module_path)
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, module_name, module)
    spec.loader.exec_module(module)
    return module


def _wifi_config(module, *, connected, connection_name):
    return module.WifiSystemConfig(
        connected=connected,
        connection_name=connection_name,
        gateway=None,
        routes=[],
        ips=[],
        dns=[],
        mac="",
        hostname="meticulous",
        domains=[],
    )


def _health(module, *, degraded, last_error=""):
    # A real WifiHealthStatus, never a MagicMock: a mock answers any attribute,
    # which is how repairWifiConnection reading a nonexistent .connected field
    # went unnoticed.
    return module.WifiHealthStatus(
        mode="client",
        link_connected=True,
        has_ipv4=True,
        gateway_reachable=not degraded,
        dns_resolves=True,
        internet_reachable=True,
        ap_active=False,
        degraded=degraded,
        last_error=last_error,
        last_recovery_action="",
        last_recovery_result="",
    )


class StopAutoConnect(Exception):
    pass


def test_repair_without_saved_client_connection_never_resets_hardware(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _wifi_config(module, connected=False, connection_name=None)

    with (
        patch.object(manager, "getCurrentConfig", return_value=current),
        patch.object(manager, "getNetworkManagerWifiConnections", return_value={}),
        patch.object(manager, "suppressAutoConnect") as suppress_auto_connect,
        patch.object(manager, "restartActiveConnection") as restart_connection,
        patch.object(manager, "restartWifiRadio") as restart_radio,
        patch.object(manager, "driverInBandReset") as reset_driver,
        patch.object(manager, "restartWifiService") as restart_service,
    ):
        result = manager.repairWifiConnection(reason="manual")

    assert result is False
    assert manager._last_health_error == "no_saved_wifi_connection"
    assert manager._last_recovery_action == "health_check"
    assert manager._last_recovery_result == "not_recoverable"
    suppress_auto_connect.assert_not_called()
    restart_connection.assert_not_called()
    restart_radio.assert_not_called()
    reset_driver.assert_not_called()
    restart_service.assert_not_called()


def test_disconnected_saved_connection_keeps_existing_repair_path(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _wifi_config(module, connected=False, connection_name=None)
    healthy = _health(module, degraded=False)

    with (
        patch.object(manager, "getCurrentConfig", return_value=current),
        patch.object(
            manager,
            "getNetworkManagerWifiConnections",
            return_value={"Home": {"ssid": "Home"}},
        ),
        patch.object(manager, "getHealthStatus", return_value=healthy) as get_health,
        patch.object(manager, "suppressAutoConnect"),
        patch.object(manager, "invalidateHealthCache"),
    ):
        result = manager.repairWifiConnection(reason="manual")

    assert result is True
    get_health.assert_called_once_with(current, force=True)
    assert manager._last_recovery_result == "not_needed"


def test_repair_health_check_still_runs_for_active_connection(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _wifi_config(module, connected=True, connection_name="Home")
    healthy = _health(module, degraded=False)

    with (
        patch.object(manager, "getCurrentConfig", return_value=current),
        patch.object(manager, "getHealthStatus", return_value=healthy) as get_health,
        patch.object(manager, "suppressAutoConnect"),
        patch.object(manager, "invalidateHealthCache"),
    ):
        result = manager.repairWifiConnection(reason="manual")

    assert result is True
    get_health.assert_called_once_with(current, force=True)
    assert manager._last_recovery_result == "not_needed"


def test_repair_records_the_link_state_of_each_health_check(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _wifi_config(module, connected=True, connection_name="Home")
    degraded = _health(module, degraded=True, last_error="gateway_unreachable")
    healthy = _health(module, degraded=False)

    with (
        patch.object(module.time, "sleep"),
        patch.object(manager, "getCurrentConfig", return_value=current),
        patch.object(manager, "getHealthStatus", side_effect=[degraded, healthy]),
        patch.object(manager, "restartActiveConnection") as restart_connection,
        patch.object(manager, "restartWifiRadio") as restart_radio,
        patch.object(manager, "_zeroconf"),
        patch.object(manager, "update_gatt_advertisement"),
        patch.object(manager, "invalidateHealthCache"),
        patch.object(module, "emit_radio_recovery_event") as emit,
    ):
        result = manager.repairWifiConnection(reason="manual")

    assert result is True
    assert manager._last_recovery_result == "recovered"
    restart_connection.assert_called_once_with(current)
    restart_radio.assert_not_called()
    recorded = {call.args[1]: call.kwargs for call in emit.call_args_list}
    assert recorded["health_assessed"]["connected"] is True
    assert recorded["step_assessed"]["connected"] is True
    assert recorded["completed"]["result"] == "recovered"


def test_manual_repair_escalates_to_the_radio_when_restarting_the_connection_fails(
    monkeypatch,
):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _wifi_config(module, connected=True, connection_name="Home")
    degraded = _health(module, degraded=True, last_error="gateway_unreachable")
    healthy = _health(module, degraded=False)

    with (
        patch.object(module.time, "sleep"),
        patch.object(manager, "getCurrentConfig", return_value=current),
        patch.object(manager, "getHealthStatus", side_effect=[degraded, degraded, healthy]),
        patch.object(manager, "restartActiveConnection") as restart_connection,
        patch.object(manager, "restartWifiRadio") as restart_radio,
        patch.object(manager, "driverInBandReset") as reset_driver,
        patch.object(manager, "_zeroconf"),
        patch.object(manager, "update_gatt_advertisement"),
        patch.object(manager, "invalidateHealthCache"),
    ):
        result = manager.repairWifiConnection(reason="manual")

    assert result is True
    restart_connection.assert_called_once_with(current)
    restart_radio.assert_called_once_with()
    reset_driver.assert_not_called()
    assert manager._last_recovery_action == "restart_wifi_radio"
    assert manager._last_recovery_result == "recovered"


def test_background_health_check_records_without_repairing(monkeypatch):
    # The health check runs inside the WifiAutoConnect loop: it must neither
    # start the repair ladder nor raise, or the thread ends until reboot.
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _wifi_config(module, connected=True, connection_name="Home")
    degraded = _health(module, degraded=True, last_error="gateway_unreachable")
    manager._health_failures = manager._health_failure_threshold - 1
    loop_passes = []

    def sleep(seconds):
        # Every pass of the loop starts with a sleep: stop at the second pass.
        loop_passes.append(seconds)
        if len(loop_passes) == 2:
            raise StopAutoConnect()

    with (
        patch.object(module.time, "sleep", side_effect=sleep),
        patch.object(module.asyncio, "new_event_loop"),
        patch.object(module.asyncio, "set_event_loop"),
        patch.object(manager, "getCurrentConfig", return_value=current),
        patch.object(manager, "getHealthStatus", return_value=degraded),
        patch.object(manager, "repairWifiConnection") as repair,
        patch.object(module, "emit_radio_recovery_event") as emit,
        pytest.raises(StopAutoConnect),
    ):
        manager.tryAutoConnect()

    repair.assert_not_called()
    emit.assert_called_once()
    assert emit.call_args.args[1] == "rejected"
    assert emit.call_args.kwargs["reason"] == "gateway_unreachable"
    assert emit.call_args.kwargs["result"] == "background_repair_disabled"
    assert manager._health_failures == manager._health_failure_threshold
