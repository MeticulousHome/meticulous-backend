import os
import json
import uuid
import datetime
import time

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)


class ProfileManager:
    _storage_dir = "./profiles"
    _known_profiles = dict()

    def init():
        if not os.path.exists(ProfileManager._storage_dir):
            os.makedirs(ProfileManager._storage_dir)
        ProfileManager.refresh_profile_list()

    def save_profile(data, set_last_changed = False):
        if "id" not in data:
            data["id"] = str(uuid.uuid4())
        name = f'{data["name"]}_{data["id"]}.json'

        if set_last_changed:
            data["last_changed"] = int(datetime.datetime.now(
                        datetime.timezone.utc).timestamp())  # Ensuring it's in UTC

        file_path = os.path.join(ProfileManager._storage_dir, name)
        with open(file_path, 'w') as f:
            json.dump(data, f, indent=4)
        ProfileManager._known_profiles[name] = data
        logger.info(f"Saved profile {name}")
        return (name, data["id"])

    def get_profile(id):
        return ProfileManager._known_profiles.get(id)

    def load_profile(id):
        profile = ProfileManager._known_profiles.get(id)
        if profile is not None:
            logger.warning("FIXME: send profile to ESP32")
        return profile

    def refresh_profile_list():
        start = time.time()
        for filename in os.listdir(ProfileManager._storage_dir):
            if not filename.endswith('.json'):
                continue

            file_path = os.path.join(ProfileManager._storage_dir, filename)
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
