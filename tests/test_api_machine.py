"""Handler-level regression tests for the /machine, /update and /timezones routes.

Only the OS and hardware boundaries are replaced: the reboot/wipe calls, the
`date` command, the sysfs backlight files, the image metadata files under /opt,
the firmware folder and the ESP32 flasher. Everything between the HTTP request
and those boundaries is the real code.
"""

import copy
import os
import io
import json
import subprocess
import threading
import time
import zipfile
from unittest.mock import MagicMock, call, patch

import pytest
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

import api.machine as api_machine
import backlight_controller
import esp_serial.esp_tool_wrapper as esp_tool_wrapper
import ota
import timezone_manager
from api.machine import (
    MachineBacklightController,
    MachineInfoHandler,
    MachineResetHandler,
    MachineTimeHandler,
    OSStatus,
    UpdateOSStatus,
)
from api.settings import TimezoneUIHandler
from api.update import UpdateFirmwareWithZipHandler
from backlight_controller import BacklightController
from config import (
    CONFIG_SYSTEM,
    DEVICE_IDENTIFIER,
    LAST_SYSTEM_VERSIONS,
    MACHINE_BATCH_NUMBER,
    MACHINE_BUILD_DATE,
    MACHINE_COLOR,
    MACHINE_SERIAL_NUMBER,
    MeticulousConfig,
)
from esp_serial.data import ESPInfo
from machine import Machine
from ota import UpdateManager
from pour_over_profiles import PourOverProfileManager
from wifi import WifiManager, WifiSystemConfig


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
        "Content-Type: application/octet-stream\r\n\r\n"
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


class ApiTestCase(AsyncHTTPTestCase):
    @pytest.fixture(autouse=True)
    def _pytest_fixtures(self, monkeypatch, tmp_path, restore_config):
        self.monkeypatch = monkeypatch
        self.tmp_path = tmp_path
        self.config_save = restore_config

    def json(self, response):
        return json.loads(response.body)


