import json

import pytest

import simple_profile
from simple_profile import SimpleProfile

DEMO_ID = "83888510-ac09-44c6-af64-2bfa12550f60"


@pytest.fixture(autouse=True)
def reset_profile_cache():
    yield
    SimpleProfile._profile = None
    SimpleProfile._loaded_from = None


def test_bundled_profile_is_the_dial_demo():
    profile = SimpleProfile.load()
    assert profile["id"] == DEMO_ID
    assert profile["name"] == "9 Bar Demo"
    assert profile["temperature"] == 90.5
    assert profile["final_weight"] == 36
    assert len(profile["stages"]) == 1
    assert profile["display"]["image"].endswith("59894b10354b95395b434b49399966f2.png")


def test_list_strips_stages_unless_full():
    assert "stages" not in SimpleProfile.list(full=False)[0]
    assert SimpleProfile.list(full=True)[0]["stages"]


def test_defaults_have_only_the_simple_profile():
    defaults = SimpleProfile.defaults()
    assert [profile["id"] for profile in defaults["default"]] == [DEMO_ID]
    assert defaults["community"] == []


def test_missing_file_is_empty_not_fatal(monkeypatch, tmp_path):
    monkeypatch.setattr(simple_profile, "SIMPLE_PROFILE_PATH", str(tmp_path / "missing.json"))
    assert SimpleProfile.get() is None
    assert SimpleProfile.list(full=True) == []
    assert SimpleProfile.defaults() == {"default": [], "community": []}


def test_path_override(monkeypatch, tmp_path):
    custom = tmp_path / "custom.json"
    custom.write_text(json.dumps({"id": "custom", "name": "Custom", "stages": []}))
    monkeypatch.setattr(simple_profile, "SIMPLE_PROFILE_PATH", str(custom))
    assert SimpleProfile.get()["id"] == "custom"
