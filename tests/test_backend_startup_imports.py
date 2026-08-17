import importlib.util
import ipaddress
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import MagicMock

import pytest


def _module(name, **attributes):
    module = ModuleType(name)
    for attribute, value in attributes.items():
        setattr(module, attribute, value)
    return module


def _load_wifi_api(monkeypatch):
    package_name = "startup_contract_api"
    package = _module(package_name)
    package.__path__ = []
    manager = MagicMock()
    config = _module(
        "config",
        MeticulousConfig={"wifi": {}},
        CONFIG_WIFI="wifi",
        WIFI_AP_NAME="ap_name",
        WIFI_AP_PASSWORD="ap_password",
        WIFI_MODE="mode",
        WIFI_MODE_AP="ap",
        WIFI_MODE_CLIENT="client",
    )
    api = MagicMock()
    api_version = SimpleNamespace(V1="v1")
    stubs = {
        package_name: package,
        f"{package_name}.base_handler": _module(
            f"{package_name}.base_handler", BaseHandler=object
        ),
        f"{package_name}.api": _module(f"{package_name}.api", API=api, APIVersion=api_version),
        "config": config,
        "wifi": _module(
            "wifi",
            WifiManager=manager,
            WifiType=MagicMock(),
            redact_ssid=lambda value: value,
        ),
        # Deliberately model current Nightly: ble_gatt no longer exports PORT.
        "ble_gatt": _module("ble_gatt"),
        "log": _module("log", MeticulousLogger=MagicMock()),
    }
    stubs["log"].MeticulousLogger.getLogger.return_value = MagicMock()
    for name, module in stubs.items():
        monkeypatch.setitem(sys.modules, name, module)

    module_name = f"{package_name}.wifi"
    module_path = Path(__file__).resolve().parents[1] / "api" / "wifi.py"
    spec = importlib.util.spec_from_file_location(module_name, module_path)
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, module_name, module)
    spec.loader.exec_module(module)
    return module, manager


def test_wifi_api_imports_against_current_ble_contract(monkeypatch):
    """Catch startup imports of symbols removed from current Nightly."""
    module, _manager = _load_wifi_api(monkeypatch)

    assert module.WiFiQRHandler is not None


@pytest.mark.parametrize(
    ("address", "version", "hostname", "expected"),
    [
        ("192.0.2.10", 4, "meticulous", "http://192.0.2.10"),
        ("2001:db8::10", 6, "meticulous", "http://[2001:db8::10]"),
        (None, None, "meticulous", "http://meticulous.local"),
    ],
)
def test_wifi_qr_uses_the_public_machine_url(monkeypatch, address, version, hostname, expected):
    module, manager = _load_wifi_api(monkeypatch)
    ips = []
    if address is not None:
        parsed = ipaddress.ip_address(address)
        assert parsed.version == version
        ips.append(SimpleNamespace(ip=parsed))
    manager.getCurrentConfig.return_value = SimpleNamespace(
        is_hotspot=lambda: False,
        ips=ips,
        hostname=hostname,
    )
    qr = MagicMock()
    create = MagicMock(return_value=qr)
    monkeypatch.setattr(module.pyqrcode, "create", create)

    module.WiFiQRHandler().generate_wifi_qr()

    create.assert_called_once_with(expected)
