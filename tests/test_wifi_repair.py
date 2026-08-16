"""Regression tests for Wi-Fi repair safety guards.

The production module has hardware-only dependencies, so these tests load it
under an isolated module name with small system dependency stubs.
"""

import importlib.util
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import MagicMock, patch

import pytest


def _module(name: str, **attributes):
    module = ModuleType(name)
    for attribute, value in attributes.items():
        setattr(module, attribute, value)
    return module


def _load_wifi_module(monkeypatch):
    class Config(dict):
        save = MagicMock()

    config_values = Config(
        {
            "wifi": {
                "mode": "client",
                "ap_name": "Meticulous",
                "ap_password": "",
                "known_wifis": {},
            },
            "user": {"hostname_override": None},
        }
    )
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


@pytest.mark.parametrize(
    ("current", "generated", "override", "expected"),
    [
        (
            "meticulousSpicyCrema-003312",
            "meticulousRenownedBody-003312",
            None,
            None,
        ),
        (
            "meticulousRenownedBody-003312",
            "meticulousSpicyCrema-003312",
            None,
            None,
        ),
        ("imx8mn-var-som", "meticulousSpicyCrema-003312", None, "meticulousSpicyCrema-003312"),
        (
            "imx8mn-var-som-003312",
            "meticulousSpicyCrema-003312",
            None,
            "meticulousSpicyCrema-003312",
        ),
        ("meticulous", "meticulousSpicyCrema-003312", None, "meticulousSpicyCrema-003312"),
        (
            "meticulousSpicyCrema-003312",
            "meticulousRenownedBody-003312",
            "custom-machine",
            "custom-machine",
        ),
        (
            "meticulousSpicyCrema-003312",
            "meticulousRenownedBody-003312",
            "none",
            None,
        ),
    ],
)
def test_hostname_update_only_targets_factory_or_explicit_override(
    monkeypatch, current, generated, override, expected
):
    module = _load_wifi_module(monkeypatch)

    assert module.WifiManager.hostnameUpdateTarget(current, generated, override) == expected


def test_identity_initialization_preserves_established_hostname(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    hostname_manager = sys.modules["hostname"].HostnameManager
    hostname_manager.generateHostname.return_value = "meticulousRenownedBody-003312"
    hostname_manager.generateDeviceName.return_value = "MeticulousSpicyCrema"

    settled = module.WifiManager.initializeIdentity("meticulousSpicyCrema-003312")

    assert settled == "meticulousSpicyCrema-003312"
    hostname_manager.setHostname.assert_not_called()
    assert module.MeticulousConfig["wifi"]["ap_name"] == "MeticulousSpicyCrema"
    module.MeticulousConfig.save.assert_called_once_with()


def test_identity_initialization_requires_hostname_change_to_settle(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    hostname_manager = sys.modules["hostname"].HostnameManager
    hostname_manager.generateHostname.return_value = "meticulousSpicyCrema-003312"
    hostname_manager.generateDeviceName.return_value = "MeticulousSpicyCrema"

    with patch.object(module.socket, "gethostname", return_value="meticulous"):
        with pytest.raises(RuntimeError, match="did not settle"):
            module.WifiManager.initializeIdentity("meticulous")

    hostname_manager.setHostname.assert_called_once_with("meticulousSpicyCrema-003312")
    module.MeticulousConfig.save.assert_not_called()


def _connected_ipv4_config(module, *, connection_name="Home", address="192.0.2.10"):
    config = _wifi_config(module, connected=True, connection_name=connection_name)
    config.ips = [
        SimpleNamespace(
            ip=SimpleNamespace(version=4),
            __str__=lambda: address,
        )
    ]
    config.gateway = "192.0.2.1"
    return config


def _health(module, *, degraded=False, dns_resolves=True):
    return module.WifiHealthStatus(
        "client",
        True,
        True,
        True,
        dns_resolves,
        True,
        False,
        degraded,
        "dns_unreachable" if degraded else "",
        "",
        "",
    )


def test_initial_shallow_health_does_not_cache_unchecked_probe_failures(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _connected_ipv4_config(module)

    manager.invalidateHealthCache()
    with patch.object(manager, "refreshHealthInBackground") as refresh:
        result = manager.getHealthStatus(current, deep=False)

    assert result.link_connected is True
    assert result.has_ipv4 is True
    assert result.gateway_reachable is False
    assert result.dns_resolves is False
    assert result.internet_reachable is False
    assert result.degraded is False
    assert manager._cached_health is None
    refresh.assert_called_once_with(current)


def test_expired_matching_health_stays_visible_during_background_refresh(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _connected_ipv4_config(module)
    previous = _health(module)

    manager._cached_health = previous
    manager._health_cache_signature = manager.getHealthCacheSignature(current)
    manager._health_cache_time = 0

    with patch.object(manager, "refreshHealthInBackground") as refresh:
        result = manager.getHealthStatus(current, deep=False)

    assert result is previous
    assert result.dns_resolves is True
    refresh.assert_called_once_with(current)


def test_changed_connection_does_not_reuse_previous_network_health(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    old_config = _connected_ipv4_config(module, connection_name="Old network")
    current = _connected_ipv4_config(module, connection_name="New network")

    manager._cached_health = _health(module)
    manager._health_cache_signature = manager.getHealthCacheSignature(old_config)
    manager._health_cache_time = 0

    with patch.object(manager, "refreshHealthInBackground") as refresh:
        result = manager.getHealthStatus(current, deep=False)

    assert result is not manager._cached_health
    assert result.degraded is False
    assert result.dns_resolves is False
    refresh.assert_called_once_with(current)


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
    healthy = MagicMock(degraded=False)

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
    healthy = MagicMock(degraded=False)

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
