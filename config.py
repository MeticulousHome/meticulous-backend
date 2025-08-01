import asyncio
import os
import random
import string
from datetime import datetime
from pathlib import Path
from enum import Enum

import sentry_sdk
import yaml
import copy
from mergedeep import merge

from log import MeticulousLogger

from manufacturing import CONFIG_MANUFACTURING, Default_manufacturing_config

_config_logger = MeticulousLogger.getLogger(__name__)

CONFIG_PATH = os.getenv("CONFIG_PATH", "/meticulous-user/config")

# Config Compontents
CONFIG_LOGGING = "logging"
CONFIG_SYSTEM = "system"
CONFIG_USER = "user"
CONFIG_WIFI = "wifi"
CONFIG_PROFILES = "profiles"

#
# SYSTEM config
#

# HTTP Authentication configuration
HTTP_AUTH_KEY = "auth_key"
HTTP_DEFAULT_AUTH_KEY = "AAAABBBBCCCCDDDEEEFFFFGGGG"

HTTP_ALLOWED_NETWORKS = "always_allowed_networks"
HTTP_DEFAULT_ALLOWED_NETWORKS = []

# Notification Logic
NOTIFICATION_KEEPALIVE = "notifications_ttl"
NOTIFICATION_DEFAULT_KEEPALIVE = 3600

# Device name (e.g. ["Roasted", "Robusta"])
DEVICE_IDENTIFIER = "machine_name"
DEVICE_DEFAULT_IDENTIFIER = []

MACHINE_SERIAL_NUMBER = "serial"
MACHINE_DEFAULT_SERIAL_NUMBER = None

MACHINE_BATCH_NUMBER = "batch_number"
MACHINE_DEFAULT_BATCH_NUMBER = None

MACHINE_COLOR = "color"
MACHINE_DEFAULT_COLOR = None

MACHINE_BUILD_DATE = "build_date"
MACHINE_DEFAULT_BUILD_DATE = None

MACHINE_ALLOW_STAGE_SKIPPING = "allow_stage_skipping"
MACHINE_ALLOW_STAGE_SKIPPING_DEFAULT = False
# Time zone
TIMEZONE_SYNC = "timezone_sync"
AUTOMATIC_TIMEZONE_SYNC = "automatic"
DEFAULT_TIMEZONE_SYNC = AUTOMATIC_TIMEZONE_SYNC

TIME_ZONE = "time_zone"
DEFAULT_TIME_ZONE = None

ROOT_PASSWORD = "root_password"
ROOT_PASSWORD_DEFAULT = None

LAST_SYSTEM_VERSIONS = "last_system_versions"
LAST_SYSTEM_VERSIONS_DEFAULT = []

#
# USER config
#

# SOUND configuration
SOUNDS_ENABLED = "enable_sounds"
SOUNDS_DEFAULT_ENABLED = True

SOUNDS_THEME = "sounds_theme"
SOUNDS_DEFAULT_THEME = "default"

# SSH configuration
SSH_ENABLED = "ssh_enabled"
SSH_DEFAULT_ENABLED = True

# Firmware pinning
DISALLOW_FIRMWARE_FLASHING = "disallow_firmware_flashing"
DISALLOW_FIRMWARE_FLASHING_DEFAULT = False

# Hidden UI features
DISABLE_UI_FEATURES = "disable_ui_features"
DISABLE_UI_FEATURES_DEFAULT = False

# DEBUG
DEBUG_SHOT_DATA_RETENTION = "debug_shot_data_retention_days"
DEBUG_SHOT_DATA_RETENTION_DEFAULT = 31

ALLOW_LEGACY_JSON = "allow_legacy_json"
ALLOW_LEGACY_JSON_DEFAULT = False

# Updates
UPDATE_CHANNEL = "update_channel"
UPDATE_CHANNEL_DEFAULT = "stable"

# Brewing
PROFILE_AUTO_START = "auto_start_shot"
PROFILE_AUTO_START_DEFAULT = False
PROFILE_AUTO_PURGE = "auto_purge_after_shot"
PROFILE_AUTO_PURGE_DEFAULT = False
PROFILE_PARTIAL_RETRACTION = "partial_retraction"
PROFILE_PARTIAL_RETRACTION_DEFAULT = 45

MACHINE_HEATING_TIMEOUT = "heating_timeout"
MACHINE_HEATING_TIMEOUT_DEFAULT = 10  # minutes

MACHINE_HEAT_ON_BOOT = "heat_on_boot"
MACHINE_HEAT_ON_BOOT_DEFAULT = True

MACHINE_DEBUG_SENDING = "allow_debug_sending"
MACHINE_DEBUG_SENDING_DEFAULT = None

PROFILE_ORDER = "profile_order"
PROFILE_ORDER_DEFAULT = []

# UI CONFIG
IDLE_SCREEN = "idle_screen"
IDLE_SCREEN_DEFAULT = "default"

