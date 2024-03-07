import os
import json
import uuid
import time

from config import *

from log import MeticulousLogger
from profile_converter.profile_converter import ComplexProfileConverter
from machine import Machine

logger = MeticulousLogger.getLogger(__name__)

PROFILE_PATH = os.getenv("PROFILE_PATH", '/meticulous-user/profiles')

class ProfileManager:
    _known_profiles = dict()

    def init():
        if not os.path.exists(PROFILE_PATH):
            os.makedirs(PROFILE_PATH)
        ProfileManager.refresh_profile_list()

    def save_profile(data, set_last_changed = False):
        if "id" not in data:
            data["id"] = str(uuid.uuid4())

        name = f'{data["id"]}.json'

        if set_last_changed:
            data["last_changed"] = time.time()

        file_path = os.path.join(PROFILE_PATH, name)
        with open(file_path, 'w') as f:
            json.dump(data, f, indent=4)

        ProfileManager._known_profiles[data["id"]] = data
        logger.info(f"Saved profile {name}")
        return (name, data["id"])

    def delete_profile(id):
        profile = ProfileManager._known_profiles.get(id)
        if not profile:
            return None

        filename = f'{profile["id"]}.json'
        file_path = os.path.join(PROFILE_PATH, filename)
        os.remove(file_path)
        del ProfileManager._known_profiles[profile["id"]]
        return {"profile": profile["name"], "id": profile["id"]}

    def get_profile(id):
        logger.info(f"Serving profile: {id}")
        logger.info(ProfileManager._known_profiles.get(id))
        return ProfileManager._known_profiles.get(id)

    def load_profile(id):
        profile = ProfileManager._known_profiles.get(id)
        if profile is not None:
            ProfileManager.send_profile_to_esp32(profile)
        return profile

    def send_profile_to_esp32(data):
        click_to_start = not MeticulousConfig[CONFIG_USER][PROFILE_AUTO_START]
        click_to_purge = not MeticulousConfig[CONFIG_USER][PROFILE_AUTO_PURGE]
        
        logger.info(f"Streaming JSON to ESP32: click_to_start={click_to_start} click_to_purge={click_to_purge} data={json.dumps(data)}")

        converter = ComplexProfileConverter(click_to_start, click_to_purge, 1000, 7000, data)
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

                if "id" not in profile:
                    profile["id"] = str(uuid.uuid4())
                    ProfileManager.save_profile(profile)
                if "last_changed" not in profile:
                    ProfileManager.save_profile(profile, set_last_changed = True)
                id =  profile["id"] 

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

    def list_profiles():
        return ProfileManager._known_profiles
