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

        complete_success: bool = True
        any_success: bool = False
        for setting_target in settings:
            value = settings.get(setting_target)
            setting = MeticulousConfig[CONFIG_USER].get(setting_target)
            if setting is not None:
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
                    save_value = False
                if setting_target == TIMEZONE_SYNC:
                    status = "Success"
                    if (
                        value == "automatic"
                        and MeticulousConfig[CONFIG_USER][TIMEZONE_SYNC] != "automatic"
                    ):

                        status = await TimezoneManager.request_and_sync_tz()
                        logger.debug(f"timezone endpoint status: {status}")

                    any_success = any_success or status == "Success"
                    self.set_status(
                        200
                        if (complete_success and status == "Success")
                        else 207 if (any_success and status == "Success") else 404
                    )

                    if status != "Success":
                        self.write(
                            {
                                "status": "error",
                                "setting": TIMEZONE_SYNC,
                                "details": f"{status}",
                            }
                        )
                        complete_success = False

                if setting_target == TIME_ZONE:
                    try:
                        status = TimezoneManager.update_timezone(value)

                    except UnicodeDecodeError as e:
                        logger.error(f"Failed setting the new timezone\n\t{e}")
                        status = f"failed to set new timezone: {e}"

                    any_success = any_success or status == "Success"
                    self.set_status(
                        200
                        if (complete_success and status == "Success")
                        else 207 if (any_success and status == "Success") else 404
                    )

                    if status != "Success":
                        complete_success = False
                        self.write(
                            {
                                "status": "error",
                                "setting": TIME_ZONE,
                                "details": f"{status}",
                            }
                        )
                    save_value = False  # the value is saved by the function TimezoneManager.update_timezone()

                if setting_target == MACHINE_HEATING_TIMEOUT:
                    try:
                        HeaterActuator.set_timeout(value)
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
                        complete_success = False
                        save_value = False

                    self.set_status(
                        200 if complete_success else 207 if any_success else 404
                    )

                if setting_target == USB_MODE:
                    try:
                        USB_MODES(value)
                        USBManager.setUSBMode(value)
                    except (ValueError, RuntimeError) as error:
                        self.set_status(400)
                        complete_success = False
                        self.write(
                            {
                                "status": "error",
                                "setting": USB_MODE,
                                "details": f"{error}",
                            }
                        )
                        logger.info(f"Invalid USB mode: {value}")
                        save_value = False

                # Handle SSH settings
                if setting_target == SSH_ENABLED:
                    try:
                        if SSHManager.set_ssh_state(value):
                            any_success = True
                        else:
                            complete_success = False
                            self.set_status(500)
                            self.write(
                                {
                                    "status": "error",
                                    "setting": SSH_ENABLED,
                                    "details": "Failed to update SSH service state",
                                }
                            )
                            save_value = False
                    except Exception as e:
                        logger.error(f"Error managing SSH service: {e}")
                        complete_success = False
                        self.set_status(500)
                        self.write(
                            {
                                "status": "error",
                                "setting": SSH_ENABLED,
                                "details": "Internal server error",
                            }
                        )
                        save_value = False

                if save_value:
                    MeticulousConfig[CONFIG_USER][setting_target] = value
            else:
                self.set_status(207 if any_success else 404)
                self.write(
                    {
                        "status": "error",
                        "setting": setting_target,
                        "details": "setting not found",
                    }
                )
                logger.info(f"Setting not found: {setting_target}")
        MeticulousConfig.save()
        UpdateManager.setChannel(MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL])
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


API.register_handler(APIVersion.V1, r"/settings[/]*(.*)", SettingsHandler),
API.register_handler(APIVersion.V1, r"/timezones/(.*)", TimezoneUIHandler),
