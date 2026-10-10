"""Handler-level regression tests for the /wifi routes.

The real api.wifi handlers drive the real WifiManager. Only the boundaries to
NetworkManager and the network are replaced: the `nmcli` package, the
subprocesses wifi.py spawns (nmcli, ping), DNS/internet probes, the clock used
for its wait loops, its background threads, zeroconf and the BLE advertisement.
"""

import copy
import json
import subprocess
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import pyqrcode
import pytest
from nmcli import Connection, Device, DeviceWifi
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

import wifi
from config import (
    CONFIG_WIFI,
    WIFI_AP_NAME,
    WIFI_AP_PASSWORD,
    WIFI_KNOWN_WIFIS,
    WIFI_MODE,
    WIFI_MODE_AP,
    WIFI_MODE_CLIENT,
    MeticulousConfig,
)
from wifi import WifiManager

AP_CONNECTION = WifiManager._conname
STATION = "wlan0"
MAC = "AA:BB:CC:DD:EE:01"
HOSTNAME = "meticulousAromaticbean-1234"
AP_NAME = "MeticulousAromaticbean"
AP_PASSWORD = "espresso-1234"
HOME_ROUTE = "dst = 192.168.1.0/24, nh = 0.0.0.0, mt = 600"


def import_api_wifi():
    # api.wifi imports the real ble_gatt. tests/test_ble_gatt_wifi.py re-imports
    # ble_gatt under mocks while it is collected and silently gets the real module
    # if it is already loaded, so api.wifi is only imported once the tests run.
    import api.wifi

    return api.wifi


@pytest.fixture(autouse=True)
def restore_config():
    original = copy.deepcopy(dict(MeticulousConfig))
    with patch.object(MeticulousConfig, "save") as save:
        yield save
    MeticulousConfig.clear()
    MeticulousConfig.update(original)


