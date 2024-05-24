import json
import os
import threading
import time
import uuid
from enum import Enum
from typing import Optional

import socketio
from config import *
from log import MeticulousLogger
from machine import Machine
from modes.italian_1_0.italian_1_0 import (convert_italian_json,
                                           generate_italian_1_0)
from profile_converter.profile_converter import ComplexProfileConverter
from profile_preprocessor import ProfilePreprocessor

logger = MeticulousLogger.getLogger(__name__)

PROFILE_PATH = os.getenv("PROFILE_PATH", '/meticulous-user/profiles')


class PROFILE_CHANGE(Enum):
    CREATE = "create"
    UPDATE = "update"
    DELETE = "delete"
    RELOAD = "full_reload"


class ProfileManager:
    _known_profiles = dict()
    _sio: socketio.AsyncServer = None
    _loop: asyncio.AbstractEventLoop = None
    _thread: threading.Thread = None
    _last_profile_changes = []

    def init(sio: socketio.AsyncServer):
        ProfileManager._sio = sio

        ProfileManager._loop = asyncio.new_event_loop()

        def start_event_loop() -> None:
            asyncio.set_event_loop(ProfileManager._loop)
            ProfileManager._loop.run_forever()

        ProfileManager._thread = threading.Thread(
            target=start_event_loop)
        ProfileManager._thread.start()

        if not os.path.exists(PROFILE_PATH):
            os.makedirs(PROFILE_PATH)

        ProfileManager.refresh_profile_list()

    def _register_profile_change(change: PROFILE_CHANGE,
                                 profile_id: str, timestamp: Optional[float] = None) -> str:
        changes_to_keep = 100

        if timestamp is None:
            timestamp = time.time()
        change_id = str(uuid.uuid4())
        change_entry = {
            "type": change,
            "profile_id": profile_id,
            "change_id": change_id,
            "timestamp": timestamp,
        }

        changes = ProfileManager._last_profile_changes
        changes .append(change_entry)
        if len(changes) > changes_to_keep:
            changes[:] = changes[-changes_to_keep:]
        ProfileManager._last_profile_changes = changes
        return change_id

    def _emit_profile_change(change: PROFILE_CHANGE,
                             profile_id: Optional[str] = None,
                             change_id: Optional[str] = None) -> None:

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

    def get_profile_changes() -> list[object]:
        return ProfileManager._last_profile_changes

    def save_profile(data, set_last_changed=False):
        if "id" not in data or data["id"] == "":
            data["id"] = str(uuid.uuid4())

        name = f'{data["id"]}.json'

        current_time = time.time()
        if set_last_changed:
            data["last_changed"] = current_time

        is_update = ProfileManager._known_profiles.get(data["id"]) is not None

        file_path = os.path.join(PROFILE_PATH, name)
        with open(file_path, 'w') as f:
            json.dump(data, f, indent=4)

        ProfileManager._known_profiles[data["id"]] = data
        logger.info(f"Saved profile {name}")
        if is_update:
            change_type = PROFILE_CHANGE.UPDATE
        else:
            change_type = PROFILE_CHANGE.CREATE

        change_id = ProfileManager._register_profile_change(
            change_type, data["id"], current_time)

        ProfileManager._emit_profile_change(
            change_type, data["id"], change_id)

        return {"name": name, "id": data["id"], "change_id": change_id}

    def delete_profile(id):
        profile = ProfileManager._known_profiles.get(id)
        if not profile:
            return None

        filename = f'{profile["id"]}.json'
        file_path = os.path.join(PROFILE_PATH, filename)
        os.remove(file_path)
        del ProfileManager._known_profiles[profile["id"]]
        change_id = ProfileManager._register_profile_change(
            PROFILE_CHANGE.DELETE, profile["id"])

        ProfileManager._emit_profile_change(
            PROFILE_CHANGE.DELETE, profile["id"])

        return {"profile": profile["name"], "id": profile["id"], "change_id": change_id}

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
        click_to_start = not MeticulousConfig[CONFIG_USER][PROFILE_AUTO_START]
        click_to_purge = not MeticulousConfig[CONFIG_USER][PROFILE_AUTO_PURGE]

        if "id" not in data:
            data["id"] = "00000000-0000-0000-0000-000000000000"

        logger.info(f"Recieved data: {data} {type(data)}")

        profile_legacy_kind = data.get('kind', None)
        logger.info(f"profile_legacy_kind {profile_legacy_kind}")

        if profile_legacy_kind and profile_legacy_kind == "italian_1_0":
            logger.info("loading italian profile")
            data["automatic_purge"] = not click_to_purge
            data["preheat"] = click_to_start
            logger.info(data)
            json_result = convert_italian_json(data)
            converter = ComplexProfileConverter(
                click_to_start, click_to_purge, 1000, 7000, json_result)
            profile = converter.get_profile()
            Machine.send_json_with_hash(profile)
            return data

        logger.info("processing simplified profile")
        preprocessed_profile = ProfilePreprocessor.processVariables(data)

        logger.info(
            f"Streaming JSON to ESP32: click_to_start={click_to_start} click_to_purge={click_to_purge} data={json.dumps(preprocessed_profile)}")

        converter = ComplexProfileConverter(
            click_to_start, click_to_purge, 1000, 7000, preprocessed_profile)
        profile = converter.get_profile()
        Machine.send_json_with_hash(profile)
        return data

    def refresh_profile_list():
        start = time.time()
        ProfileManager._known_profiles = dict()
        for filename in os.listdir(PROFILE_PATH):
            if not filename.endswith('.json'):
                continue

            file_path = os.path.join(PROFILE_PATH, filename)
            with open(file_path, 'r') as f:
                try:
                    profile = json.load(f)
                except json.decoder.JSONDecodeError as error:
                    logger.warning(
                        f"Could not decode profile {f.name}: {error}")
                    continue

                if "id" not in profile or profile["id"] == "":
                    profile["id"] = str(uuid.uuid4())
                    ProfileManager.save_profile(profile)
                if "last_changed" not in profile:
                    ProfileManager.save_profile(profile, set_last_changed=True)
                id = profile["id"]

                if id in ProfileManager._known_profiles and ProfileManager._known_profiles[id]["last_changed"] >= profile["last_changed"]:
                    continue

                ProfileManager._known_profiles[profile["id"]] = profile
        end = time.time()
        time_ms = (end-start)*1000
        if time_ms > 10:
            time_str = f"{int(time_ms)} ms"
        else:
            time_str = f"{int(time_ms*1000)} ns"
        logger.info(
            f"Refreshed profile list in {time_str} with {len(ProfileManager._known_profiles)} known profiles.")
        ProfileManager._emit_profile_change(PROFILE_CHANGE.RELOAD)

    def list_profiles():
        return ProfileManager._known_profiles
