"""Handler-level regression tests for the /sounds routes.

SYSTEM_SOUNDS and USER_SOUNDS point at tmp copies of the repo's sounds/default
theme. Only audio playback (playsound) and the pactl volume call are replaced.
"""

import copy
import io
import json
import shutil
import zipfile
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

import api.sounds as api_sounds
import sounds
from api.sounds import (
    GetThemeHandler,
    ListSoundsHandler,
    ListThemesHandler,
    PlaySoundHandler,
    SetThemeHandler,
    UploadThemeHandler,
)
from config import CONFIG_SYSTEM, CONFIG_USER, SOUNDS_ENABLED, SOUNDS_THEME, MeticulousConfig
from machine import Machine
from sounds import SoundPlayer

REPO_DEFAULT_THEME = Path(__file__).resolve().parents[1] / "sounds" / "default"
DEFAULT_THEME_SOUNDS = json.loads((REPO_DEFAULT_THEME / "config.json").read_text())


@pytest.fixture(autouse=True)
def restore_config():
    original = copy.deepcopy(dict(MeticulousConfig))
    with patch.object(MeticulousConfig, "save") as save:
        yield save
    MeticulousConfig.clear()
    MeticulousConfig.update(original)


def multipart_body(field, filename, content, boundary="gate-boundary-7f3a"):
    body = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="{field}"; filename="{filename}"\r\n'
        "Content-Type: application/zip\r\n\r\n"
    ).encode() + content
    body += f"\r\n--{boundary}--\r\n".encode()
    headers = {"Content-Type": f"multipart/form-data; boundary={boundary}"}
    return body, headers


def zip_bytes(entries):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for name, content in entries.items():
            archive.writestr(name, content)
    return buffer.getvalue()


class SoundsApiTestCase(AsyncHTTPTestCase):
    @pytest.fixture(autouse=True)
    def _pytest_fixtures(self, monkeypatch, tmp_path, restore_config):
        self.monkeypatch = monkeypatch
        self.tmp_path = tmp_path
        self.config_save = restore_config

    def setUp(self):
        super().setUp()
        self.system_sounds = self.tmp_path / "system"
        self.user_sounds = self.tmp_path / "user"
        shutil.copytree(REPO_DEFAULT_THEME, self.system_sounds / "default")
        self.user_sounds.mkdir()
        self.monkeypatch.setattr(sounds, "SYSTEM_SOUNDS", str(self.system_sounds))
        self.monkeypatch.setattr(sounds, "USER_SOUNDS", str(self.user_sounds))
        # api.sounds imported USER_SOUNDS by value for the upload destination.
        self.monkeypatch.setattr(api_sounds, "USER_SOUNDS", str(self.user_sounds))

        for name in (
            "KNOWN_THEMES",
            "CURRENT_THEME_CONFIG",
            "CURRENT_THEME_NAME",
            "DEFAULT_THEME_CONFIG",
            "_current_sound",
            "_audio_pin",
        ):
            self.monkeypatch.setattr(SoundPlayer, name, getattr(SoundPlayer, name))
        self.monkeypatch.setattr(SoundPlayer, "_current_sound", None)
        # Emulated mode skips the audio-enable GPIO line.
        self.monkeypatch.setattr(Machine, "emulated", True)

        self.playsound = MagicMock(name="playsound")
        self.pactl = MagicMock(name="subprocess")
        self.monkeypatch.setattr(sounds, "playsound", self.playsound)
        self.monkeypatch.setattr(sounds, "subprocess", self.pactl)

        MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME] = "default"
        MeticulousConfig[CONFIG_USER][SOUNDS_ENABLED] = True
        SoundPlayer.init(emulation=True, play_startup_sound=False)
        self.config_save.reset_mock()

    def get_app(self):
        return Application(
            [
                (r"/api/v1/sounds/play/(.*)", PlaySoundHandler),
                (r"/api/v1/sounds/list", ListSoundsHandler),
                (r"/api/v1/sounds/theme/list", ListThemesHandler),
                (r"/api/v1/sounds/theme/get", GetThemeHandler),
                (r"/api/v1/sounds/theme/set/(.*)", SetThemeHandler),
                (r"/api/v1/sounds/theme/upload", UploadThemeHandler),
            ]
        )

    def json(self, response):
        return json.loads(response.body)

    def add_user_theme(self, name, theme_config, files=()):
        folder = self.user_sounds / name
        folder.mkdir()
        (folder / "config.json").write_text(json.dumps(theme_config))
        for file_name in files:
            (folder / file_name).write_bytes(b"ID3")
        SoundPlayer.init(emulation=True, play_startup_sound=False)
        self.config_save.reset_mock()
        return folder


