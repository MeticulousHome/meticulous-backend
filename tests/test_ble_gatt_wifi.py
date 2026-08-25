"""Tests for ble_gatt.py wifi_connect UTF-8 handling.

These tests mock out WifiManager so they can run without system dependencies.
"""

import asyncio
import sys
from dataclasses import dataclass
from ipaddress import IPv4Address, IPv6Address
from unittest.mock import AsyncMock, MagicMock, patch

import pytest


# ---------------------------------------------------------------------------
# Stub out heavy system-level imports before importing ble_gatt
# ---------------------------------------------------------------------------
@dataclass
class FakeIPEntry:
    ip: IPv4Address


@dataclass
class FakeNetworkConfig:
    connected: bool
    hostname: str
    connection_name: str
    ips: list


# We need to mock several modules that ble_gatt imports at module level
# so we can test wifi_connect in isolation without BLE/DBus/hardware.

_mocked_modules = {
    "bless": MagicMock(),
    "bless.backends.bluezdbus.dbus.advertisement": MagicMock(),
    "psutil": MagicMock(),
    "config": MagicMock(),
    "hostname": MagicMock(),
    "log": MagicMock(),
    "notifications": MagicMock(),
}

# Setup config mock values
_mocked_modules["config"].WIFI_MODE_AP = "ap"
_mocked_modules["config"].CONFIG_WIFI = "wifi"
_mocked_modules["config"].WIFI_MODE = "mode"
_mocked_modules["config"].MeticulousConfig = {"wifi": {"mode": "sta"}}

# Setup log mock
mock_logger = MagicMock()
_mocked_modules["log"].MeticulousLogger.getLogger.return_value = mock_logger

# Setup bless mock classes
_mocked_modules["bless"].BlessServer = MagicMock
_mocked_modules["bless"].BlessGATTCharacteristic = MagicMock
_mocked_modules["bless"].GATTAttributePermissions = MagicMock()
_mocked_modules["bless"].GATTAttributePermissions.readable = 1
_mocked_modules["bless"].GATTAttributePermissions.writeable = 2
_mocked_modules["bless"].GATTCharacteristicProperties = MagicMock()
_mocked_modules["bless"].GATTCharacteristicProperties.read = 1
_mocked_modules["bless"].GATTCharacteristicProperties.notify = 2
_mocked_modules["bless"].GATTCharacteristicProperties.write = 4
_mocked_modules["bless"].GATTCharacteristicProperties.write_without_response = 8


_MISSING_MODULE = object()
_original_modules = {}


def _install_mock(mod_name, mock):
    if mod_name not in _original_modules:
        _original_modules[mod_name] = sys.modules.get(mod_name, _MISSING_MODULE)
    sys.modules[mod_name] = mock


def _setup_mocks():
    """Patch sys.modules so ble_gatt can be imported."""
    for mod_name, mock in _mocked_modules.items():
        _install_mock(mod_name, mock)


def _restore_modules():
    for mod_name, original in reversed(list(_original_modules.items())):
        if original is _MISSING_MODULE:
            sys.modules.pop(mod_name, None)
        else:
            sys.modules[mod_name] = original


_setup_mocks()

# Mock wifi module before import
wifi_mock = MagicMock()


# Create the WifiWpaPskCredentials class the code actually uses
@dataclass
class WifiWpaPskCredentials:
    ssid: str
    password: str


wifi_mock.WifiWpaPskCredentials = WifiWpaPskCredentials
wifi_mock.WifiManager = MagicMock()
_install_mock("wifi", wifi_mock)

# Now we can import ble_gatt. Its production platform guard is intentionally
# bypassed because this test has already replaced every hardware dependency.
original_ble_gatt_module = sys.modules.get("ble_gatt", _MISSING_MODULE)
original_platform = sys.platform
sys.platform = "linux"
try:
    import ble_gatt as ble_gatt_under_test  # noqa: E402

    GATTServer = ble_gatt_under_test.GATTServer
    ImprovUUID = ble_gatt_under_test.ImprovUUID
