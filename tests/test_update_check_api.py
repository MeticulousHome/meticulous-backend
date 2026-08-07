import json
import subprocess
from unittest.mock import patch

import tornado.web
from tornado.testing import AsyncHTTPTestCase

from api.api import API, APIVersion
from api.base_handler import LocalAccessHandler
from api.update import UpdateCheckHandler


class TestUpdateCheckHandler(AsyncHTTPTestCase):
    def get_app(self):
        return tornado.web.Application([(r"/api/v1/update/check", UpdateCheckHandler)])

    def setUp(self):
        super().setUp()

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update.subprocess.run")
    def test_restarts_existing_updater(self, run, _active):
        response = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 200
        assert json.loads(response.body) == {"status": "success"}
        run.assert_called_once_with(
            ["systemctl", "restart", "rauc-hawkbit-updater"], check=True
        )

    @patch("api.update.os_update_is_active", return_value=True)
    @patch("api.update.subprocess.run")
    def test_rejects_active_update_without_restart(self, run, _active):
        response = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 409

        run.assert_not_called()

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update.subprocess.run")
    def test_rejects_non_loopback_proxy_address(self, run, _active):
        response = self.fetch(
            "/api/v1/update/check",
            method="POST",
            body="",
            headers={"X-Real-IP": "203.0.113.42"},
        )

        assert response.code == 403
        assert json.loads(response.body) == {
            "status": "error",
            "error": "This endpoint can only be accessed locally",
        }
        run.assert_not_called()

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update.subprocess.run")
    def test_restart_failure_is_sanitized(self, run, _active):
        sensitive_detail = "token=secret /private/updater stderr detail"
        run.side_effect = subprocess.CalledProcessError(
            1,
            ["systemctl", "restart", "rauc-hawkbit-updater"],
            output=sensitive_detail,
            stderr=sensitive_detail,
        )

        response = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 500
        response_body = response.body.decode()
        assert json.loads(response_body) == {
            "status": "error",
            "error": "Failed to request update check",
        }
        assert sensitive_detail not in response_body

    @patch("api.update.os_update_is_active", return_value=False)
    @patch("api.update.subprocess.run")
    def test_os_error_is_sanitized(self, run, _active):
        run.side_effect = FileNotFoundError(2, "systemctl missing", "/private/systemctl")

        response = self.fetch("/api/v1/update/check", method="POST", body="")

        assert response.code == 500
        response_body = response.body.decode()
        assert json.loads(response_body) == {
            "status": "error",
            "error": "Failed to request update check",
        }
        assert "/private/systemctl" not in response_body


def test_route_is_registered_as_local_access_handler():
    handler, kwargs = API._versions[APIVersion.V1][r"/update/check"]

    assert handler is UpdateCheckHandler
    assert issubclass(handler, LocalAccessHandler)
    assert kwargs == {}
