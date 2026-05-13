import importlib
import json
import sys
import types
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest


class FakeConfig(dict):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.save_count = 0

    def save(self):
        self.save_count += 1


@pytest.fixture()
def wifi_modules(monkeypatch):
    for module_name in ("wifi", "api.wifi", "api.base_handler"):
        sys.modules.pop(module_name, None)

    config = FakeConfig(
        {
            "wifi": {
                "mode": "CLIENT",
                "APName": "MeticulousTest",
                "APPassword": "old-password",
                "KnownWifis": {},
            },
            "user": {"hostname_override": None},
            "system": {"allowed_networks": [], "auth_key": "test-key"},
        }
    )

    config_module = types.ModuleType("config")
    config_module.CONFIG_WIFI = "wifi"
    config_module.CONFIG_USER = "user"
    config_module.CONFIG_SYSTEM = "system"
    config_module.HOSTNAME_OVERRIDE = "hostname_override"
    config_module.HTTP_ALLOWED_NETWORKS = "allowed_networks"
    config_module.HTTP_AUTH_KEY = "auth_key"
    config_module.WIFI_AP_NAME = "APName"
    config_module.WIFI_AP_PASSWORD = "APPassword"
    config_module.WIFI_KNOWN_WIFIS = "KnownWifis"
    config_module.WIFI_MODE = "mode"
    config_module.WIFI_MODE_AP = "AP"
    config_module.WIFI_MODE_CLIENT = "CLIENT"
    config_module.MeticulousConfig = config
    monkeypatch.setitem(sys.modules, "config", config_module)

    logger = MagicMock()
    log_module = types.ModuleType("log")
    log_module.MeticulousLogger = MagicMock()
    log_module.MeticulousLogger.getLogger.return_value = logger
    monkeypatch.setitem(sys.modules, "log", log_module)

    nmcli_module = types.ModuleType("nmcli")
    nmcli_module.disable_use_sudo = MagicMock()
    nmcli_module.set_lang = MagicMock()
    nmcli_module.device = SimpleNamespace(
        show_all=MagicMock(),
        wifi_hotspot=MagicMock(),
        wifi=MagicMock(return_value=[]),
    )
    nmcli_module.connection = SimpleNamespace(down=MagicMock())
    monkeypatch.setitem(sys.modules, "nmcli", nmcli_module)

    hostname_module = types.ModuleType("hostname")
    hostname_module.HostnameManager = SimpleNamespace(
        generateHostname=MagicMock(return_value="meticulousTest"),
        generateDeviceName=MagicMock(return_value="MeticulousTest"),
        setHostname=MagicMock(),
    )
    monkeypatch.setitem(sys.modules, "hostname", hostname_module)

    timezone_module = types.ModuleType("timezone_manager")
    timezone_module.TimezoneManager = SimpleNamespace(tz_background_update=MagicMock())
    monkeypatch.setitem(sys.modules, "timezone_manager", timezone_module)

    machine_module = types.ModuleType("machine")
    machine_module.Machine = SimpleNamespace(enable_manufacturing=False)
    monkeypatch.setitem(sys.modules, "machine", machine_module)

    named_thread_module = types.ModuleType("named_thread")
    named_thread_module.NamedThread = MagicMock()
    monkeypatch.setitem(sys.modules, "named_thread", named_thread_module)

    sentry_module = types.ModuleType("sentry_sdk")
    monkeypatch.setitem(sys.modules, "sentry_sdk", sentry_module)

    zeroconf_module = types.ModuleType("zeroconf")
    zeroconf_module.InterfaceChoice = SimpleNamespace(All="all")
    zeroconf_module.IPVersion = SimpleNamespace(All="all")
    zeroconf_module.NonUniqueNameException = Exception
    zeroconf_module.ServiceInfo = MagicMock()
    zeroconf_module.Zeroconf = MagicMock()
    monkeypatch.setitem(sys.modules, "zeroconf", zeroconf_module)

    ble_gatt_module = types.ModuleType("ble_gatt")
    ble_gatt_module.PORT = 80
    monkeypatch.setitem(sys.modules, "ble_gatt", ble_gatt_module)

    wifi_module = importlib.import_module("wifi")
    api_wifi_module = importlib.import_module("api.wifi")
    wifi_module.WifiManager._networking_available = True
    wifi_module.WifiManager._zeroconf = SimpleNamespace(restart=MagicMock())

    context = SimpleNamespace(
        api_wifi=api_wifi_module,
        config=config,
        config_module=config_module,
        logger=logger,
        nmcli=nmcli_module,
        wifi=wifi_module,
    )
    yield context

    for module_name in ("wifi", "api.wifi", "api.base_handler"):
        sys.modules.pop(module_name, None)


