import tornado.ioloop
import tornado.web
import json
from profile import ProfileManager
from .base_handler import BaseHandler

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

class ListHandler(BaseHandler):
    def get(self):
        profiles = ProfileManager.list_profiles()
        response = []
        for profile in profiles:
            p = profiles[profile].copy()
            if "stages" in p:
                del p["stages"]
            response.append(p)
        self.write(json.dumps(response))

class SaveProfileHandler(BaseHandler):
    def post(self, save_id):
        try:
            data = json.loads(self.request.body)
            (name, profile_id) = ProfileManager.save_profile(save_id, data)
            self.write({"name": name, "id": profile_id})
        except Exception as e:
            self.set_status(400)
            self.write(f"Error: Failed to save profile")
            logger.warning("Failed to save profile:", exc_info=e, stack_info=True)

class LoadProfileHandler(BaseHandler):
    def get(self, profile_id):
        data = ProfileManager.get_profile(profile_id)
        if data:
            profile = ProfileManager.load_profile(data)
            self.write({"name": profile["name"], "id": profile["id"]})
        else:
            self.set_status(404)
            self.write(f"No profile found with id: {profile_id}")
    
    def post(self, data):
        try:
            data = json.loads(self.request.body)
            profile = ProfileManager.load_profile_from_data(data)
            self.write({"name": profile["name"], "id": profile["id"]})
        except Exception as e:
            self.set_status(400)
            self.write(f"Error: Failed to load temporary profile")
            logger.warning("Failed to save profile:", exc_info=e, stack_info=True)


class GetProfileHandler(BaseHandler):
    def get(self, profile_id):
        logger.info("Request for profile "+profile_id)
        data = ProfileManager.get_profile(profile_id)
        if data:
            self.write(data)
        else:
            self.set_status(404)
            self.write(f"No profile found with id: {profile_id}")

PROFILE_HANDLER= [
        (r"/profile/list", ListHandler),
        (r"/profile/save", SaveProfileHandler),
        (r"/profile/load", LoadProfileHandler),
        (r"/profile/get/([0-9a-fA-F-]+)", GetProfileHandler),
]

