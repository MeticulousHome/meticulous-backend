import copy
import json
import queue
import subprocess
import tempfile
import uuid
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import urlencode

import aiohttp
import pytest
import yaml
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

import ota
import system_services
import timezone_manager
from api.api import API, APIVersion
from api.password_handler import RootPasswordHandler
from api.serial import WEBPAGE, ScaleCalibrateHandler, SerialNumberHandler
from api.settings import (
    ManufacturingSettingsHandler,
    SettingsHandler,
    TimezoneUIHandler,
)
from config import (
    CONFIG_SYSTEM,
    CONFIG_USER,
    MACHINE_BATCH_NUMBER,
    MACHINE_BUILD_DATE,
    MACHINE_COLOR,
    MACHINE_SERIAL_NUMBER,
    ROOT_PASSWORD,
    SHOT_DATA_SHARING_ID,
    MeticulousConfig,
)
from esp_serial.data import ESPInfo
from machine import Machine
from manufacturing import CONFIG_MANUFACTURING, dial_schema
from notifications import NotificationManager, NotificationResponse
from profiles import ProfileManager
from sounds import SoundPlayer, Sounds
from timezone_manager import TimezoneManager

# Values every test starts from, so the outcome does not depend on whatever an
# earlier test or an old /tmp config left in the global MeticulousConfig.
USER_BASELINE = {
    "enable_sounds": True,
    "clock_format_24_hour": True,
    "debug_shot_data_retention_days": 31,
    "heating_timeout": 10,
    "partial_retraction": 45.33,
    "auto_purge_after_shot": False,
    "tare_behavior": "after_retraction",
    "shot_data_sharing": None,
    "report_contact_mail": None,
    "update_channel": "",
    "usb_mode": "host",
    "timezone_sync": "automatic",
    "time_zone": "Etc/UTC",
    "ssh_enabled": True,
    "profile_order": [],
}
SYSTEM_BASELINE = {
    SHOT_DATA_SHARING_ID: None,
    ROOT_PASSWORD: None,
    MACHINE_SERIAL_NUMBER: None,
    MACHINE_COLOR: None,
    MACHINE_BATCH_NUMBER: None,
    MACHINE_BUILD_DATE: None,
}
MANUFACTURING_BASELINE = {"enabled": False, "last_boot_mode": None, "skip_stage": False}

TIMEZONE_STATUS_COMMAND = "timedatectl status | grep 'Time zone' | awk -F'[:()]' '{print $2}'"
TIMEZONE_URL = "https://analytics.meticulousespresso.com/timezone_ip"


class RecordingPort:
    def __init__(self):
        self.writes = []

    def write(self, content):
        self.writes.append(content)


class RecordingSocket:
    def __init__(self):
        self.emits = []

    async def emit(self, event, data=None, **kwargs):
        self.emits.append((event, data))


class FakeSubprocess:
    """Stands in for the subprocess module inside timezone_manager (timedatectl)."""

    PIPE = subprocess.PIPE
    CalledProcessError = subprocess.CalledProcessError

    def __init__(self, set_timezone_stderr=None):
        self.calls = []
        self.set_timezone_stderr = set_timezone_stderr

    def run(self, command, **kwargs):
        self.calls.append((command, kwargs.get("shell")))
        if self.set_timezone_stderr is not None and command.startswith(
            "timedatectl set-timezone"
        ):
            raise subprocess.CalledProcessError(
                1, command, output="", stderr=self.set_timezone_stderr
            )
        return subprocess.CompletedProcess(command, 0, stdout="", stderr="")


class FakeSystemd:
    def __init__(self):
        self.calls = []

    def __getattr__(self, name):
        def record(*args):
            self.calls.append((name, args))

        return record


class FakeTimezoneResponse:
    def __init__(self, body):
        self.status = 200
        self.body = body

    async def text(self):
        return self.body

    async def __aenter__(self):
        return self

    async def __aexit__(self, *exc):
        return False


def fake_client_session(body, requested_urls):
    class FakeClientSession:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *exc):
            return False

        def get(self, url):
            requested_urls.append(url)
            return FakeTimezoneResponse(body)

    return FakeClientSession