class TestMachineInfoRoute(ApiTestCase):
    def setUp(self):
        super().setUp()
        for name in ("emulated", "esp_info", "enable_manufacturing"):
            self.monkeypatch.setattr(Machine, name, getattr(Machine, name))
        for name in ("ROOTFS_BUILD_DATE", "CHANNEL", "REPO_INFO", "VERSION", "is_changed"):
            self.monkeypatch.setattr(UpdateManager, name, None)
        self.monkeypatch.setattr(UpdateManager, "is_changed", False)
        self.monkeypatch.setattr(PourOverProfileManager, "_available", False)

        # Image metadata lives under /opt on the machine; point every reader at tmp files.
        self.build_date_file = self.tmp_path / "ROOTFS_BUILD_DATE"
        self.repo_info_file = self.tmp_path / "summary.txt"
        self.channel_file = self.tmp_path / "image-build-channel"
        self.version_file = self.tmp_path / "image-build-version"
        self.monkeypatch.setattr(ota, "BUILD_DATE_FILE", str(self.build_date_file))
        self.monkeypatch.setattr(ota, "REPO_INFO_FILE", str(self.repo_info_file))
        self.monkeypatch.setattr(ota, "BUILD_CHANNEL_FILE", str(self.channel_file))
        self.monkeypatch.setattr(ota, "BUILD_VERSION_FILE", str(self.version_file))

        # The hostname comes from NetworkManager through nmcli.
        self.monkeypatch.setattr(
            WifiManager,
            "getCurrentConfig",
            MagicMock(
                return_value=WifiSystemConfig(
                    connected=True,
                    connection_name="HomeNet",
                    gateway=None,
                    routes=[],
                    ips=[],
                    dns=[],
                    mac="AA:BB:CC:DD:EE:FF",
                    hostname="meticulousAromaticbean-1234",
                    domains=[],
                )
            ),
        )

    def get_app(self):
        return Application([(r"/api/v1/machine", MachineInfoHandler)])

    def write_image_metadata(self):
        self.build_date_file.write_text("Tue, 06 Oct 2026 12:34:56 +0000\n")
        self.channel_file.write_text("beta\n")
        self.version_file.write_text("2026M1446-beta\n")
        self.repo_info_file.write_text(
            "## meticulous-backend ##\n"
            "Repository: meticulous-backend\n"
            "URL: https://github.com/MeticulousHome/meticulous-backend\n"
            "Branch: beta\n"
            "Commit: abc1234\n"
            "Last commit details:\n"
            "abc1234 Fix wifi repair (2026-10-01)\n"
            "Modified files:\n"
            "\n"
            "## meticulous-dial ##\n"
            "Repository: meticulous-dial\n"
            "Branch: main\n"
            "Commit: def5678\n"
        )

    def test_reports_every_machine_field_from_esp_config_and_image_metadata(self):
        MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER] = ["aromatic", "bean"]
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] = "1234"
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_COLOR] = "black"
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_BATCH_NUMBER] = "B-07"
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_BUILD_DATE] = "2026-01-15"
        MeticulousConfig[CONFIG_SYSTEM][LAST_SYSTEM_VERSIONS] = [
            {"version": "2026M1400-beta", "date": "2026-09-01"}
        ]
        Machine.esp_info = ESPInfo(
            firmwareV="1.2.3", espPinout=1, mainVoltage=230.5, tareBehavior="auto"
        )
        Machine.enable_manufacturing = True
        UpdateManager.is_changed = True
        PourOverProfileManager._available = True
        self.write_image_metadata()

        response = self.fetch("/api/v1/machine")

        assert response.code == 200
        assert response.headers["Content-Type"] == "application/json"
        assert self.json(response) == {
            "name": "MeticulousAromaticbean",
            "hostname": "meticulousAromaticbean-1234",
            "firmware": "1.2.3",
            "mainVoltage": 230.5,
            "tare_behavior_supported": True,
            "pour_over_profiles_supported": True,
            "pour_over_profile_schema_version": 1,
            "serial": "1234",
            "color": "black",
            "batch_number": "B-07",
            "build_date": "2026-01-15",
            "software_version": "2026-10-06 12:34:56",
            "image_build_channel": "beta",
            "image_version": "2026M1446-beta",
            "repository_info": {
                "meticulous-backend": {
                    "branch": "beta",
                    "commit": "abc1234 Fix wifi repair (2026-10-01)",
                },
                "meticulous-dial": {"branch": "main", "commit": None},
            },
            "manufacturing": True,
            "upgrade_first_boot": True,
            "version_history": [{"version": "2026M1400-beta", "date": "2026-09-01"}],
        }

    def test_fresh_machine_without_esp_or_image_metadata_reports_empty_defaults(self):
        MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER] = []
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] = None
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_COLOR] = None
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_BATCH_NUMBER] = None
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_BUILD_DATE] = None
        MeticulousConfig[CONFIG_SYSTEM][LAST_SYSTEM_VERSIONS] = None
        Machine.esp_info = None
        Machine.enable_manufacturing = False

        response = self.fetch("/api/v1/machine")

        assert response.code == 200
        # firmware and mainVoltage are omitted, not null, until the ESP reports in.
        assert self.json(response) == {
            "name": "MeticulousEspresso",
            "hostname": "meticulousAromaticbean-1234",
            "tare_behavior_supported": False,
            "pour_over_profiles_supported": False,
            "pour_over_profile_schema_version": None,
            "serial": None,
            "color": "",
            "batch_number": "",
            "build_date": "",
            "software_version": None,
            "image_build_channel": None,
            "image_version": None,
            "repository_info": {},
            "manufacturing": False,
            "upgrade_first_boot": False,
            "version_history": [],
        }

    def test_firmware_without_tare_behavior_reports_tare_behavior_unsupported(self):
        Machine.esp_info = ESPInfo(firmwareV="1.0.0", espPinout=0, mainVoltage=120.0)

        body = self.json(self.fetch("/api/v1/machine"))

        assert body["firmware"] == "1.0.0"
        assert body["mainVoltage"] == 120.0
        assert body["tare_behavior_supported"] is False

    def test_unparseable_build_date_reports_null_software_version(self):
        self.build_date_file.write_text("2026-10-06 12:34:56\n")

        body = self.json(self.fetch("/api/v1/machine"))

        assert body["software_version"] is None


