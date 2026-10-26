"""Handler-level regression tests for the espresso profile routes in api/profiles.py.

The handlers run against the real ProfileManager, with its files under tmp_path.
Only two boundaries are replaced: the socket.io server (emits are recorded) and
the ESP32 serial port (the bytes Machine.write sends are recorded).
"""

import asyncio
import base64
import copy
import hashlib
import json
import math
import shutil
import threading
import time
import uuid
from pathlib import Path
from types import SimpleNamespace

import pytest
import tornado.web
import yaml
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

import profiles
from api.alarms import Alarm, AlarmManager, AlarmType
from api.api import API, APIVersion
from config import (
    ALLOW_LEGACY_JSON,
    CONFIG_PROFILES,
    CONFIG_USER,
    PROFILE_LAST,
    PROFILE_ORDER,
    UPDATE_CHANNEL,
    MeticulousConfig,
)
from images.notificationImages.base64 import WARNING_TRIANGLE_IMAGE
from limited_access import LIMITED_ACCESS_CHANNEL
from machine import Machine
from profiles import (
    ESP32_PROFILE_FIELDS,
    MANUAL_MODE_PROFILE_ID,
    MANUAL_MODE_PROFILE_NAME,
    ProfileHover,
    ProfileManager,
)
from shot_database import ShotDataBase
from shot_debug_manager import ShotDebugManager

REPO_ROOT = Path(__file__).resolve().parent.parent
SCHEMA_PATH = REPO_ROOT / "profile_schema" / "schema.json"
EXAMPLE_PROFILE_PATH = REPO_ROOT / "profile_schema" / "example_profile.json"
SIMPLE_PROFILE_PATH = REPO_ROOT / "simple_profile.json"
DEFAULT_IMAGES_SOURCE = REPO_ROOT / "images" / "default"
DEFAULT_IMAGE_SOURCE = DEFAULT_IMAGES_SOURCE / "profile_default_1.png"

# The static image route is registered with the module constant at import time.
REGISTERED_IMAGES_PATH = profiles.IMAGES_PATH


def profile_routes():
    """(registered pattern, handler) exactly as api/profiles.py registers them.

    api.profiles is imported lazily: it pulls in ble_gatt, which
    tests/test_ble_gatt_wifi.py must import itself under its own stubs.
    """
    import api.profiles as handlers

    return [
        (r"/profile/list", handlers.ListHandler),
        (r"/profile/save", handlers.SaveProfileHandler),
        (r"/profile/from_manual", handlers.CreateProfileFromManualHandler),
        (r"/profile/load", handlers.LoadProfileHandler),
        (r"/profile/defaults", handlers.ListDefaultsHandler),
        (r"/profile/image([/]*)", handlers.ListImagesHandler),
        (r"/profile/image/(.*)", tornado.web.StaticFileHandler),
        (r"/profile/load/([0-9a-fA-F-]+)", handlers.LoadProfileHandler),
        (r"/profile/get/([0-9a-fA-F-]+)", handlers.GetProfileHandler),
        (r"/profile/delete/([0-9a-fA-F-]+)", handlers.DeleteProfileHandler),
        (r"/profile/changes", handlers.ChangesHandler),
        (r"/profile/last", handlers.LastProfileHandler),
        (r"/profile/selected", handlers.SelectedProfileHandler),
        (r"/profile/legacy", handlers.LegacyProfileHandler),
    ]


UNKNOWN_ID = "00000000-1111-4222-8333-444444444444"
MALFORMED_JSON_ERROR = (
    "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"
)
MACHINE_BUSY = {"status": "error", "error": "machine is busy"}
HIGH_STRAIN = {"status": "error", "error": "high strain on motor"}
MOTOR_STRESSED_MESSAGE = (
    "Brewing has been disabled because of a recent high strain on the motor, "
    "let it rest for 10 more minutes"
)
PNG_BYTES = b"\x89PNG\r\n\x1a\n-test-image-payload"


def example_profile():
    return json.loads(EXAMPLE_PROFILE_PATH.read_text())


def simple_profile():
    return json.loads(SIMPLE_PROFILE_PATH.read_text())


def file_md5(path):
    return hashlib.md5(Path(path).read_bytes()).hexdigest()


DEFAULT_IMAGE_NAME = f"{file_md5(DEFAULT_IMAGE_SOURCE)}.png"
DEFAULT_IMAGE_ACCENT = json.loads((DEFAULT_IMAGES_SOURCE / "accent_colors.json").read_text())[
    DEFAULT_IMAGE_NAME
]


def substitute_variables(node, values):
    if isinstance(node, dict):
        return {key: substitute_variables(value, values) for key, value in node.items()}
    if isinstance(node, list):
        return [substitute_variables(value, values) for value in node]
    if isinstance(node, str) and node.startswith("$"):
        return values[node[1:]]
    return node


def expected_esp32_payload(profile):
    """Only the runtime fields reach the ESP32, with `$variables` resolved."""
    values = {variable["key"]: variable["value"] for variable in profile.get("variables", [])}
    payload = {field: profile[field] for field in ESP32_PROFILE_FIELDS if field in profile}
    payload["stages"] = substitute_variables(profile["stages"], values)
    return payload


