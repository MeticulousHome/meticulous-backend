from config import *
from netaddr import IPAddress, IPNetwork
from typing import List
from dataclasses import dataclass
from api.zeroconf_announcement import ZeroConfAnnouncement

import nmcli
import time

from hostname import HostnameManager

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
    hostname: str


class WifiManager():
    _known_wifis = []
    # Internal name used by network manager to refer to the AP configuration
    _conname = "meticulousLocalAP"
    _networking_available = True
    _zeroconf = None

    def init():
        try:
            nmcli.device.show_all()
        except Exception as e:
            logger.warning("Networking unavailable!")
            WifiManager._networking_available = False
            return

        config = WifiManager.getCurrentConfig()
        # Only update the hostname if it is a new system or if the hostname has been
        # set before. Do so in case the lookup table ever changed or the hostname is only
        # saved transient
        if config.mac != "" and config.hostname == "imx8mn-var-som" or "meticulous-" in config.hostname:
            HostnameManager.checkAndUpdateHostname(config.mac)

        WifiManager._zeroconf = ZeroConfAnnouncement(config_function = WifiManager.getCurrentConfig)

        # start AP if needed
        if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP:
            WifiManager.startHotspot()
        else:
            WifiManager.stopHotspot()

        WifiManager._zeroconf.start()

    def startHotspot():
        if not WifiManager._networking_available:
            return
        
        logger.info("Starting hotspot")
        nmcli.device.wifi_hotspot(
            con_name=WifiManager._conname,
            ssid=MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME],
            password=MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD]
        )
        WifiManager._zeroconf.restart()

    def stopHotspot():
        if not WifiManager._networking_available:
            return

        for dev in nmcli.device():
            if dev.device_type == 'wifi' and dev.connection == WifiManager._conname:
                logger.info(f"Stopping Hotspot")
                nmcli.connection.down(WifiManager._conname)
                WifiManager._zeroconf.restart()
                return

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
                WifiManager._zeroconf.restart()
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
        hostname:str = nmcli.general.get_hostname()
        if not WifiManager._networking_available:
            return WifiSystemConfig(connected, connection_name, gateway, routes, ips, dns, mac, hostname)

        for dev in nmcli.device():
            if dev.device_type == 'wifi' and dev.state == "connected":
                connected = True
                config = nmcli.device.show(dev.device)
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
                        case "GENERAL.HWADDR":
                            mac = v
                        case "GENERAL.CONNECTION":
                            connection_name = v
                        case "IP4.GATEWAY":
                            if v is not None:
                                gateway = IPAddress(v)

        return WifiSystemConfig(connected, connection_name, gateway, routes, ips, dns, mac, hostname)