class FakeClock:
    """wifi.py polls with time.time()/time.sleep(); sleeping advances the clock."""

    def __init__(self):
        self.now = 1_800_000_000.0

    def time(self):
        return self.now

    def monotonic(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


class FakeDeviceControl:
    def __init__(self, network):
        self.network = network

    def __call__(self):
        return [device for device, _ in self.network.devices.values()]

    def show(self, name):
        return dict(self.network.devices[name][1])

    def show_all(self):
        return [details for _, details in self.network.devices.values()]

    def wifi(self, rescan=None):
        self.network.calls.append(("device.wifi", rescan))
        return list(self.network.scan)

    def wifi_connect(self, ssid, password):
        self.network.calls.append(("device.wifi_connect", ssid, password))
        if self.network.join_error is not None:
            raise self.network.join_error
        self.network.join(ssid)


class FakeConnectionControl:
    def __init__(self, network):
        self.network = network

    def __call__(self):
        return list(self.network.connections)

    def delete(self, name):
        self.network.calls.append(("connection.delete", name))
        self.network.connections = [c for c in self.network.connections if c.name != name]
        if self.network.active_connection() == name:
            self.network.disconnect_station()

    def up(self, name, wait=None):
        self.network.calls.append(("connection.up", name, wait))
        if self.network.up_error is not None:
            raise self.network.up_error
        self.network.join(name)

    def down(self, name, wait=None):
        self.network.calls.append(("connection.down", name))
        if self.network.down_error is not None:
            raise self.network.down_error
        if self.network.active_connection() == name:
            self.network.disconnect_station()

    def modify(self, name, options):
        self.network.calls.append(("connection.modify", name, options))


class FakeNetworkManager:
    """Stands in for the nmcli package and the nmcli/ping subprocesses."""

    def __init__(self):
        self.calls = []
        self.commands = []
        self.devices = {}
        self.connections = []
        self.scan = []
        self.saved_key_mgmt = {}
        self.ping_returncodes = []
        self.failing_command_prefix = None
        self.join_error = None
        self.join_redirect = None
        self.up_error = None
        self.down_error = None
        self.device = FakeDeviceControl(self)
        self.connection = FakeConnectionControl(self)
        self.disconnect_station()

    def active_connection(self):
        return self.devices[STATION][0].connection

    def connect_station(self, name, ip4="192.168.1.42/24", gateway="192.168.1.1", ip6=None):
        details = {
            "GENERAL.DEVICE": STATION,
            "GENERAL.HWADDR": MAC,
            "GENERAL.CONNECTION": name,
            "IP4.ADDRESS[1]": ip4,
            "IP4.GATEWAY": gateway,
            "IP4.ROUTE[1]": HOME_ROUTE if gateway else None,
            "IP4.DNS[1]": gateway,
        }
        if ip6 is not None:
            details["IP6.ADDRESS[1]"] = ip6
        self.devices[STATION] = (Device(STATION, "wifi", "connected", name), details)

    def disconnect_station(self, state="disconnected"):
        details = {"GENERAL.DEVICE": STATION, "GENERAL.HWADDR": MAC, "GENERAL.CONNECTION": None}
        self.devices[STATION] = (Device(STATION, "wifi", state, None), details)

    def add_connection(self, name, conn_type="wifi"):
        self.connections.append(Connection(name, f"uuid-{name}", conn_type, None))

    def activate_hotspot(self):
        self.connect_station(AP_CONNECTION, ip4="10.42.0.1/24", gateway=None)

    def join(self, ssid):
        self.connect_station(self.join_redirect or ssid)

    def run(self, command, **kwargs):
        self.commands.append(list(command))
        returncode, stdout, stderr = 0, "", ""
        prefix = self.failing_command_prefix
        if prefix is not None and command[: len(prefix[0])] == prefix[0]:
            returncode, stderr = 1, prefix[1]
        elif command[:3] == ["nmcli", "-g", "802-11-wireless-security.key-mgmt"]:
            stdout = self.saved_key_mgmt.get(command[-1], "wpa-psk") + "\n"
        elif command[0] == "ping":
            returncode = self.ping_returncodes.pop(0) if self.ping_returncodes else 0
        elif command[:3] == ["nmcli", "connection", "add"]:
            self.add_connection(command[command.index("con-name") + 1])
        elif command == ["nmcli", "--wait", "35", "connection", "up", AP_CONNECTION]:
            self.activate_hotspot()
        return subprocess.CompletedProcess(command, returncode, stdout, stderr)

    def commands_except_type_lookups(self):
        return [c for c in self.commands if c[:2] != ["nmcli", "-g"]]


def scan_entry(ssid, signal, security="WPA2", in_use=False, bssid="00:11:22:33:44:55"):
    return DeviceWifi(in_use, ssid, bssid, "Infra", 6, 2437, 130, signal, security)


def hotspot_commands(channel="6"):
    return [
        [
            "nmcli",
            "connection",
            "add",
            "type",
            "wifi",
            "ifname",
            STATION,
            "con-name",
            AP_CONNECTION,
            "ssid",
            AP_NAME,
        ],
        [
            "nmcli",
            "connection",
            "modify",
            AP_CONNECTION,
            "connection.autoconnect",
            "no",
            "802-11-wireless.mode",
            "ap",
            "802-11-wireless.band",
            "bg",
            "802-11-wireless.channel",
            channel,
            "802-11-wireless.ap-isolation",
            "0",
            "802-11-wireless.powersave",
            "2",
            "802-11-wireless-security.key-mgmt",
            "wpa-psk",
            "802-11-wireless-security.auth-alg",
            "open",
            "802-11-wireless-security.psk",
            AP_PASSWORD,
            "802-11-wireless-security.proto",
            "rsn",
            "802-11-wireless-security.pairwise",
            "ccmp",
            "802-11-wireless-security.group",
            "ccmp",
            "802-11-wireless-security.pmf",
            "1",
            "802-11-wireless-security.wps-method",
            "0",
            "ipv4.method",
            "shared",
            "ipv6.method",
            "ignore",
        ],
        ["nmcli", "--wait", "35", "connection", "up", AP_CONNECTION],
    ]


def health_json(**overrides):
    health = {
        "mode": WIFI_MODE_CLIENT,
        "link_connected": True,
        "has_ipv4": True,
        "gateway_reachable": False,
        "dns_resolves": False,
        "internet_reachable": False,
        "ap_active": False,
        "degraded": False,
        "verified": False,
        "last_error": "",
        "message": "",
        "last_recovery_action": "",
        "last_recovery_result": "not_needed",
    }
    health.update(overrides)
    return health


HOTSPOT_HEALTH = health_json(
    mode=WIFI_MODE_AP,
    ap_active=True,
    verified=True,
    message="Hotspot active. Connect your phone or computer to the machine's Wi-Fi network.",
)

HOME_STATUS = {
    "connected": True,
    "connection_name": "HomeNet",
    "gateway": "192.168.1.1",
    "routes": [HOME_ROUTE],
    "ips": ["192.168.1.42"],
    "dns": ["192.168.1.1"],
    "mac": MAC,
    "hostname": HOSTNAME,
}


class WifiApiTestCase(AsyncHTTPTestCase):
    @pytest.fixture(autouse=True)
    def _pytest_fixtures(self, monkeypatch, restore_config):
        self.monkeypatch = monkeypatch
        self.config_save = restore_config

    def setUp(self):
        super().setUp()
        self.network = FakeNetworkManager()
        self.clock = FakeClock()
        self.threads = []
        self.dns_lookup = MagicMock(
            return_value=[("AF_INET", None, None, "", ("1.2.3.4", 443))]
        )

        def named_thread(name, target=None, **kwargs):
            # Background refreshes are recorded, not run, so responses stay deterministic.
            self.threads.append(name)
            return SimpleNamespace(start=lambda: None)

        self.monkeypatch.setattr(wifi, "nmcli", self.network)
        self.monkeypatch.setattr(wifi, "subprocess", SimpleNamespace(run=self.network.run))
        self.monkeypatch.setattr(wifi, "time", self.clock)
        self.monkeypatch.setattr(wifi, "NamedThread", named_thread)
        self.monkeypatch.setattr(
            wifi,
            "socket",
            SimpleNamespace(gethostname=lambda: HOSTNAME, getaddrinfo=self.dns_lookup),
        )
        self.monkeypatch.setattr(
            wifi, "shutil", SimpleNamespace(which=lambda name: f"/usr/bin/{name}")
        )
        self.monkeypatch.setattr(wifi, "sentry_sdk", MagicMock())
        self.monkeypatch.setattr(wifi, "ZEROCONF_OVERWRITE", "")

        self.internet = MagicMock(return_value=True)
        self.gatt = MagicMock()
        self.zeroconf = MagicMock()
        self.monkeypatch.setattr(WifiManager, "internetReachable", self.internet)
        self.monkeypatch.setattr(WifiManager, "update_gatt_advertisement", self.gatt)

        defaults = {
            "_zeroconf": self.zeroconf,
            "_networking_available": True,
            "_known_wifis": [],
            "_scan_cache": [],
            "_scan_cache_time": 0,
            "_scan_in_progress": False,
            "_health_cache": None,
            "_health_check_in_progress": False,
            "_last_health_error": "",
            "_last_recovery_action": "",
            "_last_recovery_result": "not_needed",
            "_health_failures": 0,
            "_last_recovery_attempt": 0,
            "_last_health_check": 0,
            "_last_connection_error_code": "",
            "_last_connection_error_message": "",
            "_auto_connect_suppressed_until": 0,
            "_last_auto_connect_suppressed_log": 0,
            "_repair_in_progress": False,
            "_thread": None,
        }
        for name, value in defaults.items():
            self.monkeypatch.setattr(WifiManager, name, value)

        MeticulousConfig[CONFIG_WIFI][WIFI_MODE] = WIFI_MODE_CLIENT
        MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME] = AP_NAME
        MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD] = AP_PASSWORD
        MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS] = {}

    def get_app(self):
        api_wifi = import_api_wifi()
        return Application(
            [
                (r"/api/v1/wifi/config", api_wifi.WiFiConfigHandler),
                (r"/api/v1/wifi/config/qr.png", api_wifi.WiFiQRHandler),
                (r"/api/v1/wifi/list", api_wifi.WiFiListHandler),
                (r"/api/v1/wifi/connect", api_wifi.WiFiConnectHandler),
                (r"/api/v1/wifi/repair", api_wifi.WiFiRepairHandler),
                (r"/api/v1/wifi/delete", api_wifi.WiFiDeleteHandler),
            ]
        )

    def json(self, response):
        return json.loads(response.body)

    def post(self, path, body):
        if not isinstance(body, (str, bytes)):
            body = json.dumps(body)
        return self.fetch(path, method="POST", body=body)

    def connect_home(self):
        self.network.add_connection("HomeNet")
        self.network.connect_station("HomeNet")

    def start_in_hotspot_mode(self):
        MeticulousConfig[CONFIG_WIFI][WIFI_MODE] = WIFI_MODE_AP
        self.network.add_connection(AP_CONNECTION)
        self.network.activate_hotspot()


