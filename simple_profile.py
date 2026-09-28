import json
import os
from typing import Optional

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

SIMPLE_PROFILE_PATH = os.getenv(
    "SIMPLE_PROFILE",
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "simple_profile.json"),
)


class SimpleProfile:
    """The bundled profile used for limited access and profile creation."""

    _profile: Optional[dict] = None
    _loaded_from: Optional[str] = None

    @classmethod
    def load(cls) -> Optional[dict]:
        try:
            with open(SIMPLE_PROFILE_PATH, "r", encoding="utf-8") as profile_file:
                profile = json.load(profile_file)
        except (OSError, json.JSONDecodeError) as error:
            logger.error(f"Could not load simple profile from {SIMPLE_PROFILE_PATH}: {error}")
            cls._profile = None
            cls._loaded_from = SIMPLE_PROFILE_PATH
            return None
        if not isinstance(profile, dict) or not isinstance(profile.get("id"), str):
            logger.error("Simple profile is not a profile object with an id")
            cls._profile = None
            cls._loaded_from = SIMPLE_PROFILE_PATH
            return None
        cls._profile = profile
        cls._loaded_from = SIMPLE_PROFILE_PATH
        logger.info(f"Loaded simple profile {profile['id']} ({profile.get('name')})")
        return profile

    @classmethod
    def get(cls) -> Optional[dict]:
        if cls._profile is None or cls._loaded_from != SIMPLE_PROFILE_PATH:
            cls.load()
        return cls._profile

    @classmethod
    def get_by_id(cls, profile_id: str) -> Optional[dict]:
        profile = cls.get()
        if profile is not None and profile.get("id") == profile_id:
            return profile
        return None

    @classmethod
    def list(cls, full: bool) -> list:
        profile = cls.get()
        if profile is None:
            return []
        entry = dict(profile)
        if not full:
            entry.pop("stages", None)
        return [entry]

    @classmethod
    def defaults(cls) -> dict:
        profile = cls.get()
        return {"default": [profile] if profile is not None else [], "community": []}
