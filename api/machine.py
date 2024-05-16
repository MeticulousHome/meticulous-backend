import json
import subprocess

from hostname import HostnameManager
from log import MeticulousLogger
from machine import Machine
from wifi import WifiManager

from .api import API, APIVersion
from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)


class MachineInfoHandler(BaseHandler):
    def get(self):
        response = {}
        config = WifiManager.getCurrentConfig()
        response["name"] = HostnameManager.generateDeviceName()
        response["hostname"] = config.hostname

        if Machine.esp_info is not None:
            response["firmware"] = Machine.esp_info.firmwareV

        response["color"] = "black"
        response["model_version"] = "v10.1.0"
        response["serial"] = "103"

        response["mock_response"] = True

        self.write(json.dumps(response))


class MachineResetHandler(BaseHandler):

    def get(self):
        subprocess.run("rm -rf /meticulous-user/*")
        subprocess.run("reboot")


API.register_handler(APIVersion.V1, r"/machine", MachineInfoHandler)
API.register_handler(
    APIVersion.V1, r"/machine/factory_reset", MachineResetHandler)