REVERSE_SCROLLING = "reverse_scrolling"
REVERSE_SCROLLING_DEFAULT = {
    "home": False,
    "keyboard": False,
    "menus": False,
}

HOSTNAME_OVERRIDE = "hostname_override"
HOSTNAME_OVERRIDE_DEFAULT = None


class USB_MODES(Enum):
    CLIENT = "client"  # Client mode with network OTG
    HOST = "host"  # OTG / HOST mode without networking
    DUAL = "dual_role"  # Dual role config


USB_MODE = "usb_mode"
USB_MODE_DEFAULT = USB_MODES.HOST.value

#
# LOGGING config
#
# Should all formated messages (sensors, data, ESPInfo, etc...) be logged
LOGGING_SENSOR_MESSAGES = "log_all_sensor_messages"
LOGGING_DEFAULT_SENSOR_MESSAGES = False

#
# WIFI related config items
#
# Wifi Config items
WIFI_MODE = "mode"
WIFI_MODE_AP = "AP"
WIFI_MODE_CLIENT = "CLIENT"

# Wifi access point configuration
WIFI_AP_NAME = "APName"
WIFI_DEFAULT_AP_NAME = "MeticulousEspresso"
WIFI_AP_PASSWORD = "APPassword"

# Could be out of string.ascii_letters, string.digits, string.punctuation
wifi_allowed_characters = string.digits

WIFI_DEFAULT_AP_PASSWORD = "".join(random.choices(wifi_allowed_characters, k=12))

WIFI_KNOWN_WIFIS = "KnownWifis"
WIFI_DEFAULT_KNOWN_WIFIS = dict()

#
# Profiling related config items and persistency
#
PROFILE_LAST = "LastProfile"
PROFILE_DEFAULT_LAST = None


DefaultConfiguration_V1 = {
    # Only needs to be incremented in case of incompatible restructurings
    "version": 1,
    CONFIG_LOGGING: {LOGGING_SENSOR_MESSAGES: LOGGING_DEFAULT_SENSOR_MESSAGES},
    CONFIG_SYSTEM: {
        HTTP_AUTH_KEY: HTTP_DEFAULT_AUTH_KEY,
        HTTP_ALLOWED_NETWORKS: HTTP_DEFAULT_ALLOWED_NETWORKS,
        NOTIFICATION_KEEPALIVE: NOTIFICATION_DEFAULT_KEEPALIVE,
        SOUNDS_THEME: SOUNDS_DEFAULT_THEME,
        DEVICE_IDENTIFIER: DEVICE_DEFAULT_IDENTIFIER,
        MACHINE_SERIAL_NUMBER: MACHINE_DEFAULT_SERIAL_NUMBER,
        MACHINE_BATCH_NUMBER: MACHINE_DEFAULT_BATCH_NUMBER,
        MACHINE_BUILD_DATE: MACHINE_DEFAULT_BUILD_DATE,
        MACHINE_COLOR: MACHINE_DEFAULT_COLOR,
        ROOT_PASSWORD: ROOT_PASSWORD_DEFAULT,
        LAST_SYSTEM_VERSIONS: LAST_SYSTEM_VERSIONS_DEFAULT,
    },
    CONFIG_USER: {
        SOUNDS_ENABLED: SOUNDS_DEFAULT_ENABLED,
        DISALLOW_FIRMWARE_FLASHING: DISALLOW_FIRMWARE_FLASHING_DEFAULT,
        DISABLE_UI_FEATURES: DISABLE_UI_FEATURES_DEFAULT,
        DEBUG_SHOT_DATA_RETENTION: DEBUG_SHOT_DATA_RETENTION_DEFAULT,
        PROFILE_AUTO_START: PROFILE_AUTO_START_DEFAULT,
        PROFILE_AUTO_PURGE: PROFILE_AUTO_PURGE_DEFAULT,
        PROFILE_PARTIAL_RETRACTION: PROFILE_PARTIAL_RETRACTION_DEFAULT,
        MACHINE_HEAT_ON_BOOT: MACHINE_HEAT_ON_BOOT_DEFAULT,
        MACHINE_HEATING_TIMEOUT: MACHINE_HEATING_TIMEOUT_DEFAULT,
        UPDATE_CHANNEL: UPDATE_CHANNEL_DEFAULT,
        IDLE_SCREEN: IDLE_SCREEN_DEFAULT,
        REVERSE_SCROLLING: REVERSE_SCROLLING_DEFAULT,
        ALLOW_LEGACY_JSON: ALLOW_LEGACY_JSON_DEFAULT,
        MACHINE_ALLOW_STAGE_SKIPPING: MACHINE_ALLOW_STAGE_SKIPPING_DEFAULT,
        USB_MODE: USB_MODE_DEFAULT,
        TIMEZONE_SYNC: DEFAULT_TIMEZONE_SYNC,
        TIME_ZONE: DEFAULT_TIME_ZONE,
        MACHINE_DEBUG_SENDING: MACHINE_DEBUG_SENDING_DEFAULT,
        SSH_ENABLED: SSH_DEFAULT_ENABLED,
        PROFILE_ORDER: PROFILE_ORDER_DEFAULT,
        HOSTNAME_OVERRIDE: HOSTNAME_OVERRIDE_DEFAULT,
    },
    CONFIG_WIFI: {
        WIFI_MODE: WIFI_MODE_AP,
        WIFI_AP_NAME: WIFI_DEFAULT_AP_NAME,
        WIFI_AP_PASSWORD: WIFI_DEFAULT_AP_PASSWORD,
        WIFI_KNOWN_WIFIS: WIFI_DEFAULT_KNOWN_WIFIS,
    },
    CONFIG_PROFILES: {PROFILE_LAST: PROFILE_DEFAULT_LAST},
    CONFIG_MANUFACTURING: copy.deepcopy(Default_manufacturing_config),
}


