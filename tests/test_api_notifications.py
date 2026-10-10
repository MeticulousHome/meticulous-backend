import copy
import json
import tempfile
import time
from datetime import datetime
from pathlib import Path

import pytest
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

import smoke_validation
from api.api import API, APIVersion
from api.notifications import GetNotificationsHandler
from api.smoke_validation import SmokeValidationHandler
from config import CONFIG_SYSTEM, NOTIFICATION_KEEPALIVE, MeticulousConfig
from esp_serial.data import ESPInfo, ShotData
from machine import Machine
from notifications import Notification, NotificationManager, NotificationResponse

TTL_SECONDS = 3600


def make_notification(message, responses, image=None, callback=None):
    notification = Notification(message, responses=responses, image=image, callback=callback)
    # A fixed timestamp makes the serialized isoformat exact.
    notification.timestamp = datetime(2026, 10, 9, 8, 30, 15, 123456)
    return notification


def serialized(notification):
    return {
        "id": notification.id,
        "message": notification.message,
        "image": notification.image,
        "responses": notification.respone_options,
        "timestamp": "2026-10-09T08:30:15.123456",
    }


class TestNotificationsAPI(AsyncHTTPTestCase):
    def setUp(self):
        self.mp = pytest.MonkeyPatch()
        system = copy.deepcopy(MeticulousConfig[CONFIG_SYSTEM])
        system[NOTIFICATION_KEEPALIVE] = TTL_SECONDS
        self.mp.setitem(MeticulousConfig, CONFIG_SYSTEM, system)

        self.update = make_notification(
            "Update available",
            [NotificationResponse.UPDATE, NotificationResponse.SKIP],
            image="data:image/png;base64,AAAA",
        )
        self.descale = make_notification("Descale soon", [NotificationResponse.OK])
        self.recently_acknowledged = make_notification("Old news", [NotificationResponse.OK])
        self.recently_acknowledged.acknowledge(NotificationResponse.OK)
        self.mp.setattr(
            NotificationManager,
            "_notifications",
            [self.update, self.descale, self.recently_acknowledged],
        )
        super().setUp()

    def tearDown(self):
        super().tearDown()
        self.mp.undo()

    def get_app(self):
        return Application(
            [
                (r"/api/v1/notifications", GetNotificationsHandler),
                (r"/api/v1/notifications/acknowledge", GetNotificationsHandler),
            ]
        )

    def acknowledge(self, payload, path="/api/v1/notifications/acknowledge"):
        body = payload if isinstance(payload, str) else json.dumps(payload)
        return self.fetch(path, method="POST", body=body)

    def test_both_routes_are_registered_to_the_same_handler(self):
        routes = API._versions[APIVersion.V1]
        assert routes["/notifications"][0] is GetNotificationsHandler
        assert routes["/notifications/acknowledge"][0] is GetNotificationsHandler

    def test_get_lists_only_unacknowledged_notifications(self):
        response = self.fetch("/api/v1/notifications")

        assert response.code == 200
        assert response.headers["Content-Type"] == "application/json"
        assert json.loads(response.body) == [serialized(self.update), serialized(self.descale)]

    def test_get_with_acknowledged_true_includes_acknowledged_ones(self):
        for flag in ("true", "TRUE"):
            response = self.fetch(f"/api/v1/notifications?acknowledged={flag}")

            assert response.code == 200
            assert json.loads(response.body) == [
                serialized(self.update),
                serialized(self.descale),
                serialized(self.recently_acknowledged),
            ]

    def test_get_with_acknowledged_other_than_true_hides_acknowledged_ones(self):
        response = self.fetch("/api/v1/notifications?acknowledged=1")

        assert json.loads(response.body) == [serialized(self.update), serialized(self.descale)]

    def test_get_drops_notifications_acknowledged_longer_than_the_ttl_ago(self):
        self.recently_acknowledged.acknowledged_timestamp = time.time() - TTL_SECONDS - 1

        response = self.fetch("/api/v1/notifications?acknowledged=true")

        assert json.loads(response.body) == [serialized(self.update), serialized(self.descale)]
        assert NotificationManager._notifications == [self.update, self.descale]

    def test_get_with_no_notifications_returns_an_empty_list(self):
        NotificationManager._notifications = []

        response = self.fetch("/api/v1/notifications")

        assert response.code == 200
        assert json.loads(response.body) == []

    def test_get_on_the_acknowledge_route_also_lists_notifications(self):
        # Both patterns map to GetNotificationsHandler, so GET works on either.
        response = self.fetch("/api/v1/notifications/acknowledge")

        assert response.code == 200
        assert json.loads(response.body) == [serialized(self.update), serialized(self.descale)]

    def test_post_acknowledges_with_the_chosen_response(self):
        response = self.acknowledge({"id": self.update.id, "response": "Skip"})

        assert response.code == 200
        assert json.loads(response.body) == {"status": "success"}
        assert self.update.acknowledged is True
        assert self.update.response == "Skip"
        assert self.descale.acknowledged is False
        listed = self.fetch("/api/v1/notifications")
        assert json.loads(listed.body) == [serialized(self.descale)]

    def test_post_on_the_list_route_also_acknowledges(self):
        response = self.acknowledge(
            {"id": self.descale.id, "response": "Ok"}, path="/api/v1/notifications"
        )

        assert response.code == 200
        assert json.loads(response.body) == {"status": "success"}
        assert self.descale.acknowledged is True

    def test_post_unknown_id_returns_404(self):
        response = self.acknowledge({"id": "not-a-notification", "response": "Ok"})

        assert response.code == 404
        assert json.loads(response.body) == {
            "status": "failure",
            "message": "Notification not found",
        }
        assert not self.update.acknowledged and not self.descale.acknowledged

    def test_post_for_an_already_acknowledged_notification_returns_404(self):
        response = self.acknowledge({"id": self.recently_acknowledged.id, "response": "Ok"})

        assert response.code == 404
        assert json.loads(response.body) == {
            "status": "failure",
            "message": "Notification not found",
        }

    def test_post_without_id_returns_404(self):
        response = self.acknowledge({"response": "Ok"})

        assert response.code == 404

    def test_post_invalid_json_returns_500(self):
        # json.loads is not guarded in the handler, so tornado answers 500.
        response = self.acknowledge("{not json")

        assert response.code == 500

    @pytest.mark.xfail(
        strict=True,
        reason="backend bug: NotificationManager.acknowledge_notification "
        "(notifications.py:126-128) calls notification.callback() after "
        "Notification.acknowledge already called it (notifications.py:71-72), "
        "so every callback runs twice per acknowledgement",
    )
    def test_post_runs_the_notification_callback_once(self):
        calls = []
        self.update.callback = lambda: calls.append(self.update.response)

        response = self.acknowledge({"id": self.update.id, "response": "Update"})

        assert response.code == 200
        assert calls == ["Update"]