class TestWifiConfigGet(WifiApiTestCase):
    def test_client_mode_reports_config_status_shallow_health_and_saved_networks(self):
        self.connect_home()
        self.network.add_connection("CafeOpen")
        self.network.add_connection("Wired connection 1", conn_type="ethernet")
        self.network.add_connection(AP_CONNECTION)
        self.network.saved_key_mgmt = {"HomeNet": "wpa-psk", "CafeOpen": ""}

        response = self.fetch("/api/v1/wifi/config")

        assert response.code == 200
        assert self.json(response) == {
            "config": {"mode": WIFI_MODE_CLIENT, "apName": AP_NAME, "apPassword": AP_PASSWORD},
            "status": HOME_STATUS,
            "health": health_json(),
            "known_wifis": {
                "HomeNet": {"ssid": "HomeNet", "type": "PSK"},
                "CafeOpen": {"ssid": "CafeOpen", "type": "OPEN"},
            },
        }
        # The shallow answer never runs reachability probes inline; it schedules them.
        assert self.threads == ["WifiHealthRefresh"]
        assert [c for c in self.network.commands if c[0] == "ping"] == []
        self.internet.assert_not_called()
        assert self.network.commands == [
            [
                "nmcli",
                "-g",
                "802-11-wireless-security.key-mgmt",
                "connection",
                "show",
                "HomeNet",
            ],
            [
                "nmcli",
                "-g",
                "802-11-wireless-security.key-mgmt",
                "connection",
                "show",
                "CafeOpen",
            ],
        ]

    def test_hotspot_mode_reports_the_active_hotspot(self):
        self.start_in_hotspot_mode()

        body = self.json(self.fetch("/api/v1/wifi/config"))

        assert body["config"] == {
            "mode": WIFI_MODE_AP,
            "apName": AP_NAME,
            "apPassword": AP_PASSWORD,
        }
        assert body["status"] == {
            "connected": True,
            "connection_name": AP_CONNECTION,
            "gateway": "",
            "routes": [],
            "ips": ["10.42.0.1"],
            "dns": [],
            "mac": MAC,
            "hostname": HOSTNAME,
        }
        assert body["health"] == HOTSPOT_HEALTH
        assert body["known_wifis"] == {}

    def test_disconnected_client_reports_not_connected_health(self):
        body = self.json(self.fetch("/api/v1/wifi/config"))

        assert body["status"] == {
            "connected": False,
            "connection_name": None,
            "gateway": "",
            "routes": [],
            "ips": [],
            "dns": [],
            "mac": MAC,
            "hostname": HOSTNAME,
        }
        assert body["health"] == health_json(
            link_connected=False,
            has_ipv4=False,
            degraded=True,
            verified=True,
            last_error="wifi_not_connected",
            message="The machine is not connected to Wi-Fi.",
        )

    def test_legacy_saved_password_is_migrated_into_network_manager_and_scrubbed(self):
        MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS] = {"OldNet": "legacy-pass"}

        body = self.json(self.fetch("/api/v1/wifi/config"))

        assert self.network.commands_except_type_lookups() == [
            [
                "nmcli",
                "connection",
                "add",
                "type",
                "wifi",
                "ifname",
                "*",
                "con-name",
                "OldNet",
                "ssid",
                "OldNet",
            ],
            [
                "nmcli",
                "connection",
                "modify",
                "OldNet",
                "connection.autoconnect",
                "no",
                "802-11-wireless-security.key-mgmt",
                "wpa-psk",
                "802-11-wireless-security.psk",
                "legacy-pass",
            ],
        ]
        assert MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS] == {
            "OldNet": {"ssid": "OldNet", "type": "PSK"}
        }
        self.config_save.assert_called_once_with()
        assert body["known_wifis"] == {"OldNet": {"ssid": "OldNet", "type": "PSK"}}


