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
    SSH_ENABLED,
)

from heater_actuator import HeaterActuator
from ssh_manager import SSHManager

from .base_handler import BaseHandler
from .api import API, APIVersion

from ota import UpdateManager
from log import MeticulousLogger
from usb import USBManager

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

        complete_success = True
        any_success = False

        for setting_target in settings:
            value = settings.get(setting_target)
            setting = MeticulousConfig[CONFIG_USER].get(setting_target)

            if setting is None:
                self.set_status(207 if any_success else 404)
                self.write(
                    {
                        "status": "error",
                        "setting": setting_target,
                        "details": "setting not found",
                    }
                )
                logger.info(f"Setting not found: {setting_target}")
                continue

            save_value = True
            if type(value) is not type(setting):
                complete_success = False
                self.set_status(404 if not any_success else 207)
                self.write(
                    {
                        "status": "error",
                        "setting": setting_target,
                        "details": f"setting value invalid, received {type(value)} and expected {type(setting)}",
                    }
                )
                continue

            if setting_target == TIMEZONE_SYNC:
                status = await self._handle_timezone_sync(value)
                any_success = any_success or status == "Success"
                if status != "Success":
                    complete_success = False

            elif setting_target == TIME_ZONE:
                status = self._handle_time_zone(value)
                any_success = any_success or status == "Success"
                save_value = status == "Success"

            elif setting_target == MACHINE_HEATING_TIMEOUT:
                if not self._handle_heating_timeout(value):
                    complete_success = False
                    save_value = False

            elif setting_target == USB_MODE:
                if not self._handle_usb_mode(value):
                    complete_success = False
                    save_value = False

            elif setting_target == SSH_ENABLED:
                save_value, complete_success, any_success = self._handle_ssh_setting(
                    value
                )

            if save_value:
                MeticulousConfig[CONFIG_USER][setting_target] = value

        MeticulousConfig.save()
        UpdateManager.setChannel(MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL])
        return self.get()

    def _handle_ssh_setting(self, value):
        try:
            if SSHManager.set_ssh_state(value):
                return True, True, True
            else:
                self.set_status(500)
                self.write(
                    {
                        "status": "error",
                        "setting": SSH_ENABLED,
                        "details": "Failed to update SSH service state",
                    }
                )
                return False, False, False
        except Exception as e:
            logger.error(f"Error managing SSH service: {e}")
            self.set_status(500)
            self.write(
                {
                    "status": "error",
                    "setting": SSH_ENABLED,
                    "details": "Internal server error",
                }
            )
            return False, False, False

    async def _handle_timezone_sync(self, value):
        status = "Success"
        if (
            value == "automatic"
            and MeticulousConfig[CONFIG_USER][TIMEZONE_SYNC] != "automatic"
        ):
            status = await TimezoneManager.request_and_sync_tz()
            logger.debug(f"timezone endpoint status: {status}")
        return status

    def _handle_time_zone(self, value):
        try:
            status = TimezoneManager.update_timezone(value)
        except UnicodeDecodeError as e:
            logger.error(f"Failed setting the new timezone\n\t{e}")
            status = f"failed to set new timezone: {e}"
        return status

    def _handle_heating_timeout(self, value):
        try:
            HeaterActuator.set_timeout(value)
            return True
        except ValueError as e:
            error_message = str(e)
            logger.warning(f"Invalid heater timeout value: {error_message}")
            self.write(
                {
                    "status": "error",
                    "setting": MACHINE_HEATING_TIMEOUT,
                    "details": f"invalid heater timeout value: {error_message} ",
                }
            )
            return False

    def _handle_usb_mode(self, value):
        try:
            USB_MODES(value)
            USBManager.setUSBMode(value)
            return True
        except (ValueError, RuntimeError) as error:
            self.set_status(400)
            self.write(
                {
                    "status": "error",
                    "setting": USB_MODE,
                    "details": f"{error}",
                }
            )
            logger.info(f"Invalid USB mode: {value}")
            return False


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


API.register_handler(APIVersion.V1, r"/settings[/]*(.*)", SettingsHandler),
API.register_handler(APIVersion.V1, r"/timezones/(.*)", TimezoneUIHandler),