class TestSmokeValidationAPI(AsyncHTTPTestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.state_file = root.joinpath("smoke-validation.json")
        self.version_file = root.joinpath("image-build-version")
        self.version_file.write_text("2026M1446-beta\n")

        self.mp = pytest.MonkeyPatch()
        self.mp.setattr(smoke_validation, "STATE_FILE", self.state_file)
        self.mp.setattr(smoke_validation, "VERSION_FILE", self.version_file)
        self.mp.setattr(Machine, "infoReady", True)
        self.mp.setattr(Machine, "esp_info", ESPInfo(firmwareV="1.4.2"))
        self.mp.setattr(Machine, "data_sensors", ShotData(state="idle", status="idle"))
        super().setUp()

    def tearDown(self):
        super().tearDown()
        self.mp.undo()
        self.temporary.cleanup()

    def get_app(self):
        return Application([(r"/api/v1/smoke-validation", SmokeValidationHandler)])

    def write_state(self, **overrides):
        state = {
            "image_version": "2026M1446-beta",
            "post_update_shot_completed": True,
            "post_update_shot_completed_at": "2026-10-09T07:00:00+00:00",
            "post_update_shot_completed_version": "2026M1446-beta",
            **overrides,
        }
        self.state_file.write_text(json.dumps(state))
        return state

    def test_route_is_registered_with_the_pattern_under_test(self):
        assert API._versions[APIVersion.V1]["/smoke-validation"][0] is SmokeValidationHandler

    def test_fresh_image_reports_no_post_update_shot_yet(self):
        response = self.fetch("/api/v1/smoke-validation")

        assert response.code == 200
        assert json.loads(response.body) == {
            "backend_initialized": True,
            "esp32_responding": True,
            "machine_state": "idle",
            "machine_status": "idle",
            "firmware_version": "1.4.2",
            "image_version": "2026M1446-beta",
            "post_update_shot_completed": False,
            "post_update_shot_completed_at": None,
            "post_update_shot_completed_version": None,
        }
        # A missing state file is answered from defaults without being created.
        assert not self.state_file.exists()

    def test_completed_shot_on_the_current_image_is_reported(self):
        self.write_state()
        Machine.data_sensors = ShotData(state="brewing", status="heating")

        response = self.fetch("/api/v1/smoke-validation")

        body = json.loads(response.body)
        assert response.code == 200
        assert body["machine_state"] == "brewing"
        assert body["machine_status"] == "heating"
        assert body["post_update_shot_completed"] is True
        assert body["post_update_shot_completed_at"] == "2026-10-09T07:00:00+00:00"
        assert body["post_update_shot_completed_version"] == "2026M1446-beta"

    def test_state_from_a_previous_image_is_reset_and_rewritten(self):
        self.write_state(
            image_version="2026M1400-beta", post_update_shot_completed_version="2026M1400-beta"
        )

        response = self.fetch("/api/v1/smoke-validation")

        reset = {
            "image_version": "2026M1446-beta",
            "post_update_shot_completed": False,
            "post_update_shot_completed_at": None,
            "post_update_shot_completed_version": None,
        }
        body = json.loads(response.body)
        assert {key: body[key] for key in reset} == reset
        assert json.loads(self.state_file.read_text()) == reset

    def test_esp32_without_info_is_reported_as_not_responding(self):
        Machine.infoReady = False

        response = self.fetch("/api/v1/smoke-validation")

        body = json.loads(response.body)
        assert body["esp32_responding"] is False
        assert body["firmware_version"] == "1.4.2"

    def test_missing_esp_info_reports_no_firmware(self):
        Machine.esp_info = None

        response = self.fetch("/api/v1/smoke-validation")

        body = json.loads(response.body)
        assert body["esp32_responding"] is False
        assert body["firmware_version"] is None

    def test_missing_version_file_reports_unknown_image(self):
        self.version_file.unlink()

        response = self.fetch("/api/v1/smoke-validation")

        assert json.loads(response.body)["image_version"] == "unknown"

    def test_unparseable_state_file_falls_back_to_defaults(self):
        self.state_file.write_text("{truncated")

        response = self.fetch("/api/v1/smoke-validation")

        body = json.loads(response.body)
        assert response.code == 200
        assert body["post_update_shot_completed"] is False
        assert body["image_version"] == "2026M1446-beta"

    def test_state_file_that_is_not_an_object_returns_500(self):
        self.state_file.write_text("[]")

        response = self.fetch("/api/v1/smoke-validation")

        assert response.code == 500
        assert json.loads(response.body) == {
            "status": "error",
            "error": "'list' object has no attribute 'get'",
        }

    def test_remote_request_through_the_proxy_returns_403(self):
        response = self.fetch(
            "/api/v1/smoke-validation",
            headers={"X-Real-IP": "192.168.1.42", "Host": "meticulous.local"},
        )

        assert response.code == 403
        assert json.loads(response.body) == {
            "status": "error",
            "error": "This endpoint can only be accessed locally",
        }

    def test_loopback_forwarded_ip_is_allowed(self):
        response = self.fetch(
            "/api/v1/smoke-validation",
            headers={"X-Real-IP": "127.0.0.1", "Host": "meticulous.local"},
        )

        assert response.code == 200