class TestWifiConfigPost(WifiApiTestCase):
    def test_new_ap_password_in_client_mode_is_saved_without_starting_the_hotspot(self):
        self.connect_home()
        self.network.scan = [scan_entry("HomeNet", 70, in_use=True)]

        response = self.post("/api/v1/wifi/config", {"apPassword": "n3w-Pa55word"})

        assert response.code == 200
        body = self.json(response)
        assert body["config"] == {
            "mode": WIFI_MODE_CLIENT,
            "apName": AP_NAME,
            "apPassword": "n3w-Pa55word",
        }
        assert body["status"] == HOME_STATUS
        assert MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD] == "n3w-Pa55word"
        self.config_save.assert_called_once_with()
        # Client mode: stop any hotspot, rescan once, re-announce.
        assert self.network.calls == [("device.wifi", True)]
        assert self.network.commands_except_type_lookups() == []
        self.zeroconf.restart.assert_called_once_with()
        self.gatt.assert_called_once_with()

    def test_switching_to_hotspot_mode_creates_and_starts_the_access_point(self):
        self.connect_home()

        response = self.post("/api/v1/wifi/config", {"mode": WIFI_MODE_AP})

        assert response.code == 200
        body = self.json(response)
        assert body["config"] == {
            "mode": WIFI_MODE_AP,
            "apName": AP_NAME,
            "apPassword": AP_PASSWORD,
        }
        assert body["status"]["connection_name"] == AP_CONNECTION
        assert body["health"] == HOTSPOT_HEALTH
        assert self.network.commands_except_type_lookups() == hotspot_commands(channel="6")
        assert MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP
        self.config_save.assert_called_once_with()

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: WifiManager.repairWifiConnection (wifi.py:1092) reads "
            "initial_health.connected, but WifiHealthStatus only has link_connected. When "
            "the hotspot fails to start, applyWifiSettings' AP recovery raises "
            "AttributeError, so POST /wifi/config answers 400 'Failed to write config' and "
            "never rolls the in-memory mode back to client"
        ),
    )
    def test_failed_hotspot_start_rolls_back_to_client_mode_and_reports_why(self):
        self.connect_home()
        self.network.failing_command_prefix = (
            ["nmcli", "connection", "add", "type", "wifi", "ifname", STATION],
            "Error: device busy",
        )

        response = self.post("/api/v1/wifi/config", {"mode": WIFI_MODE_AP})

        assert response.code == 400
        body = self.json(response)
        assert body["status"] == "error"
        assert body["error"] == (
            "The machine could not start its Wi-Fi hotspot. Please try again. "
            "If this keeps happening, restart the machine and try again."
        )
        assert body["code"] == "hotspot_start_failed: channel=11: Error: device busy"
        assert body["config"]["config"]["mode"] == WIFI_MODE_CLIENT
        assert MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_CLIENT

    def test_failed_hotspot_stop_rolls_back_to_hotspot_mode(self):
        self.start_in_hotspot_mode()
        self.network.down_error = RuntimeError("Error: Connection deactivation failed")
        self.network.scan = [scan_entry("HomeNet", 70)]

        response = self.post("/api/v1/wifi/config", {"mode": WIFI_MODE_CLIENT})

        assert response.code == 400
        body = self.json(response)
        # The rollback restarts the hotspot successfully, which clears the
        # hotspot_stop_failed error, so the client only sees the generic fallback.
        assert body["status"] == "error"
        assert body["error"] == "Failed to apply Wi-Fi settings."
        assert body["code"] == ""
        assert body["config"]["config"]["mode"] == WIFI_MODE_AP
        assert body["config"]["status"]["connection_name"] == AP_CONNECTION
        assert MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP
        assert ("connection.down", AP_CONNECTION) in self.network.calls
        assert self.network.commands_except_type_lookups() == hotspot_commands(channel="6")
        self.config_save.assert_called_once_with()

    def test_unknown_mode_changes_nothing_and_returns_the_current_config(self):
        self.connect_home()

        response = self.post("/api/v1/wifi/config", {"mode": "mesh"})

        assert response.code == 200
        assert self.json(response)["config"]["mode"] == WIFI_MODE_CLIENT
        assert self.network.calls == []
        self.config_save.assert_not_called()

    def test_invalid_json_is_rejected(self):
        response = self.post("/api/v1/wifi/config", "{mode")

        assert response.code == 400
        assert response.body == b"Invalid JSON"
        self.config_save.assert_not_called()


