import tornado.web
import json
from config import *
from wifi import WifiManager
from ble_gatt import GATTServer

from .base_handler import BaseHandler

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

class WiFiConfig:
    def __init__(self, provisioning = None, mode = None, apName = None, apPassword = None):
        self.provisioning = provisioning
        self.mode = mode
        self.apName = apName
        self.apPassword = apPassword

    def __repr__(self):
        return f"WiFiConfiguration(provisioning={self.provisioning}, mode='{self.mode}', apName='{self.apName}', apPassword='{self.apPassword}')"

    @classmethod
    def from_json(cls, json_data):
        provisioning = json_data.get('provisioning')
        mode = json_data.get('mode')
        apName = json_data.get('apName')
        apPassword = json_data.get('apPassword')
        return cls(provisioning, mode, apName, apPassword)

    def to_json(self):
        return {
            "provisioning": self.provisioning,
            "mode": self.mode,
            "apName": self.apName,
            "apPassword": self.apPassword,
        }


class WiFiConfigHandler(BaseHandler):
    def get(self):
        provisioning = GATTServer.is_provisioning()
        mode = MeticulousConfig[CONFIG_WIFI][WIFI_MODE]
        apName = MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME]
        apPassword = MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD]
        wifi_config = {
            "config": WiFiConfig(provisioning, mode, apName, apPassword).to_json(),
            "status": WifiManager.getCurrentConfig().to_json(),
        }
        self.write(json.dumps(wifi_config))

    def post(self):
        try:
            data = json.loads(self.request.body)
            if "provisioning" in data and data["provisioning"] == True:
                logger.warning("Enableing GATT provisioning")
                GATTServer.allow_wifi_provisioning()

            logger.warning(f"TODO persist wifi config:{data}")
            self.write("No action taked, needs implementation")

        except Exception as e:
            self.set_status(400)
            self.write(f"Failed to write config")
            logger.warning("Failed to accept passed config: ", exc_info=e, stack_info=True)

class WiFiListHandler(BaseHandler):
    def get(self):
        networks = dict()
        try:
            for s in WifiManager.scanForNetworks():
                if s.ssid is not None and s.ssid != "":
                    formated : dict = {"ssid": s.ssid, "signal": s.signal, "rate": s.rate, "in_use": s.in_use}
                    exists = networks.get(s.ssid)
                    # Make sure the network in use is always listed
                    if exists is None or s.in_use:
                        networks[s.ssid] = formated.copy()
                    else:
                        # Dont overwrite the in_use network
                        logger.warning(f"{exists}, {exists.get('signal')}")
                        if exists["in_use"]:
                            continue
                        if s.signal > exists["signal"]:
                            networks[s.ssid] = formated
            response = sorted(networks.values(), key=lambda x: x["signal"], reverse=True)
            response = json.dumps(response)
            self.write(response)
        except Exception as e:
            self.set_status(400)
            self.write(f"Failed to fetch wifi list")
            logger.warning("Failed to fetch / format wifi list: ", exc_info=e, stack_info=True)

class WiFiConnectHandler(BaseHandler):
    def post(self):
        try:
            data = json.loads(self.request.body)
            ssid = data['ssid']
            password = data['password']

            success = WifiManager.connectToWifi(ssid, password)

            if success:
                self.write("Successfully initiated connection to WiFi network.")
            else:
                self.set_status(400)
                self.write("Failed to conect")
        except Exception as e:
            self.set_status(400)
            self.write(f"Failed to connect")
            logger.warning("Failed to connect: ", exc_info=e, stack_info=True)

WIFI_HANDLER = [
        (r"/wifi/config", WiFiConfigHandler),
        (r"/wifi/list", WiFiListHandler),
        (r"/wifi/connect", WiFiConnectHandler),
    ]
