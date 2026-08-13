import asyncio
import json
import os
import subprocess
import tempfile
import threading
from unittest.mock import patch

import tornado.web
from tornado.testing import AsyncHTTPTestCase, gen_test

from api.api import API, APIVersion
from api.base_handler import LocalAccessHandler
from api.update import (
    HAWKBIT_CHECK_COOLDOWN_SECONDS,
    HAWKBIT_RESTART_TIMEOUT_SECONDS,
    UpdateCheckHandler,
)


class TestUpdateCheckHandler(AsyncHTTPTestCase):
    def get_app(self):
        return tornado.web.Application([(r"/api/v1/update/check", UpdateCheckHandler)])

    def setUp(self):
        self.state_directory = tempfile.TemporaryDirectory()
        self.state_path = f"{self.state_directory.name}/last-attempt"
        self.state_patch = patch("api.update.HAWKBIT_CHECK_STATE_PATH", self.state_path)
        self.emulation_patch = patch("api.update.machine_is_emulated", return_value=False)
        self.state_patch.start()
        self.emulation_patch.start()
        super().setUp()

    def tearDown(self):
        super().tearDown()
        self.emulation_patch.stop()
        self.state_patch.stop()
        self.state_directory.cleanup()

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update.subprocess.run")
    def test_restarts_existing_updater(self, run, _active):
        response = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 200
        assert json.loads(response.body) == {"status": "success"}
        run.assert_called_once_with(
            ["systemctl", "restart", "rauc-hawkbit-updater"],
            check=True,
            timeout=HAWKBIT_RESTART_TIMEOUT_SECONDS,
        )

    @patch("api.update.os_update_is_active", return_value=True)
    @patch("api.update.subprocess.run")
    def test_rejects_active_update_without_restart(self, run, _active):
        response = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 409
        assert json.loads(response.body) == {
            "status": "error",
            "error": "An update is already active",
        }
        run.assert_not_called()

    @patch("api.update.subprocess.run")
    def test_emulation_is_safe_no_op_without_production_state(self, run):
        with patch("api.update.machine_is_emulated", return_value=True):
            response = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 200
        assert json.loads(response.body) == {
            "status": "emulated",
            "message": "Update check skipped",
        }
        assert not os.path.exists(self.state_path)
        run.assert_not_called()

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update.subprocess.run")
    def test_records_attempt_before_invoking_systemctl(self, run, _active):
        def assert_state_exists(*_args, **_kwargs):
            assert os.path.exists(self.state_path)

        run.side_effect = assert_state_exists

        response = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 200

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update._boot_time", side_effect=[100.0, 100.0])
    @patch("api.update.subprocess.run")
    def test_cooldown_is_persisted_across_handler_recreation(self, run, _time, _active):
        first = self.fetch("/api/v1/update/check", method="POST", body="")
        second = self.fetch("/api/v1/update/check", method="POST", body="")

        assert first.code == 200
        assert second.code == 429
        assert second.headers["Retry-After"] == str(HAWKBIT_CHECK_COOLDOWN_SECONDS)
        assert json.loads(second.body) == {
            "status": "cooldown",
            "error": "An update check was recently requested",
            "retry_after_seconds": HAWKBIT_CHECK_COOLDOWN_SECONDS,
        }
        run.assert_called_once()

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update._boot_time", return_value=101.0)
    @patch("api.update.subprocess.run")
    def test_persisted_cooldown_is_loaded_without_process_memory(self, run, _time, _active):
        with open(self.state_path, "w", encoding="ascii") as state_file:
            state_file.write("100.0")

        response = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 429
        assert json.loads(response.body)["retry_after_seconds"] == 4_000
        run.assert_not_called()

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update._boot_time")
    @patch("api.update.subprocess.run")
    def test_cooldown_boundary_allows_restart(self, run, monotonic, _active):
        monotonic.side_effect = [100.0, 100.0 + HAWKBIT_CHECK_COOLDOWN_SECONDS]

        first = self.fetch("/api/v1/update/check", method="POST", body="")
        second = self.fetch("/api/v1/update/check", method="POST", body="")

        assert first.code == 200
        assert second.code == 200
        assert run.call_count == 2

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update.subprocess.run")
    @gen_test
    async def test_concurrent_requests_are_serialized(self, run, _active):
        started = threading.Event()
        release = threading.Event()

        def blocking_restart(*_args, **_kwargs):
            started.set()
            assert release.wait(timeout=2)

        run.side_effect = blocking_restart
        first = self.http_client.fetch(
            self.get_url("/api/v1/update/check"), method="POST", body=""
        )
        assert await asyncio.to_thread(started.wait, 2)
        second = self.http_client.fetch(
            self.get_url("/api/v1/update/check"), method="POST", body="", raise_error=False
        )
        release.set()
        first_response, second_response = await asyncio.gather(first, second)

        assert first_response.code == 200
        assert second_response.code == 429
        run.assert_called_once()

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update.subprocess.run")
    @gen_test
    async def test_restart_does_not_block_io_loop(self, run, _active):
        started = threading.Event()
        release = threading.Event()

        def blocking_restart(*_args, **_kwargs):
            started.set()
            assert release.wait(timeout=2)

        run.side_effect = blocking_restart
        response_future = self.http_client.fetch(
            self.get_url("/api/v1/update/check"), method="POST", body=""
        )
        assert await asyncio.to_thread(started.wait, 2)
        loop_remained_responsive = False
        await asyncio.sleep(0)
        loop_remained_responsive = True
        release.set()
        response = await response_future

        assert loop_remained_responsive
        assert response.code == 200

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update.subprocess.run")
    def test_timeout_is_sanitized_and_attempt_remains_on_cooldown(self, run, _active):
        sensitive_command = ["systemctl", "secret-value"]
        run.side_effect = subprocess.TimeoutExpired(sensitive_command, 60, stderr="secret")

        response = self.fetch("/api/v1/update/check", method="POST", body="")
        retry = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 500
        assert json.loads(response.body) == {
            "status": "error",
            "error": "Failed to request update check",
        }
        assert b"secret" not in response.body
        assert retry.code == 429
        run.assert_called_once()

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update.subprocess.run")
    def test_called_process_failure_is_sanitized(self, run, _active):
        run.side_effect = subprocess.CalledProcessError(
            1, ["systemctl", "secret-value"], output="secret", stderr="secret"
        )

        response = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 500
        assert json.loads(response.body) == {
            "status": "error",
            "error": "Failed to request update check",
        }
        assert b"secret" not in response.body

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update.subprocess.run")
    def test_os_error_is_sanitized(self, run, _active):
        run.side_effect = FileNotFoundError(2, "secret", "/private/systemctl")

        response = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 500
        assert json.loads(response.body) == {
            "status": "error",
            "error": "Failed to request update check",
        }
        assert b"/private/systemctl" not in response.body


def test_cooldown_exceeds_updater_start_limit_average():
    assert HAWKBIT_CHECK_COOLDOWN_SECONDS > 20_000 / 5


def test_route_is_registered_as_local_access_handler():
    handler, kwargs = API._versions[APIVersion.V1][r"/update/check"]

    assert handler is UpdateCheckHandler
    assert issubclass(handler, LocalAccessHandler)
    assert kwargs == {}
