import json
import tempfile
from pathlib import Path

import pytest
import zstandard as zstd
from sqlalchemy import create_engine
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

import api.pour_over_history as pour_over_api
import pour_over_history
from api.pour_over_history import (
    LastPourOverHandler,
    PourOverFileHandler,
    PourOverHistoryHandler,
)
from database_models import metadata
from pour_over_history import (
    MAX_POUR_OVER_RECORD_BYTES,
    PourOverHistoryManager,
    PourOverHistoryRecordTooLargeError,
    PourOverSession,
    read_compressed_record,
)
from shot_database import ShotDataBase


def free_pour_session(
    session_id: str = "11111111-2222-4333-8444-555555555555",
    started_at: str = "2026-08-16T08:00:00.000Z",
):
    return {
        "schemaVersion": 4,
        "id": session_id,
        "brewType": "pour_over",
        "mode": "free_pour",
        "name": "Free Pour",
        "source": "dial",
        "startedAt": started_at,
        "completedAt": "2026-08-16T08:02:15.000Z",
        "recipe": {
            "profileId": None,
            "profileName": "Free Pour",
            "doseG": 15,
            "temperatureC": 92,
            "targetWaterG": None,
            "targetDurationMs": None,
            "pourTargets": [],
        },
        "measurements": {
            "serverBaselineG": -0.4,
            "brewerG": 134.2,
            "setupG": 149.2,
            "doseG": 15.1,
            "waterTemperatureC": 92,
            "waterPouredG": 225.3,
            "beverageG": 196.8,
            "retainedG": 28.5,
            "durationMs": 135_000,
            "status": "measured",
        },
        "pours": [
            {
                "number": 1,
                "startTimeMs": 0,
                "endTimeMs": 10_000,
                "startWeightG": 0,
                "endWeightG": 40.2,
                "waterG": 40.2,
                "averageFlowGps": 4.02,
                "peakFlowGps": 4.8,
            }
        ],
        "samples": [
            {"t": 0, "w": 0, "f": 0.8, "p": 1},
            {"t": 10_000, "w": 40.2, "f": 0.1, "p": 0},
        ],
        "completion": "brewer_removed",
        "sync": {"status": "pending", "attempts": 0},
    }