finally:
    sys.platform = original_platform
    _restore_modules()
    if original_ble_gatt_module is _MISSING_MODULE:
        sys.modules.pop("ble_gatt", None)
    else:
        sys.modules["ble_gatt"] = original_ble_gatt_module


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------
@pytest.fixture(autouse=True)
def reset_singleton():
    """Reset the GATTServer singleton between tests."""
    mock_logger.reset_mock()
    GATTServer._singletonServer = None
    yield
    GATTServer._singletonServer = None


@pytest.fixture
def mock_wifi_manager():
    """Provide a fresh WifiManager mock."""
    wifi_mock.WifiManager.reset_mock(side_effect=True, return_value=True)
    default_config = FakeNetworkConfig(
        connected=True,
        hostname="meticulous",
        connection_name="MyWiFi",
        ips=[FakeIPEntry(ip=IPv4Address("192.168.1.100"))],
    )
    wifi_mock.WifiManager.connectToWifi.return_value = True
    wifi_mock.WifiManager.getCurrentConfig.return_value = default_config
    return wifi_mock.WifiManager


# ---------------------------------------------------------------------------
# wifi_connect – UTF-8 decoding
# ---------------------------------------------------------------------------
class TestWifiConnectUTF8:
    def test_ascii_ssid_and_password(self, mock_wifi_manager):
        result = GATTServer.wifi_connect(bytearray(b"MyNetwork"), bytearray(b"password"))
        assert result == ["http://192.168.1.100"]
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.ssid == "MyNetwork"
        assert call_args.password == "password"

    def test_utf8_german_umlauts(self, mock_wifi_manager):
        ssid = "Ünïcödé".encode("utf-8")
        passwd = "Pässwörd".encode("utf-8")
        result = GATTServer.wifi_connect(bytearray(ssid), bytearray(passwd))
        assert result is not None
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.ssid == "Ünïcödé"
        assert call_args.password == "Pässwörd"

    def test_utf8_chinese_ssid(self, mock_wifi_manager):
        ssid = "我的网络".encode("utf-8")
        passwd = "密码123".encode("utf-8")
        result = GATTServer.wifi_connect(bytearray(ssid), bytearray(passwd))
        assert result is not None
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.ssid == "我的网络"
        assert call_args.password == "密码123"

    def test_utf8_japanese_ssid(self, mock_wifi_manager):
        ssid = "東京WiFi".encode("utf-8")
        result = GATTServer.wifi_connect(bytearray(ssid), bytearray(b"pass"))
        assert result is not None
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.ssid == "東京WiFi"

    def test_utf8_korean_ssid(self, mock_wifi_manager):
        ssid = "와이파이".encode("utf-8")
        result = GATTServer.wifi_connect(bytearray(ssid), bytearray(b"pass"))
        assert result is not None
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.ssid == "와이파이"

    def test_utf8_emoji_ssid(self, mock_wifi_manager):
        ssid = "☕🏠Net".encode("utf-8")
        result = GATTServer.wifi_connect(bytearray(ssid), bytearray(b"pass"))
        assert result is not None
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.ssid == "☕🏠Net"

    def test_utf8_arabic_ssid(self, mock_wifi_manager):
        ssid = "شبكة".encode("utf-8")
        result = GATTServer.wifi_connect(bytearray(ssid), bytearray(b"pass"))
        assert result is not None
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.ssid == "شبكة"

    def test_utf8_mixed_script_password(self, mock_wifi_manager):
        passwd = "p@ss密码wörd!".encode("utf-8")
        result = GATTServer.wifi_connect(bytearray(b"Net"), bytearray(passwd))
        assert result is not None
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.password == "p@ss密码wörd!"