class TestMachineTimeRoute(ApiTestCase):
    def setUp(self):
        super().setUp()
        self.date_command = MagicMock()
        # `date --set` is the OS boundary; the handler still catches the real error type.
        self.monkeypatch.setattr(
            timezone_manager,
            "subprocess",
            MagicMock(
                check_call=self.date_command, CalledProcessError=subprocess.CalledProcessError
            ),
        )

        self.original_tz = os.environ.get("TZ")

    def tearDown(self):
        super().tearDown()
        # monkeypatch undoes after tearDown, too late for tzset, so restore TZ here.
        if self.original_tz is None:
            os.environ.pop("TZ", None)
        else:
            os.environ["TZ"] = self.original_tz
        time.tzset()

    def get_app(self):
        return Application([(r"/api/v1/machine/time", MachineTimeHandler)])

    def post_json(self, body):
        return self.fetch("/api/v1/machine/time", method="POST", body=body)

    def test_naive_iso_date_is_passed_to_date_command_unchanged(self):
        response = self.post_json(json.dumps({"date": "2026-10-09T12:34:56"}))

        assert response.code == 200
        assert self.json(response) == {"status": "success"}
        self.date_command.assert_called_once_with(["date", "--set", "2026-10-09 12:34:56"])

    def test_utc_iso_date_is_converted_to_the_machine_local_time(self):
        # POSIX TZ "TST+6" is UTC-6 and needs no tzdata.
        os.environ["TZ"] = "TST+6"
        time.tzset()

        response = self.post_json(json.dumps({"date": "2026-10-09T12:34:56Z"}))

        assert response.code == 200
        self.date_command.assert_called_once_with(["date", "--set", "2026-10-09 06:34:56"])

    def test_invalid_json_is_rejected_without_touching_the_clock(self):
        response = self.post_json("{not json")

        assert response.code == 400
        assert self.json(response) == {"error": "Invalid JSON"}
        self.date_command.assert_not_called()

    def test_missing_date_is_rejected(self):
        response = self.post_json(json.dumps({"time": "2026-10-09T12:34:56Z"}))

        assert response.code == 400
        assert self.json(response) == {"error": "Missing 'date' in request"}
        self.date_command.assert_not_called()

    def test_non_iso_date_is_rejected(self):
        response = self.post_json(json.dumps({"date": "09/10/2026 12:34"}))

        assert response.code == 400
        assert self.json(response) == {"error": "Invalid ISO date format"}
        self.date_command.assert_not_called()

    def test_failing_date_command_reports_500(self):
        self.date_command.side_effect = subprocess.CalledProcessError(1, ["date"])

        response = self.post_json(json.dumps({"date": "2026-10-09T12:34:56"}))

        assert response.code == 500
        assert self.json(response) == {"error": "Failed to set system time"}


