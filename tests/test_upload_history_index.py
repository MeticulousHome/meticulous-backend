import json
import sys
import tempfile
import types
from datetime import datetime
from pathlib import Path
from unittest.mock import patch

import zstandard as zstd
from sqlalchemy import create_engine, insert, text
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

try:
    import pyprctl  # noqa: F401
except Exception:
    # Importing the history handlers also imports the machine's thread helper.
    # Thread naming is Linux-only and unrelated to these HTTP/database tests.
    sys.modules["pyprctl"] = types.SimpleNamespace(set_name=lambda _name: None)

import api.history  # noqa: F401 - registers the production routes
from api.api import API, APIVersion
from database_models import history, metadata, profile
from shot_database import ShotDataBase


class TestUploadHistoryIndexAPI(AsyncHTTPTestCase):
    endpoint = "/api/v1/history/upload-index/last"

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.engine = create_engine(f"sqlite:///{self.root.joinpath('history.sqlite')}")
        metadata.create_all(self.engine, tables=[profile, history])
        self.previous_engine = ShotDataBase.engine
        ShotDataBase.engine = self.engine
        super().setUp()

    def tearDown(self):
        super().tearDown()
        ShotDataBase.engine = self.previous_engine
        self.engine.dispose()
        self.temporary.cleanup()

    def get_app(self):
        # Use the registered production paths so a missing route is a real 404.
        return Application(
            [
                (f"/api/v1{path}", handler, kwargs)
                for path, (handler, kwargs) in API._versions[APIVersion.V1].items()
                if path.startswith("/history/upload-index")
            ]
        )

    def save_history(self, file, timestamp):
        with self.engine.begin() as connection:
            profile_key = connection.execute(
                insert(profile).values(id=file)
            ).inserted_primary_key[0]
            connection.execute(
                insert(history).values(
                    uuid=file,
                    file=file,
                    time=datetime.fromtimestamp(timestamp),
                    profile_name="Espresso",
                    profile_id=file,
                    profile_key=profile_key,
                )
            )

    def test_empty_history_returns_null_file(self):
        response = self.fetch(self.endpoint)

        assert response.code == 200
        assert json.loads(response.body) == {"file": None}

    def assert_saved_payload_is_not_read(self, payload):
        expected = "2026-09-24/10:00:00.shot.json.zst"
        self.save_history(expected, 1000)
        # The latest file cursor can differ from the latest brew timestamp.
        self.save_history("2026-09-24/09:00:00.shot.json.zst", 2000)
        stored = self.root.joinpath(expected)
        stored.parent.mkdir(parents=True)
        stored.write_bytes(zstd.ZstdCompressor().compress(payload))
        with self.engine.begin() as connection:
            connection.execute(text("UPDATE profile SET display = 'invalid JSON'"))

        with (
            patch("shot_database.SHOT_PATH", self.root),
            patch("shot_database.open", side_effect=AssertionError("must not open brew files")),
            patch(
                "api.history.ShotManager.getLastShot",
                side_effect=AssertionError("must not load the complete brew"),
            ),
            patch(
                "shot_database.zstd.ZstdDecompressor",
                side_effect=AssertionError("must not decompress brew files"),
            ),
        ):
            response = self.fetch(self.endpoint)

        assert response.code == 200
        assert json.loads(response.body) == {"file": expected}
        assert len(response.body) < 256

        # The existing ascending index retains its shape and cursor behavior.
        page = self.fetch("/api/v1/history/upload-index?max_results=1")
        assert json.loads(page.body) == {
            "history": [{"file": "2026-09-24/09:00:00.shot.json.zst"}]
        }
        next_page = self.fetch(
            "/api/v1/history/upload-index?after=2026-09-24/09:00:00.shot.json.zst"
        )
        assert json.loads(next_page.body) == {"history": [{"file": expected}]}

    def test_latest_cursor_does_not_load_large_record(self):
        payload = json.dumps({"data": [{"weight": index} for index in range(100_000)]}).encode()
        assert len(payload) > 256 * 1024
        self.assert_saved_payload_is_not_read(payload)

    def test_latest_cursor_does_not_load_malformed_record(self):
        self.assert_saved_payload_is_not_read(b"this saved record is not valid JSON")

    def test_latest_cursor_retains_local_access_restriction(self):
        response = self.fetch(
            self.endpoint,
            headers={"Host": "machine.local", "X-Real-IP": "192.168.10.20"},
        )

        assert response.code == 403
