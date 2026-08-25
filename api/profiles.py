import functools
import json
import re
from datetime import datetime, timezone

import jsonschema
import tornado
import asyncio

from log import MeticulousLogger
from profile_preprocessor import (
    FormatException,
    UndefinedVariableException,
    VariableTypeException,
)
from profiles import IMAGES_PATH, ProfileManager
from pour_over_profiles import (
    MAX_POUR_OVER_PROFILE_BYTES,
    PourOverProfileError,
    PourOverProfileManager,
    PourOverProfileTooLargeError,
)
from .pour_over_profiles import write_pour_over_profile_error

from .api import API, APIVersion
from .base_handler import BaseHandler
from .machine import Machine
from config import MeticulousConfig, ALLOW_LEGACY_JSON, CONFIG_USER
from .emulation import LEGACY_DUMMY_PROFILE

logger = MeticulousLogger.getLogger(__name__)

POUR_OVER_BREW_TYPE_PATTERN = re.compile(rb'"brew_type"\s*:\s*"pour_over"')


class ListHandler(BaseHandler):
    def get(self):
        full_profiles = self.get_argument("full", "false").lower() == "true"
        profiles = ProfileManager.list_profiles()
        response = []
        for profile in profiles:
            p = profile.copy()
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
    async def post(self):
        data = None
        try:
            change_id = self.request.headers.get("X-Change-Id", None)
            # Tornado already owns the request bytes, but parsing a very large
            # JSON object would create another large object graph on a small
            # machine. Reject recognizable Pour Over payloads at the canonical
            # stored-profile limit before decoding them. Espresso retains its
            # existing, separate image limits and behavior.
            if len(
                self.request.body
            ) > MAX_POUR_OVER_PROFILE_BYTES and POUR_OVER_BREW_TYPE_PATTERN.search(
                self.request.body
            ):
                raise PourOverProfileTooLargeError()
            data = json.loads(
                self.request.body,
                parse_constant=lambda value: (_ for _ in ()).throw(
                    ValueError(f"Invalid number {value}")
                ),
            )
            if isinstance(data, dict) and data.get("brew_type") == "pour_over":
                loop = asyncio.get_event_loop()
                profile_response = await loop.run_in_executor(
                    None,
                    functools.partial(
                        PourOverProfileManager.save_profile,
                        data,
                        change_id=change_id,
                    ),
                )
            else:
                profile_response = ProfileManager.save_profile(data, change_id=change_id)
            self.write(profile_response)
        except PourOverProfileError as error:
            profile_id = data.get("id") if isinstance(data, dict) else None
            logger.warning(
                "Rejected Pour Over profile %s: %s",
                profile_id,
                error.__class__.__name__,
            )
            write_pour_over_profile_error(self, error)
        except jsonschema.exceptions.ValidationError as err:
            errors = {
                "status": "error",
                "error": f"JSON validation error: {err.message}",
            }

            self.set_status(400)
            self.write(errors)
            profile_id = data.get("id") if isinstance(data, dict) else None
            logger.warning("Rejected espresso profile %s: %s", profile_id, err.message)
            return
        except Exception as e:
            self.set_status(400)
            self.write({"status": "error", "error": "failed to save profile", "cause": f"{e}"})
            logger.warning("Failed to save profile:", exc_info=e, stack_info=True)


class LoadProfileHandler(BaseHandler):
    async def get(self, profile_id):
        loop = asyncio.get_event_loop()
        if not Machine.is_idle:
            self.set_status(409)
            self.write({"status": "error", "error": "machine is busy"})
            return
        try:
            data = await loop.run_in_executor(None, ProfileManager.get_profile, profile_id)
            if data:
                try:
                    profile = await loop.run_in_executor(
                        None, ProfileManager.load_profile_and_send, profile_id
                    )
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
                self.write({"status": "error", "error": "profile not found", "id": profile_id})
        except Exception as e:
            self.set_status(400)
            self.write({"status": "error", "error": f"failed to load profile {e}"})
            logger.warning("Failed to execute profile in place:", exc_info=e, stack_info=True)

    async def post(self):
        if not Machine.is_idle:
            self.set_status(409)
            self.write({"status": "error", "error": "machine is busy"})
            return
        loop = asyncio.get_event_loop()

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
                profile = await loop.run_in_executor(
                    None, ProfileManager.send_profile_to_esp32, data
                )
                if not profile:
                    self.set_status(403)
                    self.write({"status": "error", "error": "high strain on motor"})
                    return
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
            logger.warning("Failed to execute profile in place:", exc_info=e, stack_info=True)


class LegacyProfileHandler(BaseHandler):
    async def post(self):
        if not MeticulousConfig[CONFIG_USER][ALLOW_LEGACY_JSON]:
            self.set_status(404)
            return

        if not Machine.is_idle:
            self.set_status(409)
            self.write({"status": "error", "error": "machine is busy"})
            return
        loop = asyncio.get_event_loop()
        try:
            data = json.loads(self.request.body)
            try:
                ProfileManager._set_last_profile(LEGACY_DUMMY_PROFILE)
                await loop.run_in_executor(None, Machine.send_json_with_hash, data)
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
            logger.warning("Failed to execute profile in place:", exc_info=e, stack_info=True)


class GetProfileHandler(BaseHandler):
    def get(self, profile_id):
        logger.info("Request for profile " + profile_id)
        data = ProfileManager.get_profile(profile_id)
        if data:
            self.write(data)
            logger.info(data)
        else:
            self.set_status(404)
            self.write({"status": "error", "error": "profile not found", "id": profile_id})


class DeleteProfileHandler(BaseHandler):
    def get(self, profile_id):
        return self.delete(profile_id)

    def delete(self, profile_id):
        change_id = self.request.headers.get("X-Change-Id", None)
        logger.info("Deletion for profile " + profile_id)
        data = ProfileManager.delete_profile(profile_id, change_id=change_id)
        if data:
            logger.info(f"Deleted profile: {profile_id}")
            self.write(data)
        else:
            self.set_status(404)
            self.write({"status": "error", "error": "profile not found", "id": profile_id})


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
            last_modified = datetime.fromtimestamp(last_profile["load_time"], tz=timezone.utc)
            last_modified.timetz
            self.set_header(
                "Last-Modified", last_modified.strftime("%a, %d %b %Y %H:%M:%S GMT")
            )

        self.write(json.dumps(last_profile))


class SelectedProfileHandler(BaseHandler):
    def get(self):
        hover = ProfileManager.get_profile_hover()
        if not hover.get("id"):
            self.set_status(204)
            return
        self.write(json.dumps(hover))


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
API.register_handler(APIVersion.V1, r"/profile/load/([0-9a-fA-F-]+)", LoadProfileHandler),
API.register_handler(APIVersion.V1, r"/profile/get/([0-9a-fA-F-]+)", GetProfileHandler),
API.register_handler(APIVersion.V1, r"/profile/delete/([0-9a-fA-F-]+)", DeleteProfileHandler),
API.register_handler(APIVersion.V1, r"/profile/changes", ChangesHandler),
API.register_handler(APIVersion.V1, r"/profile/last", LastProfileHandler),
API.register_handler(APIVersion.V1, r"/profile/selected", SelectedProfileHandler),
API.register_handler(APIVersion.V1, r"/profile/legacy", LegacyProfileHandler),