class TestWifiQrCode(WifiApiTestCase):
    def setUp(self):
        super().setUp()
        self.qr_create = MagicMock(wraps=pyqrcode.create)
        self.monkeypatch.setattr(pyqrcode, "create", self.qr_create)
        # The machine URL uses the HTTP port api.wifi takes from ble_gatt.
        self.port = import_api_wifi().PORT

    def fetch_qr(self):
        response = self.fetch("/api/v1/wifi/config/qr.png")
        assert response.code == 200
        assert response.headers["Content-Type"] == "image/png"
        assert response.body.startswith(b"\x89PNG\r\n\x1a\n")
        return response

    def test_hotspot_qr_encodes_join_credentials_with_escaped_values(self):
        self.start_in_hotspot_mode()
        MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME] = 'Meti;cu,lous"'
        MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD] = "p:a\\ss"

        self.fetch_qr()

        self.qr_create.assert_called_once_with(
            'WIFI:S:Meti\\;cu\\,lous\\";T:WPA2;P:p\\:a\\\\ss;H:false;;'
        )

    def test_client_qr_encodes_the_machine_url_on_its_ipv4_address(self):
        self.connect_home()

        self.fetch_qr()

        self.qr_create.assert_called_once_with(f"http://192.168.1.42:{self.port}")

    def test_client_qr_brackets_an_ipv6_address(self):
        self.network.connect_station("HomeNet", ip4=None, ip6="fd00::42/64")

        self.fetch_qr()

        self.qr_create.assert_called_once_with(f"http://[fd00::42]:{self.port}")

    def test_qr_without_any_address_uses_the_mdns_hostname(self):
        self.fetch_qr()

        self.qr_create.assert_called_once_with(f"http://{HOSTNAME}.local:{self.port}")


