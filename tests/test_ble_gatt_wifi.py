"""Tests for ble_gatt.py wifi_connect UTF-8 handling.

These tests mock out WifiManager so they can run without system dependencies.
"""

import sys
from dataclasses import dataclass
from ipaddress import IPv4Address, IPv6Address
from unittest.mock import MagicMock

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
    "dbus_next": MagicMock(),
    "dbus_next.errors": MagicMock(),
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


def _setup_mocks():
    """Patch sys.modules so ble_gatt can be imported."""
    for mod_name, mock in _mocked_modules.items():
        if mod_name not in sys.modules:
            sys.modules[mod_name] = mock


_setup_mocks()

# Mock wifi module before import
wifi_mock = MagicMock()
sys.modules["wifi"] = wifi_mock


# Create the WifiWpaPskCredentials class the code actually uses
@dataclass
class WifiWpaPskCredentials:
    ssid: str
    password: str


wifi_mock.WifiWpaPskCredentials = WifiWpaPskCredentials
wifi_mock.WifiManager = MagicMock()

# Now we can import ble_gatt
from ble_gatt import GATTServer  # noqa: E402


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------
@pytest.fixture(autouse=True)
def reset_singleton():
    """Reset the GATTServer singleton between tests."""
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
        assert result is not None
        assert any("192.168.1.100" in url for url in result)
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
        assert result is not None
        assert any("192.168.1.100" in url for url in result)
        assert any("fe80::1" in url for url in result)

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