class TestPlaySoundRoute(SoundsApiTestCase):
    def test_theme_sound_is_played_non_blocking_from_the_theme_folder(self):
        response = self.fetch("/api/v1/sounds/play/notification")

        assert response.code == 200
        assert self.json(response) == {"status": "okay"}
        self.playsound.assert_called_once_with(
            str(self.system_sounds / "default" / "notification.mp3"), block=False
        )

    def test_sound_disabled_by_the_theme_reports_okay_without_playing(self):
        # sounds/default/config.json maps "startup" to {}.
        response = self.fetch("/api/v1/sounds/play/startup")

        assert response.code == 200
        assert self.json(response) == {"status": "okay"}
        self.playsound.assert_not_called()

    def test_unknown_sound_is_not_found(self):
        response = self.fetch("/api/v1/sounds/play/grinder")

        assert response.code == 404
        assert self.json(response) == {"error": "sound not found", "details": "grinder"}
        self.playsound.assert_not_called()

    def test_globally_disabled_sounds_report_okay_for_any_name_without_playing(self):
        MeticulousConfig[CONFIG_USER][SOUNDS_ENABLED] = False

        response = self.fetch("/api/v1/sounds/play/grinder")

        assert response.code == 200
        assert self.json(response) == {"status": "okay"}
        self.playsound.assert_not_called()

    def test_sound_whose_file_is_missing_is_not_found(self):
        (self.system_sounds / "default" / "notification.mp3").unlink()

        response = self.fetch("/api/v1/sounds/play/notification")

        assert response.code == 404
        assert self.json(response) == {"error": "sound not found", "details": "notification"}
        self.playsound.assert_not_called()

    def test_playing_a_sound_stops_the_one_still_playing(self):
        previous = MagicMock(name="previous sound")
        SoundPlayer._current_sound = previous

        response = self.fetch("/api/v1/sounds/play/brewing_end")

        assert response.code == 200
        previous.stop.assert_called_once_with()
        self.playsound.assert_called_once_with(
            str(self.system_sounds / "default" / "event_finished.mp3"), block=False
        )
        assert SoundPlayer._current_sound is self.playsound.return_value

    def test_playback_failure_is_reported_as_not_found(self):
        self.playsound.side_effect = RuntimeError("no audio sink")

        response = self.fetch("/api/v1/sounds/play/notification")

        assert response.code == 404
        assert self.json(response) == {"error": "sound not found", "details": "notification"}

    def test_sound_requested_before_player_init_is_not_found(self):
        SoundPlayer.KNOWN_THEMES = None

        response = self.fetch("/api/v1/sounds/play/notification")

        assert response.code == 404
        self.playsound.assert_not_called()

    def test_sounds_of_a_user_theme_are_played_from_the_user_folder(self):
        folder = self.add_user_theme("retro", {"notification": "ding.mp3"}, files=["ding.mp3"])
        MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME] = "retro"

        response = self.fetch("/api/v1/sounds/play/notification")

        assert response.code == 200
        self.playsound.assert_called_once_with(str(folder / "ding.mp3"), block=False)
        assert SoundPlayer.CURRENT_THEME_NAME == "retro"


class TestSoundListRoutes(SoundsApiTestCase):
    def test_sound_list_has_every_event_of_the_default_theme(self):
        response = self.fetch("/api/v1/sounds/list")

        assert response.code == 200
        assert self.json(response) == list(DEFAULT_THEME_SOUNDS.keys())
        assert self.json(response) == [
            "startup",
            "heating_start",
            "heating_end",
            "brewing_start",
            "brewing_end",
            "abort",
            "idle",
            "notification",
        ]

    def test_theme_list_has_only_the_system_default_theme(self):
        response = self.fetch("/api/v1/sounds/theme/list")

        assert response.code == 200
        assert self.json(response) == ["default"]

    def test_theme_list_puts_user_themes_before_system_themes(self):
        self.add_user_theme("retro", {"notification": "ding.mp3"})

        assert self.json(self.fetch("/api/v1/sounds/theme/list")) == ["retro", "default"]

    def test_user_folder_without_valid_config_is_not_a_theme(self):
        broken = self.user_sounds / "broken"
        broken.mkdir()
        (broken / "config.json").write_text("{not json")
        SoundPlayer.init(emulation=True, play_startup_sound=False)

        assert self.json(self.fetch("/api/v1/sounds/theme/list")) == ["default"]

    def test_current_theme_is_returned_as_a_bare_string(self):
        response = self.fetch("/api/v1/sounds/theme/get")

        assert response.code == 200
        assert response.body == b"default"


