import json
import subprocess

from hostname import HostnameManager
from log import MeticulousLogger
from machine import Machine
from wifi import WifiManager
from enum import Enum
import asyncio

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


class OSStatus(Enum):
    IDLE = 0
    DOWNLOADING = 1
    INSTALLING = 2
    COMPLETE = 3
    FAILED = 4

    @classmethod
    def to_string(cls, status):
        mapping = {
            cls.IDLE: "IDLE",
            cls.DOWNLOADING: "DOWNLOADING",
            cls.INSTALLING: "INSTALLING",
            cls.COMPLETE: "COMPLETE",
            cls.FAILED: "FAILED",
        }
        return mapping.get(status, None)


class UpdateOSStatus(BaseHandler):
    last_progress: float = 0
    last_status: OSStatus = OSStatus.IDLE
    last_extra_info: str = None

    data = {"progress": 0, "status": "IDLE", "info": ""}

    __sio = None

    def get(self):
        self.write(self.data)

    @classmethod
    def setSio(cls, sio):
        cls.__sio = sio

    @classmethod
    def sendStatus(
        cls, current_status: OSStatus, current_progress: float, extra_info=None
    ):
        cls.last_progress = current_progress
        cls.last_status = current_status
        if cls.__sio:
            loop = (
                asyncio.get_event_loop()
                if asyncio.get_event_loop().is_running()
                else asyncio.new_event_loop()
            )
            asyncio.set_event_loop(loop)

            async def sendUpdateStatus():
                extra_info_str = (
                    f" : {extra_info}"
                    if extra_info is not None and isinstance(extra_info, str)
                    else ""
                )
                cls.data = {
                    "progress": cls.last_progress,
                    "status": f"{OSStatus.to_string(cls.last_status)}",
                    "info": extra_info_str,
                }
                await cls.__sio.emit("OSUpdate", cls.data)

            if not loop.is_running():
                logger.warning("sending OS Update status: no loop running")
                loop.run_until_complete(sendUpdateStatus())
            else:
                logger.warning("sending OS Update status: yes loop running")
                asyncio.create_task(sendUpdateStatus())

    @classmethod
    def sendLastStatus(cls):
        cls.sendStatus(cls.last_status, cls.last_progress)


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
API.register_handler(APIVersion.V1, r"/machine/OS_update_status", UpdateOSStatus)
