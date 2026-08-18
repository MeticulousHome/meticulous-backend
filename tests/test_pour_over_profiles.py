import json
import os
import sys
import tempfile
import types
from pathlib import Path

import pytest
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

os.environ.setdefault("MOTOR_ENERGY_PATH", "/tmp/meticulous-test/motor-energy")

try:
    import pyprctl  # noqa: F401
except Exception:
    sys.modules["pyprctl"] = types.SimpleNamespace(set_name=lambda _name: None)

try:
    import gpiod  # noqa: F401
except Exception:

    class _LineRequest:
        DIRECTION_OUTPUT = 1

    sys.modules["gpiod"] = types.SimpleNamespace(line_request=_LineRequest)

import pour_over_profiles  # noqa: E402
from api.pour_over_profiles import (  # noqa: E402
    DeletePourOverProfileHandler,
    GetPourOverProfileHandler,
    GetPourOverProfileImageHandler,
    ListPourOverProfilesHandler,
    PourOverProfileSchemaHandler,
)
from api.profiles import SaveProfileHandler  # noqa: E402
from pour_over_profiles import (  # noqa: E402
    MAX_POUR_OVER_PROFILE_BYTES,
    PourOverProfileManager,
    PourOverProfileValidationError,
)


def valid_pour_over_profile():
    return {
        "version": 1,
        "brew_type": "pour_over",
        "id": "11111111-2222-4333-8444-555555555555",
        "name": "Lance 2 pour",
        "author": "Recipe author",
        "author_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "display": {
            "accentColor": "#23383F",
            "shortDescription": "A bright three-stage recipe",
            "description": "Two smaller pours followed by the main pour.",
        },
        "recipe": {
            "coffee_dose_g": 15,
            "total_water_g": 225,
            "water_temperature_c": 92,
            "target_total_time_s": 135,
            "target_total_time_max_s": 180,
        },
        "stages": [
            {
                "key": "pour-1",
                "name": "Pour 1",
                "starts_at_s": 0,
                "pour": {
                    "water_g": 35,
                    "duration_s": 10,
                    "target_cumulative_water_g": 35,
                    "flow_rate_g_s": 3.5,
                    "pattern": "spiral_out",
                },
            },
            {
                "key": "pour-2",
                "name": "Pour 2",
                "starts_at_s": 30,
                "pour": {
                    "water_g": 35,
                    "duration_s": 10,
                    "target_cumulative_water_g": 70,
                    "flow_rate_g_s": 3.5,
                    "flow_range_g_s": [3, 4],
                },
            },
            {
                "key": "pour-3",
                "name": "Pour 3",
                "starts_at_s": 60,
                "pour": {
                    "water_g": 155,
                    "duration_s": 31,
                    "target_cumulative_water_g": 225,
                    "flow_rate_g_s": 5,
                },
            },
        ],
    }


@pytest.fixture
def profile_directory(monkeypatch, tmp_path):
    monkeypatch.setattr(pour_over_profiles, "POUR_OVER_PROFILE_PATH", tmp_path)
    monkeypatch.setattr(pour_over_profiles, "MIN_FREE_PROFILE_BYTES", 0)
    PourOverProfileManager._known_profiles = {}
    PourOverProfileManager.init()
    yield tmp_path
    PourOverProfileManager._known_profiles = {}


def test_profile_validation_and_atomic_persistence(profile_directory):
    result = PourOverProfileManager.save_profile(valid_pour_over_profile())

    assert result["profile"]["recipe"]["coffee_dose_g"] == 15
    stored = profile_directory.joinpath("11111111-2222-4333-8444-555555555555.json")
    assert stored.is_file()
    assert json.loads(stored.read_text())["stages"][2]["starts_at_s"] == 60
    assert list(profile_directory.glob("*.tmp")) == []


def test_documented_example_matches_the_canonical_contract(profile_directory):
    example_path = (
        Path(__file__).parents[1].joinpath("pour_over_profile_schema/example_profile.json")
    )

    validated = PourOverProfileManager.validate_profile(
        json.loads(example_path.read_text(encoding="utf-8"))
    )

    assert validated["brew_type"] == "pour_over"
    assert len(validated["stages"]) == 2