class TestFactoryResetRoute(ApiTestCase):
    def setUp(self):
        super().setUp()
        self.monkeypatch.setattr(Machine, "emulated", False)
        self.user_root = self.tmp_path / "meticulous-user"
        self.user_root.mkdir()
        (self.user_root / "profiles").mkdir()
        (self.user_root / "profiles" / "profile.json").write_text("{}")
        (self.user_root / "config.yml").write_text("user: {}")
        (self.user_root / ".device-identity").mkdir()

        real_cleanup = api_machine.cleanup_factory_reset_data
        # The default argument binds /meticulous-user at import time, so redirect the call.
        self.boundary = MagicMock()
        self.boundary.cleanup.side_effect = lambda: real_cleanup(self.user_root)
        self.monkeypatch.setattr(
            api_machine, "cleanup_factory_reset_data", self.boundary.cleanup
        )
        self.monkeypatch.setattr(api_machine, "subprocess", MagicMock(run=self.boundary.reboot))

    def get_app(self):
        return Application([(r"/api/v1/machine/factory_reset", MachineResetHandler)])

    def assert_nothing_wiped(self):
        assert self.boundary.mock_calls == []
        assert (self.user_root / "profiles" / "profile.json").exists()
        assert (self.user_root / "config.yml").exists()

    def test_reset_without_confirmation_is_rejected(self):
        response = self.fetch("/api/v1/machine/factory_reset")

        assert response.code == 400
        assert self.json(response) == {"error": "Confirmation required. Add confirm=true"}
        self.assert_nothing_wiped()

    def test_reset_with_confirmation_other_than_true_is_rejected(self):
        response = self.fetch("/api/v1/machine/factory_reset?confirm=yes")

        assert response.code == 400
        assert self.json(response) == {"error": "Confirmation required. Add confirm=true"}
        self.assert_nothing_wiped()

    def test_confirmed_reset_is_only_simulated_on_an_emulated_machine(self):
        Machine.emulated = True

        response = self.fetch("/api/v1/machine/factory_reset?confirm=true")

        assert response.code == 200
        assert self.json(response) == {"status": "success", "message": "Emulated mode"}
        self.assert_nothing_wiped()

    def test_confirmed_reset_wipes_user_data_then_reboots(self):
        response = self.fetch("/api/v1/machine/factory_reset?confirm=true")

        assert response.code == 200
        assert response.body == b""
        assert self.boundary.mock_calls == [call.cleanup(), call.reboot("reboot")]
        assert sorted(p.name for p in self.user_root.iterdir()) == [".device-identity"]

    def test_reset_requested_from_a_remote_client_is_forbidden(self):
        response = self.fetch(
            "/api/v1/machine/factory_reset?confirm=true",
            headers={"Host": "meticulous.local", "X-Real-IP": "192.168.1.50"},
        )

        assert response.code == 403
        assert self.json(response) == {
            "status": "error",
            "error": "This endpoint can only be accessed locally",
        }
        self.assert_nothing_wiped()


