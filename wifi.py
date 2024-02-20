from config import *
from netaddr import IPAddress, IPNetwork
from typing import List
from dataclasses import dataclass
from api.zeroconf_announcement import ZeroConfAnnouncement

import nmcli
import time
import socket

from hostname import HostnameManager

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

nmcli.disable_use_sudo()
nmcli.set_lang("C.UTF-8")

# Should be something like "192.168.2.123/24,MyHostname"
ZEROCONF_OVERWRITE=os.getenv("ZEROCONF_OVERWRITE", '')
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
            "hostname": self.hostname
        }

    def is_hotspot(self):
        return self.connection_name == WifiManager._conname

class WifiManager():
    _known_wifis = []
    # Internal name used by network manager to refer to the AP configuration
    _conname = "meticulousLocalAP"
    _networking_available = True
    _zeroconf = None

    def init():
        if ZEROCONF_OVERWRITE != '':
            logger.info(f"Overwriting network configuration due to ZEROCONF_OVERWRITE={ZEROCONF_OVERWRITE}")

        try:
            nmcli.device.show_all()
        except Exception as e:
            logger.warning("Networking unavailable!")
            WifiManager._networking_available = False

        config = WifiManager.getCurrentConfig()
        # Only update the hostname if it is a new system or if the hostname has been
        # set before. Do so in case the lookup table ever changed or the hostname is only
        # saved transient
        if config.mac != "":
            logger.info(f"Current hostname is '{config.hostname}'")

            # Check if we are on a deployed machine, a container or if we are running elsewhere
            # In the later case we dont want to set the hostname
            MACHINE_HOSTNAMES = ("imx8mn-var-som", "meticulous-")
            if config.hostname.startswith(MACHINE_HOSTNAMES):
                HostnameManager.checkAndUpdateHostname(config.hostname, config.mac)

            ap_name = HostnameManager.generateDeviceName(config.mac)
            MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME] = ap_name
            MeticulousConfig.save()

        if WifiManager._zeroconf is None:
            WifiManager._zeroconf = ZeroConfAnnouncement(config_function = WifiManager.getCurrentConfig)

        # Without networking we have no chance starting the wifi or getting the creads
        if WifiManager._networking_available:
            # start AP if needed
            if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP:
                WifiManager.startHotspot()
            else:
                WifiManager.stopHotspot()

        WifiManager._zeroconf.start()

    def resetWifiMode():
        # Without networking we have no chance starting the wifi or getting the creads
        if WifiManager._networking_available:
            # start AP if needed
            if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP:
                WifiManager.startHotspot()
            else:
                WifiManager.stopHotspot()
                WifiManager.scanForNetworks(time = 1)
                WifiManager._zeroconf.restart()

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
            if len([x for x in networks if x.in_use]) > 0:
                logger.info("Already connected")
                WifiManager._zeroconf.restart()
                return True

            logger.info("Target network online, connecting now")
            try:
                nmcli.device.wifi_connect(ssid, password)
            except Exception as e:
                logger.info(f"Failed to connect to wifi: {e}")
                return False
            logger.info("Connection should be established, checking if a network is marked in-use")
            networks = WifiManager.scanForNetworks(timeout=10, target_network_ssid=ssid)
            if len([x for x in networks if x.in_use]) > 0:
                logger.info("Successfully connected")
                WifiManager._zeroconf.restart()
                return True
        logger.info("Target network was not found, no connection established")
        return False

    # Reads the IP from ZEROCONF_OVERWRITE and announces that instead
    def mockCurrentConfig():
        connected: bool = True
        connection_name: str = "MeticulousMockConnection"

        overwrite = ZEROCONF_OVERWRITE.split(",")
        mockIP = IPNetwork(overwrite[0])
        hostname : str = overwrite[1]

        gateway: IPAddress = IPAddress(mockIP.first)
        routes: list[str] = []
        ips: list[IPNetwork] = [mockIP]
        dns: list[IPAddress] = [IPAddress("8.8.8.8")]
        mac: str = "AA:BB:CC:FF:FF:FF"
        domains: list[str] = []
        return WifiSystemConfig(connected, connection_name, gateway, routes, ips, dns, mac, hostname, domains)

    def getCurrentConfig() -> WifiSystemConfig:

        if ZEROCONF_OVERWRITE  != '' :
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
            return WifiSystemConfig(connected, connection_name, gateway, routes, ips, dns, mac, hostname)

        for dev in nmcli.device():
            if dev.device_type == 'wifi':
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

        return WifiSystemConfig(connected, connection_name, gateway, routes, ips, dns, mac, hostname, domains)
