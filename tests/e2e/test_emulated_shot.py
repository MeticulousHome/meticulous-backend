import json
import os
import time
import unittest
from urllib import error, request

BACKEND_URL = os.getenv("E2E_BACKEND_URL")


def api_request(path, method="GET", body=None):
    payload = None
    headers = {}
    if body is not None:
        payload = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"

    req = request.Request(
        f"{BACKEND_URL}{path}",
        data=payload,
        headers=headers,
        method=method,
    )
    try:
        with request.urlopen(req, timeout=10) as response:
            content = response.read()
    except error.HTTPError as exc:
        details = exc.read().decode("utf-8", errors="replace")
        raise AssertionError(f"{method} {path} failed with HTTP {exc.code}: {details}") from exc

    if not content:
        return None
    return json.loads(content)


def wait_until(description, predicate, timeout=60):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.25)
    raise AssertionError(f"Timed out waiting for {description}")


def backend_is_ready():
    try:
        return api_request("/api/v1/machine")
    except (error.URLError, OSError):
        return None


@unittest.skipUnless(BACKEND_URL, "E2E_BACKEND_URL is required")
class EmulatedShotE2ETest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        wait_until("the emulated backend API", backend_is_ready)

    def test_profile_runs_against_recorded_esp_telemetry_and_is_saved(self):
        initial_history = api_request("/api/v1/history?dump_data=false")["history"]
        initial_ids = {entry["id"] for entry in initial_history}

        defaults = api_request("/api/v1/profile/defaults")
        profile_groups = [
            group for group in defaults.values() if isinstance(group, list) and group
        ]
        self.assertTrue(profile_groups, "Backend returned no default profile to execute")
        profile = profile_groups[0][0]

        profiles = api_request("/api/v1/profile/list?full=true")
        if not any(entry["id"] == profile["id"] for entry in profiles):
            saved = api_request("/api/v1/profile/save", method="POST", body=profile)
            self.assertEqual(saved["profile"]["id"], profile["id"])

        loaded = api_request("/api/v1/profile/load", method="POST", body=profile)
        self.assertEqual(loaded["id"], profile["id"])

        started = api_request("/api/v1/action/start", method="POST")
        self.assertEqual(started, {"action": "start", "status": "ok"})

        current_shot = wait_until(
            "recorded ESP telemetry to start a shot",
            lambda: api_request("/api/v1/history/current"),
        )
        self.assertGreater(len(current_shot["data"]), 0)

        def completed_history_entry():
            history = api_request("/api/v1/history?dump_data=false")["history"]
            return next((entry for entry in history if entry["id"] not in initial_ids), None)

        completed = wait_until(
            "the emulated shot to be persisted in history",
            completed_history_entry,
            timeout=90,
        )
        self.assertEqual(completed["profile"]["id"], profile["id"])

        last_shot = api_request("/api/v1/history/last")
        self.assertEqual(last_shot["id"], completed["id"])
        self.assertEqual(last_shot["profile"]["id"], profile["id"])
        self.assertGreater(len(last_shot["data"]), 0)
        statuses = {sample["status"] for sample in last_shot["data"]}
        self.assertIn("Preinfusion", statuses)
        self.assertIn("Infusion", statuses)


if __name__ == "__main__":
    unittest.main()
