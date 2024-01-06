import os
import json
import uuid

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

class ProfileManager:
    _storage_dir = "./profiles"
    _known_profiles = dict()

    def init():
        if not os.path.exists(ProfileManager._storage_dir):
            os.makedirs(ProfileManager._storage_dir)

    def save_profile(name, data):
        if "uuid" not in data:
            data["uuid"] = str(uuid.uuid4())

        file_path = os.path.join(ProfileManager._storage_dir, f"{name}.json")
        with open(file_path, 'w') as f:
            json.dump(data, f)
        ProfileManager._known_profiles[name] = data
        logger.info(f"Saved profile {name}.json")
        return (name, data["uuid"])

    def get_profile(name):
        file_path = os.path.join(ProfileManager._storage_dir, f"{name}.json")

        if os.path.exists(file_path):
            with open(file_path, 'r') as f:
                logger.info(f"Sending profile profile {name}.json")
                return json.load(f)
        else:
            logger.info(f"Requested profile does not exist")
            return None

    def list_profiles():
        for filename in os.listdir(ProfileManager._storage_dir):
            if filename.endswith('.json'):
                name = filename[:-5] 
                if name not in ProfileManager._known_profiles:
                    file_path = os.path.join(ProfileManager._storage_dir, filename)
                    with open(file_path, 'r') as f:
                        profile = json.load(f)
                        if "uuid" not in profile:
                            profile["uuid"] = str(uuid.uuid4())
                            ProfileManager.save_profile(name, profile)
                        ProfileManager._known_profiles[name] = profile
        response = dict()
        for profile in ProfileManager._known_profiles:
            p = ProfileManager._known_profiles[profile].copy()
            if "stages" in p:
                del p["stages"]
            response[profile] = p
        return response