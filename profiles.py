import hashlib
import json
import os
import shutil
import threading
import time
import uuid
from enum import Enum
from typing import Optional, Set
from urllib.parse import urlparse
import random
import datauri
import jsonschema
import socketio
from config import (
    MeticulousConfig,
    CONFIG_SYSTEM,
    CONFIG_USER,
    CONFIG_PROFILES,
    PROFILE_AUTO_PURGE,
    PROFILE_AUTO_START,
    PROFILE_LAST,
    ALLOW_LEGACY_JSON
)
import asyncio
from log import MeticulousLogger
from machine import Machine
from profile_converter.profile_converter import ComplexProfileConverter
from profile_preprocessor import ProfilePreprocessor

logger = MeticulousLogger.getLogger(__name__)

PROFILE_PATH = os.getenv("PROFILE_PATH", "/meticulous-user/profiles")
IMAGES_PATH = os.getenv("IMAGES_PATH", "/meticulous-user/profile-images/")
DEFAULT_IMAGES_PATH = os.getenv(
    "DEFAULT_IMAGES", "/opt/meticulous-backend/images/default"
)

DEFAULT_PROFILES_PATH = os.getenv(
    "DEFAULT_PROFILES", "/opt/meticulous-backend/default_profiles"
)


class PROFILE_EVENT(Enum):
    CREATE = "create"
    UPDATE = "update"
    DELETE = "delete"
    RELOAD = "full_reload"
    LOAD = "load"


