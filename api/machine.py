import json
import subprocess

from hostname import HostnameManager
from log import MeticulousLogger
from machine import Machine
from wifi import WifiManager

from .api import API, APIVersion
from .base_handler import BaseHandler
from config import (
    MeticulousConfig,
    CONFIG_SYSTEM,
    MACHINE_COLOR,
    MACHINE_SERIAL_NUMBER,
    MACHINE_BATCH_NUMBER,
    MACHINE_BUILD_DATE,
)

logger = MeticulousLogger.getLogger(__name__)


class MachineInfoHandler(BaseHandler):
    def get(self):
        response = {}
        config = WifiManager.getCurrentConfig()
        response["name"] = HostnameManager.generateDeviceName()
        response["hostname"] = config.hostname

        if Machine.esp_info is not None:
            response["firmware"] = Machine.esp_info.firmwareV

        response["serial"] = ""
        if MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] is not None:
            response["serial"] = MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]

        response["color"] = ""
        if MeticulousConfig[CONFIG_SYSTEM][MACHINE_COLOR] is not None:
            response["color"] = MeticulousConfig[CONFIG_SYSTEM][MACHINE_COLOR]

        response["batch_number"] = ""
        if MeticulousConfig[CONFIG_SYSTEM][MACHINE_BATCH_NUMBER] is not None:
            response["batch_number"] = MeticulousConfig[CONFIG_SYSTEM][
                MACHINE_BATCH_NUMBER
            ]

        response["build_date"] = ""
        if MeticulousConfig[CONFIG_SYSTEM][MACHINE_BUILD_DATE] is not None:
            response["build_date"] = MeticulousConfig[CONFIG_SYSTEM][MACHINE_BUILD_DATE]

        self.write(json.dumps(response))


class MachineResetHandler(BaseHandler):

    def get(self):
        subprocess.run("rm -rf /meticulous-user/*")
        subprocess.run("reboot")


API.register_handler(APIVersion.V1, r"/machine", MachineInfoHandler)
API.register_handler(APIVersion.V1, r"/machine/factory_reset", MachineResetHandler)
