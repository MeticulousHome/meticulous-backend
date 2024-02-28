import json

import tornado.ioloop
import tornado.web
from log import MeticulousLogger

from config import CONFIG_USER, MeticulousConfig

from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)


class SettingsHandler(BaseHandler):
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
                self.write(
                    {"status": "error", "error": "setting not found", "setting": setting_name})
        else:
            self.write(json.dumps(MeticulousConfig[CONFIG_USER]))

    def post(self):
        try:
            settings = json.loads(self.request.body)
        except json.decoder.JSONDecodeError as e:
            self.set_status(403)
            self.write({"status": "error", "error": "invalid json", "json_error": f"{e}"})
            return

        for setting_name in settings:
            value = settings.get(setting_name)
            if value not in [True, False]:
                self.set_status(404)
                self.write({"status": "error", "error": "setting value invalidm, expected boolean",
                           "setting": setting_name, "value": value})
                MeticulousConfig.load()
                return
            setting = MeticulousConfig[CONFIG_USER].get(setting_name)
            if setting != None:
                MeticulousConfig[CONFIG_USER][setting_name] = value
            else:
                self.set_status(404)
                self.write(
                    {"status": "error", "error": "setting not found", "setting": setting_name})
                MeticulousConfig.load()
                return
        MeticulousConfig.save()
        return self.get()


SETTINGS_HANDLER = [
    (r"/settings/*", SettingsHandler),
]
