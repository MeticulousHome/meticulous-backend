import json
from datetime import datetime, timezone

import jsonschema
import tornado

from log import MeticulousLogger
from profile_preprocessor import (
    FormatException,
    UndefinedVariableException,
    VariableTypeException,
)
from profiles import IMAGES_PATH, ProfileManager

from .api import API, APIVersion
from .base_handler import BaseHandler
from .machine import Machine
from config import MeticulousConfig, CONFIG_SYSTEM, ALLOW_LEGACY_JSON
from .emulation import LEGACY_DUMMY_PROFILE

logger = MeticulousLogger.getLogger(__name__)


class ListHandler(BaseHandler):
    def get(self):
        full_profiles = self.get_argument("full", "false").lower() == "true"
        profiles = ProfileManager.list_profiles()
        response = []
        for profile in profiles:
            p = profiles[profile].copy()
            if not full_profiles:
                if "stages" in p:
                    del p["stages"]
            response.append(p)
        self.write(json.dumps(response))


class ListDefaultsHandler(BaseHandler):
    def get(self):
        profiles = ProfileManager.list_default_profiles()
        self.write(json.dumps(profiles))


class SaveProfileHandler(BaseHandler):
    def post(self):
        try:
            change_id = self.request.headers.get("X-Change-Id", None)
            data = json.loads(self.request.body)
            profile_response = ProfileManager.save_profile(data, change_id=change_id)
            self.write(profile_response)
        except jsonschema.exceptions.ValidationError as err:
            errors = {
                "status": "error",
                "error": f"JSON validation error: {err.message}",
            }

            self.set_status(400)
            self.write(errors)
            logger.debug(data)
            return
        except Exception as e:
            self.set_status(400)
            self.write(
                {"status": "error", "error": "failed to save profile", "cause": f"{e}"}
            )
            logger.warning("Failed to save profile:", exc_info=e, stack_info=True)


class LoadProfileHandler(BaseHandler):
    def get(self, profile_id):
        if not Machine.is_idle:
            self.set_status(409)
            self.write({"status": "error", "error": "machine is busy"})
            return
        try:
            data = ProfileManager.get_profile(profile_id)
            if data:
                try:
                    profile = ProfileManager.load_profile_and_send(profile_id)
                    self.write({"name": profile["name"], "id": profile["id"]})
                    return
                except jsonschema.exceptions.ValidationError as err:
                    errors = {
                        "status": "error",
                        "error": f"JSON validation error: {err.message}",
                    }

                    self.set_status(400)
                    self.write(errors)
                    return
            else:
                self.set_status(404)
                self.write(
                    {"status": "error", "error": "profile not found", "id": profile_id}
                )
        except Exception as e:
            self.set_status(400)
            self.write({"status": "error", "error": f"failed to load profile {e}"})
            logger.warning(
                "Failed to execute profile in place:", exc_info=e, stack_info=True
            )

    def post(self):
        if not Machine.is_idle:
            self.set_status(409)
            self.write({"status": "error", "error": "machine is busy"})
            return

        try:

            try:
                data = json.loads(self.request.body)
            except json.JSONDecodeError as e:
                logger.warning(f"JSON decode error: {str(e)}")
                self.set_status(400)
                self.write({"status": "error", "error": f"Invalid JSON: {str(e)}"})
                return

            logger.warning(f"Parsed data: {data}")

            try:
                profile = ProfileManager.send_profile_to_esp32(data)
            except jsonschema.exceptions.ValidationError as err:
                errors = {
                    "status": "error",
                    "error": f"JSON validation error: {err.message}",
                }
                self.set_status(400)
                self.write(errors)
                return
            except (
                UndefinedVariableException,
                VariableTypeException,
                FormatException,
            ) as err:
                errors = {
                    "status": "error",
                    "error": f"variable error: {err.__class__.__name__}:{err}",
                }
                self.set_status(400)
                self.write(errors)
                return
            self.write({"name": profile["name"], "id": profile["id"]})
        except Exception as e:
            self.set_status(400)
            self.write({"status": "error", "error": f"failed to load profile {e}"})
            logger.warning(
                "Failed to execute profile in place:", exc_info=e, stack_info=True
            )