class TestBacklightRoute(ApiTestCase):
    MAX_BRIGHTNESS = 200

    def setUp(self):
        super().setUp()
        self.monkeypatch.setattr(Machine, "emulated", False)
        self.brightness_file = self.tmp_path / "brightness"
        self.max_brightness_file = self.tmp_path / "max_brightness"
        self.brightness_file.write_text("20\n")
        self.max_brightness_file.write_text(f"{self.MAX_BRIGHTNESS}\n")
        self.monkeypatch.setattr(
            backlight_controller, "BRIGHTNESS_FILE", str(self.brightness_file)
        )
        self.monkeypatch.setattr(
            backlight_controller, "MAX_BRIGHTNESS_FILE", str(self.max_brightness_file)
        )
        self.monkeypatch.setattr(BacklightController, "_MAX_BRIGHTNESS", None)
        self.monkeypatch.setattr(BacklightController, "_adjust_thread", None)
        self.dim = MagicMock(wraps=BacklightController.dim)
        self.monkeypatch.setattr(BacklightController, "dim", self.dim)

    def tearDown(self):
        thread = BacklightController._adjust_thread
        if thread is not None:
            thread.join(timeout=5)
        super().tearDown()

    def get_app(self):
        return Application([(r"/api/v1/machine/backlight", MachineBacklightController)])

    def post_json(self, body):
        return self.fetch("/api/v1/machine/backlight", method="POST", body=body)

    def wait_for_dimming(self):
        thread = BacklightController._adjust_thread
        assert thread is not None
        thread.join(timeout=5)
        assert not thread.is_alive()

    def test_get_is_not_supported(self):
        assert self.fetch("/api/v1/machine/backlight").code == 405

    def test_brightness_is_written_to_sysfs_as_a_fraction_of_max(self):
        response = self.post_json(
            json.dumps({"brightness": 0.5, "interpolation": "linear", "animation_time": 0.04})
        )

        assert response.code == 200
        assert response.body == b""
        self.dim.assert_called_once_with(0.5, "linear", 0.04)
        self.wait_for_dimming()
        assert self.brightness_file.read_text() == str(self.MAX_BRIGHTNESS // 2)

    def test_dimming_defaults_to_a_one_second_curve(self):
        # 75 steps/s for 1 s is the slow path, so only check the arguments here.
        self.monkeypatch.setattr(BacklightController, "adjust_brightness", MagicMock())

        response = self.post_json(json.dumps({"brightness": 0.8}))

        assert response.code == 200
        self.dim.assert_called_once_with(0.8, "curve", 1)
        BacklightController.adjust_brightness.assert_called_once_with(
            0.8, interpolation="curve", steps_per_second=75, target_time=1
        )

    def test_brightness_above_one_is_clamped_to_max(self):
        response = self.post_json(json.dumps({"brightness": 1.7, "animation_time": 0.04}))

        assert response.code == 200
        self.wait_for_dimming()
        assert self.brightness_file.read_text() == str(self.MAX_BRIGHTNESS)

    def test_emulated_machine_never_touches_sysfs(self):
        Machine.emulated = True

        response = self.post_json(json.dumps({"brightness": 0.5, "animation_time": 0.04}))

        assert response.code == 200
        self.dim.assert_called_once_with(0.5, "curve", 0.04)
        assert BacklightController._adjust_thread is None
        assert self.brightness_file.read_text() == "20\n"

    def test_null_brightness_is_accepted_without_dimming(self):
        response = self.post_json(json.dumps({"brightness": None}))

        assert response.code == 200
        self.dim.assert_not_called()

    def test_missing_brightness_is_rejected(self):
        response = self.post_json(json.dumps({"interpolation": "linear"}))

        assert response.code == 400
        assert self.json(response) == {
            "status": "error",
            "error": "brightness value is required",
        }
        self.dim.assert_not_called()

    def test_invalid_json_is_rejected_with_403(self):
        response = self.post_json("{brightness")

        assert response.code == 403
        body = self.json(response)
        assert body["status"] == "error"
        assert body["error"] == "invalid json"
        assert body["json_error"] == (
            "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"
        )
        self.dim.assert_not_called()


class TestOSUpdateStatusRoute(ApiTestCase):
    def setUp(self):
        super().setUp()
        for name in ("last_progress", "last_status", "last_extra_info"):
            self.monkeypatch.setattr(UpdateOSStatus, name, getattr(UpdateOSStatus, name))
        # No socket.io server in the tests: sendStatus must only record the state.
        self.monkeypatch.setattr(UpdateOSStatus, "_UpdateOSStatus__sio", None)

    def get_app(self):
        return Application([(r"/api/v1/machine/OS_update_status", UpdateOSStatus)])

    def test_idle_status_is_reported_before_any_update(self):
        UpdateOSStatus.last_progress = 0
        UpdateOSStatus.last_status = OSStatus.IDLE
        UpdateOSStatus.last_extra_info = None

        response = self.fetch("/api/v1/machine/OS_update_status")

        assert response.code == 200
        assert self.json(response) == {"progress": 0, "status": "IDLE", "info": ""}

    def test_reported_progress_is_rounded_to_a_whole_percent(self):
        UpdateOSStatus.last_extra_info = None
        UpdateOSStatus.sendStatus(OSStatus.DOWNLOADING, 42.6)

        response = self.fetch("/api/v1/machine/OS_update_status")

        assert self.json(response) == {"progress": 43, "status": "DOWNLOADING", "info": ""}

    def test_extra_info_is_appended_after_a_separator(self):
        UpdateOSStatus.last_progress = 100
        UpdateOSStatus.last_status = OSStatus.COMPLETE
        UpdateOSStatus.last_extra_info = "rebooting"

        response = self.fetch("/api/v1/machine/OS_update_status")

        assert self.json(response) == {
            "progress": 100,
            "status": "COMPLETE",
            "info": " : rebooting",
        }

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: UpdateOSStatus.sendStatus (api/machine.py:172) ignores its "
            "extra_info argument and never sets last_extra_info, so the failure reason "
            "dbus_monitor.py passes on FAILED never reaches /machine/OS_update_status"
        ),
    )
    def test_failure_reason_passed_to_send_status_is_reported(self):
        UpdateOSStatus.last_extra_info = None
        UpdateOSStatus.sendStatus(OSStatus.FAILED, 0, "Installation error")

        response = self.fetch("/api/v1/machine/OS_update_status")

        assert self.json(response) == {
            "progress": 0,
            "status": "FAILED",
            "info": " : Installation error",
        }