def test_start_hotspot_returns_false_when_nmcli_fails(wifi_modules):
    wifi_modules.nmcli.device.wifi_hotspot.side_effect = Exception("activation failed")

    assert wifi_modules.wifi.WifiManager.startHotspot() is False
    wifi_modules.wifi.WifiManager._zeroconf.restart.assert_called_once()


def test_reset_wifi_mode_reverts_to_client_when_hotspot_fails(wifi_modules, monkeypatch):
    wifi_modules.config["wifi"]["mode"] = "AP"
    monkeypatch.setattr(
        wifi_modules.wifi.WifiManager, "startHotspot", MagicMock(return_value=False)
    )
    monkeypatch.setattr(
        wifi_modules.wifi.WifiManager, "update_gatt_advertisement", MagicMock()
    )

    assert wifi_modules.wifi.WifiManager.resetWifiMode() is False
    assert wifi_modules.config["wifi"]["mode"] == "CLIENT"
    assert wifi_modules.config.save_count == 1


def test_wifi_config_post_keeps_client_mode_when_hotspot_fails(wifi_modules, monkeypatch):
    monkeypatch.setattr(
        wifi_modules.api_wifi.WifiManager, "startHotspot", MagicMock(return_value=False)
    )
    monkeypatch.setattr(
        wifi_modules.api_wifi.WifiManager, "update_gatt_advertisement", MagicMock()
    )
    handler = wifi_modules.api_wifi.WiFiConfigHandler.__new__(
        wifi_modules.api_wifi.WiFiConfigHandler
    )
    handler.request = SimpleNamespace(
        body=json.dumps({"mode": "AP", "apPassword": "new-password"}).encode()
    )
    handler.set_status = MagicMock()
    handler.write = MagicMock()
    handler.get = MagicMock()

    handler.post()

    handler.set_status.assert_called_once_with(409)
    handler.write.assert_called_once_with(
        {"status": "error", "error": "failed to start hotspot"}
    )
    assert wifi_modules.config["wifi"]["mode"] == "CLIENT"
    assert wifi_modules.config["wifi"]["APPassword"] == "old-password"
    assert wifi_modules.config.save_count == 1
    handler.get.assert_not_called()


def test_wifi_config_post_persists_ap_mode_when_hotspot_starts(wifi_modules, monkeypatch):
    monkeypatch.setattr(
        wifi_modules.api_wifi.WifiManager, "startHotspot", MagicMock(return_value=True)
    )
    monkeypatch.setattr(
        wifi_modules.api_wifi.WifiManager, "update_gatt_advertisement", MagicMock()
    )
    handler = wifi_modules.api_wifi.WiFiConfigHandler.__new__(
        wifi_modules.api_wifi.WiFiConfigHandler
    )
    handler.request = SimpleNamespace(body=json.dumps({"mode": "AP"}).encode())
    handler.set_status = MagicMock()
    handler.write = MagicMock()
    handler.get = MagicMock(return_value="response")

    assert handler.post() == "response"
    assert wifi_modules.config["wifi"]["mode"] == "AP"
    assert wifi_modules.config.save_count == 1
    handler.set_status.assert_not_called()