class TestWifiList(WifiApiTestCase):
    def test_cached_scan_is_deduplicated_filtered_and_sorted_by_signal(self):
        WifiManager._scan_cache = [
            scan_entry("HomeNet", 80, bssid="00:00:00:00:00:01"),
            scan_entry("HomeNet", 60, in_use=True, bssid="00:00:00:00:00:02"),
            scan_entry("HomeNet", 90, bssid="00:00:00:00:00:03"),
            scan_entry("CafeOpen", 70, security=""),
            scan_entry("Office", 40, security="WPA2 802.1X"),
            scan_entry("Office", 55, security="WPA2 802.1X"),
            scan_entry("Neighbor", 30, security="WPA2 WPA3"),
            scan_entry("OldRouter", 20, security="WEP"),
            scan_entry("", 99),
            scan_entry("Mystery", 50, security="XYZ"),
        ]

        response = self.fetch("/api/v1/wifi/list")

        assert response.code == 200
        assert self.json(response) == [
            {
                "type": "OPEN",
                "security": "",
                "ssid": "CafeOpen",
                "signal": 70,
                "rate": 130,
                "in_use": False,
            },
            {
                "type": "PSK",
                "security": "WPA2",
                "ssid": "HomeNet",
                "signal": 60,
                "rate": 130,
                "in_use": True,
            },
            {
                "type": "802.1X",
                "security": "WPA2 802.1X",
                "ssid": "Office",
                "signal": 55,
                "rate": 130,
                "in_use": False,
            },
            {
                "type": "SAE",
                "security": "WPA2 WPA3",
                "ssid": "Neighbor",
                "signal": 30,
                "rate": 130,
                "in_use": False,
            },
            {
                "type": "WEP",
                "security": "WEP",
                "ssid": "OldRouter",
                "signal": 20,
                "rate": 130,
                "in_use": False,
            },
        ]
        wifi.sentry_sdk.capture_message.assert_called_once_with(
            "Unknown wifi security type: XYZ", level="error"
        )
        # A cached answer never blocks; it schedules one background rescan.
        assert self.network.calls == []
        assert self.threads == ["WifiScanRefresh"]

    def test_cold_start_blocks_on_one_scan_instead_of_returning_nothing(self):
        self.network.scan = [scan_entry("HomeNet", 65)]

        response = self.fetch("/api/v1/wifi/list")

        assert response.code == 200
        assert self.json(response) == [
            {
                "type": "PSK",
                "security": "WPA2",
                "ssid": "HomeNet",
                "signal": 65,
                "rate": 130,
                "in_use": False,
            }
        ]
        assert self.network.calls == [("device.wifi", True)]
        assert WifiManager._scan_cache == self.network.scan
        assert self.threads == []

    def test_cold_start_with_radio_not_ready_returns_an_empty_list(self):
        self.network.disconnect_station(state="unavailable")

        response = self.fetch("/api/v1/wifi/list")

        assert response.code == 200
        assert self.json(response) == []
        assert self.network.calls == []
        assert WifiManager._last_health_error == "wifi_device_unavailable"

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: WiFiListHandler.getWifiList (api/wifi.py:214-220) writes the "
            "error JSON and returns None, then get() (api/wifi.py:225) appends "
            "json.dumps([]), so a failed scan answers 400 with an unparseable body"
        ),
    )
    def test_scan_failure_answers_a_single_json_error(self):
        self.monkeypatch.setattr(
            WifiManager,
            "getAvailableNetworks",
            MagicMock(side_effect=RuntimeError("NetworkManager is not running")),
        )

        response = self.fetch("/api/v1/wifi/list")

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "failed to fetch wifi list: RuntimeError",
        }


