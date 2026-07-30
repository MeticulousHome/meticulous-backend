"""Regression tests for the WiFi reliability fixes.

Covers: gateway-ping handling in health checks, scan-cache preservation,
scan suppression during connects, repair-ladder reconnects, migration
serialization, saved-type caching, error copy, and connect housekeeping
never masking a successful join.
"""

from types import SimpleNamespace
from unittest.mock import MagicMock, patch

from tests.test_wifi_repair import _load_wifi_module, _wifi_config


def _ipv4():
    return SimpleNamespace(ip=SimpleNamespace(version=4))


def _connected_config(module, name="Home"):
    return module.WifiSystemConfig(
        connected=True,
        connection_name=name,
        gateway=SimpleNamespace(format=lambda: "192.168.0.1"),
        routes=[],
        ips=[_ipv4()],
        dns=[],
        mac="",
        hostname="meticulous",
        domains=[],
    )


class FakeConfig(dict):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.saved = 0

    def save(self):
        self.saved += 1


# --- Item 2/7: gateway ping must not degrade a working connection -----------


def test_gateway_ping_failure_alone_is_not_degraded(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    config = _connected_config(module)

    with (
        patch.object(manager, "gatewayReachable", return_value=False),
        patch.object(manager, "dnsResolves", return_value=True),
        patch.object(manager, "internetReachable", return_value=True),
    ):
        health = manager.buildHealthStatus(config, deep=True)

    assert health.degraded is False
    assert health.last_error == ""
    # Reporting stays honest: the ping did fail.
    assert health.gateway_reachable is False


def test_fully_broken_link_still_maps_to_recoverable_gateway_error(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    config = _connected_config(module)

    with (
        patch.object(manager, "gatewayReachable", return_value=False),
        patch.object(manager, "dnsResolves", return_value=False),
        patch.object(manager, "internetReachable", return_value=False),
    ):
        health = manager.buildHealthStatus(config, deep=True)

    assert health.degraded is True
    assert health.last_error == "gateway_unreachable"
    assert manager.healthErrorIsRecoverable(health.last_error)


def test_health_error_is_derived_fresh_not_from_stale_state(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    manager._last_health_error = "wifi_device_unavailable"
    config = _connected_config(module)

    with (
        patch.object(manager, "gatewayReachable", return_value=True),
        patch.object(manager, "dnsResolves", return_value=False),
        patch.object(manager, "internetReachable", return_value=False),
    ):
        health = manager.buildHealthStatus(config, deep=True)

    assert health.degraded is True
    assert health.last_error == "dns_unreachable"


# --- Item 3: gateway ping retries -------------------------------------------


def test_gateway_ping_retries_and_succeeds_on_late_reply(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    gateway = SimpleNamespace(format=lambda: "192.168.0.1")

    results = [SimpleNamespace(returncode=1), SimpleNamespace(returncode=0)]
    with (
        patch.object(module.shutil, "which", return_value="/bin/ping"),
        patch.object(manager, "runCommand", side_effect=results) as run_command,
    ):
        assert manager.gatewayReachable(gateway) is True
    assert run_command.call_count == 2


def test_gateway_ping_gives_up_after_three_attempts(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    gateway = SimpleNamespace(format=lambda: "192.168.0.1")

    with (
        patch.object(module.shutil, "which", return_value="/bin/ping"),
        patch.object(
            manager, "runCommand", return_value=SimpleNamespace(returncode=1)
        ) as run_command,
    ):
        assert manager.gatewayReachable(gateway) is False
    assert run_command.call_count == 3


# --- Item 4: empty scans must not wipe the cache ----------------------------


def test_timed_out_scan_keeps_previous_results(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    previous = [SimpleNamespace(ssid="Home", security="WPA2")]
    manager._scan_cache = previous
    manager._known_wifis = previous
    manager._connect_in_progress = 0

    module.nmcli.device.wifi.return_value = []
    with patch.object(manager, "isWifiDeviceReady", return_value=True):
        result = manager.scanForNetworks(timeout=1)

    assert result == []
    assert manager._scan_cache is previous
    assert manager._known_wifis is previous


def test_successful_scan_updates_cache(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    manager._scan_cache = []
    manager._connect_in_progress = 0
    fresh = [SimpleNamespace(ssid="Cafe", security="")]

    module.nmcli.device.wifi.return_value = fresh
    with patch.object(manager, "isWifiDeviceReady", return_value=True):
        result = manager.scanForNetworks(timeout=1)

    assert result == fresh
    assert manager._scan_cache == fresh


# --- Item 5: no rescans while a connect is in flight ------------------------


def test_scan_returns_cache_during_connect(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    cached = [SimpleNamespace(ssid="Home", security="WPA2")]
    manager._scan_cache = cached
    manager._connect_in_progress = 1

    module.nmcli.device.wifi.reset_mock()
    try:
        result = manager.scanForNetworks(timeout=5)
    finally:
        manager._connect_in_progress = 0

    assert result is cached
    module.nmcli.device.wifi.assert_not_called()


def test_targeted_scan_still_allowed_during_connect(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    manager._connect_in_progress = 1
    wanted = [SimpleNamespace(ssid="Home", security="WPA2")]

    module.nmcli.device.wifi.return_value = wanted
    try:
        with patch.object(manager, "isWifiDeviceReady", return_value=True):
            result = manager.scanForNetworks(timeout=1, target_network_ssid="Home")
    finally:
        manager._connect_in_progress = 0

    assert result == wanted


def test_background_refresh_skipped_during_connect(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    manager._connect_in_progress = 1
    manager._scan_in_progress = False

    with patch.object(module, "NamedThread") as thread:
        try:
            manager.refreshScanCacheInBackground()
        finally:
            manager._connect_in_progress = 0
    thread.assert_not_called()


# --- Item 6: repair ladder re-activates the client connection ---------------


def _health(module, *, degraded, last_error=""):
    """Build a real WifiHealthStatus so attribute renames cannot hide behind
    auto-attribute mocks (the diagnostics code once read a field that does
    not exist on the dataclass)."""
    return module.WifiHealthStatus(
        mode="client",
        link_connected=not degraded,
        has_ipv4=not degraded,
        gateway_reachable=not degraded,
        dns_resolves=not degraded,
        internet_reachable=not degraded,
        ap_active=False,
        degraded=degraded,
        last_error=last_error,
        last_recovery_action="",
        last_recovery_result="",
    )


def test_repair_reconnects_client_after_radio_level_step(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _connected_config(module)
    degraded = _health(module, degraded=True, last_error="wifi_not_connected")
    healthy = _health(module, degraded=False)

    monkeypatch.setattr(module.time, "sleep", lambda seconds: None)
    with (
        patch.object(manager, "getCurrentConfig", return_value=current),
        patch.object(manager, "getHealthStatus", side_effect=[degraded, degraded, healthy]),
        patch.object(manager, "restartActiveConnection") as restart_connection,
        patch.object(manager, "restartWifiRadio") as restart_radio,
        patch.object(manager, "reconnectAfterRepairStep") as reconnect,
        patch.object(manager, "restartZeroconf"),
        patch.object(manager, "update_gatt_advertisement"),
        patch.object(manager, "invalidateHealthCache"),
    ):
        result = manager.repairWifiConnection(reason="manual")

    assert result is True
    restart_connection.assert_called_once()
    restart_radio.assert_called_once()
    # restart_connection re-ups by itself; only the radio-level step needs
    # the explicit reconnect.
    reconnect.assert_called_once_with(current)


def test_repair_diagnostics_accept_real_health_objects(monkeypatch):
    """Regression: repair once crashed with AttributeError because the
    diagnostics read health.connected, which WifiHealthStatus does not have.
    A raise here killed the auto-connect thread for good."""
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _connected_config(module)
    healthy = _health(module, degraded=False)

    with (
        patch.object(manager, "getCurrentConfig", return_value=current),
        patch.object(manager, "getHealthStatus", return_value=healthy),
        patch.object(manager, "invalidateHealthCache"),
    ):
        result = manager.repairWifiConnection(reason="manual")

    assert result is True
    assert manager._last_recovery_result == "not_needed"


def test_reconnect_after_repair_step_uses_active_connection(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _connected_config(module, name="Home")

    module.nmcli.connection.up.reset_mock()
    manager.reconnectAfterRepairStep(current)
    module.nmcli.connection.up.assert_called_once_with("Home", wait=15)


def test_reconnect_after_repair_step_falls_back_to_saved_profile(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager

    module.nmcli.connection.up.reset_mock()
    with patch.object(
        manager,
        "getNetworkManagerWifiConnections",
        return_value={"Fallback": {"ssid": "Fallback"}},
    ):
        manager.reconnectAfterRepairStep(None)
    module.nmcli.connection.up.assert_called_once_with("Fallback", wait=15)


def test_reconnect_after_repair_step_without_profiles_does_nothing(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager

    module.nmcli.connection.up.reset_mock()
    with patch.object(manager, "getNetworkManagerWifiConnections", return_value={}):
        manager.reconnectAfterRepairStep(None)
    module.nmcli.connection.up.assert_not_called()


# --- Item 8: migration runs once and is serialized --------------------------


def test_migration_runs_once_after_success(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    fake_config = FakeConfig({"wifi": {"known_wifis": {"Home": "secret"}, "mode": "client"}})
    monkeypatch.setattr(module, "MeticulousConfig", fake_config)

    with (
        patch.object(manager, "getNetworkManagerWifiConnections", return_value={}),
        patch.object(manager, "createNetworkManagerWifiProfile", return_value=True) as create,
    ):
        manager.migrateKnownWifiSecretsToNetworkManager()
        manager.migrateKnownWifiSecretsToNetworkManager()

    assert create.call_count == 1
    assert manager._migration_completed is True
    assert fake_config["wifi"]["known_wifis"]["Home"] == {"ssid": "Home", "type": "PSK"}


def test_migration_retries_while_entries_are_pending(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    fake_config = FakeConfig({"wifi": {"known_wifis": {"Home": "secret"}, "mode": "client"}})
    monkeypatch.setattr(module, "MeticulousConfig", fake_config)

    with (
        patch.object(manager, "getNetworkManagerWifiConnections", return_value={}),
        patch.object(manager, "createNetworkManagerWifiProfile", return_value=False) as create,
    ):
        manager.migrateKnownWifiSecretsToNetworkManager()
        assert manager._migration_completed is False
        manager.migrateKnownWifiSecretsToNetworkManager()

    assert create.call_count == 2


def test_saved_wifi_type_is_cached(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    result = SimpleNamespace(returncode=0, stdout="wpa-psk\n", stderr="")

    with patch.object(module.subprocess, "run", return_value=result) as run:
        assert manager.getSavedWifiType("Home") == "PSK"
        assert manager.getSavedWifiType("Home") == "PSK"
    assert run.call_count == 1

    module.nmcli.connection.return_value = []
    manager.deleteWifi("Home")
    assert "Home" not in manager._saved_wifi_type_cache


def test_saved_wifi_type_failure_is_not_cached(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    failed = SimpleNamespace(returncode=10, stdout="", stderr="unknown connection")

    with patch.object(module.subprocess, "run", return_value=failed) as run:
        assert manager.getSavedWifiType("Home") == "PSK"
        assert manager.getSavedWifiType("Home") == "PSK"
    assert run.call_count == 2
    assert "Home" not in manager._saved_wifi_type_cache


# --- Item 9: auth error copy does not claim certainty ------------------------


def test_secrets_error_message_mentions_interruption(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager

    code, message = manager.classifyConnectionError(
        Exception("Secrets were required, but not provided"), auth_expected=True
    )
    assert code == "invalid_credentials"
    assert "interrupted" in message


# --- Items 1/10: connectToWifi ----------------------------------------------


def _patch_connect_environment(manager, module, current, networks):
    return (
        patch.object(manager, "getCurrentConfig", return_value=current),
        patch.object(manager, "isWifiDeviceReady", return_value=True),
        patch.object(manager, "hasKnownWifiConnection", return_value=False),
        patch.object(manager, "getAvailableNetworks", return_value=networks),
        patch.object(manager, "waitForConnection", return_value=True),
        patch.object(manager, "restartZeroconf"),
        patch.object(manager, "invalidateHealthCache"),
        patch.object(manager, "refreshHealthInBackground"),
        patch.object(manager, "update_gatt_advertisement"),
        patch.object(manager, "persistClientModeAfterManualConnect"),
    )


def test_open_network_with_empty_password_connects_without_credentials(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _wifi_config(module, connected=False, connection_name=None)
    networks = [SimpleNamespace(ssid="Cafe", security="")]

    module.nmcli.device.wifi_connect.reset_mock()
    patches = _patch_connect_environment(manager, module, current, networks)
    with (
        patches[0],
        patches[1],
        patches[2],
        patches[3],
        patches[4],
        patches[5],
        patches[6],
        patches[7],
        patches[8],
        patches[9],
        patch.object(manager, "rememberWifi"),
    ):
        result = manager.connectToWifi({"ssid": "Cafe", "type": "PSK", "password": ""})

    assert result is True
    module.nmcli.device.wifi_connect.assert_called_once_with("Cafe", None)
    assert manager._connect_in_progress == 0


def test_successful_connect_survives_housekeeping_failure(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _wifi_config(module, connected=False, connection_name=None)
    networks = [SimpleNamespace(ssid="Home", security="WPA2")]

    module.nmcli.device.wifi_connect.reset_mock()
    patches = _patch_connect_environment(manager, module, current, networks)
    with (
        patches[0],
        patches[1],
        patches[2],
        patches[3],
        patches[4],
        patches[5],
        patches[6],
        patches[7],
        patches[8],
        patches[9],
        patch.object(manager, "rememberWifi", side_effect=OSError("disk full")),
    ):
        result = manager.connectToWifi({"ssid": "Home", "type": "PSK", "password": "hunter22"})

    assert result is True
    assert manager._connect_in_progress == 0


def test_connect_flag_cleared_after_failure(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    current = _wifi_config(module, connected=False, connection_name=None)
    networks = [SimpleNamespace(ssid="Home", security="WPA2")]

    module.nmcli.device.wifi_connect.side_effect = Exception("Connection activation failed")
    try:
        patches = _patch_connect_environment(manager, module, current, networks)
        with (
            patches[0],
            patches[1],
            patches[2],
            patches[3],
            patches[4],
            patches[5],
            patches[6],
            patches[7],
            patches[8],
            patches[9],
        ):
            result = manager.connectToWifi(
                {"ssid": "Home", "type": "PSK", "password": "hunter22"}
            )
    finally:
        module.nmcli.device.wifi_connect.side_effect = None

    assert result is False
    assert manager._connect_in_progress == 0
    assert manager._last_connection_error_code == "wifi_join_failed"


def test_restart_zeroconf_swallows_errors(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager

    manager._zeroconf = None
    manager.restartZeroconf()  # must not raise

    broken = MagicMock()
    broken.restart.side_effect = OSError("socket closed")
    manager._zeroconf = broken
    manager.restartZeroconf()  # must not raise
    broken.restart.assert_called_once()
