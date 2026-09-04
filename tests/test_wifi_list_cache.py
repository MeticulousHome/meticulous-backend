"""Tests for the Wi-Fi scan cache cold-start behavior.

`WiFiListHandler` returns the scan cache immediately so it never blocks a
worker on the steady-state 5s poll from the dial. The one exception is the
cold-start case: when both `_scan_cache` and `_known_wifis` are empty there is
nothing to show, so the first caller blocks on a single shared bounded scan
instead of returning an empty list (which the client treats as "no networks"
and stops showing its loading UI).

The production module has hardware-only dependencies, so these tests load it
under an isolated module name with small system dependency stubs, mirroring
tests/test_wifi_repair.py.
"""

import importlib.util
import sys
import threading
import time
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

    # Run NamedThread targets on a real thread so the "shared scan" test can
    # exercise concurrency, but keep the interface identical to named_thread.
    stub_modules = {
        "api": api_module,
        "api.zeroconf_announcement": zeroconf_module,
        "config": config_module,
        "hostname": _module("hostname", HostnameManager=MagicMock()),
        "log": log_module,
        "machine": _module("machine", Machine=MagicMock()),
        "named_thread": _module("named_thread", NamedThread=threading.Thread),
        "netaddr": _module("netaddr", IPAddress=MagicMock(), IPNetwork=MagicMock()),
        "nmcli": MagicMock(),
        "sentry_sdk": MagicMock(),
        "timezone_manager": _module("timezone_manager", TimezoneManager=MagicMock()),
    }
    for name, module in stub_modules.items():
        monkeypatch.setitem(sys.modules, name, module)

    module_name = "wifi_list_cache_under_test"
    module_path = Path(__file__).resolve().parents[1] / "wifi.py"
    spec = importlib.util.spec_from_file_location(module_name, module_path)
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, module_name, module)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def manager(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    mgr = module.WifiManager
    mgr._scan_cache = []
    mgr._scan_cache_time = 0
    mgr._known_wifis = []
    mgr._scan_in_progress = False
    return mgr


def _network(ssid):
    return MagicMock(ssid=ssid)


def test_cold_start_blocks_on_scan_and_returns_real_list(manager):
    scanned = [_network("Home"), _network("Cafe")]

    def fake_scan(timeout=8, target_network_ssid=None):
        manager._scan_cache = scanned
        return scanned

    with (
        patch.object(manager, "scanForNetworks", side_effect=fake_scan) as scan,
        patch.object(manager, "refreshScanCacheInBackground") as background,
    ):
        result = manager.getAvailableNetworks(refresh=True, block_if_empty=True)

    assert result == scanned
    scan.assert_called_once()
    background.assert_not_called()


def test_populated_cache_never_blocks(manager):
    manager._scan_cache = [_network("Home")]

    with (
        patch.object(manager, "scanForNetworks") as scan,
        patch.object(manager, "refreshScanCacheInBackground") as background,
    ):
        result = manager.getAvailableNetworks(refresh=True, block_if_empty=True)

    assert result == manager._scan_cache
    scan.assert_not_called()
    background.assert_called_once()


def test_block_if_empty_defaults_off_and_returns_immediately(manager):
    with (
        patch.object(manager, "scanForNetworks") as scan,
        patch.object(manager, "refreshScanCacheInBackground") as background,
    ):
        result = manager.getAvailableNetworks(refresh=True)

    assert result == []
    scan.assert_not_called()
    background.assert_called_once()


def test_concurrent_cold_start_callers_share_one_scan(manager):
    scanned = [_network("Home")]
    call_count = 0

    def fake_scan(timeout=8, target_network_ssid=None):
        nonlocal call_count
        call_count += 1
        time.sleep(0.2)
        manager._scan_cache = scanned
        return scanned

    results = []

    def worker():
        results.append(manager.getAvailableNetworks(refresh=True, block_if_empty=True))

    with (
        patch.object(manager, "scanForNetworks", side_effect=fake_scan),
        patch.object(manager, "refreshScanCacheInBackground"),
    ):
        threads = [threading.Thread(target=worker) for _ in range(4)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()

    assert call_count == 1
    assert all(result == scanned for result in results)
