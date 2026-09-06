"""Regression tests for machine identity settling at startup.

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

    module_name = "wifi_identity_under_test"
    module_path = Path(__file__).resolve().parents[1] / "wifi.py"
    spec = importlib.util.spec_from_file_location(module_name, module_path)
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, module_name, module)
    spec.loader.exec_module(module)
    return module


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


def test_identity_initialization_settles_factory_hostname(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    hostname_manager = sys.modules["hostname"].HostnameManager
    hostname_manager.generateHostname.return_value = "meticulousSpicyCrema-003312"
    hostname_manager.generateDeviceName.return_value = "MeticulousSpicyCrema"

    with patch.object(module.socket, "gethostname", return_value="meticulousSpicyCrema-003312"):
        settled = module.WifiManager.initializeIdentity("meticulous")

    assert settled == "meticulousSpicyCrema-003312"
    hostname_manager.setHostname.assert_called_once_with("meticulousSpicyCrema-003312")
    assert module.MeticulousConfig["wifi"]["ap_name"] == "MeticulousSpicyCrema"


def test_identity_initialization_continues_when_hostname_does_not_settle(monkeypatch):
    """A slow systemd-hostnamed must never keep the backend from serving."""
    module = _load_wifi_module(monkeypatch)
    hostname_manager = sys.modules["hostname"].HostnameManager
    hostname_manager.generateHostname.return_value = "meticulousSpicyCrema-003312"
    hostname_manager.generateDeviceName.return_value = "MeticulousSpicyCrema"

    with patch.object(module.socket, "gethostname", return_value="meticulous"):
        settled = module.WifiManager.initializeIdentity("meticulous")

    assert settled == "meticulous"
    hostname_manager.setHostname.assert_called_once_with("meticulousSpicyCrema-003312")
    # Identity settling is best-effort; the advertised name is still configured.
    assert module.MeticulousConfig["wifi"]["ap_name"] == "MeticulousSpicyCrema"
    module.MeticulousConfig.save.assert_called_once_with()


def test_identity_initialization_survives_missing_hostnamectl(monkeypatch):
    """Development hosts without hostnamectl must not crash-loop the backend."""
    module = _load_wifi_module(monkeypatch)
    hostname_manager = sys.modules["hostname"].HostnameManager
    hostname_manager.generateHostname.return_value = "meticulousSpicyCrema-003312"
    hostname_manager.generateDeviceName.return_value = "MeticulousSpicyCrema"
    hostname_manager.setHostname.side_effect = FileNotFoundError("hostnamectl")

    settled = module.WifiManager.initializeIdentity("meticulous")

    assert settled == "meticulous"
    hostname_manager.setHostname.assert_called_once_with("meticulousSpicyCrema-003312")
    assert module.MeticulousConfig["wifi"]["ap_name"] == "MeticulousSpicyCrema"
    module.MeticulousConfig.save.assert_called_once_with()