class ProfileManager:
    _known_profiles = dict()
    _known_images = []
    _default_profiles = []
    _profile_default_images = []
    _sio: socketio.AsyncServer = None
    _loop: asyncio.AbstractEventLoop = None
    _thread: threading.Thread = None
    _last_profile_changes = []
    _schema = None

    def init(sio: socketio.AsyncServer):
        ProfileManager._sio = sio

        ProfileManager._loop = asyncio.new_event_loop()

        def start_event_loop() -> None:
            asyncio.set_event_loop(ProfileManager._loop)
            ProfileManager._loop.run_forever()

        ProfileManager._thread = threading.Thread(target=start_event_loop)
        ProfileManager._thread.start()

        if not os.path.exists(PROFILE_PATH):
            os.makedirs(PROFILE_PATH)

        dirname = os.path.dirname(__file__)
        profile_schema = os.path.join(dirname, "profile_schema/schema.json")

        # Load JSON schema from a file
        with open(profile_schema, "r") as schema_file:
            ProfileManager._schema = json.load(schema_file)

        ProfileManager.refresh_image_list()
        ProfileManager.refresh_default_profile_list()
        ProfileManager.refresh_profile_list()
        ProfileManager._delete_unused_images()

    def _register_profile_change(
        change: PROFILE_EVENT,
        profile_id: str,
        timestamp: Optional[float] = None,
        change_id: Optional[str] = None,
    ) -> str:
        changes_to_keep = 100

        if timestamp is None:
            timestamp = time.time()
        if change_id is None:
            change_id = str(uuid.uuid4())
        change_entry = {
            "type": change,
            "profile_id": profile_id,
            "change_id": change_id,
            "timestamp": timestamp,
        }

        changes = ProfileManager._last_profile_changes
        changes.append(change_entry)
        if len(changes) > changes_to_keep:
            changes[:] = changes[-changes_to_keep:]
        ProfileManager._last_profile_changes = changes
        return change_id

    def _emit_profile_event(
        change: PROFILE_EVENT,
        profile_id: Optional[str] = None,
        change_id: Optional[str] = None,
    ) -> None:

        if not ProfileManager._loop:
            logger.warning("No event loop is running")
            return

        payload = {"change": change.value}
        if profile_id is not None:
            payload["profile_id"] = profile_id
        if change_id is not None:
            payload["change_id"] = change_id

        async def emit() -> None:
            await ProfileManager._sio.emit("profile", payload)

        asyncio.run_coroutine_threadsafe(emit(), ProfileManager._loop)

    def _set_last_profile(profile) -> None:
        last_profile = {"load_time": time.time(), "profile": profile}
        MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST] = last_profile
        MeticulousConfig.save()

    def get_profile_changes() -> list[object]:
        return ProfileManager._last_profile_changes

    def _is_relative_url(url):
        """Check if the given URL is a relative URL."""
        parsed = urlparse(url)
        return not parsed.scheme and not parsed.netloc

    def handle_image(data):
        if "image" not in data["display"] or data["display"]["image"] == "":
            data["display"]["image"] = "/api/v1/profile/image/" + random.choice(
                ProfileManager.get_default_images()
            )
        elif not ProfileManager._is_relative_url(data["display"]["image"]):
            try:
                uri = datauri.parse(data["display"]["image"])
                logger.info("The string is a data URI with base64 payload.")

                # Check if the MIME type is an image
                if not uri.media_type.startswith("image/"):
                    logger.warning("The data URI does not encode an image.")
                    raise Exception("Invalid image MIME type")

                file_content = uri.data
                file_extension = uri.media_type.split("/")[-1]

                if (
                    len(file_content) > 10 * 1024 * 1024
                ):  # size check, e.g., less than 10MB
                    logger.warning("File size exceeds limit.")
                    raise Exception("Image file too large")

                md5sum = hashlib.md5(file_content).hexdigest()
                filename = f"{md5sum}.{file_extension}"
                try:
                    with open(os.path.join(IMAGES_PATH, filename), "wb") as file:
                        file.write(file_content)
                        logger.info(f"File saved as {filename}")
                        data["display"]["image"] = f"/api/v1/profile/image/{filename}"
                except Exception as e:
                    raise Exception(f"Saving file failed: {e}")

            except datauri.DataURIError:
                logger.warning(
                    "The string is neither a relative URL nor a valid data URI with base64 payload."
                )
                pass
        elif not data["display"]["image"].startswith("/api/v1/profile/image"):
            data["display"]["image"] = (
                "/api/v1/profile/image/" + data["display"]["image"]
            )

    def generate_ramdom_accent_color():
        color = random.randrange(0, 2**24)

        hex_color = hex(color)

        std_color = "#" + hex_color[2:].zfill(6)

        return std_color

    def handle_accent_color(data):
        if "accentColor" not in data["display"] or data["display"]["accentColor"] == "":
            data["display"][
                "accentColor"
            ] = ProfileManager.generate_ramdom_accent_color()

    def save_profile(
        data,
        set_last_changed: bool = False,
        change_id: Optional[str] = None,
        skip_validation: bool = False,
    ) -> dict:

        if "id" not in data or data["id"] == "":
            data["id"] = str(uuid.uuid4())

        if "display" not in data:
            data["display"] = {}

        ProfileManager.handle_image(data)

        ProfileManager.handle_accent_color(data)

        name = f'{data["id"]}.json'

        if not skip_validation:
            errors = ProfileManager.validate_profile(data)
            if errors is not None:
                raise errors

        current_time = time.time()
        if set_last_changed:
            data["last_changed"] = current_time

        is_update = ProfileManager._known_profiles.get(data["id"]) is not None

        file_path = os.path.join(PROFILE_PATH, name)
        with open(file_path, "w") as f:
            json.dump(data, f, indent=4)

        ProfileManager._known_profiles[data["id"]] = data
        logger.info(f"Saved profile {name}")
        if is_update:
            change_type = PROFILE_EVENT.UPDATE
        else:
            change_type = PROFILE_EVENT.CREATE

        change_id = ProfileManager._register_profile_change(
            change_type, data["id"], current_time, change_id
        )

        ProfileManager._emit_profile_event(change_type, data["id"], change_id)

        return {"profile": data, "change_id": change_id}

    def delete_profile(id: str, change_id: Optional[str] = None) -> Optional[dict]:
        profile = ProfileManager._known_profiles.get(id)
        if not profile:
            return None

        filename = f'{profile["id"]}.json'
        file_path = os.path.join(PROFILE_PATH, filename)
        os.remove(file_path)
        del ProfileManager._known_profiles[profile["id"]]
        change_id = ProfileManager._register_profile_change(
            PROFILE_EVENT.DELETE, profile["id"], change_id
        )

        ProfileManager._emit_profile_event(PROFILE_EVENT.DELETE, profile["id"])

        ProfileManager._delete_unused_images()

        return {"profile": profile, "change_id": change_id}

    def get_profile(id):
        logger.info(f"Serving profile: {id}")
        logger.info(ProfileManager._known_profiles.get(id))
        return ProfileManager._known_profiles.get(id)

    def load_profile_and_send(id):
        profile = ProfileManager._known_profiles.get(id)
        if profile is not None:
            ProfileManager.send_profile_to_esp32(profile)
        return profile

    def send_profile_to_esp32(data):
        if MeticulousConfig[CONFIG_SYSTEM][ALLOW_LEGACY_JSON] and data.get("legacy",) == "true":
            Machine.send_json_with_hash(data)
            return data
        
        if data.get("legacy") is None:
            logger.info("Legacy JSON is not allowed. Please enable it in the settings if you want it.")
        
        click_to_start = not MeticulousConfig[CONFIG_USER][PROFILE_AUTO_START]
        click_to_purge = not MeticulousConfig[CONFIG_USER][PROFILE_AUTO_PURGE]
        
        if "id" not in data:
            data["id"] = str(uuid.uuid4())

        ProfileManager.handle_image(data)

        logger.info(f"Recieved data: {data} {type(data)}")

        logger.info("processing simplified profile")
        errors = ProfileManager.validate_profile(data)
        if errors is not None:
            raise errors

        try:
            preprocessed_profile = ProfilePreprocessor.processVariables(data)
        except Exception as err:
            logger.info(
                f"Profile variables could not be processed: {err.__class__.__name__}: {err}"
            )
            raise err

        logger.info(
            f"Streaming JSON to ESP32: click_to_start={click_to_start} click_to_purge={click_to_purge} data={json.dumps(preprocessed_profile)}"
        )

        converter = ComplexProfileConverter(
            click_to_start, click_to_purge, 1000, 7000, preprocessed_profile
        )
        profile = converter.get_profile()
        Machine.send_json_with_hash(profile)
        ProfileManager._set_last_profile(data)

        ProfileManager._emit_profile_event(PROFILE_EVENT.LOAD, data["id"])

        return data

    def refresh_profile_list():
        start = time.time()
        ProfileManager._known_profiles = dict()
        for filename in os.listdir(PROFILE_PATH):
            if not filename.endswith(".json"):
                continue

            file_path = os.path.join(PROFILE_PATH, filename)
            with open(file_path, "r") as f:
                try:
                    profile = json.load(f)
                except json.decoder.JSONDecodeError as error:
                    logger.warning(f"Could not decode profile {f.name}: {error}")
                    continue

                profile_changed = False

                if "id" not in profile or profile["id"] == "":
                    profile["id"] = str(uuid.uuid4())
                    profile_changed = True

                if "author" not in profile:
                    profile["author"] = ""
                    profile_changed = True

                if "author_id" not in profile or profile["author_id"] == "":
                    profile["author_id"] = str("00000000-0000-0000-0000-000000000000")
                    profile_changed = True

                if "last_changed" not in profile:
                    profile_changed = True

                if "stages" not in profile:
                    profile["stages"] = []
                    profile_changed = True
                else:
                    for stage in profile["stages"]:
                        if "key" not in stage:
                            stage["key"] = str(uuid.uuid4())
                            profile_changed = True

                logger.info(f"Profile {profile['id']} was updated on load")

                errors = ProfileManager.validate_profile(profile)
                if errors is not None:
                    logger.warning(
                        f"Profile on disk failed to be loaded: {errors.message}"
                    )
                    continue

                if profile_changed:
                    try:
                        ProfileManager.save_profile(
                            profile, set_last_changed=True, skip_validation=True
                        )
                    except Exception:
                        continue

                id = profile["id"]

                if (
                    id in ProfileManager._known_profiles
                    and ProfileManager._known_profiles[id]["last_changed"]
                    >= profile["last_changed"]
                ):
                    continue

                ProfileManager._known_profiles[profile["id"]] = profile
        end = time.time()
        time_ms = (end - start) * 1000
        if time_ms > 10:
            time_str = f"{int(time_ms)} ms"
        else:
            time_str = f"{int(time_ms*1000)} ns"
        logger.info(
            f"Refreshed profile list in {time_str} with {len(ProfileManager._known_profiles)} known profiles."
        )
        ProfileManager._emit_profile_event(PROFILE_EVENT.RELOAD)

    def refresh_default_profile_list():
        logger.info("Refreshing default profiles")
        start = time.time()
        ProfileManager._default_profiles = []
        files = os.listdir(DEFAULT_PROFILES_PATH)
        files.sort()
        for filename in files:
            if not filename.endswith(".json"):
                continue

            file_path = os.path.join(DEFAULT_PROFILES_PATH, filename)
            with open(file_path, "r") as f:
                try:
                    profile = json.load(f)
                except json.decoder.JSONDecodeError as error:
                    logger.warning(
                        f"Could not decode default profile {f.name}: {error}"
                    )
                    continue
                logger.info("Found default profile: " + filename)
                ProfileManager._default_profiles.append(profile)

        end = time.time()
        time_ms = (end - start) * 1000
        if time_ms > 10:
            time_str = f"{int(time_ms)} ms"
        else:
            time_str = f"{int(time_ms*1000)} ns"
        logger.info(
            f"Refreshed default profile list in {time_str} with {len(ProfileManager._default_profiles)} known profiles."
        )

    def refresh_image_list():
        logger.info("Refreshing default image list")
        ProfileManager._profile_default_images = []
        if not os.path.exists(DEFAULT_IMAGES_PATH):
            os.makedirs(DEFAULT_IMAGES_PATH)
            logger.error("Missing default images path!")

        if not os.path.exists(IMAGES_PATH):
            os.makedirs(IMAGES_PATH)

        for filename in os.listdir(DEFAULT_IMAGES_PATH):
            file_path = os.path.join(DEFAULT_IMAGES_PATH, filename)
            if os.path.isfile(file_path):
                file_extension = os.path.splitext(filename)[1].lower()
                if file_extension not in [
                    ".png",
                    ".jpg",
                    ".jpeg",
                    ".gif",
                    ".webm",
                    ".webp",
                ]:
                    continue
                md5_hash = ProfileManager._get_md5_hash(file_path)
                new_filename = f"{md5_hash}{file_extension}"
                dst_path = os.path.join(IMAGES_PATH, new_filename)
                shutil.copy2(file_path, dst_path)
                ProfileManager._profile_default_images.append(new_filename)
        logger.info(
            f"Found {len(ProfileManager._profile_default_images)} default images"
        )

        ProfileManager._known_images = os.listdir(IMAGES_PATH)

    def _delete_unused_images():
        logger.info("Garbage collecting unused images")

        referenced_images: Set[str] = set()
        for profile in ProfileManager._known_profiles.values():
            image_url = profile.get("display", {}).get("image", "")
            if image_url.startswith("/api/v1/profile/image/"):
                referenced_images.add(image_url.split("/")[-1])

        for image_filename in ProfileManager._known_images:
            if image_filename in ProfileManager._profile_default_images:
                continue
            if image_filename in referenced_images:
                continue

            try:
                os.remove(os.path.join(IMAGES_PATH, image_filename))
                logger.info(f"Deleted unreferenced image: {image_filename}")
            except Exception as e:
                logger.error(f"Error deleting file {image_filename}: {e}")
        ProfileManager.refresh_image_list()

    def get_default_images():
        return ProfileManager._profile_default_images

    def list_profiles():
        return ProfileManager._known_profiles

    def list_default_profiles():
        return ProfileManager._default_profiles

    def get_last_profile():
        return MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST]

    def _get_md5_hash(image_path):
        hash_md5 = hashlib.md5()
        with open(image_path, "rb") as f:
            for chunk in iter(lambda: f.read(4096), b""):
                hash_md5.update(chunk)
        return hash_md5.hexdigest()

    def validate_profile(data):

        try:
            ProfilePreprocessor.processVariables(data)
        except Exception as err:
            logger.info(
                f"Profile variables could not be processed: {err.__class__.__name__}: {err}"
            )
            return err

        if not ProfileManager._schema:
            logger.warning("No schema available, not validating")
            return None

        logger.info(f"validating profile: {data['id']}")
        try:
            jsonschema.validate(instance=data, schema=ProfileManager._schema)
            logger.info("JSON data is valid.")
        except jsonschema.exceptions.ValidationError as err:
            logger.error(f"JSON validation error: {err.message}")
            for error in sorted(err.context, key=lambda e: e.schema_path):
                logger.error(f"Schema path: {list(error.schema_path)}")
                logger.error(f"Message: {error.message}")
            return err

        return None