class TestUpdateFirmwareRoute(ApiTestCase):
    def setUp(self):
        super().setUp()
        self.firmware_root = self.tmp_path / "firmware"
        self.monkeypatch.setattr(esp_tool_wrapper, "UPDATE_PATH", str(self.firmware_root))
        self.refresh = MagicMock()
        self.update_started = threading.Event()
        # startUpdate flashes the ESP32 over UART: replace it with a recorder.
        self.start_update = MagicMock(side_effect=lambda: self.update_started.set())
        self.monkeypatch.setattr(Machine, "refreshAvailableFirmware", self.refresh)
        self.monkeypatch.setattr(Machine, "startUpdate", self.start_update)

    def get_app(self):
        return Application([(r"/api/v1/update/firmware", UpdateFirmwareWithZipHandler)])

    def upload(self, query, filename, content):
        body, headers = multipart_body("file", filename, content)
        return self.fetch(
            f"/api/v1/update/firmware{query}", method="POST", body=body, headers=headers
        )

    def assert_no_flash_started(self):
        self.refresh.assert_not_called()
        self.start_update.assert_not_called()

    def test_missing_chip_parameter_is_rejected(self):
        response = self.upload("", "firmware.bin", b"\xe9firmware")

        assert response.code == 400
        assert response.body == b"Missing 'chip' parameter"
        self.assert_no_flash_started()

    def test_unknown_chip_is_rejected_with_the_allowed_names(self):
        response = self.upload("?chip=esp8266", "firmware.bin", b"\xe9firmware")

        assert response.code == 400
        assert response.body == (
            b"Invalid 'chip' parameter. Allowed (case-insensitive): ['ESP32S1', 'ESP32S3']"
        )
        self.assert_no_flash_started()

    def test_request_without_file_is_rejected(self):
        response = self.fetch("/api/v1/update/firmware?chip=esp32s3", method="POST", body=b"")

        assert response.code == 400
        assert response.body == b"No file uploaded."
        self.assert_no_flash_started()

    def test_file_that_is_neither_zip_nor_known_image_is_rejected(self):
        response = self.upload("?chip=esp32s3", "firmware.hex", b":1000")

        assert response.code == 400
        assert response.body == (
            b"Invalid file format. Only ZIP files and certain images are accepted."
        )
        assert not self.firmware_root.exists()
        self.assert_no_flash_started()

    def test_single_image_is_stored_in_the_chip_folder_and_flashing_starts(self):
        response = self.upload("?chip=ESP32-S3", "firmware.bin", b"\xe9firmware")

        assert response.code == 200
        assert response.body == b"success"
        assert (
            self.firmware_root / "esp32-s3" / "firmware.bin"
        ).read_bytes() == b"\xe9firmware"
        self.refresh.assert_called_once_with()
        assert self.update_started.wait(timeout=5)
        self.start_update.assert_called_once_with()

    def test_zip_is_unpacked_into_the_chip_folder_and_flashing_starts(self):
        archive = zip_bytes({"firmware.bin": b"\xe9fw", "partitions.bin": b"parts"})

        response = self.upload("?chip=esp32", "esp32-firmware.zip", archive)

        assert response.code == 200
        assert response.body == b"success"
        assert (self.firmware_root / "esp32" / "firmware.bin").read_bytes() == b"\xe9fw"
        assert (self.firmware_root / "esp32" / "partitions.bin").read_bytes() == b"parts"
        self.refresh.assert_called_once_with()
        assert self.update_started.wait(timeout=5)

    def test_corrupted_zip_reports_failure_and_never_flashes(self):
        response = self.upload("?chip=esp32s3", "firmware.zip", b"PK\x03\x04 not a zip")

        assert response.code == 400
        assert response.body == (
            b"The uploaded file is not a valid ZIP archive.failure during upload"
        )
        self.assert_no_flash_started()


