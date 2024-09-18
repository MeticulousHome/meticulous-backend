import json
import pyqrcode
import io

from config import (
    MeticulousConfig,
    CONFIG_WIFI,
    WIFI_AP_NAME,
    WIFI_AP_PASSWORD,
    WIFI_KNOWN_WIFIS,
    WIFI_MODE,
    WIFI_MODE_AP,
    WIFI_MODE_CLIENT,
)
from wifi import WifiManager, WifiType
from ble_gatt import PORT

from .base_handler import BaseHandler
from .api import API, APIVersion

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class WiFiConfig:
    def __init__(self, mode=None, apName=None, apPassword=None):
        self.mode = mode
        self.apName = apName
        self.apPassword = apPassword

    def __repr__(self):
        return f"WiFiConfiguration(mode='{self.mode}', apName='{self.apName}', apPassword='{self.apPassword}')"

    @classmethod
    def from_json(cls, json_data):
        mode = json_data.get("mode")
        apName = json_data.get("apName")
        apPassword = json_data.get("apPassword")
        return cls(mode, apName, apPassword)

    def to_json(self):
        return {
            "mode": self.mode,
            "apName": self.apName,
            "apPassword": self.apPassword,
        }


class WiFiQRHandler(BaseHandler):
    def get(self):
        config = WifiManager.getCurrentConfig()
        qr_contents: str = ""
        if config.is_hotspot():
            ssid = MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME]
            password = MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD]
            qr_contents = f"WIFI:S:{ssid};T:WPA2;P:{password};H:true;;"
        elif len(config.ips) > 0:
            current_ip = config.ips[0]
            if current_ip.ip.version == 6:
                qr_contents = f"http://[{str(current_ip.ip)}]:{PORT}"
            else:
                qr_contents = f"http://{str(current_ip.ip)}:{PORT}"
        else:
            qr_contents = f"http://{str(config.hostname)}.local:{PORT}"

        buffer = io.BytesIO()

        qr = pyqrcode.create(qr_contents)
        qr.png(
            buffer,
            scale=8,
            quiet_zone=2,
            module_color=[0x00, 0x00, 0x00, 0xFF],
            background=[0xFF, 0xFF, 0xFF, 0xFF],
        )

        self.set_header("Content-Type", "image/png")
        self.write(buffer.getvalue())


class WiFiConfigHandler(BaseHandler):
    def get(self):
        mode = MeticulousConfig[CONFIG_WIFI][WIFI_MODE]
        apName = MeticulousConfig[CONFIG_WIFI][WIFI_AP_NAME]
        apPassword = MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD]
        wifi_config = {
            "config": WiFiConfig(mode, apName, apPassword).to_json(),
            "status": WifiManager.getCurrentConfig().to_json(),
            "known_wifis": MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS],
        }
        self.write(json.dumps(wifi_config))

    def post(self):
        try:
            config_changed = False
            data = json.loads(self.request.body)
            if "mode" in data and data["mode"] in [WIFI_MODE_AP, WIFI_MODE_CLIENT]:
                logger.info("Changing wifi mode")
                MeticulousConfig[CONFIG_WIFI][WIFI_MODE] = data["mode"]
                config_changed = True

            if "apPassword" in data:
                logger.info("Changing wifi ap password")
                MeticulousConfig[CONFIG_WIFI][WIFI_AP_PASSWORD] = data["apPassword"]
                config_changed = True

            if config_changed:
                MeticulousConfig.save()
                WifiManager.resetWifiMode()

            return self.get()
        except json.JSONDecodeError as e:
            self.set_status(400)
            self.write("Invalid JSON")
            logger.warning(f"Failed to parse passed JSON: {e}", stack_info=False)

        except Exception as e:
            self.set_status(400)
            self.write("Failed to write config")
            logger.warning(
                "Failed to accept passed config: ", exc_info=e, stack_info=True
            )


class WiFiListHandler(BaseHandler):
    def get(self):
        networks = dict()
        try:
            for s in WifiManager.scanForNetworks():
                if s.ssid is not None and s.ssid != "":
                    wifi_type = WifiType.from_nmcli_security(s.security)
                    if wifi_type is None:
                        continue
                    formated: dict = {
                        "type": wifi_type,
                        "security": s.security,
                        "ssid": s.ssid,
                        "signal": s.signal,
                        "rate": s.rate,
                        "in_use": s.in_use,
                    }
                    exists = networks.get(s.ssid)
                    # Make sure the network in use is always listed
                    if exists is None or s.in_use:
                        networks[s.ssid] = formated.copy()
                    else:
                        # Dont overwrite the in_use network
                        logger.info(f"{exists}, {exists.get('signal')}")
                        if exists["in_use"]:
                            continue
                        if s.signal > exists["signal"]:
                            networks[s.ssid] = formated
            response = sorted(
                networks.values(), key=lambda x: x["signal"], reverse=True
            )
            response = json.dumps(response)
            self.write(response)
        except Exception as e:
            self.set_status(400)
            self.write({"status": "error", "error": f"failed to fetch wifi list: {e}"})
            logger.warning(
                "Failed to fetch / format wifi list: ", exc_info=e, stack_info=True
            )


class WiFiConnectHandler(BaseHandler):
    def post(self):
        try:
            data = json.loads(self.request.body)
            success = WifiManager.connectToWifi(data)

            if success:
                self.write({"status": "ok"})
            else:
                self.set_status(400)
                self.write({"status": "error", "error": "failed to conect to wifi"})
        except Exception as e:
            self.set_status(400)
            self.write({"status": "error", "error": f"failed to conect to wifi: {e}"})
            logger.warning("Failed to connect: ", exc_info=e, stack_info=True)


class WiFiDeleteHandler(BaseHandler):
    def post(self):
        try:
            data = json.loads(self.request.body)
            ssid = data["ssid"]

            if ssid in MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS]:
                del MeticulousConfig[CONFIG_WIFI][WIFI_KNOWN_WIFIS][ssid]
                MeticulousConfig.save()
                self.write({"status": "ok"})
            else:
                self.set_status(400)
                self.write(
                    {"status": "error", "error": "failed to delete unknown wifi"}
                )
        except Exception as e:
            self.set_status(400)
            self.write({"status": "error", "error": f"failed to delete wifi: {e}"})
            logger.warning("Failed to connect: ", exc_info=e, stack_info=True)


API.register_handler(APIVersion.V1, r"/wifi/config", WiFiConfigHandler),
API.register_handler(APIVersion.V1, r"/wifi/config/qr.png", WiFiQRHandler),
API.register_handler(APIVersion.V1, r"/wifi/list", WiFiListHandler),
API.register_handler(APIVersion.V1, r"/wifi/connect", WiFiConnectHandler),
API.register_handler(APIVersion.V1, r"/wifi/delete", WiFiDeleteHandler),
