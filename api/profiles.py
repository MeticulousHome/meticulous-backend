import tornado.web
import json
from profile import ProfileManager
from .base_handler import BaseHandler
from .api import API, APIVersion
from .machine import Machine

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)


class ListHandler(BaseHandler):
    def get(self):
        full_profiles = self.get_argument(
            "full", "false").lower() == "true"
        profiles = ProfileManager.list_profiles()
        response = []
        for profile in profiles:
            p = profiles[profile].copy()
            if not full_profiles:
                if "stages" in p:
                    del p["stages"]
            response.append(p)
        self.write(json.dumps(response))


class SaveProfileHandler(BaseHandler):
    def post(self):
        try:
            data = json.loads(self.request.body)
            profile_response = ProfileManager.save_profile(data)
            self.write(
                {"name": profile_response["name"], "id": profile_response["id"], "change_id": profile_response["change_id"]}
            )
        except Exception as e:
            self.set_status(400)
            self.write(
                {"status": "error", "error": "failed to save profile"})
            logger.warning("Failed to save profile:",
                           exc_info=e, stack_info=True)


class LoadProfileHandler(BaseHandler):
    def get(self, profile_id):
        if not Machine.is_idle:
            self.set_status(409)
            self.write(
                {"status": "error", "error": "machine is busy"})
            return
        try:
            data = ProfileManager.get_profile(profile_id)
            if data:
                profile = ProfileManager.load_profile_and_send(profile_id)
                self.write({"name": profile["name"], "id": profile["id"]})
            else:
                self.set_status(404)
                self.write(
                    {"status": "error", "error": "profile not found", "id": profile_id})
        except Exception as e:
            self.set_status(400)
            self.write(
                {"status": "error", "error": f"failed to load profile {e}"})
            logger.warning("Failed to execute profile in place:",
                           exc_info=e, stack_info=True)

    def post(self):
        if not Machine.is_idle:
            self.set_status(409)
            self.write(
                {"status": "error", "error": "machine is busy"})
            return
        try:
            data = json.loads(self.request.body)
            profile = ProfileManager.send_profile_to_esp32(data)
            self.write({"name": profile["name"], "id": profile["id"]})
        except Exception as e:
            self.set_status(400)
            self.write(
                {"status": "error", "error": f"failed to load profile {e}"})
            logger.warning("Failed to execute profile in place:",
                           exc_info=e, stack_info=True)


class GetProfileHandler(BaseHandler):
    def get(self, profile_id):
        logger.info("Request for profile "+profile_id)
        data = ProfileManager.get_profile(profile_id)
        if data:
            self.write(data)
            logger.info(data)
        else:
            self.set_status(404)
            self.write(
                {"status": "error", "error": "profile not found", "id": profile_id})


class DeleteProfileHandler(BaseHandler):
    def get(self, profile_id):
        return self.delete(profile_id)

    def delete(self, profile_id):
        logger.info("Deletion for profile "+profile_id)
        data = ProfileManager.delete_profile(profile_id)
        if data:
            logger.info(f"Deleted profile: {data}")
            self.write(data)
        else:
            self.set_status(404)
            self.write(
                {"status": "error", "error": "profile not found", "id": profile_id})


class ChangesHandler(BaseHandler):
    def get(self):
        changes = ProfileManager.get_profile_changes()
        response = []
        for change in changes:
            c = change.copy()
            c["type"] = c["type"].value
            response.append(c)
        self.write(json.dumps(response))


API.register_handler(APIVersion.V1, r"/profile/list", ListHandler),
API.register_handler(APIVersion.V1, r"/profile/save", SaveProfileHandler),
API.register_handler(APIVersion.V1, r"/profile/load", LoadProfileHandler),
API.register_handler(
    APIVersion.V1, r"/profile/load/([0-9a-fA-F-]+)", LoadProfileHandler),
API.register_handler(
    APIVersion.V1, r"/profile/get/([0-9a-fA-F-]+)", GetProfileHandler),
API.register_handler(
    APIVersion.V1, r"/profile/delete/([0-9a-fA-F-]+)", DeleteProfileHandler),
API.register_handler(APIVersion.V1, r"/profile/changes", ChangesHandler),