class TestWifiConnect(WifiApiTestCase):
    def setUp(self):
        super().setUp()
        WifiManager._scan_cache = [
            scan_entry("HomeNet", 70),
            scan_entry("CafeOpen", 60, security=""),
            scan_entry("Office", 50, security="WPA2 802.1X"),
        ]

    def test_new_network_is_joined_through_network_manager(self):
        response = self.post(
            "/api/v1/wifi/connect", {"ssid": "HomeNet", "password": "hunter22", "type": "PSK"}
        )

        assert response.code == 200
        assert self.json(response) == {"status": "ok", "health": health_json()}
        assert self.network.calls == [("device.wifi_connect", "HomeNet", "hunter22")]
        assert WifiManager.getAutoConnectSuppressionRemaining() == 45
        self.zeroconf.restart.assert_called_once_with()
        assert self.threads == ["WifiScanRefresh", "WifiHealthRefresh", "WifiHealthRefresh"]
        # Already in client mode and no legacy entry: nothing to persist.
        self.config_save.assert_not_called()

    def test_already_connected_network_is_not_rejoined(self):
        self.connect_home()

        response = self.post(
            "/api/v1/wifi/connect", {"ssid": "HomeNet", "password": "hunter22"}
        )

        assert response.code == 200
        assert self.json(response) == {"status": "ok", "health": health_json()}
        assert self.network.calls == []
        self.zeroconf.restart.assert_called_once_with()
        self.gatt.assert_called_once_with()

    def test_joining_from_hotspot_mode_persists_client_mode(self):
        self.start_in_hotspot_mode()

        response = self.post(
            "/api/v1/wifi/connect", {"ssid": "HomeNet", "password": "hunter22"}
        )

        assert response.code == 200
        assert self.network.calls == [("device.wifi_connect", "HomeNet", "hunter22")]
        assert MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_CLIENT
        self.config_save.assert_called_once_with()

    def test_open_network_is_joined_without_password(self):
        response = self.post("/api/v1/wifi/connect", {"ssid": "CafeOpen", "type": "OPEN"})

        assert response.code == 200
        assert self.network.calls == [("device.wifi_connect", "CafeOpen", None)]

    def test_saved_network_without_password_is_brought_up_from_its_profile(self):
        self.network.add_connection("HomeNet")

        response = self.post("/api/v1/wifi/connect", {"ssid": "HomeNet"})

        assert response.code == 200
        assert self.network.calls == [("connection.up", "HomeNet", 12)]

    def test_wrong_password_is_reported_as_invalid_credentials(self):
        self.network.join_error = Exception(
            "Error: Connection activation failed: Secrets were required, but not provided."
        )

        response = self.post("/api/v1/wifi/connect", {"ssid": "HomeNet", "password": "wrong"})

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "Incorrect Wi-Fi password. Please check it and try again.",
            "code": "invalid_credentials",
        }
        self.gatt.assert_called_once_with()

    def test_network_that_disappeared_is_reported_as_not_found(self):
        self.network.join_error = Exception("Error: No network with SSID 'HomeNet' found.")

        response = self.post(
            "/api/v1/wifi/connect", {"ssid": "HomeNet", "password": "hunter22"}
        )

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "Wi-Fi network was not found. Move closer and try again.",
            "code": "network_not_found",
        }

    def test_join_that_lands_on_another_network_is_reported_as_preempted(self):
        self.network.join_redirect = "OtherNet"

        response = self.post(
            "/api/v1/wifi/connect", {"ssid": "HomeNet", "password": "hunter22"}
        )

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "Connected to OtherNet instead of HomeNet.",
            "code": "connection_preempted",
        }

    def test_psk_network_without_password_or_saved_profile_is_rejected(self):
        response = self.post("/api/v1/wifi/connect", {"ssid": "HomeNet", "type": "PSK"})

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "Wi-Fi password was not provided.",
            "code": "missing_credentials",
        }
        assert self.network.calls == []

    def test_enterprise_network_is_rejected_as_unsupported(self):
        response = self.post(
            "/api/v1/wifi/connect", {"ssid": "Office", "password": "x", "type": "802.1X"}
        )

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "Enterprise Wi-Fi networks are not supported yet.",
            "code": "unsupported_security",
        }
        assert self.network.calls == []

    def test_missing_ssid_is_rejected(self):
        response = self.post("/api/v1/wifi/connect", {"password": "hunter22"})

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "Wi-Fi network name was not provided.",
            "code": "missing_ssid",
        }

    def test_null_body_is_rejected_as_missing_credentials(self):
        response = self.post("/api/v1/wifi/connect", "null")

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "Wi-Fi credentials were not provided.",
            "code": "missing_credentials",
        }

    def test_invalid_json_is_rejected(self):
        response = self.post("/api/v1/wifi/connect", "{ssid")

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "failed to connect to wifi: JSONDecodeError",
        }

    def test_connect_without_networking_hardware_is_rejected(self):
        WifiManager._networking_available = False

        response = self.post(
            "/api/v1/wifi/connect", {"ssid": "HomeNet", "password": "hunter22"}
        )

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "Wi-Fi hardware is not available.",
            "code": "networking_unavailable",
        }
        assert self.network.calls == []