class MeticulousConfigDict(dict):
    """
    A class that extends the functionality of a standard dictionary to support
    reading from and writing to a YAML configuration file on the disk.

    Attributes:
        __path (Path): The file path for the configuration file.
        __configError (bool): Flag to indicate if there's an error in the configuration.

    Args:
        path (str, optional): The path to the YAML configuration file. Defaults to "./config/config.yml".

    Raises:
        ValueError: If the provided file extension is not .yml or .yaml.

    Methods:
        load(): Loads the configuration from the file specified by __path. If the file doesn't exist,
                it calls save() to create a new one. In case of a loading error, it backs up the
                current file and sets __configError to True.

        save(): Saves the current configuration to the file specified by __path. It creates the
                directory if it doesn't exist. The configuration is saved in YAML format, and the
                saved string is printed to the console for verification.

    Example:
        >>> default_config = { "key1" : "value1" }
        >>> config_dict = MeticulousConfigDict("./my_config.yml", default_config)
        >>> config_dict["new_key"] = "new_value"
        >>> config_dict.save()
    """

    def __init__(self, path, default_dict={}) -> None:
        super().__init__(default_dict)

        # Make attributes inheritable
        self.__path = Path(path)
        self.__configError = False
        self.__sio = None

        ext = self.__path.suffix
        if ext not in [".yml", ".yaml"]:
            raise ValueError(
                f"Invalid Extension provided! YAML (yml / yaml) expected, {ext} found"
            )

        self.load()

        _config_logger.info("Config initialized")

        cs = yaml.dump(self.copy(), default_flow_style=False, allow_unicode=True)
        for line in cs.split("\n"):
            _config_logger.debug(f"CONF: {line}")

        sentry_sdk.set_context("config", self.copy())

    # FIXME: Remove once the socket IO server lives in its own file
    def setSIO(self, sio):
        self.__sio = sio

    def hasError(self):
        return self.__configError

    def load(self):

        if not Path(self.__path).exists():
            self.save()
            _config_logger.info("Created new config")
        else:
            with open(self.__path, "r") as f:
                try:
                    disk_config = yaml.safe_load(f)
                    disk_version = disk_config.get("version")
                    if disk_version is not None and disk_version > self["version"]:
                        _config_logger.warning(
                            "Config on disk is newer than this software"
                        )
                    merge(self, disk_config)
                    _config_logger.info("Successfully loaded config from disk")
                    self.__configError = False
                except Exception as e:
                    _config_logger.warning(f"Failed to load config: {e}")
                    basename, extension = os.path.splitext(self.__path)
                    backup_path = (
                        basename
                        + "_broken_"
                        + datetime.now().strftime("%Y_%m_%d_%H_%M_%S")
                        + extension
                    )
                    os.rename(self.__path, backup_path)
                    self.__configError = True
                self.save()

    def save(self):
        sentry_sdk.set_context("config", self.copy())

        Path(self.__path).parent.mkdir(parents=True, exist_ok=True)
        with open(self.__path, "w") as f:
            yaml.dump(self.copy(), f, default_flow_style=False, allow_unicode=True)

        if self.__sio:

            # when running in an executor asyncio.get_event_loop() might fail, as there might
            # not be a loop in the thread created by asyncio
            try:
                loop = (
                    asyncio.get_event_loop()
                    if asyncio.get_event_loop().is_running()
                    else asyncio.new_event_loop()
                )
            except RuntimeError:
                loop = asyncio.new_event_loop()

            asyncio.set_event_loop(loop)

            async def sendSettingsNotification():
                await self.__sio.emit("settings", {})

            if not loop.is_running():
                loop.run_until_complete(sendSettingsNotification())
            else:
                asyncio.create_task(sendSettingsNotification())


MeticulousConfig = MeticulousConfigDict(
    os.path.join(CONFIG_PATH, "config.yml"), DefaultConfiguration_V1
)
