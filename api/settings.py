import tornado.ioloop
import tornado.web
import json

from config import MeticulousConfig, CONFIG_USER

from .base_handler import BaseHandler
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

class GetSettingsHandler(BaseHandler):
    def get(self, setting_name=None):
        if setting_name:
            setting = MeticulousConfig[CONFIG_USER].get(setting_name)
            if setting != None:
                response = {
                    setting_name: setting
                }
                self.write(json.dumps(response))
            else:
                self.set_status(404)
                self.write(f"Setting {setting_name} not found!")
        else:
            self.write(json.dumps(MeticulousConfig[CONFIG_USER]))

class UpdateSettingsHandler(BaseHandler):
    def post(self):
        try:
            settings = json.loads(self.request.body)
        except json.decoder.JSONDecodeError as e:
            self.set_status(403)
            self.write(f"Settings change failed: {e}")
            return

        for setting_name in settings:
            value = settings.get(setting_name)
            setting = MeticulousConfig[CONFIG_USER].get(setting_name)
            if setting != None:
                MeticulousConfig[CONFIG_USER][setting_name] = value
                response = {
                    setting_name: MeticulousConfig[CONFIG_USER][setting_name]
                }
                self.write(json.dumps(response))
            else:
                self.set_status(404)
                self.write(f"Setting '{setting_name}' failed: Setting not found!")
                MeticulousConfig.load()
                return
        MeticulousConfig.save()

SETTINGS_HANDLER = [
        (r"/settings/*", UpdateSettingsHandler),
        (r"/settings/?(.*)", GetSettingsHandler),
    ]