def expected_serial_writes(payload):
    """The `hash,<md5>\\x03` frame followed by the `json\\n<payload>\\x03` frame."""
    body = json.dumps(payload)
    digest = hashlib.md5(body.encode("utf-8")).hexdigest()
    return [b"hash,", digest.encode("utf-8"), b"\x03", f"json\n{body}\x03".encode("utf-8")]


class RecordingSocketIO:
    def __init__(self):
        self.emits = []

    async def emit(self, event, data=None, **kwargs):
        self.emits.append((event, data, kwargs) if kwargs else (event, data))


class RecordingPort:
    def __init__(self):
        self.writes = []

    def write(self, content):
        self.writes.append(bytes(content))


def test_espresso_profile_routes_are_registered_with_these_handlers():
    registered = API._versions[APIVersion.V1]
    for pattern, handler in profile_routes():
        assert registered[pattern][0] is handler, pattern
    assert registered[r"/profile/image/(.*)"][1] == {"path": REGISTERED_IMAGES_PATH}


class TestEspressoProfileRoutes(AsyncHTTPTestCase):
    @pytest.fixture(autouse=True)
    def profile_environment(self, tmp_path, monkeypatch):
        self.profile_path = tmp_path / "profiles"
        self.images_path = tmp_path / "profile-images"
        self.config_file = tmp_path / "config" / "config.yml"
        default_images = tmp_path / "default-images"
        for directory in (self.profile_path, self.images_path, default_images):
            directory.mkdir()
        shutil.copy2(DEFAULT_IMAGE_SOURCE, default_images)
        shutil.copy2(DEFAULT_IMAGES_SOURCE / "accent_colors.json", default_images)

        monkeypatch.setattr(profiles, "PROFILE_PATH", str(self.profile_path))
        monkeypatch.setattr(profiles, "IMAGES_PATH", str(self.images_path))
        import api.profiles as profiles_api

        monkeypatch.setattr(profiles_api, "IMAGES_PATH", str(self.images_path))
        monkeypatch.setattr(profiles, "DEFAULT_IMAGES_PATH", str(default_images))
        monkeypatch.setattr(
            profiles,
            "DEFAULT_IMAGES_PATH_ACCENT_COLORS",
            str(default_images / "accent_colors.json"),
        )

        config_snapshot = copy.deepcopy(dict(MeticulousConfig))
        monkeypatch.setattr(MeticulousConfig, "_MeticulousConfigDict__path", self.config_file)
        monkeypatch.setattr(MeticulousConfig, "_MeticulousConfigDict__sio", None)
        MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] = "stable"
        MeticulousConfig[CONFIG_USER][PROFILE_ORDER] = []
        MeticulousConfig[CONFIG_USER][ALLOW_LEGACY_JSON] = False
        MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST] = None

        # ProfileManager.init() runs its emits on a loop in a thread of its own.
        self.sio = RecordingSocketIO()
        self.sio_loop = asyncio.new_event_loop()
        loop_thread = threading.Thread(
            target=self.sio_loop.run_forever, name="ProfileManagerTest", daemon=True
        )
        loop_thread.start()
        manager_state = {
            "_sio": self.sio,
            "_loop": self.sio_loop,
            "_known_profiles": {},
            "_known_images": [],
            "_profile_default_images": [],
            "_profile_default_images_accent_colors": {},
            "_last_profile_changes": [],
            "_schema": json.loads(SCHEMA_PATH.read_text()),
            "_profile_hover": ProfileHover(),
        }
        for name, value in manager_state.items():
            monkeypatch.setattr(ProfileManager, name, value)
        ProfileManager.refresh_image_list()

        self.port = RecordingPort()
        monkeypatch.setattr(Machine, "_connection", SimpleNamespace(port=self.port))
        monkeypatch.setattr(Machine, "_stopESPcomm", False)
        monkeypatch.setattr(Machine, "is_idle", True)
        monkeypatch.setattr(Machine, "profileReady", False)
        # send_json_with_hash spins until the debug shot exists, then stores the payload.
        self.debug_shot = SimpleNamespace(nodeJSON=None)
        monkeypatch.setattr(ShotDebugManager, "_current_data", self.debug_shot)

        self.notifications = []
        monkeypatch.setattr(AlarmManager, "alarms", {})
        monkeypatch.setattr(
            AlarmManager,
            "_notify_user",
            staticmethod(lambda message, image: self.notifications.append((message, image))),
        )
        yield
        self.sio_loop.call_soon_threadsafe(self.sio_loop.stop)
        loop_thread.join(timeout=5)
        self.sio_loop.close()
        MeticulousConfig.clear()
        MeticulousConfig.update(config_snapshot)

    def get_app(self):
        routes = []
        for pattern, handler in profile_routes():
            kwargs = (
                {"path": str(self.images_path)} if pattern == r"/profile/image/(.*)" else {}
            )
            routes.append((f"/api/v1{pattern}", handler, kwargs))
        # API.get_routes() hands tornado the routes sorted by path.
        return Application(sorted(routes, key=lambda route: route[0]))

    # --- helpers -------------------------------------------------------------

    def post_json(self, path, payload, headers=None):
        body = payload if isinstance(payload, (str, bytes)) else json.dumps(payload)
        return self.fetch(f"/api/v1{path}", method="POST", body=body, headers=headers)

    def get_json(self, path):
        response = self.fetch(f"/api/v1{path}")
        assert response.code == 200, response.body
        return json.loads(response.body)

    def save(self, profile, change_id=None):
        headers = {"X-Change-Id": change_id} if change_id else None
        response = self.post_json("/profile/save", profile, headers=headers)
        assert response.code == 200, response.body
        return json.loads(response.body)

    def emitted(self):
        """Emits run on the manager loop; drain it before reading them."""

        async def settle():
            for _ in range(3):
                await asyncio.sleep(0)

        asyncio.run_coroutine_threadsafe(settle(), self.sio_loop).result(timeout=5)
        return list(self.sio.emits)

    def forget_emits(self):
        self.emitted()
        self.sio.emits.clear()

    def stored_files(self):
        return sorted(path.name for path in self.profile_path.iterdir())

    def stored_images(self):
        return sorted(path.name for path in self.images_path.iterdir())

    def assert_nothing_persisted(self):
        assert self.stored_files() == []
        assert self.stored_images() == [DEFAULT_IMAGE_NAME]
        assert ProfileManager._known_profiles == {}
        assert ProfileManager.get_profile_changes() == []
        assert MeticulousConfig[CONFIG_USER][PROFILE_ORDER] == []
        assert not self.config_file.exists()
        assert self.emitted() == []

    def assert_nothing_sent(self):
        assert self.port.writes == []
        assert self.debug_shot.nodeJSON is None
        assert MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST] is None
        assert self.emitted() == []

    # --- save ------------------------------------------------------------------

    def test_saving_a_valid_profile_persists_lists_and_announces_it(self):
        profile = example_profile()
        before = time.time()

        result = self.save(profile, change_id="client-change-1")

        after = time.time()
        assert result == {"profile": profile, "change_id": "client-change-1"}
        assert self.stored_files() == [f"{profile['id']}.json"]
        stored = json.loads((self.profile_path / f"{profile['id']}.json").read_text())
        assert stored == profile

        summary = {key: value for key, value in profile.items() if key != "stages"}
        assert self.get_json("/profile/list") == [summary]
        assert self.get_json("/profile/list?full=true") == [profile]
        assert self.get_json(f"/profile/get/{profile['id']}") == profile

        assert self.emitted() == [
            (
                "profile",
                {
                    "change": "create",
                    "profile_id": profile["id"],
                    "change_id": "client-change-1",
                },
            ),
            ("profileHover", {"id": profile["id"], "type": "focus", "from": "dial"}),
        ]
        changes = self.get_json("/profile/changes")
        assert len(changes) == 1
        timestamp = changes[0].pop("timestamp")
        assert before <= timestamp <= after
        assert changes == [
            {"type": "create", "profile_id": profile["id"], "change_id": "client-change-1"}
        ]
        assert self.get_json("/profile/selected") == {
            "id": profile["id"],
            "type": "focus",
            "from": "dial",
        }
        assert MeticulousConfig[CONFIG_USER][PROFILE_ORDER] == [profile["id"]]
        persisted_config = yaml.safe_load(self.config_file.read_text())
        assert persisted_config[CONFIG_USER][PROFILE_ORDER] == [profile["id"]]

    def test_saved_profile_survives_a_profile_manager_refresh(self):
        profile = simple_profile()
        self.save(profile)
        self.forget_emits()

        ProfileManager.refresh_profile_list()

        assert self.get_json(f"/profile/get/{profile['id']}") == profile
        assert self.get_json("/profile/list?full=true") == [profile]
        assert self.emitted() == [("profile", {"change": "full_reload"})]

    def test_refresh_stamps_last_changed_on_a_profile_saved_without_one(self):
        profile = example_profile()
        assert "last_changed" not in profile
        self.save(profile)

        before = time.time()
        ProfileManager.refresh_profile_list()
        after = time.time()

        reloaded = self.get_json(f"/profile/get/{profile['id']}")
        last_changed = reloaded.pop("last_changed")
        assert before <= last_changed <= after
        assert reloaded == profile
        on_disk = json.loads((self.profile_path / f"{profile['id']}.json").read_text())
        assert on_disk == dict(profile, last_changed=last_changed)

    def test_saving_an_existing_id_records_an_update_without_moving_the_selection(self):
        profile = example_profile()
        self.save(profile, change_id="first")
        self.forget_emits()
        renamed = dict(profile, name="Renamed E61")

        result = self.save(renamed, change_id="second")

        assert result == {"profile": renamed, "change_id": "second"}
        assert self.get_json(f"/profile/get/{profile['id']}")["name"] == "Renamed E61"
        assert self.stored_files() == [f"{profile['id']}.json"]
        assert self.emitted() == [
            (
                "profile",
                {"change": "update", "profile_id": profile["id"], "change_id": "second"},
            )
        ]
        assert [change["type"] for change in self.get_json("/profile/changes")] == [
            "create",
            "update",
        ]
        assert MeticulousConfig[CONFIG_USER][PROFILE_ORDER] == [profile["id"]]

    def test_save_without_id_or_image_assigns_a_uuid_and_the_default_image(self):
        profile = example_profile()
        del profile["id"]
        del profile["display"]

        result = self.save(profile)

        saved = result["profile"]
        assert uuid.UUID(saved["id"]).version == 4
        assert uuid.UUID(result["change_id"]).version == 4
        assert saved["display"] == {
            "image": f"/api/v1/profile/image/{DEFAULT_IMAGE_NAME}",
            "accentColor": DEFAULT_IMAGE_ACCENT,
        }
        assert saved == dict(profile, id=saved["id"], display=saved["display"])
        assert self.stored_files() == [f"{saved['id']}.json"]

    def test_save_stores_a_data_uri_image_under_its_md5_and_serves_it(self):
        profile = example_profile()
        profile["display"] = {
            "image": "data:image/png;base64," + base64.b64encode(PNG_BYTES).decode(),
            "accentColor": "#123456",
        }
        image_name = f"{hashlib.md5(PNG_BYTES).hexdigest()}.png"

        saved = self.save(profile)["profile"]

        assert saved["display"] == {
            "image": f"/api/v1/profile/image/{image_name}",
            "accentColor": "#123456",
        }
        assert self.stored_images() == sorted([DEFAULT_IMAGE_NAME, image_name])
        assert (self.images_path / image_name).read_bytes() == PNG_BYTES
        image = self.fetch(f"/api/v1/profile/image/{image_name}")
        assert image.code == 200
        assert image.headers["Content-Type"] == "image/png"
        assert image.body == PNG_BYTES

    def test_save_rejects_a_data_uri_that_is_not_an_image(self):
        profile = example_profile()
        profile["display"]["image"] = (
            "data:text/plain;base64," + base64.b64encode(b"x").decode()
        )

        response = self.post_json("/profile/save", profile)

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "failed to save profile",
            "cause": "Invalid image MIME type",
        }
        self.assert_nothing_persisted()

    def test_save_rejects_a_profile_missing_a_required_field(self):
        profile = example_profile()
        del profile["stages"]

        response = self.post_json("/profile/save", profile)

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "JSON validation error: 'stages' is a required property",
        }
        self.assert_nothing_persisted()
        assert self.fetch(f"/api/v1/profile/get/{profile['id']}").code == 404

    def test_save_rejects_a_temperature_above_the_schema_maximum(self):
        profile = example_profile()
        profile["temperature"] = 150

        response = self.post_json("/profile/save", profile)

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "JSON validation error: 150 is greater than the maximum of 100",
        }
        self.assert_nothing_persisted()

    def test_save_rejects_a_profile_with_an_undefined_variable(self):
        profile = example_profile()
        profile["stages"][0]["dynamics"]["points"][0][1] = "$missing_var"

        response = self.post_json("/profile/save", profile)

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "failed to save profile",
            "cause": "Variable missing_var is not defined",
        }
        self.assert_nothing_persisted()

    def test_save_rejects_malformed_json(self):
        response = self.post_json("/profile/save", "{")

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "failed to save profile",
            "cause": MALFORMED_JSON_ERROR,
        }
        self.assert_nothing_persisted()

    def test_save_rejects_non_finite_numbers(self):
        profile = example_profile()
        profile["temperature"] = math.nan

        response = self.post_json("/profile/save", json.dumps(profile))

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "failed to save profile",
            "cause": "Invalid number NaN",
        }
        self.assert_nothing_persisted()

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: ProfileManager.save_profile runs handle_image (profiles.py:415), "
            "which writes a data-URI image to IMAGES_PATH, before validate_profile "
            "(profiles.py:421); a rejected save leaves the uploaded image on disk"
        ),
    )
    def test_rejected_save_does_not_leave_its_uploaded_image_on_disk(self):
        profile = example_profile()
        del profile["stages"]
        profile["display"]["image"] = "data:image/png;base64," + base64.b64encode(
            PNG_BYTES
        ).decode("ascii")

        response = self.post_json("/profile/save", profile)

        assert response.code == 400
        assert self.stored_images() == [DEFAULT_IMAGE_NAME]

    # --- get / list ------------------------------------------------------------

    def test_get_of_an_unknown_profile_is_404(self):
        response = self.fetch(f"/api/v1/profile/get/{UNKNOWN_ID}")

        assert response.code == 404
        assert json.loads(response.body) == {
            "status": "error",
            "error": "profile not found",
            "id": UNKNOWN_ID,
        }

    def test_list_is_empty_without_profiles(self):
        assert self.get_json("/profile/list") == []
        assert self.get_json("/profile/list?full=true") == []

    def test_list_follows_the_configured_profile_order(self):
        first = example_profile()
        second = simple_profile()
        self.save(first)
        self.save(second)
        assert MeticulousConfig[CONFIG_USER][PROFILE_ORDER] == [first["id"], second["id"]]

        MeticulousConfig[CONFIG_USER][PROFILE_ORDER] = [second["id"], first["id"]]

        assert [p["id"] for p in self.get_json("/profile/list")] == [second["id"], first["id"]]
        assert self.get_json("/profile/list?full=true") == [second, first]

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: ProfileManager.list_profiles appends the stale loop variable `id` "
            "(profiles.py:744) instead of profile['id'] for a profile missing from "
            "profile_order, so the order gains a duplicate and never the missing profile"
        ),
    )
    def test_list_appends_a_profile_missing_from_the_order_to_the_order(self):
        first = example_profile()
        second = simple_profile()
        self.save(first)
        self.save(second)
        # profile_order is client-writable through the settings API.
        MeticulousConfig[CONFIG_USER][PROFILE_ORDER] = [first["id"]]

        assert [p["id"] for p in self.get_json("/profile/list")] == [first["id"], second["id"]]
        assert MeticulousConfig[CONFIG_USER][PROFILE_ORDER] == [first["id"], second["id"]]

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: with an empty profile_order, ProfileManager.list_profiles reads the "
            "never-bound loop variable `id` (profiles.py:744), raising UnboundLocalError, "
            "so GET /profile/list answers 500"
        ),
    )
    def test_list_with_an_empty_order_still_lists_the_profiles(self):
        profile = example_profile()
        self.save(profile)
        MeticulousConfig[CONFIG_USER][PROFILE_ORDER] = []

        response = self.fetch("/api/v1/profile/list")

        assert response.code == 200
        assert [p["id"] for p in json.loads(response.body)] == [profile["id"]]
        assert MeticulousConfig[CONFIG_USER][PROFILE_ORDER] == [profile["id"]]

    # --- load ------------------------------------------------------------------

    def test_loading_a_saved_profile_streams_only_its_runtime_fields_to_the_esp32(self):
        profile = example_profile()
        self.save(profile)
        self.forget_emits()
        before = time.time()

        response = self.fetch(f"/api/v1/profile/load/{profile['id']}")

        after = time.time()
        assert response.code == 200
        assert json.loads(response.body) == {"name": profile["name"], "id": profile["id"]}
        payload = expected_esp32_payload(profile)
        assert payload["stages"][1]["dynamics"]["points"] == [[0, 8]]
        assert self.port.writes == expected_serial_writes(payload)
        assert self.debug_shot.nodeJSON == payload
        assert Machine.profileReady is True
        assert self.emitted() == [
            ("profile", {"change": "load", "profile_id": profile["id"]}),
            ("profileHover", {"id": profile["id"], "type": "focus", "from": "dial"}),
        ]

        last = MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST]
        assert last["profile"] == profile
        assert before <= last["load_time"] <= after
        persisted = yaml.safe_load(self.config_file.read_text())
        assert persisted[CONFIG_PROFILES][PROFILE_LAST] == last
        assert self.get_json("/profile/last") == last
        assert self.stored_files() == [f"{profile['id']}.json"]

    def test_loading_a_profile_body_streams_it_without_saving_it(self):
        profile = simple_profile()

        response = self.post_json("/profile/load", profile)

        assert response.code == 200
        assert json.loads(response.body) == {"name": "Simple", "id": profile["id"]}
        payload = expected_esp32_payload(profile)
        assert payload["stages"][0]["dynamics"]["points"] == [[0, 10], [3, 10]]
        assert self.port.writes == expected_serial_writes(payload)
        assert self.emitted() == [
            ("profile", {"change": "load", "profile_id": profile["id"]}),
            ("profileHover", {"id": profile["id"], "type": "focus", "from": "dial"}),
        ]
        assert MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST]["profile"] == profile
        assert self.stored_files() == []
        assert self.get_json("/profile/list") == []
        assert ProfileManager.get_profile_changes() == []

    def test_loading_a_profile_body_without_an_id_assigns_one(self):
        profile = example_profile()
        del profile["id"]

        response = self.post_json("/profile/load", profile)

        assert response.code == 200
        body = json.loads(response.body)
        assert uuid.UUID(body["id"]).version == 4
        assert body == {"name": profile["name"], "id": body["id"]}
        assert self.port.writes == expected_serial_writes(expected_esp32_payload(profile))
        assert self.emitted()[0] == ("profile", {"change": "load", "profile_id": body["id"]})
        assert MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST]["profile"]["id"] == body["id"]

    def test_loading_while_the_machine_is_busy_is_refused(self):
        profile = example_profile()
        self.save(profile)
        self.forget_emits()
        Machine.is_idle = False

        by_id = self.fetch(f"/api/v1/profile/load/{profile['id']}")
        by_body = self.post_json("/profile/load", profile)

        assert (by_id.code, json.loads(by_id.body)) == (409, MACHINE_BUSY)
        assert (by_body.code, json.loads(by_body.body)) == (409, MACHINE_BUSY)
        self.assert_nothing_sent()

    def test_loading_an_unknown_profile_id_is_404(self):
        response = self.fetch(f"/api/v1/profile/load/{UNKNOWN_ID}")

        assert response.code == 404
        assert json.loads(response.body) == {
            "status": "error",
            "error": "profile not found",
            "id": UNKNOWN_ID,
        }
        self.assert_nothing_sent()

    def test_loading_a_schema_invalid_profile_body_is_rejected_before_the_esp32(self):
        profile = example_profile()
        profile["temperature"] = 150

        response = self.post_json("/profile/load", profile)

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "JSON validation error: 150 is greater than the maximum of 100",
        }
        self.assert_nothing_sent()

    def test_loading_a_profile_body_with_an_undefined_variable_is_rejected(self):
        profile = example_profile()
        profile["stages"][0]["dynamics"]["points"][0][1] = "$missing_var"

        response = self.post_json("/profile/load", profile)

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": "variable error: UndefinedVariableException:Variable missing_var is not defined",
        }
        self.assert_nothing_sent()

    def test_loading_malformed_json_is_rejected(self):
        response = self.post_json("/profile/load", "{")

        assert response.code == 400
        assert json.loads(response.body) == {
            "status": "error",
            "error": f"Invalid JSON: {MALFORMED_JSON_ERROR}",
        }
        self.assert_nothing_sent()

    def test_loading_a_profile_body_while_the_motor_is_stressed_is_refused(self):
        AlarmManager.alarms[AlarmType.MOTOR_STRESSED.value] = Alarm(
            AlarmType.MOTOR_STRESSED, math.inf
        )

        response = self.post_json("/profile/load", example_profile())

        assert response.code == 403
        assert json.loads(response.body) == HIGH_STRAIN
        assert self.notifications == [(MOTOR_STRESSED_MESSAGE, WARNING_TRIANGLE_IMAGE)]
        self.assert_nothing_sent()

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: ProfileManager.load_profile_and_send (profiles.py:501-505) drops "
            "the False that send_profile_to_esp32 returns under the motor-stress alarm, so "
            "GET /profile/load/<id> (api/profiles.py:209-212) answers 200 with the profile "
            "although nothing was sent, where POST /profile/load answers 403"
        ),
    )
    def test_loading_a_saved_profile_while_the_motor_is_stressed_is_refused(self):
        profile = example_profile()
        self.save(profile)
        AlarmManager.alarms[AlarmType.MOTOR_STRESSED.value] = Alarm(
            AlarmType.MOTOR_STRESSED, math.inf
        )

        response = self.fetch(f"/api/v1/profile/load/{profile['id']}")

        assert self.port.writes == []
        assert response.code == 403
        assert json.loads(response.body) == HIGH_STRAIN

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: LoadProfileHandler.get(self, profile_id) (api/profiles.py:182) is "
            "also routed from /profile/load, which has no id group, so GET /profile/load "
            "raises TypeError and answers 500 instead of 405"
        ),
    )
    def test_get_on_the_load_route_without_an_id_is_method_not_allowed(self):
        response = self.fetch("/api/v1/profile/load")

        assert response.code == 405
        assert self.port.writes == []

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: send_profile_to_esp32 calls handle_image (profiles.py:518), which "
            "indexes data['display'] (profiles.py:219), but the schema does not require "
            "display and only save_profile defaults it (profiles.py:412); POST /profile/load "
            'of a schema-valid profile without display answers 400 "failed to load profile '
            "'display'\""
        ),
    )
    def test_loading_a_schema_valid_profile_body_without_display(self):
        profile = example_profile()
        del profile["display"]

        response = self.post_json("/profile/load", profile)

        assert response.code == 200
        assert json.loads(response.body) == {"name": profile["name"], "id": profile["id"]}
        assert self.port.writes == expected_serial_writes(expected_esp32_payload(profile))

    def test_limited_access_loads_the_bundled_simple_profile_by_id(self):
        MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] = LIMITED_ACCESS_CHANNEL
        profile = simple_profile()

        response = self.fetch(f"/api/v1/profile/load/{profile['id']}")

        assert response.code == 200
        assert json.loads(response.body) == {"name": "Simple", "id": profile["id"]}
        assert self.port.writes == expected_serial_writes(expected_esp32_payload(profile))
        assert self.emitted()[0] == ("profile", {"change": "load", "profile_id": profile["id"]})
        assert ProfileManager._known_profiles == {}

    # --- legacy ----------------------------------------------------------------

    def legacy_document(self):
        return {
            "id": "legacy-1",
            "name": "Legacy JSON",
            "author": "someone",
            "stages": [{"name": "initialize", "nodes": []}],
        }

    def test_legacy_route_is_404_unless_legacy_json_is_allowed(self):
        response = self.post_json("/profile/legacy", self.legacy_document())

        assert response.code == 404
        assert response.body == b""
        self.assert_nothing_sent()

    def test_legacy_route_streams_the_raw_document_and_records_the_placeholder(self):
        from api.emulation import LEGACY_DUMMY_PROFILE

        MeticulousConfig[CONFIG_USER][ALLOW_LEGACY_JSON] = True
        document = self.legacy_document()

        response = self.post_json("/profile/legacy", document)

        assert response.code == 200
        assert json.loads(response.body) == {"name": "Legacy JSON", "id": "legacy-1"}
        # The legacy route sends the whole document, metadata included.
        assert self.port.writes == expected_serial_writes(document)
        assert self.debug_shot.nodeJSON == document
        assert (
            MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST]["profile"] == LEGACY_DUMMY_PROFILE
        )
        assert self.emitted() == []

    def test_legacy_route_while_the_machine_is_busy_is_refused(self):
        MeticulousConfig[CONFIG_USER][ALLOW_LEGACY_JSON] = True
        Machine.is_idle = False

        response = self.post_json("/profile/legacy", self.legacy_document())

        assert (response.code, json.loads(response.body)) == (409, MACHINE_BUSY)
        self.assert_nothing_sent()

    # --- delete ----------------------------------------------------------------

    def test_deleting_a_profile_removes_it_from_disk_list_order_and_selection(self):
        profile = example_profile()
        self.save(profile, change_id="create-1")
        self.forget_emits()

        response = self.fetch(f"/api/v1/profile/delete/{profile['id']}", method="DELETE")

        assert response.code == 200
        result = json.loads(response.body)
        assert result["profile"] == profile
        assert uuid.UUID(result["change_id"]).version == 4
        assert self.stored_files() == []
        assert self.get_json("/profile/list") == []
        assert self.fetch(f"/api/v1/profile/get/{profile['id']}").code == 404
        assert MeticulousConfig[CONFIG_USER][PROFILE_ORDER] == []
        assert yaml.safe_load(self.config_file.read_text())[CONFIG_USER][PROFILE_ORDER] == []

        emitted = self.emitted()
        assert len(emitted) == 1
        event, payload = emitted[0]
        assert event == "profile"
        assert (payload["change"], payload["profile_id"]) == ("delete", profile["id"])

        changes = self.get_json("/profile/changes")
        assert [(c["type"], c["profile_id"], c["change_id"]) for c in changes] == [
            ("create", profile["id"], "create-1"),
            ("delete", profile["id"], result["change_id"]),
        ]
        # The hover is cleared silently: the dial moves its own carousel on delete.
        assert self.fetch("/api/v1/profile/selected").code == 204

    def test_get_on_the_delete_route_also_deletes(self):
        profile = example_profile()
        self.save(profile)

        response = self.fetch(f"/api/v1/profile/delete/{profile['id']}")

        assert response.code == 200
        assert json.loads(response.body)["profile"] == profile
        assert self.stored_files() == []

    def test_deleting_an_unknown_profile_is_404(self):
        response = self.fetch(f"/api/v1/profile/delete/{UNKNOWN_ID}", method="DELETE")

        assert response.code == 404
        assert json.loads(response.body) == {
            "status": "error",
            "error": "profile not found",
            "id": UNKNOWN_ID,
        }
        assert self.emitted() == []
        assert ProfileManager.get_profile_changes() == []

    def test_deleting_a_profile_removes_an_image_it_uploaded_before_boot(self):
        image_name = f"{hashlib.md5(PNG_BYTES).hexdigest()}.png"
        (self.images_path / image_name).write_bytes(PNG_BYTES)
        profile = simple_profile()
        profile["display"]["image"] = f"/api/v1/profile/image/{image_name}"
        (self.profile_path / f"{profile['id']}.json").write_text(json.dumps(profile))
        ProfileManager.refresh_image_list()
        ProfileManager.refresh_profile_list()
        assert self.get_json(f"/profile/get/{profile['id']}") == profile

        response = self.fetch(f"/api/v1/profile/delete/{profile['id']}", method="DELETE")

        assert response.code == 200
        assert self.stored_images() == [DEFAULT_IMAGE_NAME]

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: _delete_unused_images (profiles.py:717) only considers the "
            "_known_images snapshot from the last refresh_image_list, and handle_image "
            "(profiles.py:247) does not add new uploads to it, so deleting the profile that "
            "uploaded an image in the same session leaves the image on disk"
        ),
    )
    def test_deleting_a_profile_removes_an_image_it_uploaded_in_this_session(self):
        profile = example_profile()
        profile["display"]["image"] = "data:image/png;base64," + base64.b64encode(
            PNG_BYTES
        ).decode("ascii")
        self.save(profile)

        response = self.fetch(f"/api/v1/profile/delete/{profile['id']}", method="DELETE")

        assert response.code == 200
        assert self.stored_images() == [DEFAULT_IMAGE_NAME]

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: delete_profile passes change_id positionally into the timestamp "
            "parameter of _register_profile_change (profiles.py:479-481), so a client "
            "X-Change-Id becomes the change timestamp and a random change_id is returned"
        ),
    )
    def test_delete_records_the_client_change_id(self):
        profile = example_profile()
        self.save(profile)

        response = self.fetch(
            f"/api/v1/profile/delete/{profile['id']}",
            method="DELETE",
            headers={"X-Change-Id": "client-delete-1"},
        )

        assert json.loads(response.body)["change_id"] == "client-delete-1"
        delete_change = self.get_json("/profile/changes")[-1]
        assert isinstance(delete_change.pop("timestamp"), float)
        assert delete_change == {
            "type": "delete",
            "profile_id": profile["id"],
            "change_id": "client-delete-1",
        }

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: delete_profile emits the profile delete event without its "
            "change_id (profiles.py:483), unlike save_profile (profiles.py:451), so clients "
            "cannot match the event to the change they made"
        ),
    )
    def test_delete_event_carries_the_change_id(self):
        profile = example_profile()
        self.save(profile)
        self.forget_emits()

        response = self.fetch(f"/api/v1/profile/delete/{profile['id']}", method="DELETE")

        change_id = json.loads(response.body)["change_id"]
        assert self.emitted() == [
            (
                "profile",
                {"change": "delete", "profile_id": profile["id"], "change_id": change_id},
            )
        ]

    # --- changes ---------------------------------------------------------------

    def test_changes_is_empty_before_any_change(self):
        assert self.get_json("/profile/changes") == []

    def test_changes_lists_every_change_in_order_with_the_client_change_ids(self):
        first = example_profile()
        second = simple_profile()
        self.save(first, change_id="c1")
        self.save(second, change_id="c2")
        self.save(dict(first, name="Edited"), change_id="c3")
        delete = json.loads(
            self.fetch(f"/api/v1/profile/delete/{second['id']}", method="DELETE").body
        )

        changes = self.get_json("/profile/changes")

        assert [(c["type"], c["profile_id"], c["change_id"]) for c in changes] == [
            ("create", first["id"], "c1"),
            ("create", second["id"], "c2"),
            ("update", first["id"], "c3"),
            ("delete", second["id"], delete["change_id"]),
        ]
        timestamps = [c["timestamp"] for c in changes]
        assert timestamps == sorted(timestamps)

    def test_changes_keeps_only_the_latest_hundred(self):
        profile = example_profile()
        for index in range(101):
            self.save(profile, change_id=f"change-{index}")

        changes = self.get_json("/profile/changes")

        assert [c["change_id"] for c in changes] == [f"change-{i}" for i in range(1, 101)]
        assert [c["type"] for c in changes] == ["update"] * 100

    # --- last / selected -------------------------------------------------------

    def test_last_is_204_before_any_load(self):
        response = self.fetch("/api/v1/profile/last")

        assert response.code == 204
        assert response.body == b""

    def test_last_returns_the_recorded_profile_with_its_load_time_as_last_modified(self):
        last = {"load_time": 1700000000.0, "profile": example_profile()}
        MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST] = last

        response = self.fetch("/api/v1/profile/last")

        assert response.code == 200
        assert response.headers["Last-Modified"] == "Tue, 14 Nov 2023 22:13:20 GMT"
        assert json.loads(response.body) == last

    def test_last_without_a_load_time_has_no_last_modified_header(self):
        last = {"load_time": None, "profile": example_profile()}
        MeticulousConfig[CONFIG_PROFILES][PROFILE_LAST] = last

        response = self.fetch("/api/v1/profile/last")

        assert response.code == 200
        assert "Last-Modified" not in response.headers
        assert json.loads(response.body) == last

    def test_selected_is_204_without_a_hovered_profile(self):
        response = self.fetch("/api/v1/profile/selected")

        assert response.code == 204
        assert response.body == b""

    def test_selected_returns_the_hovered_profile(self):
        ProfileManager._profile_hover = ProfileHover(id=UNKNOWN_ID, type="hover", from_="app")

        assert self.get_json("/profile/selected") == {
            "id": UNKNOWN_ID,
            "type": "hover",
            "from": "app",
        }

    # --- images / defaults -----------------------------------------------------

    def test_image_list_returns_the_default_images(self):
        assert self.get_json("/profile/image") == [DEFAULT_IMAGE_NAME]
        assert self.get_json("/profile/image/") == [DEFAULT_IMAGE_NAME]

    def test_image_route_serves_a_stored_image_and_404s_a_missing_one(self):
        image = self.fetch(f"/api/v1/profile/image/{DEFAULT_IMAGE_NAME}")

        assert image.code == 200
        assert image.headers["Content-Type"] == "image/png"
        assert image.body == DEFAULT_IMAGE_SOURCE.read_bytes()
        assert self.fetch("/api/v1/profile/image/missing.png").code == 404

    def test_defaults_returns_the_bundled_simple_profile(self):
        assert self.get_json("/profile/defaults") == {
            "default": [simple_profile()],
            "community": [],
        }

    # --- from_manual -----------------------------------------------------------

    def test_profile_built_from_a_manual_brew_is_saved_like_any_other_profile(self):
        searches = []
        shot = {
            "id": "8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6",
            "time": 1757193600.0,
            "file": "2026-09-07/10:00:00.shot.json.zst",
            "name": MANUAL_MODE_PROFILE_NAME,
            "data": [
                {
                    "shot": {
                        "pressure": pressure,
                        "flow": 1.2,
                        "weight": weight,
                        "setpoints": {"active": "pressure", "pressure": pressure},
                    },
                    "time": profile_time,
                    "profile_time": profile_time,
                    "status": "Manual",
                }
                for profile_time, pressure, weight in ((1000, 6.0, 0.0), (5000, 9.0, 36.0))
            ],
            "profile": dict(ProfileManager.build_manual_mode_profile(), temperature=93.5),
        }

        def search_history(params):
            searches.append(params)
            return [shot]

        # The history database is the boundary here; the builder and save are real.
        original_search = ShotDataBase.__dict__["search_history"]
        ShotDataBase.search_history = staticmethod(search_history)
        try:
            response = self.post_json(
                "/profile/from_manual", {"name": "Morning manual"}, {"X-Change-Id": "m-1"}
            )
        finally:
            ShotDataBase.search_history = original_search

        assert response.code == 200
        result = json.loads(response.body)
        saved = result["profile"]
        assert result["change_id"] == "m-1"
        assert [search.ids for search in searches] == [[MANUAL_MODE_PROFILE_ID]]
        assert saved["name"] == "Morning manual"
        assert saved["temperature"] == 93.5
        assert "manual" not in saved
        assert json.loads((self.profile_path / f"{saved['id']}.json").read_text()) == saved
        assert self.get_json(f"/profile/get/{saved['id']}") == saved
        assert [p["id"] for p in self.get_json("/profile/list")] == [saved["id"]]
        assert self.emitted()[0] == (
            "profile",
            {"change": "create", "profile_id": saved["id"], "change_id": "m-1"},
        )