class LegacyProfileHandler(BaseHandler):
    def post(self):
        if not MeticulousConfig[CONFIG_SYSTEM][ALLOW_LEGACY_JSON]:
            self.set_status(404)
            return

        if not Machine.is_idle:
            self.set_status(409)
            self.write({"status": "error", "error": "machine is busy"})
            return
        try:
            data = json.loads(self.request.body)
            try:
                ProfileManager._set_last_profile(LEGACY_DUMMY_PROFILE)

                Machine.send_json_with_hash(data)
            except (
                UndefinedVariableException,
                VariableTypeException,
                FormatException,
            ) as err:
                errors = {
                    "status": "error",
                    "error": f"variable error: {err.__class__.__name__}:{err}",
                }
                self.set_status(400)
                self.write(errors)
                return
            self.write({"name": data["name"], "id": data["id"]})
        except Exception as e:
            self.set_status(400)
            self.write({"status": "error", "error": f"failed to load profile {e}"})
            logger.warning(
                "Failed to execute profile in place:", exc_info=e, stack_info=True
            )


class GetProfileHandler(BaseHandler):
    def get(self, profile_id):
        logger.info("Request for profile " + profile_id)
        data = ProfileManager.get_profile(profile_id)
        if data:
            self.write(data)
            logger.info(data)
        else:
            self.set_status(404)
            self.write(
                {"status": "error", "error": "profile not found", "id": profile_id}
            )


class DeleteProfileHandler(BaseHandler):
    def get(self, profile_id):
        return self.delete(profile_id)

    def delete(self, profile_id):
        change_id = self.request.headers.get("X-Change-Id", None)
        logger.info("Deletion for profile " + profile_id)
        data = ProfileManager.delete_profile(profile_id, change_id=change_id)
        if data:
            logger.info(f"Deleted profile: {data}")
            self.write(data)
        else:
            self.set_status(404)
            self.write(
                {"status": "error", "error": "profile not found", "id": profile_id}
            )


class ChangesHandler(BaseHandler):
    def get(self):
        changes = ProfileManager.get_profile_changes()
        response = []
        for change in changes:
            c = change.copy()
            c["type"] = c["type"].value
            response.append(c)
        self.write(json.dumps(response))


class LastProfileHandler(BaseHandler):
    def get(self):
        last_profile = ProfileManager.get_last_profile()
        if last_profile is None:
            self.set_status(204)
            return

        if last_profile["load_time"] is not None:
            last_modified = datetime.fromtimestamp(
                last_profile["load_time"], tz=timezone.utc
            )
            last_modified.timetz
            self.set_header(
                "Last-Modified", last_modified.strftime("%a, %d %b %Y %H:%M:%S GMT")
            )

        self.write(json.dumps(last_profile))


class ListImagesHandler(BaseHandler):
    def get(self, ignored):
        self.write(json.dumps(ProfileManager.get_default_images()))


API.register_handler(APIVersion.V1, r"/profile/list", ListHandler),
API.register_handler(APIVersion.V1, r"/profile/save", SaveProfileHandler),
API.register_handler(APIVersion.V1, r"/profile/load", LoadProfileHandler),
API.register_handler(APIVersion.V1, r"/profile/defaults", ListDefaultsHandler),
API.register_handler(APIVersion.V1, r"/profile/image([/]*)", ListImagesHandler),
API.register_handler(
    APIVersion.V1,
    r"/profile/image/(.*)",
    tornado.web.StaticFileHandler,
    **{"path": IMAGES_PATH},
),
API.register_handler(
    APIVersion.V1, r"/profile/load/([0-9a-fA-F-]+)", LoadProfileHandler
),
API.register_handler(APIVersion.V1, r"/profile/get/([0-9a-fA-F-]+)", GetProfileHandler),
API.register_handler(
    APIVersion.V1, r"/profile/delete/([0-9a-fA-F-]+)", DeleteProfileHandler
),
API.register_handler(APIVersion.V1, r"/profile/changes", ChangesHandler),
API.register_handler(APIVersion.V1, r"/profile/last", LastProfileHandler),
API.register_handler(APIVersion.V1, r"/profile/legacy", LegacyProfileHandler),
