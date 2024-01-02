from config import *
from netaddr import IPAddress, IPNetwork
from typing import List
from dataclasses import dataclass

import nmcli
import time

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

nmcli.disable_use_sudo()

@dataclass
class WifiSystemConfig():
    """Class Representing the current network configuration"""
    connected: bool
    connection_name: str
    gateway: IPAddress
    routes: List[str]
    ips: List[IPNetwork]
    dns: List[IPAddress]
    mac: str


class WifiManager():
    _known_wifis = []
    # Internal name used by network manager to refer to the AP configuration
    _conname = "meticulousLocalAP"
    _networking_available = True

    def init():
        try:
            nmcli.device.show_all()
        except Exception as e:
            logger.warning("Networking unavailable!")
            WifiManager._networking_available = False
            return

        # start AP if needed
        if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP:
            WifiManager.startHotspot()
        else:
            WifiManager.stopHotspot()

    def startHotspot():
        if not WifiManager._networking_available:
            return
        
        logger.info("Starting hotspot")
        return nmcli.device.wifi_hotspot(
            con_name=WifiManager._conname,
            ssid=MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME],
            password=MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD]
        )

    def stopHotspot():
        if not WifiManager._networking_available:
            return

        for dev in nmcli.device():
            if dev.device_type == 'wifi' and dev.connection == WifiManager._conname:
                logger.info(f"Stopping Hotspot")
                return nmcli.connection.down(WifiManager._conname)

    def scanForNetworks(timeout: int = 10, target_network_ssid: str = None):
        if not WifiManager._networking_available:
            return []

        if target_network_ssid == "":
            target_network_ssid = None

        target_timeout = time.time() + timeout
        while time.time() < target_timeout:
            logger.info(
                f"Requesting scan results: Time left: {target_timeout - time.time()}s")
            wifis = []
            try:
                wifis = nmcli.device.wifi()
            except Exception as e:
                logger.info(
                    f"Failed to scan for wifis: {e}, retrying if timeout is not reached")
                wifis = []

            if target_network_ssid != None:
                wifis = [w for w in wifis if w.ssid == target_network_ssid]

            if len(wifis) > 0:
                break

        WifiManager._known_wifis = wifis
        return wifis

    def connectToWifi(ssid: str, password: str):
        if not WifiManager._networking_available:
            return False

        logger.info(f"Connecting to wifi: {ssid}")

        networks = WifiManager.scanForNetworks(
            timeout=30, target_network_ssid=ssid)
        logger.info(networks)
        if len(networks) > 0:
            logger.info("Target network online, connecting now")
            try:
                nmcli.device.wifi_connect(ssid, password)
            except Exception as e:
                logger.info(f"Failed to connect to wifi: {e}")
                return False

            if len([x for x in networks if x.in_use]) > 0:
                logger.info("Successfully connected")
                return True
        return False

    def getCurrentConfig() -> WifiSystemConfig:

        connected: bool = False
        connection_name: str = None
        gateway: IPAddress = None
        routes: list[str] = []
        ips: list[IPNetwork] = []
        dns: list[IPAddress] = []
        mac: str = ""

        if not WifiManager._networking_available:
            return WifiSystemConfig(connected, connection_name, gateway, routes, ips, dns, mac, hostname)

        for dev in nmcli.device():
            if dev.device_type == 'wifi' and dev.state == "connected":
                connected = True
                config = nmcli.device.show(dev.device)
                for k, v in config.items():
                    match k:
                        case str(k) if "IP4.ADDRESS" in k or "IP6.ADDRESS" in k:
                            ip = IPNetwork(v)
                            ips.append(ip)
                        case str(k) if "IP4.ROUTE" in k or "IP6.ROUTE" in k:
                            routes.append(v)
                        case str(k) if "IP4.DNS" in k or "IP6.DNS" in k:
                            ip = IPAddress(v)
                            dns.append(ip)
                        case "GENERAL.HWADDR":
                            mac = v
                        case "GENERAL.CONNECTION":
                            connection_name = v
                        case "IP4.GATEWAY":
                            gateway = IPAddress(v)

        return WifiSystemConfig(connected, connection_name, gateway, routes, ips, dns, mac)
