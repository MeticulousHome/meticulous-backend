import json

from config import (
    CONFIG_USER,
    MeticulousConfig,
    UPDATE_CHANNEL,
    MACHINE_HEATING_TIMEOUT,
)
from heater_actuator import HeaterActuator

from .base_handler import BaseHandler
from .api import API, APIVersion

from ota import UpdateManager
from log import MeticulousLogger

from timezone_manager import TimezoneManager

logger = MeticulousLogger.getLogger(__name__)

class SettingsHandler(BaseHandler):
    def get(self, setting_name=None):
        if setting_name:
            setting = MeticulousConfig[CONFIG_USER].get(setting_name)
            if setting is not None:
                response = {setting_name: setting}
                self.write(json.dumps(response))
            else:
                self.set_status(404)
                self.write(
                    {
                        "status": "error",
                        "error": "setting not found",
                        "setting": setting_name,
                    }
                )
        else:
            self.write(json.dumps(MeticulousConfig[CONFIG_USER]))

    def post(self):
        try:
            settings = json.loads(self.request.body)
        except json.decoder.JSONDecodeError as e:
            self.set_status(403)
            self.write(
                {"status": "error", "error": "invalid json", "json_error": f"{e}"}
            )
            return

        for setting_name in settings:
            value = settings.get(setting_name)
            setting = MeticulousConfig[CONFIG_USER].get(setting_name)
            if setting is not None:
                if type(value) is not type(setting):
                    self.set_status(404)
                    self.write(
                        {
                            "status": "error",
                            "error": "setting value invalidm, expected boolean",
                            "setting": setting_name,
                            "value": value,
                        }
                    )
                    MeticulousConfig.load()
                    return

                if setting_name == MACHINE_HEATING_TIMEOUT:
                    try:
                        HeaterActuator.set_timeout(value)
                    except ValueError as e:
                        error_message = str(e)
                        logger.warning(f"Invalid heater timeout value: {error_message}")
                        self.set_status(400)
                        self.write(
                            {
                                "status": "error",
                                "error": "invalid heater timeout value",
                                "details": error_message,
                            }
                        )
                        return

                MeticulousConfig[CONFIG_USER][setting_name] = value
            else:
                self.set_status(404)
                self.write(
                    {
                        "status": "error",
                        "error": "setting not found",
                        "setting": setting_name,
                    }
                )
                logger.info(f"Setting not found: {setting_name}")
                MeticulousConfig.load()
                return
        MeticulousConfig.save()
        UpdateManager.setChannel(MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL])
        return self.get()


class TimezoneUIProvider(BaseHandler):
    def get(self):
        self.write(TimezoneManager.get_organized_timezones())

API.register_handler(APIVersion.V1, r"/settings/*", SettingsHandler),
API.register_handler(APIVersion.V1, r"/settings/UI_timezones", TimezoneUIProvider),

