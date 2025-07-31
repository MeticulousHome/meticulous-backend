import asyncio
import os
import socket
import time
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
    Enterprise = "802.1X"
    WEP = "WEP"

    @staticmethod
    def from_nmcli_security(security):
        if security == "":
            return WifiType.Open
        elif "802.1X" in security:
            return WifiType.Enterprise
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


# Define a union type for WiFi credentials
WiFiCredentials = (
    WifiWpaEnterpriseCredentials | WifiOpenCredentials | WifiWpaPskCredentials
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
            self.gateway.format()
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


class WifiManager:
    _known_wifis = []
    _thread = None
    # Internal name used by network manager to refer to the AP configuration
    _conname = "meticulousLocalAP"
    _networking_available = True
    _zeroconf = None

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
        elif hostname_override is not None:
            # If we have a hostname override, we set it to the override
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
            logger.warning(
                "Cannot update GATT advertisement - server or loop not ready"
            )

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

            # Check if we are already connected to something
            current = WifiManager.getCurrentConfig()
            if current.connected:
                TimezoneManager.tz_background_update()
                continue

            networks = WifiManager.scanForNetworks(timeout=10)
            previousNetworks = MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS]

            for network in networks:
                # Check if we are looking for a specific network in the factory
                if manufacturing_mode and network.ssid == "MeticulousEPW":
                    credentials = WifiWpaPskCredentials(
                        ssid=network.ssid, password="23456789"
                    )
                    success = WifiManager.connectToWifi(credentials)
                    if success:
                        break

                if network.ssid in previousNetworks:
                    logger.info(f"Found known WIFI {network.ssid}. Connecting")
                    credentials = previousNetworks[network.ssid]
                    if type(credentials) is str:
                        credentials = WifiWpaPskCredentials(
                            ssid=network.ssid, password=credentials
                        )
                        WifiManager.rememberWifi(credentials)
                    success = WifiManager.connectToWifi(credentials)
                    if success:
                        break

    def resetWifiMode():
        # Without networking we have no chance starting the wifi or getting the creads
        if WifiManager._networking_available:
            # start AP if needed
            if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP:
                WifiManager.startHotspot()
            else:
                WifiManager.stopHotspot()
                WifiManager.scanForNetworks(timeout=1)
                WifiManager._zeroconf.restart()
            WifiManager.update_gatt_advertisement()

    def startHotspot():
        if not WifiManager._networking_available:
            return

        logger.info("Starting hotspot")
        try:
            nmcli.device.wifi_hotspot(
                con_name=WifiManager._conname,
                ssid=MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME],
                password=MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD],
            )
        except Exception as e:
            logger.error(f"Starting hotspot failed: {e}")
        WifiManager._zeroconf.restart()

    def stopHotspot():
        if not WifiManager._networking_available:
            return

        for dev in nmcli.device():
            if dev.device_type == "wifi" and dev.connection == WifiManager._conname:
                logger.info("Stopping Hotspot")
                try:
                    nmcli.connection.down(WifiManager._conname)
                except Exception as e:
                    logger.error(f"Stopping hotspot failed: {e}")
                WifiManager._zeroconf.restart()
                return

    def scanForNetworks(timeout: int = 10, target_network_ssid: str = None):
        if not WifiManager._networking_available:
            return []

        if target_network_ssid == "":
            target_network_ssid = None

        target_timeout = time.time() + timeout
        retries = 0
        while time.time() < target_timeout:
            if retries < 3:
                logger.info(
                    f"Requesting scan results: Time left: {target_timeout - time.time()}s"
                )
            elif retries == 3:
                logger.info("Scans returning very fast, stopping logging")

            wifis = []
            try:
                wifis = nmcli.device.wifi()
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

        logger.info(f"Scanning finished after {retries}")

        WifiManager._known_wifis = wifis
        return wifis

    @staticmethod
    def deleteWifi(ssid: str) -> bool:
        if ssid in MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS]:
            nmcli.connection.delete(ssid)
            del MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS][ssid]
            MeticulousConfig.save()

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

    def connectToWifi(credentials: WiFiCredentials) -> bool:  # noqa: C901

        if not WifiManager._networking_available:
            return False

        if credentials is None:
            return False

        if type(credentials) is not dict:
            credentials = credentials.to_dict()

        wifi_type = credentials.get("type", None)
        if wifi_type is None:
            wifi_type = WifiType.PreSharedKey
            credentials["type"] = wifi_type

        ssid = credentials.get("ssid", None)
        if ssid is None:
            return False

        logger.info(f"Connecting to wifi: {ssid}")

        networks = WifiManager.scanForNetworks(timeout=30, target_network_ssid=ssid)
        logger.info(networks)
        if len(networks) > 0:
            if len([x for x in networks if x.in_use]) > 0:
                logger.info("Already connected")
                WifiManager._zeroconf.restart()
                WifiManager.update_gatt_advertisement()
                return True

            logger.info("Target network online, connecting now")
            needs_fix = False
            try:
                if wifi_type == WifiType.Open:
                    nmcli.device.wifi_connect(ssid, None)
                elif wifi_type == WifiType.PreSharedKey:
                    nmcli.device.wifi_connect(ssid, credentials.get("password", ""))
                elif wifi_type == WifiType.Enterprise:
                    logger.error("Enterprise wifi not yet implemented")
                    return False

            except Exception as e:
                error_msg = str(e)
                if (
                    "802-11-wireless-security.key-mgmt: property is missing"
                    in error_msg
                ):
                    needs_fix = True
                else:
                    logger.error(f"Failed to connect to wifi: {e}")
                    WifiManager.update_gatt_advertisement()
                    return False
            if needs_fix:
                try:
                    WifiManager.fixWifiConnection(ssid, wifi_type)
                except Exception as e:
                    logger.error(f"Failed to connect to wifi: {e}")
                    WifiManager.update_gatt_advertisement()
                    return False

            logger.info(
                "Connection should be established, checking if a network is marked in-use"
            )
            networks = WifiManager.scanForNetworks(timeout=10, target_network_ssid=ssid)
            if len([x for x in networks if x.in_use]) > 0:
                logger.info("Successfully connected")
                WifiManager._zeroconf.restart()
                MeticulousConfig[CONFIG_WIFI][WIFI_MODE] = WIFI_MODE_CLIENT
                WifiManager.rememberWifi(credentials)
                WifiManager.update_gatt_advertisement()
                return True

        logger.info("Target network was not found, no connection established")
        WifiManager.update_gatt_advertisement()
        return False

    def rememberWifi(credentials: WiFiCredentials):
        if type(credentials) is not dict:
            credentials = credentials.to_dict()

        if "type" not in credentials:
            credentials["type"] = WifiType.PreSharedKey

        if type(credentials.get("type")) is WifiType:
            credentials["type"] = credentials["type"].value

        MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS][
            credentials.get("ssid")
        ] = credentials
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