class TestTimezonesRoute(ApiTestCase):
    TIMEZONES = {
        "Mexico": {"Cancun": "America/Cancun", "Mexico City": "America/Mexico_City"},
        "Germany": {"Berlin": "Europe/Berlin", "Busingen": "Europe/Busingen"},
        "Netherlands": {"Amsterdam": "Europe/Amsterdam"},
    }

    def setUp(self):
        super().setUp()
        timezones_file = self.tmp_path / "UI_timezones.json"
        timezones_file.write_text(json.dumps(self.TIMEZONES))
        # An existing file short-circuits the timedatectl-based generator.
        self.monkeypatch.setattr(
            timezone_manager, "TIMEZONE_JSON_FILE_PATH", str(timezones_file)
        )
        self.monkeypatch.setattr(TimezoneUIHandler, "_TimezoneUIHandler__timezone_map", {})

    def get_app(self):
        return Application([(r"/api/v1/timezones/(.*)", TimezoneUIHandler)])

    def test_empty_region_lists_all_countries(self):
        response = self.fetch("/api/v1/timezones/")

        assert response.code == 200
        assert self.json(response) == {"countries": ["Mexico", "Germany", "Netherlands"]}

    def test_countries_are_filtered_by_lowercase_prefix(self):
        response = self.fetch("/api/v1/timezones/countries?filter=ne")

        assert response.code == 200
        assert self.json(response) == {"countries": ["Netherlands"]}

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "backend bug: TimezoneUIHandler (api/settings.py:239) lowercases the country "
            "but not the filter, so any capitalised filter such as 'Mex' matches nothing"
        ),
    )
    def test_country_filter_is_case_insensitive(self):
        response = self.fetch("/api/v1/timezones/countries?filter=Mex")

        assert self.json(response) == {"countries": ["Mexico"]}

    def test_cities_of_a_country_are_listed_as_name_to_zone_pairs(self):
        response = self.fetch("/api/v1/timezones/cities?filter=Mexico")

        assert response.code == 200
        assert self.json(response) == {
            "cities": [
                {"Cancun": "America/Cancun"},
                {"Mexico City": "America/Mexico_City"},
            ]
        }

    def test_cities_of_an_unknown_country_are_rejected(self):
        response = self.fetch("/api/v1/timezones/cities?filter=Atlantis")

        assert response.code == 403
        assert self.json(response) == {"status": "error", "error": "invalid country requested"}

    def test_unknown_region_type_is_rejected(self):
        response = self.fetch("/api/v1/timezones/planets")

        assert response.code == 403
        assert self.json(response) == {
            "status": "error",
            "error": "invalid region type requested",
        }
