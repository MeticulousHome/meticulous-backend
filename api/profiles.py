import tornado.ioloop
import tornado.web
import json
from profile import ProfileManager

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)


class ProfileHandler(tornado.web.RequestHandler):
    def get(self, profile_id):
        data = ProfileManager.load_profile(profile_id)
        if data:
            self.write(data)
        else:
            self.set_status(404)
            self.write(f"No profile found with UUID: {profile_id}")

    def post(self, profile_name):
        try:
            data = json.loads(self.request.body)
            (name, profile_id) = ProfileManager.save_profile(profile_name, data)
            self.write({"name": name, "uuid": profile_id})
        except Exception as e:
            self.set_status(400)
            self.write(f"Error: Failed to save profile")
            logger.warning("Failed to save profile:", exc_info=e, stack_info=True)


class ListHandler(tornado.web.RequestHandler):
    def get(self):
        profiles = ProfileManager.list_profiles()
        self.write(json.dumps(profiles))

PROFILE_HANDLER= [
        (r"/profiles", ListHandler),
        (r"/profile/([0-9a-zA-Z-_]+)", ProfileHandler),
]