class MachineBoundaryTestCase(AsyncHTTPTestCase):
    """Real handlers; only the ESP32 serial port, disk paths and sockets are faked."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.mp = pytest.MonkeyPatch()

        # MeticulousConfig is a process-wide singleton and the handlers replace
        # whole sections, so every section a test can touch is swapped for a copy.
        for section, baseline in (
            (CONFIG_USER, USER_BASELINE),
            (CONFIG_SYSTEM, SYSTEM_BASELINE),
            (CONFIG_MANUFACTURING, MANUFACTURING_BASELINE),
        ):
            values = copy.deepcopy(MeticulousConfig[section])
            values.update(copy.deepcopy(baseline))
            self.mp.setitem(MeticulousConfig, section, values)
        self.config_file = self.root.joinpath("config", "config.yml")
        self.mp.setattr(MeticulousConfig, "_MeticulousConfigDict__path", self.config_file)
        self.config_socket = RecordingSocket()
        self.mp.setattr(MeticulousConfig, "_MeticulousConfigDict__sio", self.config_socket)

        self.port = RecordingPort()
        self.mp.setattr(Machine, "_connection", SimpleNamespace(port=self.port))
        self.mp.setattr(Machine, "_stopESPcomm", False)
        self.mp.setattr(Machine, "esp_info", None)
        self.mp.setattr(Machine, "_pending_tare_behavior_writes", [])
        self.mp.setattr(Machine, "enable_manufacturing", False)

        self.mp.setattr(NotificationManager, "_notifications", [])
        self.mp.setattr(NotificationManager, "_queue", queue.Queue())
        self.sounds = []
        self.mp.setattr(SoundPlayer, "play_event_sound", self.sounds.append)
        super().setUp()

    def tearDown(self):
        super().tearDown()
        self.mp.undo()
        self.temporary.cleanup()

    def saved_config(self):
        return yaml.safe_load(self.config_file.read_text())

    def post_json(self, path, payload):
        body = payload if isinstance(payload, str) else json.dumps(payload)
        return self.fetch(path, method="POST", body=body)


class TestSettingsAPI(MachineBoundaryTestCase):
    def setUp(self):
        super().setUp()
        self.hawkbit_dir = self.root.joinpath("hawkbit")
        self.mp.setattr(ota, "HAWKBIT_CONFIG_DIR", str(self.hawkbit_dir))
        self.timedatectl = FakeSubprocess()
        self.mp.setattr(timezone_manager, "subprocess", self.timedatectl)
        self.mp.setattr(TimezoneManager, "_TimezoneManager__system_synced", False)
        self.profile_socket = RecordingSocket()
        self.mp.setattr(ProfileManager, "_sio", self.profile_socket)
        self.mp.setattr(ProfileManager, "_loop", self.io_loop.asyncio_loop)

    def get_app(self):
        return Application([(r"/api/v1/settings[/]*(.*)", SettingsHandler)])

    def post_settings(self, payload, path="/api/v1/settings"):
        return self.post_json(path, payload)

    def assert_saved(self, **changes):
        expected = {**MeticulousConfig[CONFIG_USER]}
        for key, value in changes.items():
            assert MeticulousConfig[CONFIG_USER][key] == value
        assert self.saved_config()[CONFIG_USER] == expected
        assert self.config_socket.emits == [("settings", {})]

    def assert_rejected(self, response, status, error):
        assert response.code == status
        assert json.loads(response.body) == {"status": "error", "error": error}
        assert MeticulousConfig[CONFIG_USER] == {
            **MeticulousConfig[CONFIG_USER],
            **USER_BASELINE,
        }
        assert not self.config_file.exists()
        assert self.config_socket.emits == []

    def test_routes_are_registered_with_the_patterns_under_test(self):
        routes = API._versions[APIVersion.V1]
        assert routes["/settings[/]*(.*)"][0] is SettingsHandler
        assert routes["/manufacturing[/]*"][0] is ManufacturingSettingsHandler
        assert routes["/timezones/(.*)"][0] is TimezoneUIHandler

    def test_get_returns_the_whole_user_section(self):
        response = self.fetch("/api/v1/settings")

        assert response.code == 200
        assert response.headers["Content-Type"] == "application/json"
        assert json.loads(response.body) == MeticulousConfig[CONFIG_USER]

    def test_get_with_trailing_slash_returns_the_whole_user_section(self):
        response = self.fetch("/api/v1/settings/")

        assert response.code == 200
        assert json.loads(response.body) == MeticulousConfig[CONFIG_USER]

    def test_get_single_setting_returns_only_that_key(self):
        response = self.fetch("/api/v1/settings/heating_timeout")

        assert response.code == 200
        assert json.loads(response.body) == {"heating_timeout": 10}

    def test_get_accepts_repeated_slashes_and_no_slash_before_the_setting(self):
        # "[/]*" matches zero or more slashes, so both spellings reach the same key.
        for path in ("/api/v1/settings//enable_sounds", "/api/v1/settingsenable_sounds"):
            response = self.fetch(path)
            assert response.code == 200
            assert json.loads(response.body) == {"enable_sounds": True}

    def test_get_unknown_setting_returns_404(self):
        response = self.fetch("/api/v1/settings/not_a_setting")

        assert response.code == 404
        assert json.loads(response.body) == {
            "status": "error",
            "error": "setting not found",
            "setting": "not_a_setting",
        }

    def test_post_plain_setting_saves_and_returns_the_whole_section(self):
        response = self.post_settings({"enable_sounds": False, "clock_format_24_hour": False})

        assert response.code == 200
        assert json.loads(response.body) == MeticulousConfig[CONFIG_USER]
        self.assert_saved(enable_sounds=False, clock_format_24_hour=False)
        assert self.port.writes == []

    def test_post_ignores_the_setting_name_in_the_path(self):
        # Only the body keys are applied; the path segment is not consulted.
        response = self.post_settings(
            {"clock_format_24_hour": False}, path="/api/v1/settings/enable_sounds"
        )

        assert response.code == 200
        self.assert_saved(clock_format_24_hour=False, enable_sounds=True)

    def test_post_invalid_json_returns_403(self):
        try:
            json.loads("{not json")
        except json.JSONDecodeError as error:
            json_error = str(error)

        response = self.post_settings("{not json")

        assert response.code == 403
        assert json.loads(response.body) == {
            "status": "error",
            "error": "invalid json",
            "json_error": json_error,
        }
        assert not self.config_file.exists()

    def test_post_unknown_setting_returns_404_without_saving(self):
        response = self.post_settings({"not_a_setting": True})

        self.assert_rejected(response, 404, "'setting not_a_setting not found'")

    def test_post_wrong_type_returns_404_without_saving(self):
        response = self.post_settings({"enable_sounds": "yes"})

        self.assert_rejected(
            response,
            404,
            "\"setting value invalid, received <class 'str'> and expected <class 'bool'>\"",
        )

    def test_post_rejects_heating_timeout_given_as_bool(self):
        # bool is a subclass of int, but the check compares exact types.
        response = self.post_settings({"heating_timeout": True})

        self.assert_rejected(
            response,
            404,
            "\"setting value invalid, received <class 'bool'> and expected <class 'int'>\"",
        )
        assert self.port.writes == []

    def test_post_rejected_request_keeps_earlier_keys_out_of_the_config(self):
        response = self.post_settings({"enable_sounds": False, "not_a_setting": 1})

        self.assert_rejected(response, 404, "'setting not_a_setting not found'")

    @pytest.mark.xfail(
        strict=True,
        reason="backend bug: api/settings.py:124-200 runs each setting's side effect while "
        "iterating, so heating_timeout reaches the ESP32 even though a later key makes the "
        "request fail with 404 and the config keeps the old value",
    )
    def test_post_rejected_request_sends_nothing_to_the_esp32(self):
        response = self.post_settings({"heating_timeout": 15, "not_a_setting": 1})

        assert response.code == 404
        assert self.port.writes == []

    @pytest.mark.xfail(
        strict=True,
        reason="backend bug: api/settings.py:190-191 calls ShotDataSharing.on_setting_changed, "
        "which writes a new shot_data_sharing_id into the live system config "
        "(shot_data_sharing.py:246) before a later key rejects the request with 404",
    )
    def test_post_rejected_request_leaves_the_sharing_id_untouched(self):
        response = self.post_settings({"shot_data_sharing": True, "not_a_setting": 1})

        assert response.code == 404
        assert MeticulousConfig[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] is None

    def test_post_heating_timeout_writes_the_timeout_to_the_esp32(self):
        response = self.post_settings({"heating_timeout": 15})

        assert response.code == 200
        assert self.port.writes == [b"heater_timeout,15\x03"]
        self.assert_saved(heating_timeout=15)

    def test_post_heating_timeout_above_one_hour_returns_400(self):
        response = self.post_settings({"heating_timeout": 61})

        self.assert_rejected(
            response,
            400,
            "Invalid heater timeout value: Timeout must be between 0 and 60 minutes",
        )
        assert self.port.writes == []

    def test_post_partial_retraction_integer_is_stored_as_float_and_synced(self):
        response = self.post_settings({"partial_retraction": 50})

        assert response.code == 200
        assert self.port.writes == [b"nvs_request,write,partial_retraction_key,50.0\x03"]
        self.assert_saved(partial_retraction=50.0)
        assert type(MeticulousConfig[CONFIG_USER]["partial_retraction"]) is float

    def test_post_partial_retraction_matching_the_esp32_skips_the_write(self):
        self.mp.setattr(Machine, "esp_info", ESPInfo(partialRetraction=50.0))

        response = self.post_settings({"partial_retraction": 50.0})

        assert response.code == 200
        assert self.port.writes == []
        self.assert_saved(partial_retraction=50.0)

    def test_post_partial_retraction_out_of_range_returns_400(self):
        response = self.post_settings({"partial_retraction": 68.0})

        self.assert_rejected(
            response, 400, "partial_retraction must be between 36.26 and 67.99 mm"
        )
        assert self.port.writes == []

    def test_post_auto_purge_writes_the_flag_to_the_esp32(self):
        response = self.post_settings({"auto_purge_after_shot": True})

        assert response.code == 200
        assert self.port.writes == [b"nvs_request,write,auto_purge_after_shot_key,true\x03"]
        self.assert_saved(auto_purge_after_shot=True)

    def test_post_auto_purge_updates_the_cached_esp32_value(self):
        self.mp.setattr(Machine, "esp_info", ESPInfo(autoPurgeAfterShot=True))

        response = self.post_settings({"auto_purge_after_shot": False})

        assert response.code == 200
        assert self.port.writes == [b"nvs_request,write,auto_purge_after_shot_key,false\x03"]
        assert Machine.esp_info.autoPurgeAfterShot is False

    def test_post_tare_behavior_is_written_when_firmware_supports_it(self):
        self.mp.setattr(Machine, "esp_info", ESPInfo(tareBehavior="after_retraction"))

        response = self.post_settings({"tare_behavior": "before_retraction"})

        assert response.code == 200
        assert self.port.writes == [
            b"nvs_request,write,tare_behavior_key,before_retraction\x03"
        ]
        assert Machine._pending_tare_behavior_writes == ["before_retraction"]
        self.assert_saved(tare_behavior="before_retraction")

    def test_post_tare_behavior_is_saved_but_not_sent_to_older_firmware(self):
        response = self.post_settings({"tare_behavior": "before_retraction"})

        assert response.code == 200
        assert self.port.writes == []
        self.assert_saved(tare_behavior="before_retraction")

    def test_post_unknown_tare_behavior_returns_400(self):
        response = self.post_settings({"tare_behavior": "sideways"})

        self.assert_rejected(response, 400, "unsupported tare behavior: sideways")

    def test_post_shot_data_sharing_opt_in_generates_a_sharing_id(self):
        response = self.post_settings({"shot_data_sharing": True})

        assert response.code == 200
        sharing_id = MeticulousConfig[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID]
        assert uuid.UUID(sharing_id).version == 4
        assert self.saved_config()[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] == sharing_id
        self.assert_saved(shot_data_sharing=True)

    def test_post_shot_data_sharing_opt_out_forgets_the_sharing_id(self):
        MeticulousConfig[CONFIG_USER]["shot_data_sharing"] = True
        MeticulousConfig[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] = "previous-id"

        response = self.post_settings({"shot_data_sharing": False})

        assert response.code == 200
        assert MeticulousConfig[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] is None
        self.assert_saved(shot_data_sharing=False)

    def test_post_shot_data_sharing_null_is_rejected(self):
        response = self.post_settings({"shot_data_sharing": None})

        self.assert_rejected(
            response,
            404,
            "\"setting value invalid, received <class 'NoneType'> "
            "and expected <class 'NoneType'>\"",
        )

    def test_post_report_contact_mail_is_trimmed(self):
        response = self.post_settings({"report_contact_mail": "  barista@example.com "})

        assert response.code == 200
        self.assert_saved(report_contact_mail="barista@example.com")

    def test_post_empty_report_contact_mail_clears_it(self):
        MeticulousConfig[CONFIG_USER]["report_contact_mail"] = "barista@example.com"

        response = self.post_settings({"report_contact_mail": "   "})

        assert response.code == 200
        self.assert_saved(report_contact_mail=None)

    def test_post_malformed_report_contact_mail_returns_400(self):
        response = self.post_settings({"report_contact_mail": "barista"})

        self.assert_rejected(response, 400, "report_contact_mail must look like name@domain")

    def test_post_non_string_report_contact_mail_returns_400(self):
        response = self.post_settings({"report_contact_mail": 42})

        self.assert_rejected(response, 400, "report_contact_mail must be a string or null")

    def test_post_update_channel_writes_the_hawkbit_channel_file(self):
        self.hawkbit_dir.mkdir()

        response = self.post_settings({"update_channel": "beta"})

        assert response.code == 200
        assert self.hawkbit_dir.joinpath("channel").read_text() == "beta\n"
        self.assert_saved(update_channel="beta")

    def test_post_update_channel_without_hawkbit_dir_only_saves_the_setting(self):
        response = self.post_settings({"update_channel": "beta"})

        assert response.code == 200
        assert not self.hawkbit_dir.exists()
        self.assert_saved(update_channel="beta")

    def test_post_valid_usb_mode_is_saved(self):
        # USBManager.setUSBMode is disabled and returns before touching the PD controller.
        response = self.post_settings({"usb_mode": "client"})

        assert response.code == 200
        self.assert_saved(usb_mode="client")

    def test_post_unknown_usb_mode_returns_400(self):
        response = self.post_settings({"usb_mode": "otg"})

        self.assert_rejected(
            response, 400, "Failed to set the USB mode: 'otg' is not a valid USB_MODES"
        )

    def test_post_profile_order_emits_a_profile_reload(self):
        response = self.post_settings({"profile_order": ["a", "b"]})

        assert response.code == 200
        assert self.profile_socket.emits == [("profile", {"change": "full_reload"})]
        self.assert_saved(profile_order=["a", "b"])

    def test_post_ssh_disabled_stops_and_disables_the_unit(self):
        systemd = FakeSystemd()
        requested = []
        bus = SimpleNamespace(get=lambda name: requested.append(name) or systemd)
        self.mp.setattr(system_services, "SystemBus", lambda: bus)

        response = self.post_settings({"ssh_enabled": False})

        assert response.code == 200
        assert requested == [".systemd1"]
        assert systemd.calls == [
            ("StopUnit", ("ssh.service", "fail")),
            ("DisableUnitFiles", (["ssh.service"], False)),
            ("Reload", ()),
        ]
        self.assert_saved(ssh_enabled=False)

    def test_post_ssh_enabled_enables_and_starts_the_unit(self):
        MeticulousConfig[CONFIG_USER]["ssh_enabled"] = False
        systemd = FakeSystemd()
        self.mp.setattr(
            system_services, "SystemBus", lambda: SimpleNamespace(get=lambda _: systemd)
        )

        response = self.post_settings({"ssh_enabled": True})

        assert response.code == 200
        assert systemd.calls == [
            ("EnableUnitFiles", (["ssh.service"], False, False)),
            ("StartUnit", ("ssh.service", "fail")),
            ("Reload", ()),
        ]
        self.assert_saved(ssh_enabled=True)

    @pytest.mark.xfail(
        strict=True,
        reason="backend bug: api/settings.py:140-148 sets 500 when the SSH unit cannot be "
        "changed but neither returns nor raises, so the new ssh_enabled is saved anyway and "
        "the full settings JSON is appended to the error body (invalid JSON)",
    )
    def test_post_ssh_failure_returns_500_and_keeps_the_old_value(self):
        def no_bus():
            raise RuntimeError("D-Bus unavailable")

        self.mp.setattr(system_services, "SystemBus", no_bus)

        response = self.post_settings({"ssh_enabled": False})

        assert response.code == 500
        assert MeticulousConfig[CONFIG_USER]["ssh_enabled"] is True
        assert not self.config_file.exists()
        assert json.loads(response.body) == {
            "status": "error",
            "setting": "ssh_enabled",
            "details": "Failed to update SSH service state",
        }

    def test_post_time_zone_sets_the_system_timezone(self):
        response = self.post_settings({"time_zone": "America/Mexico_City"})

        assert response.code == 200
        assert self.timedatectl.calls == [
            ("timedatectl set-timezone America/Mexico_City", True),
            (TIMEZONE_STATUS_COMMAND, True),
        ]
        self.assert_saved(time_zone="America/Mexico_City")

    def test_post_unchanged_time_zone_does_not_call_timedatectl(self):
        response = self.post_settings({"time_zone": "Etc/UTC"})

        assert response.code == 200
        assert self.timedatectl.calls == []
        self.assert_saved(time_zone="Etc/UTC")

    def test_post_time_zone_rejected_by_timedatectl_returns_400_with_redacted_zone(self):
        self.timedatectl.set_timezone_stderr = "Invalid time zone"

        response = self.post_settings({"time_zone": "Mars/Olympus_Mons"})

        self.assert_rejected(
            response,
            400,
            "Error updating timezone: Error setting system time zone: Command "
            "'timedatectl set-timezone Mars/*****' returned non-zero exit status 1. "
            "[ Out:  | Err: Invalid time zone ]",
        )

    def test_post_manual_timezone_sync_only_saves_the_mode(self):
        response = self.post_settings({"timezone_sync": "manual"})

        assert response.code == 200
        assert self.timedatectl.calls == []
        self.assert_saved(timezone_sync="manual", time_zone="Etc/UTC")

    def test_post_automatic_timezone_sync_applies_the_fetched_zone(self):
        requested = []
        body = json.dumps({"tz": "Europe/Berlin"})
        self.mp.setattr(aiohttp, "ClientSession", fake_client_session(body, requested))
        MeticulousConfig[CONFIG_USER]["timezone_sync"] = "manual"

        response = self.post_settings({"timezone_sync": "automatic"})

        assert response.code == 200
        assert requested == [TIMEZONE_URL]
        assert self.timedatectl.calls[0] == ("timedatectl set-timezone Europe/Berlin", True)
        self.assert_saved(timezone_sync="automatic", time_zone="Europe/Berlin")

    def test_post_automatic_timezone_sync_without_known_zone_keeps_the_zone(self):
        body = json.dumps({"tz": None})
        self.mp.setattr(aiohttp, "ClientSession", fake_client_session(body, []))
        MeticulousConfig[CONFIG_USER]["timezone_sync"] = "manual"

        response = self.post_settings({"timezone_sync": "automatic"})

        assert response.code == 200
        assert self.timedatectl.calls == []
        self.assert_saved(timezone_sync="automatic", time_zone="Etc/UTC")

    def test_post_automatic_timezone_sync_failing_to_apply_returns_400(self):
        body = json.dumps({"tz": "Europe/Berlin"})
        self.mp.setattr(aiohttp, "ClientSession", fake_client_session(body, []))
        self.timedatectl.set_timezone_stderr = "Access denied"

        response = self.post_settings({"timezone_sync": "automatic"})

        self.assert_rejected(
            response, 400, "failed to sync timezone: failed to set the provided timezone"
        )


class TestManufacturingAPI(MachineBoundaryTestCase):
    def get_app(self):
        return Application([(r"/api/v1/manufacturing[/]*", ManufacturingSettingsHandler)])

    def test_get_outside_manufacturing_returns_204_without_body(self):
        response = self.fetch("/api/v1/manufacturing")

        assert response.code == 204
        assert response.body == b""

    def test_get_in_manufacturing_returns_the_dial_schema(self):
        Machine.enable_manufacturing = True

        response = self.fetch("/api/v1/manufacturing/")

        assert response.code == 200
        assert json.loads(response.body) == dial_schema

    def test_post_outside_manufacturing_returns_410(self):
        response = self.post_json("/api/v1/manufacturing", {"skip_stage": True})

        assert response.code == 410
        assert json.loads(response.body) == {
            "status": "error",
            "error": "no configuration available",
        }
        assert MeticulousConfig[CONFIG_MANUFACTURING] == MANUFACTURING_BASELINE
        assert not self.config_file.exists()

    def test_post_in_manufacturing_saves_and_returns_the_section(self):
        Machine.enable_manufacturing = True

        response = self.post_json("/api/v1/manufacturing", {"skip_stage": True})

        expected = {**MANUFACTURING_BASELINE, "skip_stage": True}
        assert response.code == 200
        assert json.loads(response.body) == expected
        assert MeticulousConfig[CONFIG_MANUFACTURING] == expected
        assert self.saved_config()[CONFIG_MANUFACTURING] == expected
        assert self.config_socket.emits == [("settings", {})]

    def test_post_invalid_json_returns_403(self):
        Machine.enable_manufacturing = True

        try:
            json.loads("{")
        except json.JSONDecodeError as error:
            json_error = str(error)

        response = self.post_json("/api/v1/manufacturing", "{")

        assert response.code == 403
        assert json.loads(response.body) == {
            "status": "error",
            "error": "invalid json",
            "json_error": json_error,
        }

    def test_post_unknown_key_returns_404_without_saving(self):
        Machine.enable_manufacturing = True

        response = self.post_json("/api/v1/manufacturing", {"skip_stage": True, "turbo": True})

        assert response.code == 404
        assert json.loads(response.body) == {
            "status": "error",
            "error": "'setting turbo not found'",
        }
        assert MeticulousConfig[CONFIG_MANUFACTURING] == MANUFACTURING_BASELINE
        assert not self.config_file.exists()

    def test_post_wrong_type_returns_404_without_saving(self):
        Machine.enable_manufacturing = True

        response = self.post_json("/api/v1/manufacturing", {"skip_stage": "yes"})

        assert response.code == 404
        assert MeticulousConfig[CONFIG_MANUFACTURING] == MANUFACTURING_BASELINE
        assert not self.config_file.exists()

    @pytest.mark.xfail(
        strict=True,
        reason="backend bug: api/settings.py:278 reports type(setting_target) (always str) "
        "as the expected type instead of the type of the stored value",
    )
    def test_post_wrong_type_names_the_expected_type(self):
        Machine.enable_manufacturing = True

        response = self.post_json("/api/v1/manufacturing", {"skip_stage": "yes"})

        assert json.loads(response.body)["error"] == (
            "\"setting value invalid, received <class 'str'> and expected <class 'bool'>\""
        )


class TestTimezoneUIAPI(AsyncHTTPTestCase):
    TIMEZONES = {
        "Mexico": {"Cancun": "America/Cancun", "Mexico City": "America/Mexico_City"},
        "Germany": {"Berlin": "Europe/Berlin"},
    }

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        timezones_file = Path(self.temporary.name).joinpath("UI_timezones.json")
        timezones_file.write_text(json.dumps(self.TIMEZONES))
        self.mp = pytest.MonkeyPatch()
        self.mp.setattr(timezone_manager, "TIMEZONE_JSON_FILE_PATH", str(timezones_file))
        super().setUp()

    def tearDown(self):
        super().tearDown()
        self.mp.undo()
        self.temporary.cleanup()

    def get_app(self):
        return Application([(r"/api/v1/timezones/(.*)", TimezoneUIHandler)])

    def test_empty_region_lists_all_countries(self):
        response = self.fetch("/api/v1/timezones/")

        assert response.code == 200
        assert json.loads(response.body) == {"countries": ["Mexico", "Germany"]}

    def test_countries_filter_matches_the_lowercase_prefix(self):
        response = self.fetch("/api/v1/timezones/countries?filter=ge")

        assert response.code == 200
        assert json.loads(response.body) == {"countries": ["Germany"]}

    @pytest.mark.xfail(
        strict=True,
        reason="backend bug: api/settings.py:239 lowercases the country but not the filter, "
        "so any filter containing an uppercase letter matches nothing",
    )
    def test_countries_filter_ignores_case(self):
        response = self.fetch("/api/v1/timezones/countries?filter=Ge")

        assert json.loads(response.body) == {"countries": ["Germany"]}

    def test_cities_lists_the_zones_of_a_country(self):
        response = self.fetch("/api/v1/timezones/cities?filter=Mexico")

        assert response.code == 200
        assert json.loads(response.body) == {
            "cities": [{"Cancun": "America/Cancun"}, {"Mexico City": "America/Mexico_City"}]
        }

    def test_cities_of_an_unknown_country_returns_403(self):
        response = self.fetch("/api/v1/timezones/cities?filter=Atlantis")

        assert response.code == 403
        assert json.loads(response.body) == {
            "status": "error",
            "error": "invalid country requested",
        }

    def test_unknown_region_type_returns_403(self):
        response = self.fetch("/api/v1/timezones/regions")

        assert response.code == 403
        assert json.loads(response.body) == {
            "status": "error",
            "error": "invalid region type requested",
        }

    def test_undecodable_filter_is_rejected_by_tornado_with_400(self):
        # get_argument raises HTTPError(400) itself, so the handler's
        # UnicodeDecodeError branch (403) is never reached.
        response = self.fetch("/api/v1/timezones/countries?filter=%ff")

        assert response.code == 400


class TestSerialAPI(MachineBoundaryTestCase):
    FORM = {
        "color": "white",
        "serial": "MET-0042",
        "batch_number": "B7",
        "build_date": "2026-10-09",
    }

    def get_app(self):
        return Application(
            [
                (r"/api/v1/serial", SerialNumberHandler),
                (r"/api/v1/scaleCalibrate", ScaleCalibrateHandler),
            ]
        )

    def post_form(self, form):
        return self.fetch("/api/v1/serial", method="POST", body=urlencode(form))

    def test_routes_are_registered_with_the_patterns_under_test(self):
        routes = API._versions[APIVersion.V1]
        assert routes["/serial"][0] is SerialNumberHandler
        assert routes["/scaleCalibrate"][0] is ScaleCalibrateHandler

    def test_get_serves_the_serial_form(self):
        response = self.fetch("/api/v1/serial")

        assert response.code == 200
        assert response.headers["Content-Type"] == "text/html"
        assert response.body.decode() == WEBPAGE

    def test_post_writes_the_identity_to_the_esp32_nvs(self):
        response = self.post_form(self.FORM)

        assert response.code == 200
        assert response.headers["Content-Type"] == "text/html"
        assert response.body.decode() == (
            "Received Data:<br>Color: white<br>Serial: MET-0042<br>"
            "Batch Number: B7<br>Build Date: 2026-10-09"
        )
        assert self.port.writes == [
            b"nvs_request,write,color_key,white\x03",
            b"nvs_request,write,serial_number_key,MET-0042\x03",
            b"nvs_request,write,batch_number_key,B7\x03",
            b"nvs_request,write,build_date_key,2026-10-09\x03",
        ]

    def test_post_stores_the_identity_in_the_system_config(self):
        self.post_form(self.FORM)

        expected = {
            MACHINE_SERIAL_NUMBER: "MET-0042",
            MACHINE_COLOR: "white",
            MACHINE_BATCH_NUMBER: "B7",
            MACHINE_BUILD_DATE: "2026-10-09",
        }
        saved_system = self.saved_config()[CONFIG_SYSTEM]
        for key, value in expected.items():
            assert MeticulousConfig[CONFIG_SYSTEM][key] == value
            assert saved_system[key] == value
        assert self.config_socket.emits == [("settings", {})]

    def test_post_raises_a_notification_with_the_identity(self):
        self.post_form(self.FORM)

        [notification] = NotificationManager._notifications
        assert notification.message == (
            "\nSerial number: MET-0042\n\nBatch number: B7\n\nColor: white\n\n"
            "Build Date: 2026-10-09\n" + " " * 12
        )
        assert notification.respone_options == [NotificationResponse.OK]
        assert NotificationManager._queue.get_nowait() is notification
        assert self.sounds == [Sounds.NOTIFICATION]

    def test_post_without_color_defaults_to_black(self):
        form = {key: value for key, value in self.FORM.items() if key != "color"}

        response = self.post_form(form)

        assert response.code == 200
        assert self.port.writes[0] == b"nvs_request,write,color_key,black\x03"

    def test_post_empty_field_answers_200_with_a_message_and_writes_nothing(self):
        for field, message in (
            ("color", "Color is required"),
            ("serial", "Serial is required"),
            ("batch_number", "Batch number is required"),
            ("build_date", "Build date is required"),
        ):
            response = self.post_form({**self.FORM, field: ""})

            assert response.code == 200
            assert response.body.decode() == message
        assert self.port.writes == []
        assert not self.config_file.exists()

    def test_post_missing_field_returns_400(self):
        form = {key: value for key, value in self.FORM.items() if key != "serial"}

        response = self.post_form(form)

        assert response.code == 400
        assert self.port.writes == []
        assert not self.config_file.exists()

    def test_scale_calibrate_sends_the_master_calibration_action(self):
        response = self.fetch("/api/v1/scaleCalibrate")

        assert response.code == 200
        assert response.body == b""
        assert self.port.writes == [b"action,scale_master_calibration\x03"]


class TestRootPasswordAPI(MachineBoundaryTestCase):
    def get_app(self):
        return Application([(r"/api/v1/machine/root-password", RootPasswordHandler)])

    def test_route_is_registered_with_the_pattern_under_test(self):
        assert API._versions[APIVersion.V1]["/machine/root-password"][0] is RootPasswordHandler

    def test_returns_the_configured_password(self):
        MeticulousConfig[CONFIG_SYSTEM][ROOT_PASSWORD] = "s3cretpw9"

        response = self.fetch("/api/v1/machine/root-password")

        assert response.code == 200
        assert json.loads(response.body) == {"status": "success", "root_password": "s3cretpw9"}

    def test_falls_back_to_root_when_no_password_was_generated(self):
        response = self.fetch("/api/v1/machine/root-password")

        assert response.code == 200
        assert json.loads(response.body) == {"status": "success", "root_password": "root"}

    def test_remote_request_through_the_proxy_returns_403(self):
        MeticulousConfig[CONFIG_SYSTEM][ROOT_PASSWORD] = "s3cretpw9"

        response = self.fetch(
            "/api/v1/machine/root-password",
            headers={"X-Real-IP": "192.168.1.42", "Host": "meticulous.local"},
        )

        assert response.code == 403
        assert json.loads(response.body) == {
            "status": "error",
            "error": "This endpoint can only be accessed locally",
        }

    def test_remote_ip_is_allowed_when_the_host_is_localhost(self):
        # The check only refuses when both the forwarded IP and the Host are non-local.
        response = self.fetch(
            "/api/v1/machine/root-password",
            headers={"X-Real-IP": "192.168.1.42", "Host": "localhost"},
        )

        assert response.code == 200
        assert json.loads(response.body)["root_password"] == "root"
