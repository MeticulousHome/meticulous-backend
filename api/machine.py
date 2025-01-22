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
from ota import UpdateManager
from backlight_controller import BacklightController

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
    __is_recovery_update: bool = False
    last_progress: float = 0
    last_status: OSStatus = OSStatus.IDLE
    last_extra_info: str = None

    __sio = None

    @classmethod
    def to_json(cls):
        extra_info_str = (
            f" : {cls.last_extra_info}"
            if cls.last_extra_info is not None and isinstance(cls.last_extra_info, str)
            else ""
        )
        return {
            "progress": round(cls.last_progress),
            "status": f"{OSStatus.to_string(cls.last_status)}",
            "info": extra_info_str,
        }

    def get(self):
        self.write(self.to_json())

    @classmethod
    def setSio(cls, sio):
        cls.__sio = sio

    @classmethod
    def markAsRecoveryUpdate(cls, is_recovery):
        cls.__is_recovery_update = is_recovery
        logger.info(
            "Marking update as"
            + (" not" if not cls.__is_recovery_update else "")
            + " recovery"
        )

    @classmethod
    def isRecoveryUpdate(cls):
        return cls.__is_recovery_update

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
                await cls.__sio.emit("OSUpdate", cls.to_json())

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
            response["mainVoltage"] = Machine.esp_info.mainVoltage

        serial = MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]
        if serial is None or serial == "":
            serial = Machine.generate_random_serial()
            MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] = serial
            MeticulousConfig.save()
        response["serial"] = serial

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

        software_version = UpdateManager.getBuildTimestamp()
        if software_version is not None:
            response["software_version"] = software_version.strftime(
                "%Y-%m-%d %H:%M:%S"
            )
        else:
            response["software_version"] = None

        self.write(json.dumps(response))


class MachineResetHandler(BaseHandler):

    def get(self):
        subprocess.run("rm -rf /meticulous-user/*")
        subprocess.run("reboot")


class MachineBacklightController(BaseHandler):
    def post(self):
        try:
            settings = json.loads(self.request.body)
        except json.decoder.JSONDecodeError as e:
            self.set_status(403)
            self.write(
                {"status": "error", "error": "invalid json", "json_error": f"{e}"}
            )
            return
        if "brightness" in settings:
            brightness = settings.get("brightness")

            if brightness == 1:
                logger.info("Dimming up")
                BacklightController.dim_up()
            else:
                logger.info("Dimming down")
                BacklightController.dim_down()

        else:
            self.set_status(400)
            self.write(
                {
                    "status": "error",
                    "error": "brightness value is required",
                }
            )
            return


API.register_handler(APIVersion.V1, r"/machine", MachineInfoHandler)
API.register_handler(APIVersion.V1, r"/machine/backlight", MachineBacklightController)
API.register_handler(APIVersion.V1, r"/machine/factory_reset", MachineResetHandler)
API.register_handler(APIVersion.V1, r"/machine/OS_update_status", UpdateOSStatus)
