import json
import subprocess

from hostname import HostnameManager
from log import MeticulousLogger
from machine import Machine
from wifi import WifiManager

from .api import API, APIVersion
from .base_handler import BaseHandler
from config import MeticulousConfig, CONFIG_SYSTEM, MACHINE_COLOR, MACHINE_SERIAL_NUMBER
logger = MeticulousLogger.getLogger(__name__)


class MachineInfoHandler(BaseHandler):
    def get(self):
        response = {}
        config = WifiManager.getCurrentConfig()
        response["name"] = HostnameManager.generateDeviceName()
        response["hostname"] = config.hostname
        response["color"] = ""
        response["serial"] = ""
        response["model_version"] = "v10.1.0"

        if Machine.esp_info is not None:
            response["firmware"] = Machine.esp_info.firmwareV

        if MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] is not None:
            response["serial"] = MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]
            
        if MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] is not None:
            response["color"] = MeticulousConfig[CONFIG_SYSTEM][MACHINE_COLOR]

        self.write(json.dumps(response))


class MachineResetHandler(BaseHandler):

    def get(self):
        subprocess.run("rm -rf /meticulous-user/*")
        subprocess.run("reboot")


API.register_handler(APIVersion.V1, r"/machine", MachineInfoHandler)
API.register_handler(
    APIVersion.V1, r"/machine/factory_reset", MachineResetHandler)
