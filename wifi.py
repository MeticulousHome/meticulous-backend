import asyncio
import os
import shutil
import socket
import subprocess
import threading
import time
import urllib.request
from dataclasses import dataclass
from enum import Enum
from typing import List, Literal

import nmcli
import sentry_sdk
from netaddr import IPAddress, IPNetwork

from api.zeroconf_announcement import ZeroConfAnnouncement
from config import (
    CONFIG_WIFI,
    CONFIG_USER,
    HOSTNAME_OVERRIDE,
    WIFI_AP_NAME,
    WIFI_AP_PASSWORD,
    WIFI_KNOWN_WIFIS,
    WIFI_MODE,
    WIFI_MODE_AP,
    WIFI_MODE_CLIENT,
    MeticulousConfig,
)
from hostname import HostnameManager
from timezone_manager import TimezoneManager
from machine import Machine

from log import MeticulousLogger
from named_thread import NamedThread

logger = MeticulousLogger.getLogger(__name__)

nmcli.disable_use_sudo()
nmcli.set_lang("C.UTF-8")

# Should be something like "192.168.2.123/24,MyHostname"
ZEROCONF_OVERWRITE = os.getenv("ZEROCONF_OVERWRITE", "")


class WifiType(str, Enum):
    Open = "OPEN"
    PreSharedKey = "PSK"
    PSK_SAE = "SAE"
    Enterprise = "802.1X"
    WEP = "WEP"

    @staticmethod
    def from_nmcli_security(security):
        security = security.strip().upper()
        if security == "":
            return WifiType.Open
        elif "802.1X" in security:
            return WifiType.Enterprise
        elif "WPA3" in security:
            return WifiType.PSK_SAE
        elif "WPA" in security:
            return WifiType.PreSharedKey
        # WEP is ancient and needs to die. (Well it already mostly did).
        # We dont support it and only log it as an error.
        elif "WEP" in security:
            return WifiType.WEP

        error_msg = f"Unknown wifi security type: {security}"
        logger.error(error_msg)
        sentry_sdk.capture_message(error_msg, level="error")

        return None

    @staticmethod
    def is_valid_wifi_type(type: str):
        match type:
            case "OPEN" | "PSK" | "SAE" | "802.1X" | "WEP":
                return True
            case _:
                return False


@dataclass
class BaseWiFiCredentials:
    type: WifiType = None
    security: str = ""
    ssid: str = ""

    def to_dict(self) -> str:
        return self.__dict__.copy()


@dataclass
class WifiWpaEnterpriseCredentials(BaseWiFiCredentials):
    type: Literal["802.1X"] = "802.1X"
    # TODO: add more fields after implementation


@dataclass
class WifiOpenCredentials(BaseWiFiCredentials):
    type: Literal["OPEN"] = "OPEN"


@dataclass
class WifiWpaPskCredentials(BaseWiFiCredentials):
    type: Literal["PSK"] = "PSK"
    password: str = ""


@dataclass
class WifiWpaSaeCredentials(WifiWpaPskCredentials):
    type: Literal["SAE"] = "SAE"


# Define a union type for WiFi credentials
WiFiCredentials = (
    WifiWpaEnterpriseCredentials
    | WifiOpenCredentials
    | WifiWpaPskCredentials
    | WifiWpaSaeCredentials
)


@dataclass
class WifiSystemConfig:
    """Class Representing the current network configuration"""

    connected: bool
    connection_name: str
    gateway: IPAddress
    routes: List[str]
    ips: List[IPNetwork]
    dns: List[IPAddress]
    mac: str
    hostname: str
    domains: List[str]

    def to_json(self):
        gateway = ""
        if self.gateway is not None:
            gateway = self.gateway.format()
        return {
            "connected": self.connected,
            "connection_name": self.connection_name,
            "gateway": gateway,
            "routes": self.routes,
            "ips": [ip.ip.format() for ip in self.ips],
            "dns": [dns.format() for dns in self.dns],
            "mac": self.mac,
            "hostname": self.hostname,
        }

    def is_hotspot(self):
        return self.connection_name == WifiManager._conname


@dataclass
class WifiHealthStatus:
    """Represents whether the active WiFi path is actually usable."""

    mode: str
    link_connected: bool
    has_ipv4: bool
    gateway_reachable: bool
    dns_resolves: bool
    internet_reachable: bool
    ap_active: bool
    degraded: bool
    last_error: str
    last_recovery_action: str
    last_recovery_result: str
    message: str = ""

    def to_json(self):
        return {
            "mode": self.mode,
            "link_connected": self.link_connected,
            "has_ipv4": self.has_ipv4,
            "gateway_reachable": self.gateway_reachable,
            "dns_resolves": self.dns_resolves,
            "internet_reachable": self.internet_reachable,
            "ap_active": self.ap_active,
            "degraded": self.degraded,
            "last_error": self.last_error,
            "message": self.message
            or WifiManager.getHealthErrorMessage(self.last_error),
            "last_recovery_action": self.last_recovery_action,
            "last_recovery_result": self.last_recovery_result,
        }


