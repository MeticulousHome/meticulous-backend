from typing import Any
import yaml
import os
from pathlib import Path
from datetime import datetime
from mergedeep import merge

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

CONFIG_PATH = os.getenv("CONFIG_PATH", '/meticulous-user/config')

# Config Compontents
CONFIG_LOGGING = "logging"
CONFIG_SYSTEM = "system"
CONFIG_USER = "user"
CONFIG_WIFI = "wifi"

#
# SYSTEM config
#
## GATT configuration
GATT_DEFAULT_NAME = "MeticulousEspresso"
GATT_NAME = "gatt_device_name"

## HTTP Authentication configuration
HTTP_AUTH_KEY = "auth_key"
HTTP_DEFAULT_AUTH_KEY = "AAAABBBBCCCCDDDEEEFFFFGGGG"

HTTP_ALLOWED_NETWORKS = "always_allowed_networks"
HTTP_DEFAULT_ALLOWED_NETWORKS = []

## Notification Logic
NOTIFICATION_KEEPALIVE = "notifications_ttl"
NOTIFICATION_DEFAULT_KEEPALIVE = 3600

#
# USER config
#
## SOUND configuration
SOUNDS_ENABLED = "enable_sounds"
SOUNDS_DEFAULT_ENABLED = True

## Firmware pinning
DISALLOW_FIRMWARE_FLASHING = "disallow_firmware_flashing"
DISALLOW_FIRMWARE_FLASHING_DEFAULT = False

#
# LOGGING config
#
## Should all formated messages (sensors, data, ESPInfo, etc...) be logged
LOGGING_SENSOR_MESSAGES = "log_all_sensor_messages"
LOGGING_DEFAULT_SENSOR_MESSAGES = False

#
# WIFI related config items
#
## Wifi Config items
WIFI_MODE = "mode"
WIFI_MODE_AP = "AP"
WIFI_MODE_CLIENT = "CLIENT"

## Wifi access point configuration
WIFI_AP_NAME = "APName"
WIFI_DEFAULT_AP_NAME = "MeticulousEspresso"
WIFI_AP_PASSWORD = "APPassword"
WIFI_DEFAULT_AP_PASSWORD = "meticulous"



DefaultConfiguration_V1 = {
    # Only needs to be incremented in case of incompatible restructurings
    "version": 1,
    CONFIG_LOGGING: {
        LOGGING_SENSOR_MESSAGES: LOGGING_DEFAULT_SENSOR_MESSAGES
    },
    CONFIG_SYSTEM : {
        HTTP_AUTH_KEY: HTTP_DEFAULT_AUTH_KEY,
        HTTP_ALLOWED_NETWORKS: HTTP_DEFAULT_ALLOWED_NETWORKS,
        NOTIFICATION_KEEPALIVE: NOTIFICATION_DEFAULT_KEEPALIVE,
    },
    CONFIG_USER:{
        SOUNDS_ENABLED: SOUNDS_DEFAULT_ENABLED,
        DISALLOW_FIRMWARE_FLASHING: DISALLOW_FIRMWARE_FLASHING_DEFAULT,
    },
    CONFIG_WIFI: {
        WIFI_MODE: WIFI_MODE_AP,
        WIFI_AP_NAME: WIFI_DEFAULT_AP_NAME,
        WIFI_AP_PASSWORD: WIFI_DEFAULT_AP_PASSWORD
    },
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

        self.__path = Path(path)
        self.__configError = False

        ext = self.__path.suffix
        if ext not in [".yml", ".yaml"]:
            raise ValueError(
                f"Invalid Extension provided! YAML (yml / yaml) expected, {ext} found")

        self.load()

        logger.info("Config initialized")

        cs = yaml.dump(
            self.copy(),  default_flow_style=False, allow_unicode=True)
        for line in cs.split('\n'):
            logger.debug(f"CONF: {line}")
    def hasError(self):
        return self.__configError

    def load(self):
        if not Path(self.__path).exists():
            self.save()
            logger.info("Created new config")
        else:
            with open(self.__path, "r") as f:
                try:
                    disk_config = yaml.safe_load(f)
                    disk_version = disk_config.get("version")
                    if disk_version is not None and disk_version > self["version"]:
                        logger.warning("Config on disk is newer than this software")
                    merge(self, disk_config)
                    logger.info("Successfully loaded config from disk")
                    self.__configError = False
                except Exception as e:
                    logger.warning(f"Failed to load config: {e}")
                    basename, extension = os.path.splitext(self.__path)
                    backup_path = basename + "_broken_" +  datetime.now().strftime("%Y_%m_%d_%H_%M_%S") +extension
                    os.rename(self.__path, backup_path)
                    self.__configError = True
                self.save()

    def save(self):
        Path(self.__path).parent.mkdir(parents=True, exist_ok=True)
        with open(self.__path, "w") as f:
            yaml.dump(
                self.copy(), f, default_flow_style=False, allow_unicode=True)

MeticulousConfig = MeticulousConfigDict(os.path.join(CONFIG_PATH, "config.yml"), DefaultConfiguration_V1)