# ---------------------------------------------------------------------------
# wifi_connect – invalid / malformed bytes
# ---------------------------------------------------------------------------
class TestWifiConnectInvalidEncoding:
    def test_invalid_utf8_ssid_returns_none(self, mock_wifi_manager):
        """Lone continuation byte – not valid UTF-8."""
        bad_ssid = bytearray([0x80, 0x81, 0x82])
        result = GATTServer.wifi_connect(bad_ssid, bytearray(b"pass"))
        assert result is None
        mock_wifi_manager.connectToWifi.assert_not_called()

    def test_invalid_utf8_password_returns_none(self, mock_wifi_manager):
        bad_passwd = bytearray([0xFE, 0xFF])
        result = GATTServer.wifi_connect(bytearray(b"Net"), bad_passwd)
        assert result is None
        mock_wifi_manager.connectToWifi.assert_not_called()

    def test_truncated_utf8_sequence_ssid(self, mock_wifi_manager):
        """Truncated multi-byte sequence (first byte of 3-byte char only)."""
        bad_ssid = bytearray(b"Net") + bytearray([0xE4])  # incomplete 3-byte seq
        result = GATTServer.wifi_connect(bad_ssid, bytearray(b"pass"))
        assert result is None

    def test_latin1_ssid_fails_utf8_decode(self, mock_wifi_manager):
        """Latin-1 encoded 'café' has 0xE9 which is invalid as a standalone UTF-8 byte."""
        latin1_ssid = "café".encode("latin-1")  # b'caf\xe9'
        result = GATTServer.wifi_connect(bytearray(latin1_ssid), bytearray(b"pass"))
        assert result is None

    def test_overlong_utf8_encoding(self, mock_wifi_manager):
        """Overlong UTF-8 encoding of '/' (should be rejected by strict decode)."""
        overlong = bytearray([0xC0, 0xAF])
        result = GATTServer.wifi_connect(overlong, bytearray(b"pass"))
        assert result is None


# ---------------------------------------------------------------------------
# wifi_connect – edge cases
# ---------------------------------------------------------------------------
class TestWifiConnectEdgeCases:
    def test_empty_ssid(self, mock_wifi_manager):
        result = GATTServer.wifi_connect(bytearray(b""), bytearray(b"pass"))
        assert result is not None
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.ssid == ""

    def test_empty_password_open_network(self, mock_wifi_manager):
        result = GATTServer.wifi_connect(bytearray(b"OpenNet"), bytearray(b""))
        assert result is not None
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.password == ""

    def test_wifi_manager_connect_fails(self, mock_wifi_manager):
        mock_wifi_manager.connectToWifi.return_value = False
        result = GATTServer.wifi_connect(bytearray(b"Net"), bytearray(b"pass"))
        assert result is None

    def test_wifi_manager_throws_exception(self, mock_wifi_manager):
        mock_wifi_manager.connectToWifi.side_effect = Exception("NetworkManager error")
        result = GATTServer.wifi_connect(bytearray(b"Net"), bytearray(b"pass"))
        assert result is None

    def test_ipv6_address_in_result(self, mock_wifi_manager):
        config = FakeNetworkConfig(
            connected=True,
            hostname="meticulous",
            connection_name="Net",
            ips=[
                FakeIPEntry(ip=IPv4Address("192.168.1.100")),
                FakeIPEntry(ip=IPv6Address("fe80::1")),
            ],
        )
        mock_wifi_manager.getCurrentConfig.return_value = config
        result = GATTServer.wifi_connect(bytearray(b"Net"), bytearray(b"pass"))
        assert result == ["http://192.168.1.100", "http://[fe80::1]"]

    def test_ssid_with_spaces(self, mock_wifi_manager):
        ssid = b"My Home Network"
        result = GATTServer.wifi_connect(bytearray(ssid), bytearray(b"pass"))
        assert result is not None
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.ssid == "My Home Network"

    def test_password_with_special_ascii(self, mock_wifi_manager):
        passwd = b"p@$$w0rd!#%^&*()"
        result = GATTServer.wifi_connect(bytearray(b"Net"), bytearray(passwd))
        assert result is not None
        call_args = mock_wifi_manager.connectToWifi.call_args[0][0]
        assert call_args.password == "p@$$w0rd!#%^&*()"