class WifiManager:
    _known_wifis = []
    _thread = None
    # Internal name used by network manager to refer to the AP configuration
    _conname = "meticulousLocalAP"
    _networking_available = True
    _zeroconf = None
    _last_health_error = ""
    _last_recovery_action = ""
    _last_recovery_result = "not_needed"
    _health_failures = 0
    _last_recovery_attempt = 0
    _health_check_interval = 60
    _last_health_check = 0
    _recovery_cooldown = 300
    _health_failure_threshold = 3
    _cached_health = None
    _health_cache_time = 0
    _health_cache_ttl = 10
    _last_connection_error_code = ""
    _last_connection_error_message = ""
    _auto_connect_suppressed_until = 0
    _last_auto_connect_suppressed_log = 0
    _repair_in_progress = False
    _health_check_lock = threading.Lock()

    def clearLastConnectionError():
        WifiManager._last_connection_error_code = ""
        WifiManager._last_connection_error_message = ""

    def setLastConnectionError(code: str, message: str):
        WifiManager._last_connection_error_code = code
        WifiManager._last_connection_error_message = message
        logger.warning(f"WiFi connection failed: {code}: {message}")

    def getLastConnectionError():
        return {
            "code": WifiManager._last_connection_error_code or "connection_failed",
            "error": WifiManager._last_connection_error_message
            or "Could not connect to Wi-Fi.",
        }

    def suppressAutoConnect(seconds: int, reason: str):
        WifiManager._auto_connect_suppressed_until = time.time() + seconds
        logger.info(f"Suppressing WiFi auto-connect for {seconds}s: {reason}")

    def isAutoConnectSuppressed() -> bool:
        return time.time() < WifiManager._auto_connect_suppressed_until

    def getAutoConnectSuppressionRemaining() -> int:
        return max(0, int(WifiManager._auto_connect_suppressed_until - time.time()))

    def getWifiDeviceState():
        try:
            for dev in nmcli.device():
                if dev.device_type == "wifi":
                    return dev.state
        except Exception as e:
            logger.warning(f"Failed to read WiFi device state: {e}")
        return None

    def isWifiDeviceReady() -> bool:
        return WifiManager.getWifiDeviceState() in {
            "connected",
            "connecting",
            "disconnected",
        }

    def getWifiStationDeviceName():
        try:
            for dev in nmcli.device():
                if (
                    dev.device_type == "wifi"
                    and not dev.device.startswith("p2p-")
                    and dev.state != "unmanaged"
                ):
                    return dev.device
        except Exception as e:
            logger.warning(f"Failed to read WiFi device name: {e}")
        return "wlan0"

    def classifyConnectionError(error: Exception, auth_expected: bool = False):
        error_msg = str(error)
        lower_error = error_msg.lower()
        auth_markers = [
            "secrets were required",
            "no secrets were provided",
            "invalid secrets",
            "wrong password",
            "bad password",
            "authentication",
            "802-11-wireless-security.psk",
        ]
        auth_activation_markers = [
            "activation failed",
            "connection activation failed",
            "supplicant-disconnect",
            "no secrets",
            "4way_handshake",
            "disconnected during association",
            "asking for new key",
        ]
        not_found_markers = [
            "no network with ssid",
            "wi-fi network could not be found",
            "network could not be found",
        ]

        if any(marker in lower_error for marker in auth_markers) or (
            auth_expected
            and any(marker in lower_error for marker in auth_activation_markers)
        ):
            return (
                "invalid_credentials",
                "Incorrect Wi-Fi password. Please check it and try again.",
            )
        if any(marker in lower_error for marker in not_found_markers):
            return (
                "network_not_found",
                "Wi-Fi network was not found. Move closer and try again.",
            )
        return ("connection_failed", f"Could not connect to Wi-Fi: {error_msg}")

    def invalidateHealthCache():
        WifiManager._cached_health = None
        WifiManager._health_cache_time = 0

    def init():
        logger.info("Wifi initializing")
        if ZEROCONF_OVERWRITE != "":
            logger.info(
                f"Overwriting network configuration due to ZEROCONF_OVERWRITE={ZEROCONF_OVERWRITE}"
            )

        try:
            nmcli.device.show_all()
        except Exception as e:
            logger.warning(f"Networking unavailable! {e}")
            WifiManager._networking_available = False

        config = WifiManager.getCurrentConfig()

        # Only update the hostname if it is a new system or if the hostname has been
        # set before. Do so in case the lookup table ever changed or the hostname is only
        # saved transient
        logger.info(f"Current hostname is '{config.hostname}'")

        hostname_override = MeticulousConfig[CONFIG_USER][HOSTNAME_OVERRIDE]
        # Check if we are on a deployed machine, a container or if we are running elsewhere
        # In the later case we dont want to set the hostname
        MACHINE_HOSTNAMES = ("imx8mn-var-som", "meticulous")
        if config.hostname.startswith(MACHINE_HOSTNAMES) and hostname_override is None:
            new_hostname = HostnameManager.generateHostname()
            if config.hostname != new_hostname:
                logger.info(f"Changing hostname new = {new_hostname}")
                HostnameManager.setHostname(new_hostname)
        elif hostname_override is not None and hostname_override != "none":
            logger.info(f"Hostname override is set to {hostname_override}")
            if config.hostname != str(hostname_override):
                logger.info(f"Setting hostname to override: {hostname_override}")
                HostnameManager.setHostname(str(hostname_override))

        ap_name = HostnameManager.generateDeviceName()
        MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME] = ap_name[:31]
        MeticulousConfig.save()

        if WifiManager._zeroconf is None:
            logger.info("Creating Zeroconf Object")
            WifiManager._zeroconf = ZeroConfAnnouncement(
                config_function=WifiManager.getCurrentConfig
            )

        # Without networking we have no chance starting the wifi or getting the creads
        if WifiManager._networking_available:
            # start AP if needed
            if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP:
                WifiManager.startHotspot()
            else:
                WifiManager.stopHotspot()

            WifiManager._thread = NamedThread(
                "WifiAutoConnect", target=WifiManager.tryAutoConnect
            )
            WifiManager._thread.start()

        WifiManager._zeroconf.start()

    def update_gatt_advertisement():
        """Helper method to safely update GATT advertisement"""
        from ble_gatt import GATTServer

        server = GATTServer.getServer()
        if server and server.loop and server.loop.is_running():
            asyncio.run_coroutine_threadsafe(server.update_advertisement(), server.loop)
        else:
            logger.warning("Cannot update GATT advertisement - server or loop not ready")

    def networking_available():
        return WifiManager._networking_available

    def tryAutoConnect():
        logger.info("Starting Networking background Thread")
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        while True:
            time.sleep(10)

            manufacturing_mode = Machine.enable_manufacturing
            if (
                MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP
                and not manufacturing_mode
            ):
                continue

            if WifiManager._repair_in_progress or WifiManager.isAutoConnectSuppressed():
                now = time.time()
                if now - WifiManager._last_auto_connect_suppressed_log > 30:
                    remaining = WifiManager.getAutoConnectSuppressionRemaining()
                    logger.info(
                        f"Skipping WiFi auto-connect while manual connect or repair is in progress ({remaining}s remaining)"
                    )
                    WifiManager._last_auto_connect_suppressed_log = now
                continue

            # Check if we are already connected to something
            current = WifiManager.getCurrentConfig()
            if current.connected:
                WifiManager.maybeRecoverCurrentConnection(current)
                TimezoneManager.tz_background_update()
                continue

            networks = WifiManager.scanForNetworks(timeout=10)

            known_networks = WifiManager.getKnownWifis()

            for network in networks:
                # Check if we are looking for a specific network in the factory
                if manufacturing_mode and network.ssid == "MeticulousEPW":
                    credentials = WifiWpaPskCredentials(ssid=network.ssid, password="23456789")
                    success = WifiManager.connectToWifi(credentials, source="auto")
                    if success:
                        break

                if network.ssid in known_networks:
                    logger.info(f"Found known WIFI {network.ssid}. Connecting")
                    credentials = {
                        "ssid": network.ssid,
                        "type": WifiType.from_nmcli_security(network.security),
                    }
                    success = WifiManager.connectToWifi(credentials, source="auto")
                    if success:
                        break

    def resetWifiMode():
        # Without networking we have no chance starting the wifi or getting the creads
        if WifiManager._networking_available:
            # start AP if needed
            if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP:
                success = WifiManager.startHotspot()
            else:
                success = WifiManager.stopHotspot()
                WifiManager.scanForNetworks(timeout=1)
                WifiManager._zeroconf.restart()
            WifiManager.update_gatt_advertisement()
            return success
        return False

    def startHotspot():
        if not WifiManager._networking_available:
            WifiManager._last_health_error = "networking_unavailable"
            return False

        logger.info("Starting hotspot")
        WifiManager.invalidateHealthCache()
        last_error = ""

        for channel in [6, 1, 11]:
            WifiManager.deleteConnectionProfile(WifiManager._conname)
            logger.info(f"Starting hotspot on channel {channel}")

            add_result = WifiManager.runCommand(
                [
                    "nmcli",
                    "connection",
                    "add",
                    "type",
                    "wifi",
                    "ifname",
                    WifiManager.getWifiStationDeviceName(),
                    "con-name",
                    WifiManager._conname,
                    "ssid",
                    MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME],
                ],
                timeout=10,
            )
            if add_result is None or add_result.returncode != 0:
                stderr = (
                    add_result.stderr.strip()
                    if add_result is not None
                    else "command failed"
                )
                stdout = add_result.stdout.strip() if add_result is not None else ""
                last_error = f"channel={channel}: {stderr or stdout or 'connection add failed'}"
                logger.error(f"Starting hotspot failed: {last_error}")
                WifiManager.deleteConnectionProfile(WifiManager._conname)
                time.sleep(1)
                continue

            modify_result = WifiManager.runCommand(
                [
                    "nmcli",
                    "connection",
                    "modify",
                    WifiManager._conname,
                    "connection.autoconnect",
                    "no",
                    "802-11-wireless.mode",
                    "ap",
                    "802-11-wireless.band",
                    "bg",
                    "802-11-wireless.channel",
                    str(channel),
                    "802-11-wireless.ap-isolation",
                    "0",
                    "802-11-wireless.powersave",
                    "2",
                    "802-11-wireless-security.key-mgmt",
                    "wpa-psk",
                    "802-11-wireless-security.auth-alg",
                    "open",
                    "802-11-wireless-security.psk",
                    MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD],
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
                timeout=10,
            )
            if modify_result is None or modify_result.returncode != 0:
                stderr = (
                    modify_result.stderr.strip()
                    if modify_result is not None
                    else "command failed"
                )
                stdout = modify_result.stdout.strip() if modify_result is not None else ""
                last_error = f"channel={channel}: {stderr or stdout or 'connection modify failed'}"
                logger.error(f"Starting hotspot failed: {last_error}")
                WifiManager.deleteConnectionProfile(WifiManager._conname)
                time.sleep(1)
                continue

            command = [
                "nmcli",
                "--wait",
                "35",
                "connection",
                "up",
                WifiManager._conname,
            ]
            result = WifiManager.runCommand(command, timeout=35)
            if result is not None and result.returncode == 0:
                if WifiManager.waitForHotspot(timeout=10):
                    logger.info(f"Hotspot started on channel {channel}")
                    WifiManager._last_health_error = ""
                    WifiManager._zeroconf.restart()
                    return True

                last_error = f"channel={channel}: hotspot did not become active"
                logger.error(f"Starting hotspot failed: {last_error}")
            else:
                stderr = (
                    result.stderr.strip() if result is not None else "command failed"
                )
                stdout = result.stdout.strip() if result is not None else ""
                last_error = f"channel={channel}: {stderr or stdout or 'unknown error'}"
                logger.error(f"Starting hotspot failed: {last_error}")

            WifiManager.deleteConnectionProfile(WifiManager._conname)
            time.sleep(1)

        WifiManager._last_health_error = f"hotspot_start_failed: {last_error}"
        WifiManager._zeroconf.restart()
        return False

    def stopHotspot():
        if not WifiManager._networking_available:
            WifiManager._last_health_error = "networking_unavailable"
            return False

        for dev in nmcli.device():
            if dev.device_type == "wifi" and dev.connection == WifiManager._conname:
                logger.info("Stopping Hotspot")
                WifiManager.invalidateHealthCache()
                try:
                    nmcli.connection.down(WifiManager._conname)
                except Exception as e:
                    logger.error(f"Stopping hotspot failed: {e}")
                    WifiManager._last_health_error = f"hotspot_stop_failed: {e}"
                    WifiManager._zeroconf.restart()
                    return False
                WifiManager._zeroconf.restart()
                if WifiManager.waitForHotspotStopped(timeout=10):
                    WifiManager._last_health_error = ""
                    return True
                WifiManager._last_health_error = "hotspot_still_active"
                return False
        return True

    def waitForHotspot(timeout: int = 10):
        target_timeout = time.time() + timeout
        while time.time() < target_timeout:
            try:
                if WifiManager.getCurrentConfig().is_hotspot():
                    return True
            except Exception as e:
                logger.info(f"Failed to verify hotspot state: {e}")
            time.sleep(1)
        return False

    def waitForHotspotStopped(timeout: int = 10):
        target_timeout = time.time() + timeout
        while time.time() < target_timeout:
            try:
                if not WifiManager.getCurrentConfig().is_hotspot():
                    return True
            except Exception as e:
                logger.info(f"Failed to verify hotspot stop state: {e}")
            time.sleep(1)
        return False

    def applyWifiSettings(mode=None, ap_password=None):
        previous_mode = MeticulousConfig[CONFIG_WIFI][WIFI_MODE]
        previous_password = MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD]

        if mode is None:
            mode = previous_mode
        if ap_password is None:
            ap_password = previous_password

        MeticulousConfig[CONFIG_WIFI][WIFI_MODE] = mode
        MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD] = ap_password
        WifiManager.invalidateHealthCache()

        success = WifiManager.resetWifiMode()
        if not success and mode == WIFI_MODE_AP:
            logger.warning("Hotspot mode change failed; attempting AP recovery")
            success = WifiManager.repairWifiConnection(reason="apply_ap_mode")

        if success:
            MeticulousConfig.save()
            return True

        logger.warning("WiFi config change failed; rolling back persisted config")
        MeticulousConfig[CONFIG_WIFI][WIFI_MODE] = previous_mode
        MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD] = previous_password
        MeticulousConfig.save()
        WifiManager.resetWifiMode()
        return False

    def runCommand(command: list[str], timeout: int = 10):
        try:
            return subprocess.run(
                command,
                check=False,
                capture_output=True,
                text=True,
                timeout=timeout,
            )
        except Exception as e:
            logger.warning(f"Failed to run command {command}: {e}")
            return None

    def gatewayReachable(gateway: IPAddress):
        if gateway is None:
            return False

        if shutil.which("ping") is None:
            logger.info("Skipping gateway ping health check: ping command not found")
            return True

        result = WifiManager.runCommand(
            ["ping", "-c", "1", "-W", "2", gateway.format()], timeout=4
        )
        return result is not None and result.returncode == 0

    def dnsResolves():
        try:
            socket.getaddrinfo("meticuloushome.com", 443)
            return True
        except Exception as e:
            logger.info(f"DNS health check failed: {e}")
            return False

    def internetReachable():
        probes = [
            "https://meticuloushome.com",
            "https://www.cloudflare.com/cdn-cgi/trace",
        ]
        for probe in probes:
            try:
                request = urllib.request.Request(
                    probe,
                    headers={"User-Agent": "meticulous-wifi-health/1.0"},
                )
                with urllib.request.urlopen(request, timeout=2) as response:
                    response.read(1)
                    if response.status < 500:
                        return True
            except Exception as e:
                logger.info(f"Internet health check failed for {probe}: {e}")
        return False

    def healthErrorIsRecoverable(error: str):
        return error in {
            "wifi_not_connected",
            "missing_ipv4",
            "gateway_unreachable",
            "hotspot_not_active",
        }

    def getHealthErrorMessage(error: str):
        if not error:
            return ""

        if error.startswith("hotspot_start_failed"):
            return (
                "The machine could not start its Wi-Fi hotspot. Please try again. "
                "If this keeps happening, restart the machine and try again."
            )
        if error.startswith("hotspot_stop_failed"):
            return (
                "The machine could not leave hotspot mode. Please try again or "
                "restart the machine."
            )

        match error:
            case "internet_unreachable":
                return (
                    "Connected to Wi-Fi, but this network does not appear to have "
                    "internet access. Check the router, modem, or phone hotspot data connection."
                )
            case "dns_unreachable":
                return (
                    "Connected to Wi-Fi, but DNS is not working. Check the router's "
                    "internet or DNS settings, or try another network."
                )
            case "gateway_unreachable":
                return (
                    "Connected to Wi-Fi, but the router is not responding. Move closer "
                    "or restart the router."
                )
            case "missing_ipv4":
                return (
                    "Connected to Wi-Fi, but the router did not assign an IP address. "
                    "Check DHCP settings on the router."
                )
            case "wifi_not_connected":
                return "The machine is not connected to Wi-Fi."
            case "wifi_device_unavailable":
                return "Wi-Fi is still starting. Wait a moment and try again."
            case "hotspot_not_active":
                return "The machine could not start its Wi-Fi hotspot. Please try again."
            case "networking_unavailable":
                return "Wi-Fi hardware is not available."
            case _:
                return "Wi-Fi could not be verified. Please check the network and try again."

    def getLastHealthErrorMessage():
        return WifiManager.getHealthErrorMessage(WifiManager._last_health_error)

    def getHealthStatus(
        config: WifiSystemConfig = None, force: bool = False
    ) -> WifiHealthStatus:
        if (
            not force
            and WifiManager._cached_health is not None
            and time.time() - WifiManager._health_cache_time < WifiManager._health_cache_ttl
        ):
            return WifiManager._cached_health

        with WifiManager._health_check_lock:
            if (
                not force
                and WifiManager._cached_health is not None
                and time.time() - WifiManager._health_cache_time
                < WifiManager._health_cache_ttl
            ):
                return WifiManager._cached_health

            return WifiManager.buildHealthStatus(config)

    def buildHealthStatus(config: WifiSystemConfig = None) -> WifiHealthStatus:
        if config is None:
            config = WifiManager.getCurrentConfig()

        mode = MeticulousConfig[CONFIG_WIFI][WIFI_MODE]
        has_ipv4 = any(ip.ip.version == 4 for ip in config.ips)
        ap_active = config.is_hotspot()
        gateway_reachable = False
        dns_resolves = False
        internet_reachable = False

        if mode == WIFI_MODE_AP:
            degraded = not ap_active
            last_error = WifiManager._last_health_error
            if degraded and last_error == "":
                last_error = "hotspot_not_active"
            health = WifiHealthStatus(
                mode,
                config.connected,
                has_ipv4,
                gateway_reachable,
                dns_resolves,
                internet_reachable,
                ap_active,
                degraded,
                last_error,
                WifiManager._last_recovery_action,
                WifiManager._last_recovery_result,
                "Hotspot active. Connect your phone or computer to the machine's Wi-Fi network."
                if ap_active
                else "",
            )
            WifiManager._cached_health = health
            WifiManager._health_cache_time = time.time()
            return health

        if config.connected and has_ipv4:
            gateway_reachable = WifiManager.gatewayReachable(config.gateway)
            dns_resolves = WifiManager.dnsResolves()
            internet_reachable = WifiManager.internetReachable()

        degraded = not (
            config.connected
            and has_ipv4
            and gateway_reachable
            and dns_resolves
            and internet_reachable
        )

        last_error = WifiManager._last_health_error
        if degraded and last_error == "":
            if not config.connected:
                last_error = "wifi_not_connected"
            elif not has_ipv4:
                last_error = "missing_ipv4"
            elif not gateway_reachable:
                last_error = "gateway_unreachable"
            elif not dns_resolves:
                last_error = "dns_unreachable"
            elif not internet_reachable:
                last_error = "internet_unreachable"

        health = WifiHealthStatus(
            mode,
            config.connected,
            has_ipv4,
            gateway_reachable,
            dns_resolves,
            internet_reachable,
            ap_active,
            degraded,
            last_error,
            WifiManager._last_recovery_action,
            WifiManager._last_recovery_result,
        )
        WifiManager._cached_health = health
        WifiManager._health_cache_time = time.time()
        return health

    def maybeRecoverCurrentConnection(config: WifiSystemConfig = None):
        if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP:
            return False

        now = time.time()
        if now - WifiManager._last_health_check < WifiManager._health_check_interval:
            return False
        WifiManager._last_health_check = now

        health = WifiManager.getHealthStatus(config, force=True)
        if not health.degraded:
            WifiManager._health_failures = 0
            WifiManager._last_health_error = ""
            WifiManager._last_recovery_result = "not_needed"
            return False

        WifiManager._health_failures += 1
        WifiManager._last_health_error = health.last_error
        logger.warning(
            f"WiFi health degraded ({WifiManager._health_failures}/"
            f"{WifiManager._health_failure_threshold}): {health.to_json()}"
        )

        if not WifiManager.healthErrorIsRecoverable(health.last_error):
            WifiManager._last_recovery_action = "health_check"
            WifiManager._last_recovery_result = "not_recoverable"
            logger.warning(
                f"WiFi health issue is not recoverable by driver reset: {health.last_error}"
            )
            return False

        if WifiManager._health_failures < WifiManager._health_failure_threshold:
            return False

        if now - WifiManager._last_recovery_attempt < WifiManager._recovery_cooldown:
            logger.warning("WiFi recovery skipped due to cooldown")
            return False

        return WifiManager.repairWifiConnection(reason=health.last_error)

    def repairWifiConnection(reason: str = "manual"):
        if not WifiManager._networking_available:
            WifiManager._last_recovery_result = "failed"
            WifiManager._last_health_error = "networking_unavailable"
            return False

        if WifiManager._repair_in_progress:
            logger.warning("WiFi repair requested while another repair is in progress")
            WifiManager._last_recovery_result = "in_progress"
            return False

        WifiManager._repair_in_progress = True
        WifiManager.suppressAutoConnect(180, f"wifi repair in progress: {reason}")
        try:
            WifiManager._last_recovery_attempt = time.time()
            current = WifiManager.getCurrentConfig()
            logger.warning(f"Starting WiFi repair workflow. reason={reason}")
            WifiManager.invalidateHealthCache()

            initial_health = WifiManager.getHealthStatus(current, force=True)
            if not initial_health.degraded:
                WifiManager._last_recovery_action = "health_check"
                WifiManager._last_recovery_result = "not_needed"
                return True

            if not WifiManager.healthErrorIsRecoverable(initial_health.last_error):
                WifiManager._last_recovery_action = "health_check"
                WifiManager._last_recovery_result = "not_recoverable"
                WifiManager._last_health_error = initial_health.last_error
                logger.warning(
                    f"WiFi repair skipped for non-recoverable health issue: {initial_health.last_error}"
                )
                return False

            if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP:
                steps = [
                    ("start_hotspot", WifiManager.startHotspot),
                    ("restart_wifi_radio", WifiManager.restartWifiRadio),
                    ("driver_in_band_reset", WifiManager.driverInBandReset),
                    ("restart_wifi_service", WifiManager.restartWifiService),
                ]
            else:
                steps = [
                    ("restart_connection", lambda: WifiManager.restartActiveConnection(current)),
                    ("restart_wifi_radio", WifiManager.restartWifiRadio),
                    ("driver_in_band_reset", WifiManager.driverInBandReset),
                    ("restart_wifi_service", WifiManager.restartWifiService),
                ]

            for action, step in steps:
                WifiManager._last_recovery_action = action
                WifiManager._last_recovery_result = "in_progress"
                try:
                    step()
                    if (
                        MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP
                        and action != "start_hotspot"
                    ):
                        WifiManager.startHotspot()
                except Exception as e:
                    logger.warning(f"WiFi repair step failed: {action}: {e}")

                time.sleep(5)
                health = WifiManager.getHealthStatus(force=True)
                if not health.degraded:
                    logger.warning(f"WiFi repair succeeded with {action}")
                    WifiManager._health_failures = 0
                    WifiManager._last_health_error = ""
                    WifiManager._last_recovery_result = "recovered"
                    WifiManager._zeroconf.restart()
                    WifiManager.update_gatt_advertisement()
                    return True

                if not WifiManager.healthErrorIsRecoverable(health.last_error):
                    WifiManager._last_recovery_result = "not_recoverable"
                    WifiManager._last_health_error = health.last_error
                    logger.warning(
                        f"Stopping WiFi repair after {action}; issue is not recoverable by driver reset: {health.last_error}"
                    )
                    return False

            WifiManager._last_recovery_result = "failed"
            WifiManager._last_health_error = WifiManager.getHealthStatus(
                force=True
            ).last_error
            WifiManager.update_gatt_advertisement()
            return False
        finally:
            WifiManager._repair_in_progress = False
            WifiManager.suppressAutoConnect(15, "wifi repair finished")

    def restartActiveConnection(config: WifiSystemConfig):
        if not config.connection_name:
            logger.warning("No active WiFi connection to restart")
            return

        logger.warning(f"Restarting active WiFi connection: {config.connection_name}")
        try:
            nmcli.connection.down(config.connection_name)
        except Exception as e:
            logger.warning(f"Failed to bring WiFi connection down: {e}")
        time.sleep(2)
        nmcli.connection.up(config.connection_name, wait=15)

    def restartWifiRadio():
        logger.warning("Restarting WiFi radio")
        WifiManager.runCommand(["nmcli", "radio", "wifi", "off"], timeout=10)
        time.sleep(3)
        WifiManager.runCommand(["nmcli", "radio", "wifi", "on"], timeout=10)

    def driverInBandReset():
        reset_script = "/etc/wifi/variscite-wifi.d/iw612-wifi"
        if not os.path.exists(reset_script):
            logger.warning(f"WiFi driver reset script not found: {reset_script}")
            return
        logger.warning("Running WiFi driver in-band reset")
        WifiManager.runCommand([reset_script, "in-band-reset"], timeout=15)

    def restartWifiService():
        logger.warning("Restarting variscite WiFi service")
        WifiManager.runCommand(["systemctl", "restart", "variscite-wifi.service"], timeout=30)

    def scanForNetworks(timeout: int = 10, target_network_ssid: str = None):
        if not WifiManager._networking_available:
            return []

        if target_network_ssid == "":
            target_network_ssid = None

        if not WifiManager.isWifiDeviceReady():
            state = WifiManager.getWifiDeviceState()
            WifiManager._last_health_error = "wifi_device_unavailable"
            logger.warning(f"WiFi device is not ready for scanning: state={state}")
            return []

        target_timeout = time.time() + timeout
        retries = 0
        last_forced_rescan = 0
        while time.time() < target_timeout:
            if retries < 3:
                logger.info(
                    f"Requesting scan results: Time left: {target_timeout - time.time()}s"
                )
            elif retries == 3:
                logger.info("Scans returning very fast, stopping logging")

            wifis = []
            try:
                force_rescan = retries == 0 or time.time() - last_forced_rescan > 5
                if force_rescan:
                    last_forced_rescan = time.time()
                wifis = nmcli.device.wifi(rescan=force_rescan)
            except Exception as e:
                logger.info(
                    f"Failed to scan for wifis: {e}, retrying if timeout is not reached"
                )
                wifis = []

            if target_network_ssid is not None:
                wifis = [w for w in wifis if w.ssid == target_network_ssid]

            if len(wifis) > 0:
                break
            retries += 1
            time.sleep(min(0.5, max(0, target_timeout - time.time())))

        logger.info(f"Scanning finished after {retries}")

        WifiManager._known_wifis = wifis
        return wifis

    @staticmethod
    def deleteConnectionProfile(name: str) -> bool:
        deleted = False
        for connection in nmcli.connection():
            if connection.name == name and connection.conn_type == "wifi":
                logger.info(f"Deleting Wi-Fi connection profile: {name}")
                nmcli.connection.delete(name)
                deleted = True
        return deleted

    @staticmethod
    def deleteWifi(ssid: str) -> bool:
        deleted = False
        if ssid in MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS]:
            del MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS][ssid]
            MeticulousConfig.save()

            deleted = True

        for connection in nmcli.connection():
            if (
                connection.name == ssid
                and connection.conn_type == "wifi"
                and connection.name != WifiManager._conname
            ):
                nmcli.connection.delete(ssid)
                deleted = True

        return deleted

    @staticmethod
    def getNetworkManagerWifiConnections():
        known_wifis = {}
        try:
            for connection in nmcli.connection():
                if (
                    connection.conn_type == "wifi"
                    and connection.name != WifiManager._conname
                ):
                    known_wifis[connection.name] = {
                        "ssid": connection.name,
                        "type": WifiManager.getSavedWifiType(connection.name),
                    }
        except Exception as e:
            logger.warning(f"Failed to list known Wi-Fi connections: {e}")
        return known_wifis

    @staticmethod
    def getSavedWifiType(ssid: str):
        try:
            result = subprocess.run(
                [
                    "nmcli",
                    "-g",
                    "802-11-wireless-security.key-mgmt",
                    "connection",
                    "show",
                    ssid,
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            key_mgmt = result.stdout.strip().lower()
            if key_mgmt in {"", "none"}:
                return WifiType.Open.value
            if "sae" in key_mgmt:
                return WifiType.PSK_SAE.value
            if "wpa-psk" in key_mgmt:
                return WifiType.PreSharedKey.value
            if "802-1x" in key_mgmt:
                return WifiType.Enterprise.value
        except Exception as e:
            logger.warning(f"Failed to read saved Wi-Fi type for {ssid}: {e}")
        return WifiType.PreSharedKey.value

    @staticmethod
    def scrubKnownWifiSecrets():
        known_wifis = MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS]
        if not known_wifis:
            return

        scrubbed = {}
        changed = False
        for ssid, entry in known_wifis.items():
            if type(entry) is str:
                scrubbed[ssid] = {"ssid": ssid, "type": WifiType.PreSharedKey.value}
                changed = True
                continue

            if type(entry) is dict:
                sanitized = {
                    "ssid": entry.get("ssid", ssid),
                    "type": entry.get("type", WifiType.PreSharedKey.value),
                }
                scrubbed[ssid] = sanitized
                if sanitized != entry:
                    changed = True
                continue

            scrubbed[ssid] = {"ssid": ssid, "type": WifiType.PreSharedKey.value}
            changed = True

        if changed:
            MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS] = scrubbed
            MeticulousConfig.save()

    @staticmethod
    def getKnownWifis():
        WifiManager.scrubKnownWifiSecrets()
        return WifiManager.getNetworkManagerWifiConnections()

    @staticmethod
    def hasKnownWifiConnection(ssid: str) -> bool:
        return ssid in WifiManager.getNetworkManagerWifiConnections()

    @staticmethod
    def persistClientModeAfterManualConnect(ssid: str):
        if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_CLIENT:
            return

        logger.info(
            f"Persisting Wi-Fi client mode after successful manual connection to {ssid}"
        )
        MeticulousConfig[CONFIG_WIFI][WIFI_MODE] = WIFI_MODE_CLIENT
        MeticulousConfig.save()
        WifiManager.invalidateHealthCache()

    @staticmethod
    def fixWifiConnection(ssid, wifi_type: WifiType):
        logger.info(f"Fixing wifi connection for {ssid} with type {wifi_type}")

        keymgmt = None
        match wifi_type:
            case WifiType.Open:
                keymgmt = "none"
            case WifiType.PreSharedKey:
                keymgmt = "wpa-psk"
            case WifiType.Enterprise:
                keymgmt = "802-1x"
            case WifiType.WEP:
                keymgmt = "wep"
            case WifiType.PSK_SAE:
                keymgmt = "sae"
            case _:
                raise ValueError(f"Unknown WifiType: {wifi_type}")
        nmcli.connection.modify(
            ssid,
            {
                "802-11-wireless.ssid": ssid,
                "802-11-wireless-security.key-mgmt": keymgmt,
            },
        )
        nmcli.connection.up(ssid, wait=10)

    def connectToWifi(credentials: WiFiCredentials, source: str = "manual") -> bool:  # noqa: C901

        WifiManager.clearLastConnectionError()

        if not WifiManager._networking_available:
            WifiManager.setLastConnectionError(
                "networking_unavailable", "Wi-Fi hardware is not available."
            )
            return False

        if credentials is None:
            WifiManager.setLastConnectionError(
                "missing_credentials", "Wi-Fi credentials were not provided."
            )
            return False

        if type(credentials) is not dict:
            credentials = credentials.to_dict()

        wifi_type = credentials.get("type", None)
        if wifi_type is None:
            wifi_type = WifiType.PreSharedKey
            credentials["type"] = wifi_type

        ssid = credentials.get("ssid", None)
        if ssid is None:
            WifiManager.setLastConnectionError(
                "missing_ssid", "Wi-Fi network name was not provided."
            )
            return False

        is_auto_connect = source == "auto"
        if is_auto_connect and WifiManager.isAutoConnectSuppressed():
            logger.info(
                f"Skipping auto-connect to {ssid}; manual WiFi connect is in progress"
            )
            return False
        if not is_auto_connect:
            WifiManager.suppressAutoConnect(90, f"manual connect to {ssid}")

        logger.info(f"Connecting to wifi: {ssid}")

        networks = WifiManager.scanForNetworks(timeout=30, target_network_ssid=ssid)
        logger.info(networks)
        if len(networks) > 0:
            if len([x for x in networks if x.in_use]) > 0:
                logger.info("Already connected")
                if not is_auto_connect:
                    WifiManager.persistClientModeAfterManualConnect(ssid)
                    WifiManager.suppressAutoConnect(
                        45, f"manual connect to {ssid} completed"
                    )
                WifiManager._zeroconf.restart()
                WifiManager.update_gatt_advertisement()
                return True

            for network in networks:
                if network.ssid == ssid:
                    if (
                        wifi_type == WifiType.PreSharedKey
                        and "WPA3" in network.security.upper()
                    ):
                        wifi_type = WifiType.PSK_SAE
                        credentials["type"] = WifiType.PSK_SAE
                        logger.info("Network supports WPA3, switching to SAE")
                        break

            logger.info("Target network online, connecting now")
            needs_fix = False
            try:
                if wifi_type == WifiType.Open:
                    nmcli.device.wifi_connect(ssid, None)
                elif wifi_type == WifiType.PreSharedKey or wifi_type == WifiType.PSK_SAE:
                    password = credentials.get("password")
                    if password is None and WifiManager.hasKnownWifiConnection(ssid):
                        logger.info(f"Connecting to known Wi-Fi profile: {ssid}")
                        nmcli.connection.up(ssid, wait=15)
                    elif password is None:
                        WifiManager.setLastConnectionError(
                            "missing_credentials",
                            "Wi-Fi password was not provided.",
                        )
                        WifiManager.update_gatt_advertisement()
                        return False
                    else:
                        nmcli.device.wifi_connect(ssid, password)
                elif wifi_type == WifiType.Enterprise:
                    logger.error("Enterprise wifi not yet implemented")
                    WifiManager.setLastConnectionError(
                        "unsupported_security",
                        "Enterprise Wi-Fi networks are not supported yet.",
                    )
                    return False

            except Exception as e:
                error_msg = str(e)
                if "802-11-wireless-security.key-mgmt: property is missing" in error_msg:
                    needs_fix = True
                else:
                    auth_expected = wifi_type in [
                        WifiType.PreSharedKey,
                        WifiType.PSK_SAE,
                    ]
                    code, message = WifiManager.classifyConnectionError(
                        e, auth_expected=auth_expected
                    )
                    WifiManager.setLastConnectionError(code, message)
                    if code == "invalid_credentials":
                        WifiManager.deleteWifi(ssid)
                    logger.error(f"Failed to connect to wifi: {e}")
                    WifiManager.update_gatt_advertisement()
                    return False
            if needs_fix:
                try:
                    WifiManager.fixWifiConnection(ssid, wifi_type)
                except Exception as e:
                    auth_expected = wifi_type in [
                        WifiType.PreSharedKey,
                        WifiType.PSK_SAE,
                    ]
                    code, message = WifiManager.classifyConnectionError(
                        e, auth_expected=auth_expected
                    )
                    WifiManager.setLastConnectionError(code, message)
                    if code == "invalid_credentials":
                        WifiManager.deleteWifi(ssid)
                    logger.error(f"Failed to connect to wifi: {e}")
                    WifiManager.update_gatt_advertisement()
                    return False

            logger.info(
                "Connection should be established, checking if a network is marked in-use"
            )
            networks = WifiManager.scanForNetworks(timeout=10, target_network_ssid=ssid)
            if len([x for x in networks if x.in_use]) > 0:
                current = WifiManager.getCurrentConfig()
                if current.connection_name != ssid:
                    WifiManager.setLastConnectionError(
                        "connection_preempted",
                        f"Connected to {current.connection_name or 'another network'} instead of {ssid}.",
                    )
                    if not is_auto_connect:
                        WifiManager.suppressAutoConnect(
                            30, f"manual connect to {ssid} was preempted"
                        )
                    WifiManager.update_gatt_advertisement()
                    return False
                logger.info("Successfully connected")
                if not is_auto_connect:
                    WifiManager.suppressAutoConnect(
                        45, f"manual connect to {ssid} completed"
                    )
                WifiManager._zeroconf.restart()
                if not is_auto_connect:
                    WifiManager.persistClientModeAfterManualConnect(ssid)
                health = WifiManager.getHealthStatus(force=True)
                if health.degraded:
                    logger.warning(
                        f"WiFi connected but health is degraded: {health.to_json()}"
                    )
                    if WifiManager.healthErrorIsRecoverable(health.last_error):
                        WifiManager.repairWifiConnection(reason="connect_validation")
                WifiManager.rememberWifi(credentials)
                WifiManager.update_gatt_advertisement()
                return True

            if wifi_type == WifiType.PreSharedKey or wifi_type == WifiType.PSK_SAE:
                WifiManager.setLastConnectionError(
                    "invalid_credentials",
                    "Incorrect Wi-Fi password. Please check it and try again.",
                )
                if credentials.get("password") is None:
                    WifiManager.deleteWifi(ssid)
            else:
                WifiManager.setLastConnectionError(
                    "connection_timeout",
                    "Could not verify the Wi-Fi connection. Please try again.",
                )
            WifiManager.update_gatt_advertisement()
            return False

        logger.info("Target network was not found, no connection established")
        if not WifiManager.isWifiDeviceReady():
            WifiManager.setLastConnectionError(
                "wifi_device_unavailable",
                "Wi-Fi radio is not ready. Please wait a moment and try again.",
            )
        else:
            WifiManager.setLastConnectionError(
                "network_not_found",
                "Wi-Fi network was not found. Move closer and try again.",
            )
        WifiManager.update_gatt_advertisement()
        return False

    def rememberWifi(credentials: WiFiCredentials):
        if type(credentials) is not dict:
            credentials = credentials.to_dict()

        ssid = credentials.get("ssid")
        if not ssid:
            return

        if ssid in MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS]:
            del MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS][ssid]
            MeticulousConfig.save()

    # Reads the IP from ZEROCONF_OVERWRITE and announces that instead
    def mockCurrentConfig():
        connected: bool = True
        connection_name: str = "MeticulousMockConnection"

        overwrite = ZEROCONF_OVERWRITE.split(",")
        mockIP = IPNetwork(overwrite[0])
        hostname: str = overwrite[1]

        gateway: IPAddress = IPAddress(mockIP.first)
        routes: list[str] = []
        ips: list[IPNetwork] = [mockIP]
        dns: list[IPAddress] = [IPAddress("8.8.8.8")]
        mac: str = "AA:BB:CC:FF:FF:FF"
        domains: list[str] = []
        return WifiSystemConfig(
            connected,
            connection_name,
            gateway,
            routes,
            ips,
            dns,
            mac,
            hostname,
            domains,
        )

    def getCurrentConfig() -> WifiSystemConfig:

        if ZEROCONF_OVERWRITE != "":
            return WifiManager.mockCurrentConfig()

        connected: bool = False
        connection_name: str = None
        gateway: IPAddress = None
        routes: list[str] = []
        ips: list[IPNetwork] = []
        dns: list[IPAddress] = []
        domains: list[str] = []
        mac: str = ""
        hostname: str = socket.gethostname()

        if not WifiManager._networking_available:
            return WifiSystemConfig(
                connected,
                connection_name,
                gateway,
                routes,
                ips,
                dns,
                mac,
                hostname,
                domains,
            )

        for dev in nmcli.device():
            if dev.device_type == "wifi":
                config = nmcli.device.show(dev.device)
                if dev.state == "connected":
                    connected = True
                    for k, v in config.items():
                        match k:
                            case str(k) if "IP4.ADDRESS" in k or "IP6.ADDRESS" in k:
                                if v is not None:
                                    ip = IPNetwork(v)
                                    ips.append(ip)
                            case str(k) if "IP4.ROUTE" in k or "IP6.ROUTE" in k:
                                if v is not None:
                                    routes.append(v)
                            case str(k) if "IP4.DNS" in k or "IP6.DNS" in k:
                                if v is not None:
                                    ip = IPAddress(v)
                                    dns.append(ip)
                            case str(k) if "IP4.DOMAIN" in k:
                                if v is not None and v != "domain_not_set.invalid":
                                    domains.append(v)
                            case "GENERAL.HWADDR":
                                mac = v
                            case "GENERAL.CONNECTION":
                                connection_name = v
                            case "IP4.GATEWAY":
                                if v is not None:
                                    gateway = IPAddress(v)
                elif mac == "" and config.get("GENERAL.HWADDR"):
                    mac = config.get("GENERAL.HWADDR")

        return WifiSystemConfig(
            connected,
            connection_name,
            gateway,
            routes,
            ips,
            dns,
            mac,
            hostname,
            domains,
        )