class TestPourOverHistoryAPI(AsyncHTTPTestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.history_path = root.joinpath("pour-over")
        self.engine = create_engine(f"sqlite:///{root.joinpath('history.sqlite')}")
        metadata.create_all(self.engine)
        self.previous_engine = ShotDataBase.engine
        self.previous_manager_path = pour_over_history.POUR_OVER_PATH
        self.previous_api_path = pour_over_api.POUR_OVER_PATH
        ShotDataBase.engine = self.engine
        pour_over_history.POUR_OVER_PATH = self.history_path
        pour_over_api.POUR_OVER_PATH = self.history_path
        super().setUp()

    def tearDown(self):
        super().tearDown()
        self.engine.dispose()
        ShotDataBase.engine = self.previous_engine
        pour_over_history.POUR_OVER_PATH = self.previous_manager_path
        pour_over_api.POUR_OVER_PATH = self.previous_api_path
        self.temporary.cleanup()

    def get_app(self):
        return Application(
            [
                (r"/api/v1/history/pour-over", PourOverHistoryHandler),
                (r"/api/v1/history/pour-over/last", LastPourOverHandler),
                (
                    r"/api/v1/history/pour-over/files/(.*)",
                    PourOverFileHandler,
                    {"path": self.history_path},
                ),
            ]
        )

    def save(self, payload=None, headers=None):
        return self.fetch(
            "/api/v1/history/pour-over",
            method="POST",
            body=json.dumps(payload or free_pour_session()),
            headers=headers,
        )

    def test_saves_compressed_record_and_returns_it_from_history(self):
        response = self.save()

        assert response.code == 201
        result = json.loads(response.body)
        relative_path = result["history"]["file"]
        stored_path = self.history_path.joinpath(relative_path)
        assert stored_path.is_file()
        saved = json.loads(zstd.ZstdDecompressor().decompress(stored_path.read_bytes()))
        assert saved["brewType"] == "pour_over"
        assert saved["measurements"]["serverBaselineG"] == -0.4

        last = self.fetch("/api/v1/history/pour-over/last")
        assert last.code == 200
        assert json.loads(last.body)["id"] == free_pour_session()["id"]

        raw = self.fetch(f"/api/v1/history/pour-over/files/{relative_path}")
        assert raw.code == 200
        assert json.loads(raw.body)["id"] == free_pour_session()["id"]

    def test_duplicate_session_is_idempotent(self):
        first = self.save()
        second = self.save()

        assert first.code == 201
        assert second.code == 200
        assert json.loads(second.body)["status"] == "existing"
        assert len(PourOverHistoryManager.search()) == 1
        assert len(list(self.history_path.rglob("*.zst"))) == 1

    def test_rejects_reused_session_id_with_different_measurements(self):
        original = free_pour_session()
        changed = free_pour_session()
        changed["measurements"]["waterPouredG"] = 250

        assert self.save(original).code == 201
        response = self.save(changed)

        assert response.code == 409
        assert len(PourOverHistoryManager.search()) == 1

    def test_rejects_invalid_contract_without_writing_history(self):
        invalid = free_pour_session()
        invalid["samples"] = []

        response = self.save(invalid)

        assert response.code == 422
        assert PourOverHistoryManager.search() == []
        assert not list(self.history_path.rglob("*.zst"))

    def test_write_endpoint_is_local_only(self):
        response = self.save(headers={"Host": "machine.local", "X-Real-IP": "192.168.10.20"})

        assert response.code == 403
        assert PourOverHistoryManager.search() == []

    def test_profile_mode_and_legacy_contract_are_accepted(self):
        profile = free_pour_session()
        profile["mode"] = "profile"
        profile["name"] = "Three Pour 1:15"
        profile["recipe"]["profileId"] = "profile-1"
        profile["recipe"]["targetWaterG"] = 225
        profile["recipe"]["targetDurationMs"] = 135_000
        profile["recipe"]["pourTargets"] = [
            {"number": 1, "startTimeMs": 0, "stopWeightG": 40, "flowGps": 4}
        ]
        assert self.save(profile).code == 201

        legacy = free_pour_session(
            session_id="33333333-4444-4555-8666-777777777777",
            started_at="2026-08-16T07:00:00.000Z",
        )
        legacy["schemaVersion"] = 2
        legacy["measurements"]["emptyServerG"] = 182.4
        legacy["measurements"].pop("serverBaselineG")
        legacy["recipe"].pop("temperatureC")
        legacy["recipe"].pop("targetDurationMs")
        legacy["measurements"].pop("doseG")
        legacy["measurements"].pop("waterTemperatureC")

        assert self.save(legacy).code == 201
        records = PourOverHistoryManager.search(descending=False)
        assert [record["schemaVersion"] for record in records] == [2, 4]
        response = self.fetch("/api/v1/history/pour-over?mode=free_pour&max_results=1")
        assert response.code == 200
        assert json.loads(response.body)["history"][0]["id"] == legacy["id"]

    def test_prunes_oldest_records_at_the_history_limit(self):
        previous_max_records = pour_over_history.MAX_POUR_OVER_HISTORY_RECORDS
        previous_min_free = pour_over_history.MIN_POUR_OVER_FREE_BYTES
        try:
            pour_over_history.MAX_POUR_OVER_HISTORY_RECORDS = 2
            pour_over_history.MIN_POUR_OVER_FREE_BYTES = 0
            sessions = [
                free_pour_session(
                    session_id=f"00000000-0000-4000-8000-{index:012d}",
                    started_at=f"2026-08-16T0{index}:00:00.000Z",
                )
                for index in (1, 2, 3)
            ]
            for session in sessions:
                session["completedAt"] = session["startedAt"]
                assert self.save(session).code == 201

            records = PourOverHistoryManager.search(descending=False)
            assert [record["id"] for record in records] == [
                sessions[1]["id"],
                sessions[2]["id"],
            ]
            assert len(list(self.history_path.rglob("*.zst"))) == 2
        finally:
            pour_over_history.MAX_POUR_OVER_HISTORY_RECORDS = previous_max_records
            pour_over_history.MIN_POUR_OVER_FREE_BYTES = previous_min_free


def test_session_validation_rejects_non_finite_and_reversed_ranges():
    payload = free_pour_session()
    payload["samples"][0]["f"] = float("inf")
    try:
        PourOverSession.model_validate(payload)
        raise AssertionError("Expected non-finite flow to be rejected")
    except ValueError:
        pass

    payload = free_pour_session()
    payload["measurements"]["durationMs"] = 600_001
    with pytest.raises(ValueError):
        PourOverSession.model_validate(payload)

    payload = free_pour_session()
    payload["samples"][-1]["t"] = 600_001
    with pytest.raises(ValueError):
        PourOverSession.model_validate(payload)

    payload = free_pour_session()
    payload["samples"] = payload["samples"] * 1_501
    with pytest.raises(ValueError):
        PourOverSession.model_validate(payload)


def test_compressed_record_reader_bounds_expansion(tmp_path):
    compressed_path = tmp_path.joinpath("oversized.pour-over.json.zst")
    compressed_path.write_bytes(
        zstd.ZstdCompressor(level=10).compress(b"0" * (MAX_POUR_OVER_RECORD_BYTES + 1))
    )
    with pytest.raises(PourOverHistoryRecordTooLargeError):
        read_compressed_record(compressed_path)

    payload = free_pour_session()
    payload["recipe"]["pourTargets"] = [
        {"number": 1, "startTimeMs": 0, "stopWeightG": 40, "flowRangeGps": [5, 4]}
    ]
    try:
        PourOverSession.model_validate(payload)
        raise AssertionError("Expected reversed flow range to be rejected")
    except ValueError:
        pass
