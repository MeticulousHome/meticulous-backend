import json
from io import BytesIO
import zipfile
import os

from .base_handler import BaseHandler
from .api import API, APIVersion
from .machine import Machine

from sounds import SoundPlayer, USER_SOUNDS
from config import MeticulousConfig, CONFIG_SYSTEM, SOUNDS_THEME, SOUNDS_VOLUME
from sound_volume_controller import SoundVolumeController

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class PlaySoundHandler(BaseHandler):
    def get(self, sound):
        if SoundPlayer.play_sound(sound):
            self.write({"status": "okay"})
            self.finish()
        else:
            self.set_status(404)
            self.write({"error": "sound not found", "details": sound})


class ListSoundsHandler(BaseHandler):
    def get(self):
        self.write(json.dumps([f"{key}" for key in SoundPlayer.get_theme().keys()]))


class ListThemesHandler(BaseHandler):
    def get(self):
        self.write(json.dumps([f"{theme}" for theme in SoundPlayer.availableThemes()]))


class GetThemeHandler(BaseHandler):
    def get(self):
        self.write(MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME])


class SetThemeHandler(BaseHandler):
    def post(self, theme):
        return self.get(theme)

    def get(self, theme):
        if SoundPlayer.set_theme(theme):
            self.write({"status": "okay"})
            self.finish()
        else:
            self.set_status(404)
            self.write({"error": "theme not found", "details": theme})


class UploadThemeHandler(BaseHandler):
    def post(self):

        # Ensure there is a file in the request
        if "file" not in self.request.files:
            self.set_status(400)
            self.write(
                {"error": "invalid zip", "details": "'file' not found in request"}
            )
            return

        fileinfo = self.request.files["file"][0]
        zip_bytes = BytesIO(fileinfo["body"])

        try:
            with zipfile.ZipFile(zip_bytes, "r") as zip_ref:
                # Validate the structure of the zip file
                root_folders = {
                    name.split("/")[0] for name in zip_ref.namelist() if "/" in name
                }
                if len(root_folders) != 1:
                    self.set_status(400)
                    self.write(
                        {
                            "error": "invalid zip",
                            "details": "Zip must contain exactly one folder with the themes name at the root.",
                        }
                    )
                    return

                root_folder = root_folders.pop()
                has_config = f"{root_folder}/config.json" in zip_ref.namelist()
                if not has_config:
                    self.set_status(400)
                    self.write(
                        {
                            "error": "invalid zip",
                            "details": "No config.json found in the root folder.",
                        }
                    )
                    return

                # Check for subfolders and validate config.json
                for name in zip_ref.namelist():

                    if name.endswith("config.json"):
                        config_data = zip_ref.read(name)
                        try:
                            # parse to check valid JSON
                            json.loads(config_data)
                        except json.JSONDecodeError:
                            self.set_status(400)
                            self.write(
                                {
                                    "error": "invalid zip",
                                    "details": "config.json is not valid JSON.",
                                }
                            )
                            return

                # Extraction destination
                extraction_path = os.path.abspath(USER_SOUNDS)
                logger.info(f"Extracting zip to {extraction_path}")
                # If all checks pass, extract the zip
                zip_ref.extractall(extraction_path)
                logger.info("Reloading sound player")
                SoundPlayer.init(Machine.emulated, False)
            self.write("Zip file uploaded and unpacked successfully.")
        except zipfile.BadZipFile:
            self.set_status(400)
            self.write({"error": "invalid zip", "details": "zip file corrupted"})
        except ValueError as e:
            self.set_status(400)
            self.write({"error": "invalid zip", "details": str(e)})

        except Exception as e:
            self.set_status(500)
            self.write({"error": "unknown issue", "details": str(e)})


class SoundVolumeHandler(BaseHandler):
    def get(self):
        saved_volume = MeticulousConfig[CONFIG_SYSTEM].get(SOUNDS_VOLUME)
        if saved_volume is not None:
            self.write({"volume": int(saved_volume)})
            return

        volume = SoundVolumeController.get_volume()
        if volume is not None:
            self.write({"volume": volume})
        else:
            self.set_status(500)
            self.write({"error": "Failed to get volume"})

    def post(self):
        try:
            settings = json.loads(self.request.body)
            volume_level = int(settings.get("volume", 0))
        except:
            self.set_status(400)
            self.write({"error": "invalid request"})
            return

        if volume_level < 0 or volume_level > 100:
            self.set_status(400)
            self.write({"error": "volume must be between 0 and 100"})
            return

        try:
            SoundVolumeController.set_volume(volume_level)
            MeticulousConfig[CONFIG_SYSTEM][SOUNDS_VOLUME] = volume_level
            MeticulousConfig.save()
            self.write({"status": "okay", "volume": volume_level})
        except Exception as e:
            logger.error(f"Failed to set volume: {e}", exc_info=True)
            self.set_status(500)
            self.write({"error": "Failed to set volume", "details": str(e)})


API.register_handler(APIVersion.V1, r"/sounds/play/(.*)", PlaySoundHandler),
API.register_handler(APIVersion.V1, r"/sounds/list", ListSoundsHandler),
API.register_handler(APIVersion.V1, r"/sounds/theme/list", ListThemesHandler),
API.register_handler(APIVersion.V1, r"/sounds/theme/get", GetThemeHandler),
API.register_handler(APIVersion.V1, r"/sounds/theme/set/(.*)", SetThemeHandler),
API.register_handler(APIVersion.V1, r"/sounds/theme/upload", UploadThemeHandler),
API.register_handler(APIVersion.V1, r"/sounds/volume", SoundVolumeHandler)