class TestSetThemeRoute(SoundsApiTestCase):
    def test_get_selects_a_known_theme_and_persists_it(self):
        self.add_user_theme("retro", {"notification": "ding.mp3"})

        response = self.fetch("/api/v1/sounds/theme/set/retro")

        assert response.code == 200
        assert self.json(response) == {"status": "okay"}
        assert MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME] == "retro"
        self.config_save.assert_called_once_with()
        assert self.fetch("/api/v1/sounds/theme/get").body == b"retro"

    def test_post_selects_a_known_theme_like_get(self):
        self.add_user_theme("retro", {"notification": "ding.mp3"})

        response = self.fetch("/api/v1/sounds/theme/set/retro", method="POST", body=b"")

        assert response.code == 200
        assert self.json(response) == {"status": "okay"}
        assert MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME] == "retro"

    def test_theme_missing_events_falls_back_to_the_default_mapping(self):
        self.add_user_theme("retro", {"notification": "ding.mp3"})

        self.fetch("/api/v1/sounds/theme/set/retro")

        assert SoundPlayer.get_theme() == {**DEFAULT_THEME_SOUNDS, "notification": "ding.mp3"}

    def test_unknown_theme_is_not_found_and_reverts_to_default(self):
        self.add_user_theme("retro", {"notification": "ding.mp3"})
        self.fetch("/api/v1/sounds/theme/set/retro")
        self.config_save.reset_mock()

        response = self.fetch("/api/v1/sounds/theme/set/vaporwave")

        assert response.code == 404
        assert self.json(response) == {"error": "theme not found", "details": "vaporwave"}
        assert MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME] == "default"
        self.config_save.assert_called_once_with()

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: SoundPlayer.set_theme (sounds.py:126-128) aliases "
            "DEFAULT_THEME_CONFIG as CURRENT_THEME_CONFIG and update()s it in place, so a "
            "theme's extra sounds leak into the default theme after switching back"
        ),
    )
    def test_switching_back_to_default_drops_sounds_only_the_previous_theme_had(self):
        self.add_user_theme("retro", {"notification": "ding.mp3", "grinder": "grind.mp3"})
        self.fetch("/api/v1/sounds/theme/set/retro")

        self.fetch("/api/v1/sounds/theme/set/default")

        assert self.json(self.fetch("/api/v1/sounds/list")) == list(DEFAULT_THEME_SOUNDS)


class TestUploadThemeRoute(SoundsApiTestCase):
    def upload(self, content, field="file"):
        body, headers = multipart_body(field, "theme.zip", content)
        return self.fetch(
            "/api/v1/sounds/theme/upload", method="POST", body=body, headers=headers
        )

    def assert_nothing_extracted(self):
        assert list(self.user_sounds.iterdir()) == []

    def test_valid_theme_zip_is_extracted_and_becomes_selectable(self):
        archive = zip_bytes(
            {
                "retro/config.json": json.dumps({"notification": "ding.mp3"}),
                "retro/ding.mp3": b"ID3",
            }
        )

        response = self.upload(archive)

        assert response.code == 200
        assert response.body == b"Zip file uploaded and unpacked successfully."
        assert json.loads((self.user_sounds / "retro" / "config.json").read_text()) == {
            "notification": "ding.mp3"
        }
        assert (self.user_sounds / "retro" / "ding.mp3").read_bytes() == b"ID3"
        assert self.json(self.fetch("/api/v1/sounds/theme/list")) == ["retro", "default"]
        # The reload keeps the selected theme and never plays the startup sound.
        assert MeticulousConfig[CONFIG_SYSTEM][SOUNDS_THEME] == "default"
        self.playsound.assert_not_called()

    def test_request_without_file_field_is_rejected(self):
        response = self.upload(zip_bytes({"retro/config.json": "{}"}), field="theme")

        assert response.code == 400
        assert self.json(response) == {
            "error": "invalid zip",
            "details": "'file' not found in request",
        }
        self.assert_nothing_extracted()

    def test_zip_with_two_root_folders_is_rejected(self):
        archive = zip_bytes({"retro/config.json": "{}", "modern/config.json": "{}"})

        response = self.upload(archive)

        assert response.code == 400
        assert self.json(response) == {
            "error": "invalid zip",
            "details": "Zip must contain exactly one folder with the themes name at the root.",
        }
        self.assert_nothing_extracted()

    def test_zip_with_files_only_at_the_root_is_rejected(self):
        response = self.upload(zip_bytes({"config.json": "{}", "ding.mp3": b"ID3"}))

        assert response.code == 400
        assert self.json(response)["details"] == (
            "Zip must contain exactly one folder with the themes name at the root."
        )
        self.assert_nothing_extracted()

    def test_zip_without_config_in_the_theme_folder_is_rejected(self):
        archive = zip_bytes({"retro/sounds/config.json": "{}", "retro/ding.mp3": b"ID3"})

        response = self.upload(archive)

        assert response.code == 400
        assert self.json(response) == {
            "error": "invalid zip",
            "details": "No config.json found in the root folder.",
        }
        self.assert_nothing_extracted()

    def test_zip_with_invalid_config_json_is_rejected(self):
        response = self.upload(zip_bytes({"retro/config.json": "{not json"}))

        assert response.code == 400
        assert self.json(response) == {
            "error": "invalid zip",
            "details": "config.json is not valid JSON.",
        }
        self.assert_nothing_extracted()

    def test_corrupted_zip_is_rejected(self):
        response = self.upload(b"PK\x03\x04 truncated")

        assert response.code == 400
        assert self.json(response) == {"error": "invalid zip", "details": "zip file corrupted"}
        self.assert_nothing_extracted()