def test_debian_package_includes_canonical_pour_over_schema():
    dockerfile = (
        Path(__file__).parents[1].joinpath("Dockerfile.deb").read_text(encoding="utf-8")
    )

    assert "opt/meticulous-backend/pour_over_profile_schema" in dockerfile
    assert (
        "COPY pour_over_profile_schema/ "
        "/pkg/opt/meticulous-backend/pour_over_profile_schema/"
    ) in dockerfile


@pytest.mark.parametrize(
    ("mutate", "expected_path"),
    [
        (
            lambda profile: profile["recipe"].update(coffee_dose_g=4.9),
            ["recipe", "coffee_dose_g"],
        ),
        (
            lambda profile: profile["recipe"].update(water_temperature_c=69),
            ["recipe", "water_temperature_c"],
        ),
        (
            lambda profile: profile["stages"][0].update(starts_at_s=1),
            ["stages", 0, "starts_at_s"],
        ),
        (
            lambda profile: profile["stages"][1]["pour"].update(target_cumulative_water_g=71),
            ["stages", 1, "pour", "target_cumulative_water_g"],
        ),
        (
            lambda profile: profile["stages"][2]["pour"].update(flow_rate_g_s=8),
            ["stages", 2, "pour", "flow_rate_g_s"],
        ),
        (lambda profile: profile.update(unknown=True), []),
    ],
)
def test_rejects_schema_and_semantic_inconsistency(profile_directory, mutate, expected_path):
    profile = valid_pour_over_profile()
    mutate(profile)

    with pytest.raises(PourOverProfileValidationError) as error:
        PourOverProfileManager.save_profile(profile)

    assert expected_path in [issue["path"] for issue in error.value.issues]
    assert list(profile_directory.glob("*.json")) == []


def test_rejects_overlap_and_total_time_before_final_pour(profile_directory):
    profile = valid_pour_over_profile()
    profile["stages"][1]["starts_at_s"] = 5
    profile["recipe"]["target_total_time_s"] = 80

    with pytest.raises(PourOverProfileValidationError) as error:
        PourOverProfileManager.save_profile(profile)

    codes = {issue["code"] for issue in error.value.issues}
    assert "overlap" in codes
    assert "time_consistency" in codes


def test_refresh_skips_corrupt_or_filename_mismatched_profiles(profile_directory):
    profile_directory.joinpath("broken.json").write_text("{not json")
    profile_directory.joinpath("wrong-id.json").write_text(
        json.dumps(valid_pour_over_profile())
    )

    PourOverProfileManager.refresh_profile_list()

    assert PourOverProfileManager.list_profiles() == []


def test_optional_feature_init_failure_does_not_raise_or_advertise_available(
    monkeypatch, tmp_path
):
    previous_schema = PourOverProfileManager._schema
    previous_validator = PourOverProfileManager._validator
    previous_available = PourOverProfileManager._available
    monkeypatch.setattr(
        pour_over_profiles,
        "POUR_OVER_PROFILE_SCHEMA_PATH",
        tmp_path.joinpath("missing-schema.json"),
    )
    PourOverProfileManager._schema = None
    PourOverProfileManager._validator = None

    PourOverProfileManager.init()

    assert PourOverProfileManager.is_available() is False
    PourOverProfileManager._schema = previous_schema
    PourOverProfileManager._validator = previous_validator
    PourOverProfileManager._available = previous_available


