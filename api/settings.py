import json

from config import (
    CONFIG_USER,
    MeticulousConfig,
    UPDATE_CHANNEL,
    MACHINE_HEATING_TIMEOUT,
    USB_MODE,
    USB_MODES,
    TIMEZONE_SYNC,
    TIME_ZONE,
    AUTOMATIC_TIMEZONE_SYNC,
)

from heater_actuator import HeaterActuator

from .base_handler import BaseHandler
from .api import API, APIVersion

from manufacturing import dial_schema, CONFIG_MANUFACTURING

from ota import UpdateManager
from log import MeticulousLogger
from usb import USBManager
import copy
from timezone_manager import TimezoneManager

from machine import Machine

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

    def validate_setting(self, setting_target, value):
        if setting_target not in MeticulousConfig[CONFIG_USER]:
            error_message = f"setting {setting_target} not found"
            raise KeyError(error_message)

        if type(value) is not type(MeticulousConfig[CONFIG_USER][setting_target]):
            error_message = f"setting value invalid, received {type(value)} and expected {type(setting_target)}"
            raise KeyError(error_message)

    async def update_timezone_sync(self, value) -> str:
        if value == AUTOMATIC_TIMEZONE_SYNC:
            try:
                new_tz = await TimezoneManager.request_and_sync_tz()
                return new_tz
            except Exception as e:
                error_message = f"failed to sync timezone: {e}"
                raise Exception(error_message)

    def update_timezone(self, value):
        try:
            TimezoneManager.update_timezone(value)
        except UnicodeDecodeError as e:
            error_message = f"failed to set new timezone: {e}"
            raise Exception(error_message)

    def update_heater_timeout(self, value):
        try:
            HeaterActuator.set_timeout(value)
        except ValueError as e:
            error_message = f"Invalid heater timeout value: {str(e)}"
            raise Exception(error_message)

    def update_usb_mode(self, value):
        try:
            USB_MODES(value)
            USBManager.setUSBMode(value)
        except (ValueError, RuntimeError) as e:
            error_message = f"Failed to set the USB mode: {str(e)}"
            raise Exception(error_message)

    async def post(self, setting_name=None):
        try:
            settings = json.loads(self.request.body)
        except json.decoder.JSONDecodeError as e:
            self.set_status(403)
            self.write(
                {"status": "error", "error": "invalid json", "json_error": f"{e}"}
            )
            return

        workConfig = copy.deepcopy(MeticulousConfig[CONFIG_USER])

        try:
            for setting_target in settings:
                value = settings.get(setting_target)

                self.validate_setting(setting_target, value)

                if setting_target == TIMEZONE_SYNC:
                    new_tz = await self.update_timezone_sync(value)
                    if new_tz is not None:
                        self.validate_setting(TIME_ZONE, new_tz)
                        workConfig[TIME_ZONE] = new_tz

                if setting_target == TIME_ZONE:
                    self.update_timezone(value)

                if setting_target == MACHINE_HEATING_TIMEOUT:
                    self.update_heater_timeout(value)

                if setting_target == USB_MODE:
                    self.update_usb_mode(value)

                if setting_target == UPDATE_CHANNEL:
                    UpdateManager.setChannel(value)

                # If we made it here without exception we can update the setting
                workConfig[setting_target] = value

        except KeyError as e:  # The variable is invalid in some way
            self.set_status(404)
            self.write({"status": "error", "error": f"{e}"})
            return

        except Exception as e:  # The variable specific callbacks could not be activated
            self.set_status(400)
            self.write({"status": "error", "error": f"{e}"})
            return

        MeticulousConfig[CONFIG_USER] = workConfig

        MeticulousConfig.save()  # ! Add mutex to protect access to it when writing?

        return self.get()


class TimezoneUIHandler(BaseHandler):

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


class ManufacturingSettingsHandler(BaseHandler):

    # When the dial request data from the endpoint it will provide the schema if
    # the machine is on manufacturing mode
    def get(self):
        if Machine.enable_manufacturing is False:
            self.set_status(204)  # Report no content
        else:
            self.set_status(200)
            self.write(dial_schema)  # Report the schema

    def validate_setting(self, setting_target, value):
        configuration = MeticulousConfig[CONFIG_MANUFACTURING]
        if setting_target not in configuration:
            error_message = f"setting {setting_target} not found"
            raise KeyError(error_message)

        if type(value) is not type(configuration[setting_target]):
            error_message = f"setting value invalid, received {type(value)} and expected {type(setting_target)}"
            raise KeyError(error_message)

    # the body is a json
    # {
    #   <key>: <value>
    # }
    def post(self):

        if Machine.enable_manufacturing is False:
            self.set_status(410)
            self.write({"status": "error", "error": "no configuration available"})
            return

        try:
            config = json.loads(self.request.body)
        except json.decoder.JSONDecodeError as e:
            self.set_status(403)
            self.write(
                {"status": "error", "error": "invalid json", "json_error": f"{e}"}
            )
            return

        workConfig = copy.deepcopy(MeticulousConfig[CONFIG_MANUFACTURING])

        try:
            for config_key in config:
                new_value = config.get(config_key)
                self.validate_setting(config_key, new_value)

                workConfig[config_key] = new_value

        except KeyError as e:  # The variable is invalid in some way
            self.set_status(404)
            self.write({"status": "error", "error": f"{e}"})
            return

        MeticulousConfig[CONFIG_MANUFACTURING] = workConfig
        self.set_status(200)
        self.write(json.dumps(MeticulousConfig[CONFIG_MANUFACTURING]))

        MeticulousConfig.save()  # ! Add mutex to protect access to it when writing?
        return


API.register_handler(APIVersion.V1, r"/manufacturing[/]*", ManufacturingSettingsHandler)
API.register_handler(APIVersion.V1, r"/settings[/]*(.*)", SettingsHandler),
API.register_handler(APIVersion.V1, r"/timezones/(.*)", TimezoneUIHandler),
