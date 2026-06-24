import json

from lab_tools import LabTools
from log import MeticulousLogger
from machine import Machine

from .api import API, APIVersion
from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)


class LabMotorHeaterHandler(BaseHandler):
    def get(self):
        self.write(
            {
                "status": "ok",
                "running": LabTools.active_profile_id is not None and not Machine.is_idle,
                "active_profile_id": LabTools.active_profile_id,
            }
        )

    def post(self):
        try:
            data = json.loads(self.request.body)
            request = LabTools.parse_motor_heater_request(data)
            profile = LabTools.start_motor_heater(request)
            self.write({"status": "ok", "profile": {"name": profile["name"], "id": profile["id"]}})
        except ValueError as e:
            self.set_status(400)
            self.write({"status": "error", "error": str(e)})
        except RuntimeError as e:
            self.set_status(409)
            self.write({"status": "error", "error": str(e)})
        except Exception as e:
            self.set_status(500)
            self.write({"status": "error", "error": f"failed to start lab control: {e}"})
            logger.warning("Failed to start lab motor/heater control", exc_info=e, stack_info=True)


class LabMotorHeaterStopHandler(BaseHandler):
    def post(self):
        LabTools.stop()
        self.write({"status": "ok"})


API.register_handler(APIVersion.V1, r"/lab/motor-heater", LabMotorHeaterHandler)
API.register_handler(APIVersion.V1, r"/lab/motor-heater/stop", LabMotorHeaterStopHandler)