class TestPourOverProfilesAPI(AsyncHTTPTestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.previous_path = pour_over_profiles.POUR_OVER_PROFILE_PATH
        self.previous_min_free = pour_over_profiles.MIN_FREE_PROFILE_BYTES
        pour_over_profiles.POUR_OVER_PROFILE_PATH = Path(self.temporary.name)
        pour_over_profiles.MIN_FREE_PROFILE_BYTES = 0
        PourOverProfileManager._known_profiles = {}
        PourOverProfileManager.init()
        super().setUp()

    def tearDown(self):
        super().tearDown()
        pour_over_profiles.POUR_OVER_PROFILE_PATH = self.previous_path
        pour_over_profiles.MIN_FREE_PROFILE_BYTES = self.previous_min_free
        PourOverProfileManager._known_profiles = {}
        self.temporary.cleanup()

    def get_app(self):
        return Application(
            [
                (r"/api/v1/profile/save", SaveProfileHandler),
                (r"/api/v1/pour-over/profile/list", ListPourOverProfilesHandler),
                (
                    r"/api/v1/pour-over/profile/get/([0-9a-fA-F-]+)",
                    GetPourOverProfileHandler,
                ),
                (
                    r"/api/v1/pour-over/profile/image/([0-9a-fA-F-]+)",
                    GetPourOverProfileImageHandler,
                ),
                (
                    r"/api/v1/pour-over/profile/delete/([0-9a-fA-F-]+)",
                    DeletePourOverProfileHandler,
                ),
                (r"/api/v1/pour-over/profile/schema", PourOverProfileSchemaHandler),
            ]
        )

    def test_existing_profile_save_endpoint_dispatches_pour_over(self):
        profile = valid_pour_over_profile()
        profile["display"]["image"] = "data:image/jpeg;base64,/9j/2Q=="
        saved = self.fetch(
            "/api/v1/profile/save",
            method="POST",
            body=json.dumps(profile),
        )
        assert saved.code == 200
        assert json.loads(saved.body)["profile"]["brew_type"] == "pour_over"
        assert "image" not in PourOverProfileManager._known_profiles[profile["id"]]["display"]

        listed = json.loads(self.fetch("/api/v1/pour-over/profile/list").body)
        assert len(listed["profiles"]) == 1
        assert "image" not in listed["profiles"][0].get("display", {})

        fetched = self.fetch(f"/api/v1/pour-over/profile/get/{profile['id']}")
        assert fetched.code == 200
        assert json.loads(fetched.body)["name"] == profile["name"]
        assert json.loads(fetched.body)["display"]["image"].startswith(
            "data:image/jpeg;base64,"
        )

        without_image = self.fetch(
            f"/api/v1/pour-over/profile/get/{profile['id']}?include_image=false"
        )
        assert "image" not in json.loads(without_image.body).get("display", {})

        image = self.fetch(f"/api/v1/pour-over/profile/image/{profile['id']}")
        assert image.code == 200
        assert image.headers["Content-Type"].startswith("image/jpeg")
        assert image.body == b"\xff\xd8\xff\xd9"

        deleted = self.fetch(
            f"/api/v1/pour-over/profile/delete/{profile['id']}",
            method="DELETE",
        )
        assert deleted.code == 200
        assert self.fetch(f"/api/v1/pour-over/profile/get/{profile['id']}").code == 404

    def test_profile_image_endpoint_returns_404_without_an_image(self):
        profile = valid_pour_over_profile()
        saved = self.fetch("/api/v1/profile/save", method="POST", body=json.dumps(profile))
        assert saved.code == 200

        response = self.fetch(f"/api/v1/pour-over/profile/image/{profile['id']}")

        assert response.code == 404

    def test_delete_endpoint_does_not_mutate_on_get(self):
        profile = valid_pour_over_profile()
        assert (
            self.fetch("/api/v1/profile/save", method="POST", body=json.dumps(profile)).code
            == 200
        )

        response = self.fetch(f"/api/v1/pour-over/profile/delete/{profile['id']}")

        assert response.code == 405
        assert PourOverProfileManager.get_profile(profile["id"]) is not None

    def test_profile_api_returns_503_when_optional_storage_is_unavailable(self):
        PourOverProfileManager._available = False
        try:
            response = self.fetch("/api/v1/pour-over/profile/list")
            assert response.code == 503
            assert json.loads(response.body)["error"] == "Pour Over profiles are unavailable"
        finally:
            PourOverProfileManager._available = True

    def test_invalid_profile_returns_field_specific_422(self):
        profile = valid_pour_over_profile()
        profile["recipe"]["coffee_dose_g"] = 41

        response = self.fetch(
            "/api/v1/profile/save",
            method="POST",
            body=json.dumps(profile),
        )

        assert response.code == 422
        body = json.loads(response.body)
        assert body["error"] == "Invalid Pour Over profile"
        assert ["recipe", "coffee_dose_g"] in [issue["path"] for issue in body["details"]]

    def test_oversized_pour_over_body_is_rejected_before_json_parsing(self):
        body = (
            b'{"brew_type":"pour_over","padding":"' + b"x" * MAX_POUR_OVER_PROFILE_BYTES + b'"}'
        )

        response = self.fetch(
            "/api/v1/profile/save",
            method="POST",
            body=body,
        )

        assert response.code == 413
        assert json.loads(response.body)["error"] == "Pour Over profile is too large"

    def test_schema_endpoint_reports_version_one(self):
        response = self.fetch("/api/v1/pour-over/profile/schema")
        assert response.code == 200
        assert json.loads(response.body)["properties"]["version"]["const"] == 1
