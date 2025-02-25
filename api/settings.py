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
    SSH_ENABLED,
)

from heater_actuator import HeaterActuator
from ssh_manager import SSHManager

from .base_handler import BaseHandler
from .api import API, APIVersion

from ota import UpdateManager
from log import MeticulousLogger
from usb import USBManager
import copy
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

    def handle_manufacturing_status(self, value):
        try:
            if not isinstance(value, dict):
                return value

            current_config = MeticulousConfig.get("manufacturing", {})

            if (
                current_config.get("enable", True)
                and not value.get("enable", True)
                and value.get("first_boot", True)
            ):

                from ssh_manager import SSHManager

                logger.info(
                    "Manufacturing mode exit detected, generating root password"
                )

                # Generate password
                if not SSHManager.generate_root_password():
                    logger.error("Failed to generate root password")

                # Mark that it is no longer first_boot
                value["first_boot"] = False

            return value

        except Exception as e:
            logger.error(f"Error handling manufacturing update: {e}")
            return value

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

        MeticulousConfig.save()
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
