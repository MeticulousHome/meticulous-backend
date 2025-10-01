from enum import Enum, auto
import os
import json
from play_sound import playsound
import gpiod
import subprocess

from log import MeticulousLogger
from config import (
    MeticulousConfig,
    CONFIG_USER,
    CONFIG_SYSTEM,
    SOUNDS_THEME,
    SOUNDS_ENABLED,
)

logger = MeticulousLogger.getLogger(__name__)

USER_SOUNDS = os.getenv("USER_SOUNDS", "/meticulous-user/sounds")
SYSTEM_SOUNDS = os.getenv("SYSTEM_SOUNDS", "/opt/meticulous-backend/sounds")


class Sounds(Enum):
    STARTUP = auto()
    HEATING_START = auto()
    HEATING_END = auto()
    BREWING_START = auto()
    BREWING_END = auto()
    ABORT = auto()
    IDLE = auto()
    NOTIFICATION = auto()


AUDIO_ENABLE_GPIO_CHIP = 4
AUDIO_ENABLE_GPIO_PIN = 25


class SoundPlayer:
    SUPPORTED_FORMATS = [".mp3", ".wav", ".ogg", ".flac"]
    DEFAULT_THEME_NAME = "default"
    KNOWN_THEMES = None

    CURRENT_THEME_CONFIG = {}
    CURRENT_THEME_NAME = ""
    DEFAULT_THEME_CONFIG = {}

    _audio_pin = None

    @staticmethod
    def init(emulation=False, play_startup_sound=True):
        if not emulation:
            config = gpiod.line_request()
            config.consumer = __name__
            config.request_type = gpiod.line_request.DIRECTION_OUTPUT
            try:
                chip = gpiod.chip(AUDIO_ENABLE_GPIO_CHIP)
                SoundPlayer._audio_pin = chip.get_line(AUDIO_ENABLE_GPIO_PIN)
                SoundPlayer._audio_pin.request(config)
                SoundPlayer._audio_pin.set_value(1)
            except Exception as e:
                logger.error(f"Failed to set GPIO pin: {e}")

        # Theme detection
        themes = {}

        system_themes = SoundPlayer.scan_folder(SYSTEM_SOUNDS)

        if USER_SOUNDS != "":
            user_themes = SoundPlayer.scan_folder(USER_SOUNDS)
            themes.update(user_themes)

        themes.update(system_themes)

        SoundPlayer.KNOWN_THEMES = themes
        SoundPlayer.DEFAULT_THEME_CONFIG = SoundPlayer._load_theme(
            SoundPlayer.DEFAULT_THEME_NAME
        )

        try:
            subprocess.run(["pactl", "--", "set-sink-volume", "0", "100%"])
        except Exception as e:
            logger.error(f"failed to set audio volume: {e}")

        SoundPlayer.set_theme(MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME])
        if play_startup_sound:
            SoundPlayer.play_event_sound(Sounds.STARTUP)

    @staticmethod
    def availableThemes():
        if SoundPlayer.KNOWN_THEMES is None:
            return {}
        return SoundPlayer.KNOWN_THEMES

    @staticmethod
    def scan_folder(folder_path):
        themefolders = {}
        logger.info(f"Scanning Theme folder {folder_path}")
        for root, dirs, files in os.walk(folder_path):
            for dir in dirs:
                subfolder_path = os.path.join(root, dir)
                if SoundPlayer._has_config_file(subfolder_path):
                    themefolders[dir] = subfolder_path
                    logger.info(f"Found sound theme '{subfolder_path}'")
        return themefolders

    @staticmethod
    def _has_config_file(subfolder_path):
        try:
            config_path = os.path.join(subfolder_path, "config.json")
            with open(config_path) as f:
                json.load(f)
                return True
        except Exception:
            logger.info(f"'{subfolder_path}' has no config.json")
            pass
        return False

    @staticmethod
    def set_theme(theme_name):
        """
        Set the current theme if it exists.
        :param theme_name: The name of the theme to set as the current theme.
        """
        if SoundPlayer.KNOWN_THEMES is None:
            logger.warning(
                "Manipulating theme before SoundPlayer was initialized. Ignoring."
            )
            return False

        if theme_name in SoundPlayer.availableThemes() or theme_name is None:
            MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME] = theme_name
            MeticulousConfig.save()

            SoundPlayer.CURRENT_THEME_CONFIG = SoundPlayer.DEFAULT_THEME_CONFIG
            new_theme = SoundPlayer._load_theme(theme_name)
            SoundPlayer.CURRENT_THEME_CONFIG.update(new_theme)
            SoundPlayer.CURRENT_THEME_NAME = theme_name
            logger.info(f"Sound theme '{theme_name}' loaded")
            return True
        else:
            logger.error(f"Error: Theme '{theme_name}' is not available.")
            # Prevent infinite recursion
            if theme_name != SoundPlayer.DEFAULT_THEME_NAME:
                SoundPlayer.set_theme(SoundPlayer.DEFAULT_THEME_NAME)
            return False

    @staticmethod
    def get_theme():
        return SoundPlayer.CURRENT_THEME_CONFIG

    @staticmethod
    def _load_theme(theme_name):
        if theme_name not in SoundPlayer.availableThemes():
            return False

        try:
            theme_folder = SoundPlayer.availableThemes()[theme_name]
            theme_config_file = open(os.path.join(theme_folder, "config.json"))
            theme_config = json.load(theme_config_file)
            return theme_config
        except Exception:
            return {}

    @staticmethod
    def play_event_sound(sound_event: Sounds):
        return SoundPlayer.play_sound(sound_event.name.lower())

    @staticmethod
    def play_sound(sound_name):
        if not MeticulousConfig[CONFIG_USER][SOUNDS_ENABLED]:
            logger.info("No sounds enabled")
            return True

        if SoundPlayer.KNOWN_THEMES is None:
            logger.warning(
                "Playing sound before SoundPlayer was initialized. Ignoring."
            )
            return False

        # Just in case we have a stale mapping
        if (
            SoundPlayer.CURRENT_THEME_NAME
            != MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME]
        ):
            SoundPlayer.set_theme(MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME])

        if sound_name not in SoundPlayer.CURRENT_THEME_CONFIG:
            logger.info("Sound not found")
            return False

        theme_name = SoundPlayer.CURRENT_THEME_NAME
        theme_path = SoundPlayer.KNOWN_THEMES[theme_name]
        file_name = SoundPlayer.CURRENT_THEME_CONFIG.get(sound_name, {})

        # Sound is disabled by the theme
        if file_name in [{}, "", None]:
            logger.info(f"Sound {sound_name} is disabled by the theme")
            return True

        file_path = os.path.join(theme_path, file_name)
        file_path = os.path.abspath(file_path)
        logger.info(f"Playing {sound_name} from {file_path}")

        if not os.path.exists(file_path):
            logger.warning(f"Sound file not found: {file_path}")
            return False

        try:
            playsound(file_path, block=False)
        except Exception as e:
            logger.exception(f"Failed to play sound: {e}")
            return False

        return True
