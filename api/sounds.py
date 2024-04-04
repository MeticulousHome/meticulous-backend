import json
from .base_handler import BaseHandler
from .api import API, APIVersion

from sounds import SoundPlayer
from config import MeticulousConfig, CONFIG_SYSTEM, SOUNDS_THEME

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)


class PlaySoundHandler(BaseHandler):
    def get(self, sound):
        if SoundPlayer.play_sound(sound):
            self.finish()
        else:
            self.set_status(404)
            self.write(
                {"error": "sound not found", "details": sound})


class ListSoundsHandler(BaseHandler):
    def get(self):
        self.write(json.dumps(
            [f"{key}" for key in SoundPlayer.get_theme().keys()]
        ))


class ListThemesHandler(BaseHandler):
    def get(self):
        self.write(json.dumps(
            [f"{theme}" for theme in SoundPlayer.availableThemes()]
        ))


class GetThemeHandler(BaseHandler):
    def get(self):
        self.write(MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME])


class SetThemeHandler(BaseHandler):
    def post(self, theme):
        return self.get(theme)

    def get(self, theme):
        if SoundPlayer.set_theme(theme):
            self.finish()
        else:
            self.set_status(404)
            self.write(
                {"error": "theme not found", "details": theme})


API.register_handler(APIVersion.V1, r"/sounds/play/(.*)", PlaySoundHandler),
API.register_handler(APIVersion.V1, r"/sounds/list",
                     ListSoundsHandler),
API.register_handler(APIVersion.V1, r"/sounds/theme/list",
                     ListThemesHandler),
API.register_handler(APIVersion.V1, r"/sounds/theme/get",
                     GetThemeHandler),
API.register_handler(APIVersion.V1, r"/sounds/theme/set/(.*)",
                     SetThemeHandler),
