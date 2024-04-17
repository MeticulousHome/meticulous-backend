import json
from machine import Machine
from wifi import WifiManager
from .base_handler import BaseHandler
from .api import API, APIVersion

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)


class MachineInfoHandler(BaseHandler):
    def get(self):
        response = {}
        config = WifiManager.getCurrentConfig()
        response["name"] = config.hostname

        if Machine.esp_info is not None:
            response["firmware"] = Machine.esp_info.firmwareV

        response["color"] = "black"
        response["model_version"] = "v10.1.0"
        response["serial"] = "103"

        response["mock_response"] = True

        self.write(json.dumps(response))


API.register_handler(APIVersion.V1, r"/machine", MachineInfoHandler),