# ---------------------------------------------------------------------------
# BLE advertisement update recovery
# ---------------------------------------------------------------------------
class TestAdvertisementUpdateRecovery:
    def test_transient_registration_failure_does_not_stop_gatt(self, mock_wifi_manager):
        server = GATTServer()
        server.ADVERTISEMENT_UPDATE_RETRY_DELAYS = (0,)
        server.bless_gatt_server = MagicMock()
        server.bless_gatt_server.app = MagicMock()
        server._replace_ble_advertisement = AsyncMock(
            side_effect=[RuntimeError("Failed to register advertisement"), None]
        )
        server.stop = MagicMock()

        async def exercise_update_loop():
            task = asyncio.create_task(server._update_data_loop())
            await asyncio.sleep(0)
            server.update_trigger.set()

            for _ in range(20):
                if server._replace_ble_advertisement.await_count == 2:
                    break
                await asyncio.sleep(0)

            assert server._replace_ble_advertisement.await_count == 2
            assert not task.done()

            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task

        try:
            asyncio.run(exercise_update_loop())
            server.stop.assert_not_called()
        finally:
            server.loop.close()

    def test_failed_registration_is_cleaned_up_for_retry(self, mock_wifi_manager):
        server = GATTServer()
        old_advertisement = MagicMock(path="/advertisement/0")
        failed_advertisement = MagicMock(path="/advertisement/0")
        recovered_advertisement = MagicMock(path="/advertisement/0")

        app = MagicMock()
        app.advertisements = [old_advertisement]
        iface = MagicMock()
        iface.call_unregister_advertisement = AsyncMock()
        iface.call_register_advertisement = AsyncMock(
            side_effect=[RuntimeError("Failed to register advertisement"), None]
        )
        bus = MagicMock()

        server.bless_gatt_server = MagicMock()
        server.bless_gatt_server.app = app
        server.bless_gatt_server.adapter.get_interface.return_value = iface
        server.bless_gatt_server.bus = bus

        async def fail_then_recover():
            with pytest.raises(RuntimeError, match="Failed to register advertisement"):
                await server._replace_ble_advertisement()

            assert app.advertisements == []
            await server._replace_ble_advertisement()

        try:
            with patch.object(
                ble_gatt_under_test,
                "BlueZLEAdvertisement",
                side_effect=[failed_advertisement, recovered_advertisement],
            ):
                asyncio.run(fail_then_recover())

            assert app.advertisements == [recovered_advertisement]
            iface.call_unregister_advertisement.assert_awaited_once_with(old_advertisement.path)
            assert iface.call_register_advertisement.await_count == 2
            bus.unexport.assert_any_call(old_advertisement.path, old_advertisement)
            bus.unexport.assert_any_call(failed_advertisement.path, failed_advertisement)
        finally:
            server.loop.close()


class TestCredentialLogRedaction:
    @staticmethod
    def logged_output():
        return repr(mock_logger.method_calls)

    def test_wifi_connect_does_not_log_ssid_or_password(self, mock_wifi_manager):
        ssid = "SENTINEL-PRIVATE-NETWORK"
        password = "SENTINEL-SECRET-PASSWORD"

        result = GATTServer.wifi_connect(
            bytearray(ssid.encode("utf-8")),
            bytearray(password.encode("utf-8")),
        )

        assert result is not None
        logged = self.logged_output()
        assert ssid not in logged
        assert password not in logged
        assert "ssid_bytes=" in logged
        assert "password_bytes=" in logged

    def test_wifi_connect_exception_does_not_log_exception_credentials(self, mock_wifi_manager):
        password = "SENTINEL-EXCEPTION-SECRET"
        mock_wifi_manager.connectToWifi.side_effect = RuntimeError(
            f"NetworkManager rejected {password}"
        )

        result = GATTServer.wifi_connect(
            bytearray(b"PrivateNetwork"), bytearray(password.encode("utf-8"))
        )

        assert result is None
        logged = self.logged_output()
        assert password not in logged
        assert "RuntimeError" in logged

    def test_ble_write_does_not_log_raw_payload(self, mock_wifi_manager):
        server = GATTServer.getServer()
        server.improv_server = MagicMock()
        server.improv_server.handle_write.return_value = (None, None)
        characteristic = MagicMock()
        characteristic.service_uuid = ImprovUUID.SERVICE_UUID.value
        characteristic.uuid = ImprovUUID.RPC_COMMAND_UUID.value
        payload = bytearray(b"SENTINEL-RAW-BLE-CREDENTIAL-PACKET")

        GATTServer.write_request(characteristic, payload)

        logged = self.logged_output()
        assert payload.decode() not in logged
        assert payload.hex() not in logged
        assert f"{len(payload)} bytes" in logged
