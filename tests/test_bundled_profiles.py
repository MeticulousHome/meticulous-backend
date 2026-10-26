"""Profiles the backend ships must pass the validation it applies to every profile.

simple_profile.json is served as the default and used for limited access, and
the schema repository's example is what clients are pointed to. Both go through
ProfileManager.validate_profile, the check the save and load handlers run.
"""

import copy
import json
from pathlib import Path

import pytest

from profiles import ProfileManager
from simple_profile import SIMPLE_PROFILE_PATH, SimpleProfile

REPO_ROOT = Path(__file__).resolve().parent.parent
SCHEMA = REPO_ROOT / "profile_schema" / "schema.json"


@pytest.fixture
def schema(monkeypatch):
    monkeypatch.setattr(ProfileManager, "_schema", json.loads(SCHEMA.read_text()))


@pytest.mark.parametrize(
    "path",
    [Path(SIMPLE_PROFILE_PATH), REPO_ROOT / "profile_schema" / "example_profile.json"],
    ids=lambda path: path.name,
)
def test_bundled_profile_passes_the_backend_validation(schema, path):
    profile = json.loads(path.read_text())

    error = ProfileManager.validate_profile(copy.deepcopy(profile))

    assert error is None, f"{path.name}: {error}"


def test_the_default_profile_served_is_the_bundled_one(schema):
    SimpleProfile.load()
    served = SimpleProfile.get()

    assert served == json.loads(Path(SIMPLE_PROFILE_PATH).read_text())
    assert ProfileManager.validate_profile(copy.deepcopy(served)) is None
