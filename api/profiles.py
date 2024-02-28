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
    def post(self):
        try:
            data = json.loads(self.request.body)
            (name, profile_id) = ProfileManager.save_profile(data)
            self.write({"name": name, "id": profile_id})
        except Exception as e:
            self.set_status(400)
            self.write({"status":"error", "error": "failed to save profile", "data":data})
            logger.warning("Failed to save profile:", exc_info=e, stack_info=True)

class LoadProfileHandler(BaseHandler):
    def get(self, profile_id):
        data = ProfileManager.get_profile(profile_id)
        if data:
            profile = ProfileManager.load_profile(profile_id)
            self.write({"name": profile["name"], "id": profile["id"]})
        else:
            self.set_status(404)
            self.write({"status":"error", "error": "profile not found", "id":profile_id})

    
    def post(self):
        try:
            data = json.loads(self.request.body)
            profile = ProfileManager.send_profile_to_esp32(data)
            self.write({"name": profile["name"], "id": profile["id"]})
        except Exception as e:
            self.set_status(400)
            self.write({"status":"error", "error": "failed to load temporary profile"})
            logger.warning("Failed to execute profile in place:", exc_info=e, stack_info=True)


class GetProfileHandler(BaseHandler):
    def get(self, profile_id):
        logger.info("Request for profile "+profile_id)
        data = ProfileManager.get_profile(profile_id)
        if data:
            self.write(data)
            logger.info(data)
        else:
            self.set_status(404)
            self.write({"status":"error", "error": "profile not found", "id":profile_id})

class DeleteProfileHandler(BaseHandler):
    def get(self, profile_id):
        logger.info("Deletion for profile "+profile_id)
        data = ProfileManager.delete_profile(profile_id)
        if data:
            logger.info(f"Deleted profile: {data}")
            self.write(data)
        else:
            self.set_status(404)
            self.write({"status":"error", "error": "profile not found", "id":profile_id})

PROFILE_HANDLER= [
        (r"/profile/list", ListHandler),
        (r"/profile/save", SaveProfileHandler),
        (r"/profile/load", LoadProfileHandler),
        (r"/profile/load/(?P<profile_id>\w+)", LoadProfileHandler),
        (r"/profile/get/([0-9a-fA-F-]+)", GetProfileHandler),
        (r"/profile/delete/([0-9a-fA-F-]+)", DeleteProfileHandler),
]

