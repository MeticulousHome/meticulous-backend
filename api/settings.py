import json

from config import (
    CONFIG_USER,
    MeticulousConfig,
    UPDATE_CHANNEL,
    MACHINE_HEATING_TIMEOUT,
    TIMEZONE_SYNC,
    TIME_ZONE,
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

    async def post(self, setting_name=None):
        try:
            settings = json.loads(self.request.body)
        except json.decoder.JSONDecodeError as e:
            self.set_status(403)
            self.write(
                {"status": "error", "error": "invalid json", "json_error": f"{e}"}
            )
            return

        for setting_target in settings:
            value = settings.get(setting_target)
            setting = MeticulousConfig[CONFIG_USER].get(setting_target)
            if setting is not None:
                if type(value) is not type(setting):
                    self.set_status(404)
                    self.write(
                        {
                            "status": "error",
                            "error": f"setting value invalid, expected {type(setting)}",
                            "setting": setting_target,
                            "value": value,
                        }
                    )
                    MeticulousConfig.load()
                    return
                if setting_target == TIMEZONE_SYNC:
                    status = "Success"
                    if (
                        value == "automatic"
                        and MeticulousConfig[CONFIG_USER][TIMEZONE_SYNC] != "automatic"
                    ):

                        status = await TimezoneManager.request_and_sync_tz()
                        logger.debug(f"timezone endpoint status: {status}")
                    MeticulousConfig[CONFIG_USER][setting_target] = value
                    MeticulousConfig.save()
                    self.set_status(200)
                    self.write({"status": status})
                    return

                if setting_target == TIME_ZONE:
                    try:
                        status = TimezoneManager.update_timezone(value)

                    except UnicodeDecodeError as e:
                        logger.error(f"Failed setting the new timezone\n\t{e}")
                        status = f"failed to set new timezone: {e}"

                    self.set_status(200 if status == "Success" else 400)
                    self.write({"status": f"{status}"})
                    MeticulousConfig[CONFIG_USER][setting_target] = value
                    return

                if setting_target == MACHINE_HEATING_TIMEOUT:
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

                MeticulousConfig[CONFIG_USER][setting_target] = value
            else:
                self.set_status(404)
                self.write(
                    {
                        "status": "error",
                        "error": "setting not found",
                        "setting": setting_target,
                    }
                )
                logger.info(f"Setting not found: {setting_target}")
                MeticulousConfig.load()
                return
        MeticulousConfig.save()
        UpdateManager.setChannel(MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL])
        return self.get()


class TimezoneUIProvider(BaseHandler):

    __timezone_map: dict = {}

    def get(self, region_type=None):
        if region_type is None or region_type == "":
            region_type = "countries"
        try:
            conditional_filter = self.get_argument("filter", "")
        except UnicodeDecodeError:
            self.set_status(403)
            self.write({"status": "error", "error": "String cannot be decoded"})
            return

        if not self.__timezone_map:
            self.__timezone_map = TimezoneManager.get_UI_timezones()

        return_array: list[str] = []
        error = ""
        match region_type:
            case "countries":
                return_array = [
                    country
                    for country in self.__timezone_map.keys()
                    if country.lower().startswith(conditional_filter)
                ]
            case "cities":
                cities_in_country: dict = self.__timezone_map.get(conditional_filter)
                if cities_in_country is not None:
                    return_array = [
                        {city: cities_in_country.get(city)}
                        for city in cities_in_country.keys()
                    ]
                else:
                    error = "invalid country requested"
            case _:
                error = "invalid region type requested"

        self.set_status(200 if error == "" else 403)
        self.write(
            {f"{region_type}": return_array}
            if error == ""
            else {"status": "error", "error": f"{error}"}
        )


API.register_handler(APIVersion.V1, r"/settings/(.*)", SettingsHandler),
API.register_handler(APIVersion.V1, r"/timezones/(.*)", TimezoneUIProvider),
