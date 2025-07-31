import os
import json
import subprocess
from log import MeticulousLogger
from config import (
    MeticulousConfig,
    CONFIG_USER,
    TIME_ZONE,
    TIMEZONE_SYNC,
    DEFAULT_TIME_ZONE,
)
import asyncio
from datetime import datetime

logger = MeticulousLogger.getLogger(__name__)

TIMEZONE_JSON_FILE_PATH: str = os.getenv(
    "TIMEZONE_JSON_FILE_PATH", "/usr/share/zoneinfo/UI_timezones.json"
)


class TimezoneManagerError(Exception):
    pass


class TimezoneManager:

    __system_timezone: str = ""
    __system_synced: bool = False

    @staticmethod
    def init():
        TimezoneManager.validate_timezones_json()
        TimezoneManager.__system_timezone = TimezoneManager.get_system_timezone()
        target_timezone = MeticulousConfig[CONFIG_USER][TIME_ZONE]
        if target_timezone != TimezoneManager.__system_timezone:

            if target_timezone == DEFAULT_TIME_ZONE:
                logger.info("No timezone was ever configured by user")
                logger.info("Setting system timezone to Etc/UTC as a default")
                target_timezone = "Etc/UTC"
                MeticulousConfig[CONFIG_USER][TIME_ZONE] = target_timezone
            else:
                logger.warning(
                    f"user config and system timezones confilct, updating system config to {MeticulousConfig[CONFIG_USER][TIME_ZONE]}"
                )

            # Try to set the system_timezone to the user specified tz
            try:
                TimezoneManager.set_system_timezone(target_timezone)
            except TimezoneManagerError as e:
                # If fails, set the system_timezone as the user timezone and report the error
                logger.error(
                    f"failed to set system TZ, syncing user TZ with system to {TimezoneManager.__system_timezone}. Error: {e}"
                )
                MeticulousConfig[CONFIG_USER][
                    TIME_ZONE
                ] = TimezoneManager.__system_timezone
            finally:
                MeticulousConfig.save()

    @staticmethod
    def update_timezone(new_timezone: str) -> None:
        stripped_new_tz = new_timezone.rstrip('"').lstrip('"')
        if MeticulousConfig[CONFIG_USER][TIME_ZONE] != stripped_new_tz:
            try:
                TimezoneManager.set_system_timezone(stripped_new_tz)
                logger.debug("update timezone status: Success")
            except TimezoneManagerError as e:
                logger.error(f"Error updating timezone: {e}")
                raise TimezoneManagerError(f"Error updating timezone: {e}")

    @staticmethod
    def set_system_timezone(new_timezone: str) -> str:
        logger.debug("setting system timezone")
        try:
            command = f"timedatectl set-timezone {new_timezone}"
            cmd_result = subprocess.run(
                command,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                check=True,
                shell=True,
            )

            if len(cmd_result.stderr) > 0 or len(cmd_result.stdout) > 0:
                error = f"[ Out:{cmd_result.stdout} | Err: {cmd_result.stderr} ]"
                raise Exception(error)

            logger.info(
                f"new system time zone: {TimezoneManager.get_system_timezone()}"
            )
        except Exception as e:
            message = f"Error setting system time zone: {e}"
            logger.error(message)
            raise TimezoneManagerError(message)

    @staticmethod
    def get_system_timezone():
        system_timezone = ""
        try:
            command = (
                "timedatectl status | grep 'Time zone' | awk -F'[:()]' '{print $2}'"
            )
            cmd_result = subprocess.run(
                command,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                check=True,
                shell=True,
            )

            if len(cmd_result.stderr) > 0:
                logger.error("error getting system time zone, using last saved by user")

            system_timezone = cmd_result.stdout.lstrip().rstrip()
            return system_timezone

        except Exception:
            logger.error("error getting system time zone, using last saved by user")

    @staticmethod
    def validate_timezones_json():
        if os.path.isfile(TIMEZONE_JSON_FILE_PATH):
            return
        logger.warning("Json file for UI timezones does not exist, generating")
        try:
            with open(TIMEZONE_JSON_FILE_PATH, "w") as json_file:
                json_file.write(json.dumps(TimezoneManager.get_organized_timezones()))
        except Exception as e:
            logger.error(f"Error generating json file: {e}")

    @staticmethod
    def get_organized_timezones() -> dict:
        """
        make use of linux embedded timezone information to compile and provide
        the user information to make the timezone selection easier

        return: json containing timezones grouped by country and identified by zone

        i.e

        {
        ...
            "Mexico":{
                "Bahia Banderas": "America/Bahia_Banderas",
                "Cancun": "America/Cancun",
                "Chihuahua": "America/Chihuahua",
                "Ciudad Juarez": "America/Ciudad_Juarez",
                ...
            }
        }
        """
        # Read the country -> timezones mapping from /usr/share/zoneinfo/zone.tab
        country_to_timezones = {}

        # Create a map from country code to country name
        country_code_to_name = {}

        # map country code to country name
        with open("/usr/share/zoneinfo/iso3166.tab") as f:
            for line in f:
                # Skip comments and empty lines
                if line.startswith("#") or not line.strip():
                    continue
                parts = line.split("\t")
                country_code = parts[0]
                country_name = parts[1].rstrip("\n")
                country_code_to_name.setdefault(country_code, country_name)

        # map timezones to country code
        with open("/usr/share/zoneinfo/zone.tab") as f:
            for line in f:
                # Skip comments and empty lines
                if line.startswith("#") or not line.strip():
                    continue
                parts = line.split("\t")
                country_code = parts[0]
                timezone = parts[2].rstrip()
                country_name = country_code_to_name.get(country_code, "Unknown Country")
                country_to_timezones.setdefault(country_name, []).append(timezone)

        # Group timezones by country
        country_timezones = {}
        try:
            result = subprocess.run(
                ["timedatectl", "list-timezones"],
                stdout=subprocess.PIPE,  # Capture the standard output
                stderr=subprocess.PIPE,  # Capture the standard error
                text=True,  # Decode output as a string
                check=True,  # Raise an exception on non-zero exit codes
            )
            _all_timezones = result.stdout.splitlines()

            for tz in _all_timezones:
                fallback_subregion = tz.split("/")[
                    0
                ]  # if there is no sub-region, take the region as sub region
                zone = tz.split("/")[1:]  # Skip the first segment (Region)

                # separate words liked by "_"
                zone = [" ".join(zone[i].split("_")) for i in range(len(zone))]

                if zone.__len__() > 0:
                    sub_region = " - ".join(
                        zone
                    )  # Combine state/city into "state - city" format
                else:
                    sub_region = "".join(
                        fallback_subregion
                    )  # Combine state/city into "state - city" format

                if tz.startswith("Etc/"):
                    country_timezones.setdefault("ETC", {})[sub_region] = tz
                elif tz in sum(
                    country_to_timezones.values(), []
                ):  # Flatten the country -> timezone mapping
                    for country, timezones in country_to_timezones.items():
                        if tz in timezones:
                            # if the first sub-region is the same as the country (Argentina, looking at You) don't
                            # keep that as subregion
                            if zone[0] == country and len(zone) > 1:
                                sub_region = " - ".join(zone[1:])
                            country_timezones.setdefault(country, {})[sub_region] = tz
                            break
                else:
                    country_timezones.setdefault("LINKS", {})[sub_region] = tz

            return country_timezones

        except Exception:
            logger.error("Could not fetch timezones from system")
            return {}

    @staticmethod
    def get_UI_timezones() -> dict:
        try:
            UI_TIMEZONES_JSON: dict = {}
            TimezoneManager.validate_timezones_json()
            with open(TIMEZONE_JSON_FILE_PATH, "r") as json_file:
                UI_TIMEZONES_JSON = json.loads(json_file.read())
                return UI_TIMEZONES_JSON
        except Exception:
            logger.error(
                "Json file for UI timezones does not exist and could not be created"
            )
            return {}

    @staticmethod
    def tz_background_update():
        tz_config = MeticulousConfig[CONFIG_USER][TIMEZONE_SYNC]
        if tz_config == "automatic" and not TimezoneManager.__system_synced:
            try:
                logger.info(
                    "Timezone is set to automatic, fetching timezone in the background"
                )
                loop = asyncio.get_event_loop()
                loop.run_until_complete(TimezoneManager.request_and_sync_tz())
            except Exception as e:
                logger.error(f"Error while fetching timezone in the background: {e}")

    @staticmethod
    async def request_and_sync_tz() -> str:

        async def request_tz_task() -> str:
            # nonlocal error
            import aiohttp

            TZ_GETTER_URL = "https://analytics.meticulousespresso.com/timezone_ip"

            async with aiohttp.ClientSession() as session:
                while True:  # Retry loop
                    await asyncio.sleep(1)
                    async with session.get(TZ_GETTER_URL) as response:
                        if response.status == 200:  # Break on success
                            str_content = await response.text()
                            tz = json.loads(str_content).get("tz")
                            if tz is not None:
                                TimezoneManager.update_timezone(
                                    tz
                                )  # raises TimezoneManagerError if fails
                                TimezoneManager.__system_synced = True
                                return tz
                            else:
                                logger.warning("No timezone known for this IP")
                                TimezoneManager.__system_synced = True
                                return None
                        else:
                            logger.warning(
                                f"timezone fetch failed with status code: {response.status}, re-fetching"
                            )

        try:
            new_tz = await asyncio.wait_for(request_tz_task(), timeout=20)
            return new_tz
        except asyncio.TimeoutError:
            logger.error(
                "time out error, server could not be contacted or took too long to answer"
            )
            raise Exception(
                "time out error, server could not be contacted or took too long to answer"
            )
        except TimezoneManagerError as e:
            logger.error(f"failed to set the provided timezone\n\t{e}")
            raise Exception("failed to set the provided timezone")

    @staticmethod
    def set_system_datetime(newDate: datetime):
        """This will raise an exception if the date command fails."""

        # Format the datetime in a form acceptable for the date command.
        formatted_date = newDate.strftime("%Y-%m-%d %H:%M:%S")
        logger.info("Setting system date to %s", formatted_date)
        cmd = ["date", "--set", formatted_date]

        subprocess.check_call(cmd)
