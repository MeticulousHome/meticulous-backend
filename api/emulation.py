import tornado.web
import json
from config import *
from .wifi import WiFiConfig, WiFiQRHandler, WifiManager
from .api import API, APIVersion
from .base_handler import BaseHandler

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)


class WiFiConfigHandler(BaseHandler):
    def get(self):
        mode = MeticulousConfig[CONFIG_WIFI][WIFI_MODE]
        apName = MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME]
        apPassword = MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD]
        knownWifis = MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS]
        knownWifis["hello miguel"] = "This hopefully works"
        knownWifis["Test123"] = "Password123"
        knownWifis["Placeholder1"] = "Placeholder1"
        knownWifis["Placeholder2"] = "Placeholder2"
        knownWifis["Placeholder3"] = "Placeholder3"
        knownWifis["Placeholder4"] = "Placeholder4"
        knownWifis["Placeholder5"] = "Placeholder5"
        knownWifis["Placeholder6"] = "Placeholder6"

        wifi_config = {
            "config": WiFiConfig(mode, apName, apPassword).to_json(),
            "status": {
                "connected": True,
                "connection_name": "MeticulousWifi",
                "gateway": "192.168.2.1",
                "routes": [
                    "dst = 192.168.2.0/24, nh = 0.0.0.0, mt = 600",
                    "dst = 0.0.0.0/0, nh = 192.168.2.1, mt = 600",
                    "dst = fe80::/64, nh = ::, mt = 1024",
                    "dst = fd00:0:0:d00d::/64, nh = ::, mt = 600",
                    "dst = 2003:fb:ef0c:e100::/64, nh = ::, mt = 600",
                    "dst = fd00:0:0:d00d::/64, nh = fe80::3e37:12ff:fe90:92e5, mt = 605",
                    "dst = 2003:fb:ef0c:e100::/56, nh = fe80::3e37:12ff:fe90:92e5, mt = 600",
                    "dst = ::/0, nh = fe80::3e37:12ff:fe90:92e5, mt = 600"
                ],
                "ips": [
                    "192.168.2.223",
                    "2003:fb:ef0c:e100:54a5:91c1:7bd4:d23e",
                    "fd00::d00d:269c:62bd:3a17:df38",
                    "fe80::4e8:f348:8479:1afe"
                ],
                "dns": [
                    "192.168.2.1",
                    "fd00::d00d:3e37:12ff:fe90:92e5",
                    "2003:fb:ef0c:e100:3e37:12ff:fe90:92e5"
                ],
                "mac": "C0:EE:40:A4:7D:A9",
                "hostname": "imx8mn-var-som"
            },
            "known_wifis": knownWifis
        }
        self.write(json.dumps(wifi_config))

    def post(self):
        try:
            data = json.loads(self.request.body)
            if "mode" in data and data["mode"] in [WIFI_MODE_AP, WIFI_MODE_CLIENT]:
                logger.warning("Changing wifi mode")
                MeticulousConfig[CONFIG_WIFI][WIFI_MODE] = data["mode"]
                MeticulousConfig.save()
                WifiManager.resetWifiMode()
                del data["mode"]

            if "apPassword" in data:
                logger.warning("Changing wifi ap password")
                MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD] = data["apPassword"]
                MeticulousConfig.save()
                WifiManager.resetWifiMode()
                del data["apPassword"]

            logger.info(f"Unused request entries: {data}")

            return self.get()
        except json.JSONDecodeError as e:
            self.set_status(400)
            self.write(f"Invalid JSON")
            logger.warning(
                f"Failed to parse passed JSON: {e}", stack_info=False)

        except Exception as e:
            self.set_status(400)
            self.write(f"Failed to write config")
            logger.warning("Failed to accept passed config: ",
                           exc_info=e, stack_info=True)


class WiFiListHandler(BaseHandler):
    def get(self):
        networks = [
            {
                "ssid": "MeticulousWifi",
                "signal": 75,
                "rate": 195,
                "in_use": True
            },
            {
                "ssid": "StarGate Legacy",
                "signal": 59,
                "rate": 195,
                "in_use": True
            },
            {
                "ssid": "StarGate Enterprise",
                "signal": 57,
                "rate": 405,
                "in_use": True
            },
            {
                "ssid": "StarGate Picard",
                "signal": 57,
                "rate": 405,
                "in_use": True
            },
        ]

        self.write(json.dumps(networks))


class WiFiConnectHandler(BaseHandler):
    def post(self):
        try:
            data = json.loads(self.request.body)
            ssid = data['ssid']
            password = data['password']

            self.write({"status": "ok", "ssid": ssid, "password": password})
        except Exception as e:
            self.set_status(400)
            self.write(f"Error: {str(e)}")


def register_emulation_handlers():
    API.register_handler(APIVersion.V1, r"/wifi/config", WiFiConfigHandler),
    API.register_handler(APIVersion.V1, r"/wifi/list", WiFiListHandler),
    API.register_handler(APIVersion.V1, r"/wifi/connect", WiFiConnectHandler),
    API.register_handler(APIVersion.V1, r"/wifi/config/qr.png", WiFiQRHandler),
