import json

from log import MeticulousLogger
from pour_over_profiles import (
    PourOverProfileLimitError,
    PourOverProfileManager,
    PourOverProfileStorageError,
    PourOverProfileTooLargeError,
    PourOverProfileUnavailableError,
    PourOverProfileValidationError,
)

from .api import API, APIVersion
from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)


class ListPourOverProfilesHandler(BaseHandler):
    def get(self):
        try:
            self.write({"profiles": PourOverProfileManager.list_profiles()})
        except PourOverProfileUnavailableError as error:
            write_pour_over_profile_error(self, error)


class GetPourOverProfileHandler(BaseHandler):
    def get(self, profile_id):
        include_image = self.get_argument("include_image", "true").lower() == "true"
        try:
            profile = PourOverProfileManager.get_profile(
                profile_id, include_image=include_image
            )
        except (PourOverProfileUnavailableError, PourOverProfileStorageError) as error:
            write_pour_over_profile_error(self, error)
            return
        if profile is None:
            self.set_status(404)
            self.write({"status": "error", "error": "Pour Over profile not found"})
            return
        self.write(profile)


class GetPourOverProfileImageHandler(BaseHandler):
    def get(self, profile_id):
        try:
            image = PourOverProfileManager.get_profile_image(profile_id)
        except (PourOverProfileUnavailableError, PourOverProfileStorageError) as error:
            write_pour_over_profile_error(self, error)
            return
        if image is None:
            self.set_status(404)
            self.write({"status": "error", "error": "Pour Over profile image not found"})
            return
        self.set_header("Content-Type", "image/jpeg")
        self.set_header("Cache-Control", "private, max-age=300")
        self.write(image)


class DeletePourOverProfileHandler(BaseHandler):
    def delete(self, profile_id):
        change_id = self.request.headers.get("X-Change-Id", None)
        try:
            result = PourOverProfileManager.delete_profile(profile_id, change_id=change_id)
        except (PourOverProfileStorageError, PourOverProfileUnavailableError) as error:
            write_pour_over_profile_error(self, error)
            return
        if result is None:
            self.set_status(404)
            self.write({"status": "error", "error": "Pour Over profile not found"})
            return
        self.write(result)


class PourOverProfileSchemaHandler(BaseHandler):
    def get(self):
        try:
            PourOverProfileManager._require_available()
        except PourOverProfileUnavailableError as error:
            write_pour_over_profile_error(self, error)
            return
        self.set_header("Content-Type", "application/schema+json")
        self.write(json.dumps(PourOverProfileManager._schema))


def write_pour_over_profile_error(handler, error):
    if isinstance(error, PourOverProfileValidationError):
        handler.set_status(422)
        handler.write(
            {
                "status": "error",
                "error": "Invalid Pour Over profile",
                "details": error.issues,
            }
        )
    elif isinstance(error, PourOverProfileTooLargeError):
        handler.set_status(413)
        handler.write({"status": "error", "error": "Pour Over profile is too large"})
    elif isinstance(error, PourOverProfileLimitError):
        handler.set_status(409)
        handler.write({"status": "error", "error": "Pour Over profile limit reached"})
    elif isinstance(error, PourOverProfileStorageError):
        handler.set_status(507)
        handler.write(
            {"status": "error", "error": "Could not access Pour Over profile storage"}
        )
    elif isinstance(error, PourOverProfileUnavailableError):
        handler.set_status(503)
        handler.write({"status": "error", "error": "Pour Over profiles are unavailable"})
    else:
        handler.set_status(500)
        handler.write({"status": "error", "error": "Could not process Pour Over profile"})


API.register_handler(APIVersion.V1, r"/pour-over/profile/list", ListPourOverProfilesHandler)
API.register_handler(
    APIVersion.V1,
    r"/pour-over/profile/get/([0-9a-fA-F-]+)",
    GetPourOverProfileHandler,
)
API.register_handler(
    APIVersion.V1,
    r"/pour-over/profile/image/([0-9a-fA-F-]+)",
    GetPourOverProfileImageHandler,
)
API.register_handler(
    APIVersion.V1,
    r"/pour-over/profile/delete/([0-9a-fA-F-]+)",
    DeletePourOverProfileHandler,
)
API.register_handler(
    APIVersion.V1,
    r"/pour-over/profile/schema",
    PourOverProfileSchemaHandler,
)