class TestWifiDelete(WifiApiTestCase):
    def test_saved_network_manager_profile_is_deleted(self):
        self.network.add_connection("HomeNet")

        response = self.post("/api/v1/wifi/delete", {"ssid": "HomeNet"})

        assert response.code == 200
        assert self.json(response) == {"status": "ok"}
        assert self.network.calls == [("connection.delete", "HomeNet")]
        self.config_save.assert_not_called()

    def test_legacy_config_entry_is_removed_and_saved(self):
        MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS] = {
            "OldNet": {"ssid": "OldNet", "type": "PSK"},
            "HomeNet": {"ssid": "HomeNet", "type": "PSK"},
        }

        response = self.post("/api/v1/wifi/delete", {"ssid": "OldNet"})

        assert response.code == 200
        assert self.json(response) == {"status": "ok"}
        assert MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS] == {
            "HomeNet": {"ssid": "HomeNet", "type": "PSK"}
        }
        self.config_save.assert_called_once_with()
        assert self.network.calls == []

    def test_unknown_network_is_rejected(self):
        self.network.add_connection("HomeNet")

        response = self.post("/api/v1/wifi/delete", {"ssid": "Ghost"})

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "failed to delete unknown wifi",
        }
        assert self.network.calls == []

    def test_the_hotspot_profile_cannot_be_deleted(self):
        self.network.add_connection(AP_CONNECTION)

        response = self.post("/api/v1/wifi/delete", {"ssid": AP_CONNECTION})

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "failed to delete unknown wifi",
        }
        assert self.network.calls == []

    def test_body_without_ssid_is_rejected(self):
        response = self.post("/api/v1/wifi/delete", {"name": "HomeNet"})

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "failed to delete wifi: KeyError",
        }


class TestWifiRepair(WifiApiTestCase):
    def repair(self):
        return self.post("/api/v1/wifi/repair", b"")

    def test_repair_without_networking_hardware_is_rejected(self):
        WifiManager._networking_available = False

        response = self.repair()

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "Wi-Fi hardware is not available.",
            "code": "networking_unavailable",
            "health": health_json(
                link_connected=False,
                has_ipv4=False,
                degraded=True,
                verified=True,
                last_error="networking_unavailable",
                message="Wi-Fi hardware is not available.",
                last_recovery_result="failed",
            ),
        }

    def test_repair_without_any_saved_network_never_touches_the_radio(self):
        response = self.repair()

        no_saved_message = (
            "There is no saved Wi-Fi connection to repair. Choose a Wi-Fi network on the "
            "machine or connect it from the Meticulous app."
        )
        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": no_saved_message,
            "code": "no_saved_wifi_connection",
            "health": health_json(
                link_connected=False,
                has_ipv4=False,
                degraded=True,
                verified=True,
                last_error="no_saved_wifi_connection",
                message=no_saved_message,
                last_recovery_action="health_check",
                last_recovery_result="not_recoverable",
            ),
        }
        assert self.network.calls == []
        assert self.network.commands == []
        assert WifiManager.isAutoConnectSuppressed() is False

    def test_repair_while_another_repair_runs_reports_generic_failure(self):
        self.connect_home()
        WifiManager._repair_in_progress = True

        response = self.repair()

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "Wi-Fi repair could not complete.",
            "code": "wifi_repair_failed",
            "health": health_json(
                gateway_reachable=True,
                dns_resolves=True,
                internet_reachable=True,
                verified=True,
                last_recovery_result="in_progress",
            ),
        }
        # The deep health check pings the gateway and resolves DNS.
        assert self.network.commands == [["ping", "-c", "1", "-W", "2", "192.168.1.1"]]
        self.dns_lookup.assert_called_once_with("meticuloushome.com", 443)
        assert self.network.calls == []

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: WifiManager.repairWifiConnection (wifi.py:1092) reads "
            "initial_health.connected, but WifiHealthStatus only has link_connected, so "
            "POST /wifi/repair with a saved or active connection always answers 400 "
            "'failed to repair wifi: ... has no attribute connected'"
        ),
    )
    def test_repair_of_a_healthy_connection_reports_ok(self):
        self.connect_home()

        response = self.repair()

        assert response.code == 200
        body = self.json(response)
        assert body["status"] == "ok"
        assert body["health"]["degraded"] is False
        assert body["health"]["verified"] is True
        assert self.network.calls == []

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: WifiManager.repairWifiConnection reads .connected on "
            "WifiHealthStatus (wifi.py:1092 and, after each repair step, wifi.py:1150), "
            "which has only link_connected, so a degraded connection is never repaired "
            "through POST /wifi/repair"
        ),
    )
    def test_repair_restarts_a_connection_whose_gateway_stopped_answering(self):
        self.connect_home()
        self.network.ping_returncodes = [1, 0]

        response = self.repair()

        assert response.code == 200
        body = self.json(response)
        assert body["status"] == "ok"
        assert body["health"]["degraded"] is False
        assert self.network.calls == [
            ("connection.down", "HomeNet"),
            ("connection.up", "HomeNet", 15),
        ]
        self.zeroconf.restart.assert_called_once_with()
