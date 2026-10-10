"""The shot history routes (api/history.py) through the real handlers.

Protects what the Dial and the mobile app show and upload as history: the shot
list, search, current and last shot, statistics, ratings, the upload index, and
the shot and debug files served. Shots are recorded through ShotManager, the
production write path, so a change to the stored format fails here. Limits come
from the handlers' own responses and ShotDataBase; backend bugs found are strict
xfails naming the file and line.
"""

import json
import os
import threading
import zipfile
from datetime import datetime
from pathlib import Path

import pytest
import tornado.web
import zstandard as zstd
from sqlalchemy import select
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

import api.history as history_api
import config as cfg
import shot_database as sdb_module
import shot_debug_manager as sdm_module
import shot_manager as sm_module
from api.api import API, APIVersion
from api.history import (
    CompressedDebugHistoryHandler,
    CurrentShotHandler,
    HistoryHandler,
    LastDebugFileHandler,
    LastShotHandler,
    ProfileSearchHandler,
    ShotRatingHandler,
    StatisticsHandler,
    UploadHistoryIndexHandler,
    ZstdHistoryHandler,
)
from config import (
    CONFIG_PROFILES,
    CONFIG_SYSTEM,
    DEVICE_IDENTIFIER,
    PROFILE_LAST,
    MeticulousConfig,
)
from database_models import metadata, shot_annotation, shot_rating
from esp_serial.data import ShotData
from machine import Machine
from shot_database import ShotDataBase
from shot_debug_manager import ShotDebugManager
from shot_manager import ShotManager

API_PREFIX = "/api/v1"
# Captured before any test patches the module, these are the machine paths.
IMPORTED_SHOT_PATH = cfg.SHOT_PATH
IMPORTED_DEBUG_HISTORY_PATH = cfg.DEBUG_HISTORY_PATH

# Every route api/history.py registers, with the handler it must be served by.
HISTORY_ROUTES = {
    "/history/search": ProfileSearchHandler,
    "/history/current": CurrentShotHandler,
    "/history/last": LastShotHandler,
    "/history/upload-index": UploadHistoryIndexHandler,
    "/history/stats": StatisticsHandler,
    "/history": HistoryHandler,
    "/history/last-debug-file": LastDebugFileHandler,
    "/history/rating/(.*)": ShotRatingHandler,
    "/history/debug": tornado.web.RedirectHandler,
    "/history/debug.zip": CompressedDebugHistoryHandler,
    "/history/debug/(.*)": ZstdHistoryHandler,
    "/history/files": tornado.web.RedirectHandler,
    "/history/files/(.*)": ZstdHistoryHandler,
}

DAY = "2026-10-09"
FIRST_SHOT_TIME = datetime(2026, 10, 9, 8, 30, 0).timestamp()
SECOND_SHOT_TIME = datetime(2026, 10, 9, 8, 31, 0).timestamp()
THIRD_SHOT_TIME = datetime(2026, 10, 9, 8, 32, 0).timestamp()
FIRST_SHOT_FILE = f"{DAY}/08:30:00.shot.json.zst"
SECOND_SHOT_FILE = f"{DAY}/08:31:00.shot.json.zst"
THIRD_SHOT_FILE = f"{DAY}/08:32:00.shot.json.zst"


def make_profile(profile_id, name, stage_names, temperature=93.0):
    # Exactly the profile columns the history database stores and returns.
    return {
        "id": profile_id,
        "author": "Meticulous",
        "author_id": "author-0001",
        "display": {"accentColor": "#FF7A00"},
        "final_weight": 36.0,
        "last_changed": 1760000000.0,
        "name": name,
        "temperature": temperature,
        "stages": [
            {"key": f"{profile_id}-stage-{index}", "name": stage, "type": "pressure"}
            for index, stage in enumerate(stage_names)
        ],
        "variables": [],
        "previous_authors": [],
    }


CLASSIC = make_profile("p-classic", "Classic", ["Preinfusion", "Extraction"])
CLASSIC_V2 = make_profile("p-classic-v2", "Classic", ["Preinfusion", "Extraction"], 94.0)
BLOOM = make_profile("p-bloom", "Bloom", ["Bloom", "Pour"])


def brewing_point(profile, time_ms, pressure, weight, gravimetric_flow=1.5):
    return ShotData(
        pressure=pressure,
        flow=2.0,
        weight=weight,
        temperature=92.5,
        status="brewing",
        profile=profile["name"],
        time=time_ms,
        profile_time=time_ms - 200,
        state="brewing",
        is_extracting=True,
        gravimetric_flow=gravimetric_flow,
        main_controller_kind="Pressure",
        main_setpoint=9.0,
    )


def stored_point(time_ms, pressure, weight, gravimetric_flow=1.5):
    # The data point layout Shot.addShotData writes and the Dial graphs.
    return {
        "shot": {
            "pressure": pressure,
            "flow": 2.0,
            "weight": weight,
            "gravimetric_flow": gravimetric_flow,
            "setpoints": {"active": "pressure", "pressure": 9.0},
        },
        "time": time_ms,
        "profile_time": time_ms - 200,
        "status": "brewing",
    }


def join_compression_threads():
    for thread in threading.enumerate():
        if thread.name in ("ShotCompr", "DebugShotCompr"):
            thread.join(timeout=30)


def remove_reflected_fts_tables():
    for table_name in ("profile_fts", "stage_fts"):
        if table_name in metadata.tables:
            metadata.remove(metadata.tables[table_name])


class HistoryAPITestCase(AsyncHTTPTestCase):
    @pytest.fixture(autouse=True)
    def fresh_machine(self, tmp_path, monkeypatch):
        self.monkeypatch = monkeypatch
        history_root = tmp_path / "history"
        self.shot_path = history_root / "shots"
        self.debug_path = history_root / "debug"
        db_file = history_root / "history.sqlite"
        db_url = f"sqlite:///{db_file}"

        for module in (cfg, sdb_module):
            monkeypatch.setattr(module, "HISTORY_PATH", str(history_root))
            monkeypatch.setattr(module, "ABSOLUTE_DATABASE_FILE", db_file)
            monkeypatch.setattr(module, "DATABASE_URL", db_url)
        for module in (cfg, sdb_module, sm_module, history_api):
            monkeypatch.setattr(module, "SHOT_PATH", self.shot_path)
        for module in (cfg, sdm_module, history_api):
            monkeypatch.setattr(module, "DEBUG_HISTORY_PATH", str(self.debug_path))

        for attribute in ("engine", "session", "stage_fts_table", "profile_fts_table"):
            monkeypatch.setattr(ShotDataBase, attribute, None)
        for attribute in ("_last_shot", "_current_shot", "db_history_id"):
            monkeypatch.setattr(ShotManager, attribute, None)
        monkeypatch.setattr(Machine, "mileage", None)
        monkeypatch.setitem(MeticulousConfig[CONFIG_PROFILES], PROFILE_LAST, None)
        monkeypatch.setitem(
            MeticulousConfig[CONFIG_SYSTEM], DEVICE_IDENTIFIER, ["Meticulous42"]
        )

        remove_reflected_fts_tables()
        ShotDataBase.init()
        metadata.create_all(ShotDataBase.engine)
        yield
        join_compression_threads()
        ShotDataBase.engine.dispose()
        # init() reflects the FTS tables into the shared metadata; leaving them
        # bound to this temporary database breaks later tests.
        remove_reflected_fts_tables()

    def get_app(self):
        registered = API._versions[APIVersion.V1]
        routes = []
        for path in sorted(HISTORY_ROUTES):
            handler, kwargs = registered[path]
            kwargs = dict(kwargs)
            if path == "/history/files/(.*)":
                kwargs["path"] = self.shot_path
            elif path == "/history/debug/(.*)":
                kwargs["path"] = str(self.debug_path)
            routes.append((f"{API_PREFIX}{path}", handler, kwargs))
        return Application(routes)

    # -- production write paths -------------------------------------------------

    def record_shot(self, profile, start_time, points, push_to_brew_time=0):
        """Brew a shot through ShotManager exactly as the machine does and return its id."""
        self.monkeypatch.setitem(
            MeticulousConfig[CONFIG_PROFILES],
            PROFILE_LAST,
            {"load_time": start_time, "profile": profile},
        )
        ShotManager.start(push_to_brew_time)
        # The shot file name is derived from the start time, pin it.
        ShotManager._current_shot.startTime = start_time
        shot_id = ShotManager._current_shot.id
        for point in points:
            ShotManager.handleShotData(point)
        ShotManager.stop()
        join_compression_threads()
        return shot_id

    def record_three_shots(self):
        first = self.record_shot(
            CLASSIC,
            FIRST_SHOT_TIME,
            [brewing_point(CLASSIC, 1000, 9.0, 0.0), brewing_point(CLASSIC, 1100, 8.5, 0.4)],
            push_to_brew_time=1500,
        )
        second = self.record_shot(
            BLOOM, SECOND_SHOT_TIME, [brewing_point(BLOOM, 1000, 2.0, 1.0)]
        )
        third = self.record_shot(
            CLASSIC_V2, THIRD_SHOT_TIME, [brewing_point(CLASSIC_V2, 1000, 9.1, 0.0)]
        )
        return first, second, third

    def write_debug_shot(self, relative_path, payload, mtime):
        file_path = Path(self.debug_path, relative_path)
        ShotDebugManager._compress_debug_json_to_path(json.dumps(payload), file_path)
        os.utime(file_path, (mtime, mtime))
        return file_path

    # -- request helpers ----------------------------------------------------------

    def get_json(self, url, **kwargs):
        response = self.fetch(f"{API_PREFIX}{url}", **kwargs)
        return response, json.loads(response.body)

    def post_json(self, url, body):
        if not isinstance(body, (str, bytes)):
            body = json.dumps(body)
        response = self.fetch(f"{API_PREFIX}{url}", method="POST", body=body)
        return response, json.loads(response.body)

    def expected_entry(
        self, shot_id, db_key, profile, profile_key, start_time, file, data, ptb
    ):
        return {
            "id": shot_id,
            "db_key": db_key,
            "time": start_time,
            "file": file,
            "debug_file": None,
            "name": profile["name"],
            "data": data,
            "push_to_brew_time": ptb,
            "profile": {**profile, "db_key": profile_key},
        }


class TestRouteRegistration(HistoryAPITestCase):
    def test_every_history_route_is_registered_with_its_handler(self):
        registered = API._versions[APIVersion.V1]
        for path, handler in HISTORY_ROUTES.items():
            assert registered[path][0] is handler, path

    def test_static_routes_serve_the_shot_and_debug_directories(self):
        registered = API._versions[APIVersion.V1]
        assert registered["/history/files/(.*)"][1] == {"path": IMPORTED_SHOT_PATH}
        assert registered["/history/debug/(.*)"][1] == {"path": IMPORTED_DEBUG_HISTORY_PATH}
        assert registered["/history/files"][1] == {"url": "/api/v1/history/files/"}
        assert registered["/history/debug"][1] == {"url": "/api/v1/history/debug/"}


class TestHistoryList(HistoryAPITestCase):
    def test_fresh_machine_lists_no_shots(self):
        response, body = self.get_json("/history")

        assert response.code == 200
        assert body == {"history": []}

    def test_lists_recorded_shots_newest_first_with_shot_data(self):
        first, second, third = self.record_three_shots()

        response, body = self.get_json("/history")

        assert response.code == 200
        assert response.headers["Content-Type"] == "application/json; charset=UTF-8"
        assert body["history"] == [
            self.expected_entry(
                third,
                3,
                CLASSIC_V2,
                3,
                THIRD_SHOT_TIME,
                THIRD_SHOT_FILE,
                [stored_point(1000, 9.1, 0.0)],
                0,
            ),
            self.expected_entry(
                second,
                2,
                BLOOM,
                2,
                SECOND_SHOT_TIME,
                SECOND_SHOT_FILE,
                [stored_point(1000, 2.0, 1.0)],
                0,
            ),
            self.expected_entry(
                first,
                1,
                CLASSIC,
                1,
                FIRST_SHOT_TIME,
                FIRST_SHOT_FILE,
                [stored_point(1000, 9.0, 0.0), stored_point(1100, 8.5, 0.4)],
                1500,
            ),
        ]

    def test_dump_data_false_omits_shot_file_contents(self):
        first, _, _ = self.record_three_shots()

        _, body = self.get_json("/history?dump_data=false&sort=asc&max_results=1")

        assert body["history"] == [
            self.expected_entry(
                first, 1, CLASSIC, 1, FIRST_SHOT_TIME, FIRST_SHOT_FILE, None, None
            )
        ]

    def test_query_parameters_filter_sort_and_limit(self):
        first, second, third = self.record_three_shots()

        def ids(url):
            response, body = self.get_json(url)
            assert response.code == 200
            return [entry["id"] for entry in body["history"]]

        assert ids("/history?sort=asc") == [first, second, third]
        assert ids("/history?max_results=2") == [third, second]
        # max_results <= 0 disables the limit.
        assert ids("/history?max_results=0") == [third, second, first]
        assert ids("/history?query=bloom") == [second]
        assert ids("/history?query=Preinfusion") == [third, first]
        assert ids(f"/history?ids={first}&ids={third}") == [third, first]
        assert ids("/history?ids=2") == [second]
        assert ids("/history?ids=p-classic-v2") == [third]
        assert ids("/history?order_by=profile&sort=asc") == [second, first, third]
        assert ids(f"/history?start_date={SECOND_SHOT_TIME}") == [third, second]
        assert ids(f"/history?end_date={SECOND_SHOT_TIME}") == [second, first]
        assert ids("/history?ids=no-such-shot") == []

    def test_shot_with_unreadable_file_is_left_out_of_the_listing(self):
        first, second, _ = self.record_three_shots()
        Path(self.shot_path, THIRD_SHOT_FILE).unlink()

        _, body = self.get_json("/history")

        assert [entry["id"] for entry in body["history"]] == [second, first]

    def test_non_finite_values_in_shot_file_are_served_as_zero(self):
        shot_id = self.record_shot(
            CLASSIC,
            FIRST_SHOT_TIME,
            [brewing_point(CLASSIC, 1000, 9.0, 0.0, gravimetric_flow=float("inf"))],
        )

        response = self.fetch(f"{API_PREFIX}/history")

        body = json.loads(response.body, parse_constant=pytest.fail)
        assert body["history"][0]["id"] == shot_id
        assert body["history"][0]["data"] == [stored_point(1000, 9.0, 0.0, 0.0)]

    def test_linked_debug_file_is_reported(self):
        self.record_shot(CLASSIC, FIRST_SHOT_TIME, [brewing_point(CLASSIC, 1000, 9.0, 0.0)])
        ShotDataBase.link_debug_file(ShotManager.db_history_id, FIRST_SHOT_FILE)

        _, body = self.get_json("/history")

        assert body["history"][0]["debug_file"] == FIRST_SHOT_FILE

    @pytest.mark.xfail(
        strict=True,
        reason="backend bug: HistoryHandler.get (api/history.py:258) does not catch "
        "pydantic.ValidationError, so a non-integer max_results answers 500 while "
        "the POST search answers 422 for the same input",
    )
    def test_non_integer_max_results_is_rejected_with_422(self):
        response = self.fetch(f"{API_PREFIX}/history?max_results=many")

        assert response.code == 422

    @pytest.mark.xfail(
        strict=True,
        reason="backend bug: HistoryHandler.get (api/history.py:258) does not catch "
        "pydantic.ValidationError, so an unknown sort value answers 500 instead of 422",
    )
    def test_unknown_sort_value_is_rejected_with_422(self):
        response = self.fetch(f"{API_PREFIX}/history?sort=sideways")

        assert response.code == 422


class TestHistorySearchPost(HistoryAPITestCase):
    def test_empty_search_on_fresh_machine_returns_no_shots(self):
        response, body = self.post_json("/history", {})

        assert response.code == 200
        assert body == {"history": []}

    def test_search_body_filters_and_limits(self):
        first, second, third = self.record_three_shots()

        response, body = self.post_json(
            "/history", {"ids": [first, 2], "sort": "asc", "dump_data": False}
        )

        assert response.code == 200
        assert [entry["id"] for entry in body["history"]] == [first, second]
        assert [entry["data"] for entry in body["history"]] == [None, None]

        _, body = self.post_json("/history", {"query": "classic", "max_results": 1})
        assert [entry["id"] for entry in body["history"]] == [third]
        assert body["history"][0]["data"] == [stored_point(1000, 9.1, 0.0)]

    def test_malformed_json_body_is_rejected_with_400(self):
        response, body = self.post_json("/history", "{not json")

        assert response.code == 400
        assert body == {
            "error": "Invalid JSON",
            "details": "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
        }

    def test_empty_body_is_rejected_with_400(self):
        response, body = self.post_json("/history", "")

        assert response.code == 400
        assert body == {
            "error": "Invalid JSON",
            "details": "Expecting value: line 1 column 1 (char 0)",
        }

    def test_invalid_search_parameters_are_rejected_with_422(self):
        response, body = self.post_json("/history", {"sort": "sideways", "max_results": "x"})

        assert response.code == 422
        assert [(error["loc"], error["type"]) for error in body] == [
            (["sort"], "enum"),
            (["max_results"], "int_parsing"),
        ]

    @pytest.mark.xfail(
        strict=True,
        reason="backend bug: HistoryHandler.post (api/history.py:243) unpacks the body "
        "with SearchParams(**data); a JSON body that is not an object raises an "
        "uncaught TypeError and answers 500 instead of 422",
    )
    def test_json_body_that_is_not_an_object_is_rejected_with_422(self):
        response = self.fetch(f"{API_PREFIX}/history", method="POST", body="[]")

        assert response.code == 422


class TestCurrentShot(HistoryAPITestCase):
    def test_idle_machine_has_no_current_shot(self):
        response = self.fetch(f"{API_PREFIX}/history/current")

        assert response.code == 200
        assert response.body == b"null"

    def test_current_shot_reports_live_data_in_history_entry_shape(self):
        self.monkeypatch.setitem(
            MeticulousConfig[CONFIG_PROFILES],
            PROFILE_LAST,
            {"load_time": FIRST_SHOT_TIME, "profile": CLASSIC},
        )
        ShotManager.start(push_to_brew_time=1200)
        ShotManager._current_shot.startTime = FIRST_SHOT_TIME
        ShotManager.handleShotData(brewing_point(CLASSIC, 1000, 9.0, 0.0))
        shot_id = ShotManager._current_shot.id

        response, body = self.get_json("/history/current")

        assert response.code == 200
        assert body == {
            "db_key": None,
            "file": FIRST_SHOT_FILE,
            "time": FIRST_SHOT_TIME,
            "profile_name": "Classic",
            "data": [stored_point(1000, 9.0, 0.0)],
            "id": shot_id,
            "push_to_brew_time": 1200,
            "profile": {"db_key": None, **CLASSIC},
        }

    def test_current_shot_without_known_profile_reports_only_db_key(self):
        ShotManager.start()
        ShotManager._current_shot.startTime = FIRST_SHOT_TIME
        ShotManager.handleShotData(brewing_point(CLASSIC, 1000, 9.0, 0.0))

        _, body = self.get_json("/history/current")

        assert body["profile"] == {"db_key": None}
        assert body["profile_name"] == "Classic"


class TestLastShot(HistoryAPITestCase):
    def test_fresh_machine_has_no_last_shot(self):
        response = self.fetch(f"{API_PREFIX}/history/last")

        assert response.code == 200
        assert response.body == b"null"

    def test_last_shot_is_the_newest_recorded_shot(self):
        self.record_three_shots()
        fourth_time = datetime(2026, 10, 9, 9, 0, 0).timestamp()
        fourth = self.record_shot(CLASSIC, fourth_time, [brewing_point(CLASSIC, 1, 1, 0)])

        response, body = self.get_json("/history/last")

        assert response.code == 200
        # Classic was already stored, so the shot reuses profile key 1.
        assert body == self.expected_entry(
            fourth,
            4,
            CLASSIC,
            1,
            fourth_time,
            f"{DAY}/09:00:00.shot.json.zst",
            [stored_point(1, 1, 0)],
            0,
        )

    def test_last_shot_is_read_from_the_database_after_a_restart(self):
        _, _, third = self.record_three_shots()
        ShotManager._last_shot = None

        _, body = self.get_json("/history/last")

        assert body == self.expected_entry(
            third,
            3,
            CLASSIC_V2,
            3,
            THIRD_SHOT_TIME,
            THIRD_SHOT_FILE,
            [stored_point(1000, 9.1, 0.0)],
            0,
        )


class TestProfileSearch(HistoryAPITestCase):
    def test_fresh_machine_suggests_nothing(self):
        response, body = self.get_json("/history/search")

        assert response.code == 200
        assert body == {"profiles": []}

        _, body = self.get_json("/history/search?query=classic")
        assert body == {"profiles": []}

    def test_without_query_suggests_profile_names_by_shot_count(self):
        self.record_three_shots()

        _, body = self.get_json("/history/search")

        assert body == {
            "profiles": [
                {"profile": "Classic", "type": "profile"},
                {"profile": "Bloom", "type": "profile"},
            ]
        }

    def test_query_matches_profile_and_stage_names(self):
        self.record_three_shots()

        _, body = self.get_json("/history/search?query=bloom")
        assert body == {
            "profiles": [
                {"profile": "Bloom", "type": "profile"},
                {"profile": "Bloom", "type": "stage", "name": "Bloom"},
            ]
        }

        _, body = self.get_json("/history/search?query=infus")
        assert body == {
            "profiles": [{"profile": "Classic", "type": "stage", "name": "Preinfusion"}]
        }

        _, body = self.get_json("/history/search?query=nothing-like-this")
        assert body == {"profiles": []}


class TestStatistics(HistoryAPITestCase):
    def test_fresh_machine_has_no_saved_shots(self):
        response, body = self.get_json("/history/stats")

        assert response.code == 200
        assert body == {"totalSavedShots": 0, "byProfile": [], "mileage": None}

    def test_counts_shots_and_profile_versions_and_reports_esp_mileage(self):
        self.record_three_shots()
        self.record_shot(
            CLASSIC,
            datetime(2026, 10, 9, 9, 0, 0).timestamp(),
            [brewing_point(CLASSIC, 1, 1, 0)],
        )
        self.monkeypatch.setattr(Machine, "mileage", 1234)

        _, body = self.get_json("/history/stats")

        assert body == {
            "totalSavedShots": 4,
            "byProfile": [
                {"name": "Classic", "count": 3, "profileVersions": 2},
                {"name": "Bloom", "count": 1, "profileVersions": 1},
            ],
            "mileage": 1234,
        }


class TestUploadIndex(HistoryAPITestCase):
    def test_fresh_machine_has_empty_index(self):
        response, body = self.get_json("/history/upload-index")

        assert response.code == 200
        assert body == {"history": []}

    def test_index_is_ordered_by_file_and_paged_by_cursor(self):
        self.record_three_shots()

        _, body = self.get_json("/history/upload-index")
        assert body == {
            "history": [
                {"file": FIRST_SHOT_FILE},
                {"file": SECOND_SHOT_FILE},
                {"file": THIRD_SHOT_FILE},
            ]
        }

        _, body = self.get_json(f"/history/upload-index?after={FIRST_SHOT_FILE}&max_results=1")
        assert body == {"history": [{"file": SECOND_SHOT_FILE}]}

        _, body = self.get_json(f"/history/upload-index?after={THIRD_SHOT_FILE}")
        assert body == {"history": []}

    def test_max_results_is_clamped_to_at_least_one(self):
        self.record_three_shots()

        _, body = self.get_json("/history/upload-index?max_results=-5")

        assert body == {"history": [{"file": FIRST_SHOT_FILE}]}

    def test_non_integer_max_results_is_rejected_with_400(self):
        response, body = self.get_json("/history/upload-index?max_results=lots")

        assert response.code == 400
        assert body == {"status": "error", "error": "max_results must be an integer"}

    def test_remote_request_is_refused(self):
        response, body = self.get_json(
            "/history/upload-index",
            headers={"X-Real-IP": "192.168.1.50", "Host": "meticulous.local"},
        )

        assert response.code == 403
        assert body == {
            "status": "error",
            "error": "This endpoint can only be accessed locally",
        }


class TestShotRating(HistoryAPITestCase):
    def ratings_in_db(self):
        with ShotDataBase.engine.connect() as connection:
            return connection.execute(
                select(shot_annotation.c.history_uuid, shot_rating.c.basic).select_from(
                    shot_annotation.join(
                        shot_rating, shot_annotation.c.id == shot_rating.c.annotation_id
                    )
                )
            ).fetchall()

    def test_unrated_and_unknown_shots_have_no_rating(self):
        first, _, _ = self.record_three_shots()

        response, body = self.get_json(f"/history/rating/{first}")
        assert response.code == 200
        assert body == {"shot_id": first, "rating": None}

        response, body = self.get_json("/history/rating/no-such-shot")
        assert response.code == 200
        assert body == {"shot_id": "no-such-shot", "rating": None}

    def test_get_with_empty_shot_id_reports_no_rating(self):
        response, body = self.get_json("/history/rating/")

        assert response.code == 200
        assert body == {"shot_id": "", "rating": None}

    def test_like_dislike_and_clear_are_stored(self):
        first, second, _ = self.record_three_shots()

        response, body = self.post_json(f"/history/rating/{first}", {"rating": "like"})
        assert response.code == 200
        assert body == {"status": "ok", "shot_id": first, "rating": "like"}
        assert self.ratings_in_db() == [(first, "like")]
        assert self.get_json(f"/history/rating/{first}")[1]["rating"] == "like"

        response, body = self.post_json(f"/history/rating/{first}", {"rating": "dislike"})
        assert body == {"status": "ok", "shot_id": first, "rating": "dislike"}
        assert self.ratings_in_db() == [(first, "dislike")]

        self.post_json(f"/history/rating/{second}", {"rating": "like"})
        response, body = self.post_json(f"/history/rating/{first}", {"rating": None})
        assert response.code == 200
        assert body == {"status": "ok", "shot_id": first, "rating": None}
        assert self.ratings_in_db() == [(second, "like")]
        assert self.get_json(f"/history/rating/{first}")[1]["rating"] is None

    def test_body_without_rating_clears_it(self):
        first, _, _ = self.record_three_shots()
        self.post_json(f"/history/rating/{first}", {"rating": "like"})

        response, body = self.post_json(f"/history/rating/{first}", {})

        assert response.code == 200
        assert body == {"status": "ok", "shot_id": first, "rating": None}
        assert self.ratings_in_db() == []

    def test_rating_unknown_shot_is_404(self):
        response, body = self.post_json("/history/rating/no-such-shot", {"rating": "like"})

        assert response.code == 404
        assert body == {"status": "error", "error": "Shot not found or rating failed"}
        assert self.ratings_in_db() == []

    def test_rating_by_database_key_is_404(self):
        # Ratings are keyed by the shot uuid, not the integer db_key.
        self.record_three_shots()

        response, _ = self.post_json("/history/rating/1", {"rating": "like"})

        assert response.code == 404

    def test_invalid_rating_value_is_rejected(self):
        first, _, _ = self.record_three_shots()

        response, body = self.post_json(f"/history/rating/{first}", {"rating": "LIKE"})

        assert response.code == 400
        assert body == {
            "status": "error",
            "error": "Invalid rating value. Use 'like', 'dislike', or null",
        }
        assert self.ratings_in_db() == []

    def test_malformed_json_is_rejected(self):
        first, _, _ = self.record_three_shots()

        response, body = self.post_json(f"/history/rating/{first}", "like")

        assert response.code == 400
        assert body == {"status": "error", "error": "Invalid JSON"}

    def test_post_with_empty_shot_id_is_rejected(self):
        response, body = self.post_json("/history/rating/", {"rating": "like"})

        assert response.code == 400
        assert body == {"status": "error", "error": "Invalid shot ID"}

    @pytest.mark.xfail(
        strict=True,
        reason="backend bug: ShotRatingHandler.post (api/history.py:303) calls "
        "data.get() on any JSON value; a body that is not an object raises "
        "AttributeError and answers 500 instead of 400",
    )
    def test_json_body_that_is_not_an_object_is_rejected_with_400(self):
        first, _, _ = self.record_three_shots()

        response = self.fetch(
            f"{API_PREFIX}/history/rating/{first}", method="POST", body='["like"]'
        )

        assert response.code == 400


class TestShotFiles(HistoryAPITestCase):
    def test_files_route_redirects_to_directory_listing(self):
        response = self.fetch(f"{API_PREFIX}/history/files", follow_redirects=False)

        assert response.code == 301
        assert response.headers["Location"] == "/api/v1/history/files/"

    def test_fresh_machine_listing_is_404(self):
        # SHOT_PATH is only created when the first shot is written.
        response, body = self.get_json("/history/files/")

        assert response.code == 404
        assert body == {"status": "error", "error": "history entry not found", "path": ""}

    def test_listings_show_day_folders_and_shot_files_newest_first(self):
        self.record_three_shots()
        for index, name in enumerate([FIRST_SHOT_FILE, SECOND_SHOT_FILE, THIRD_SHOT_FILE]):
            mtime = FIRST_SHOT_TIME + index
            os.utime(Path(self.shot_path, name), (mtime, mtime))

        response, body = self.get_json("/history/files/")
        assert response.code == 200
        assert body == [{"name": DAY, "url": DAY}]

        response, body = self.get_json(f"/history/files/{DAY}")
        assert response.code == 200
        assert body == [
            {"name": "08:32:00.shot.json", "url": "08:32:00.shot.json.zst"},
            {"name": "08:31:00.shot.json", "url": "08:31:00.shot.json.zst"},
            {"name": "08:30:00.shot.json", "url": "08:30:00.shot.json.zst"},
        ]

    def test_shot_file_is_served_decompressed_as_written(self):
        shot_id, _, _ = self.record_three_shots()
        expected = {
            "time": FIRST_SHOT_TIME,
            "profile_name": "Classic",
            "data": [stored_point(1000, 9.0, 0.0), stored_point(1100, 8.5, 0.4)],
            "id": shot_id,
            "push_to_brew_time": 1500,
            "profile": CLASSIC,
            "file": FIRST_SHOT_FILE,
        }

        for url in (FIRST_SHOT_FILE, FIRST_SHOT_FILE.removesuffix(".zst")):
            response, body = self.get_json(f"/history/files/{url}")
            assert response.code == 200, url
            assert response.headers["Content-Type"] == "application/json"
            assert body == {key: value for key, value in expected.items() if key != "file"}

    def test_compressed_query_serves_raw_file_as_named_attachment(self):
        self.record_three_shots()
        on_disk = Path(self.shot_path, FIRST_SHOT_FILE).read_bytes()

        response = self.fetch(f"{API_PREFIX}/history/files/{FIRST_SHOT_FILE}?compressed")

        assert response.code == 200
        assert response.body == on_disk
        # StaticFileHandler replaces the handler's octet-stream with the file's mimetype.
        assert response.headers["Content-Type"] == "application/zstd"
        assert response.headers["Content-Disposition"] == (
            'attachment; filename="Meticulous42_2026_10_09_08:30:00.shot.json.zst"'
        )
        assert "Etag" not in response.headers

    def test_non_finite_values_are_served_as_zero(self):
        self.record_shot(
            CLASSIC,
            FIRST_SHOT_TIME,
            [brewing_point(CLASSIC, 1000, 9.0, 0.0, gravimetric_flow=float("nan"))],
        )

        response = self.fetch(f"{API_PREFIX}/history/files/{FIRST_SHOT_FILE}")

        body = json.loads(response.body, parse_constant=pytest.fail)
        assert body["data"] == [stored_point(1000, 9.0, 0.0, 0.0)]

    def test_missing_shot_file_is_404(self):
        self.record_three_shots()

        for url in (f"{DAY}/23:59:59.shot.json.zst", "1999-01-01/00:00:00.shot.json"):
            response, body = self.get_json(f"/history/files/{url}")
            assert response.code == 404, url
            assert body == {"status": "error", "error": "history entry not found", "path": url}

        response, _ = self.get_json(f"/history/files/{DAY}/23:59:59.shot.json.zst?compressed")
        assert response.code == 404


class TestDebugFiles(HistoryAPITestCase):
    def test_debug_route_redirects_to_directory_listing(self):
        response = self.fetch(f"{API_PREFIX}/history/debug", follow_redirects=False)

        assert response.code == 301
        assert response.headers["Location"] == "/api/v1/history/debug/"

    def test_fresh_machine_debug_listing_is_404(self):
        response, body = self.get_json("/history/debug/")

        assert response.code == 404
        assert body == {"status": "error", "error": "history entry not found", "path": ""}

    def test_debug_listing_and_file(self):
        payload = {"type": "shot", "profile_name": "Classic", "data": [], "logs": []}
        self.write_debug_shot(f"{DAY}/08:30:00.shot.json.zst", payload, FIRST_SHOT_TIME)
        self.write_debug_shot(f"{DAY}/08:40:00.purge.json.zst", payload, SECOND_SHOT_TIME)

        _, body = self.get_json(f"/history/debug/{DAY}")
        assert body == [
            {"name": "08:40:00.purge.json", "url": "08:40:00.purge.json.zst"},
            {"name": "08:30:00.shot.json", "url": "08:30:00.shot.json.zst"},
        ]

        response, body = self.get_json(f"/history/debug/{DAY}/08:30:00.shot.json.zst")
        assert response.code == 200
        assert body == payload

    def test_compressed_csv_debug_file_is_served_as_csv(self):
        csv_path = Path(self.debug_path, DAY, "sensors.csv.zst")
        csv_path.parent.mkdir(parents=True)
        csv_path.write_bytes(zstd.ZstdCompressor().compress(b"t,p\n0,9.0\n"))

        response = self.fetch(f"{API_PREFIX}/history/debug/{DAY}/sensors.csv.zst")

        assert response.code == 200
        assert response.headers["Content-Type"] == "text/csv"
        assert response.body == b"t,p\n0,9.0\n"

    def test_last_debug_file_on_fresh_machine_is_404(self):
        response, body = self.get_json("/history/last-debug-file", follow_redirects=False)

        assert response.code == 404
        assert body == {"status": "error", "error": "No debug files found"}

    def test_last_debug_file_with_empty_debug_directory_is_404(self):
        self.debug_path.mkdir(parents=True)

        response, body = self.get_json("/history/last-debug-file", follow_redirects=False)

        assert response.code == 404
        assert body == {"status": "error", "error": "No debug files found"}

    def test_last_debug_file_redirects_to_most_recently_written_file(self):
        self.write_debug_shot("2026-10-08/23:00:00.shot.json.zst", {}, THIRD_SHOT_TIME)
        self.write_debug_shot(f"{DAY}/08:30:00.shot.json.zst", {}, FIRST_SHOT_TIME)

        response = self.fetch(f"{API_PREFIX}/history/last-debug-file", follow_redirects=False)

        assert response.code == 302
        assert response.headers["Location"] == (
            "/api/v1/history/debug/2026-10-08/23:00:00.shot.json.zst"
        )
        response, body = self.get_json(response.headers["Location"].removeprefix(API_PREFIX))
        assert response.code == 200
        assert body == {}

    def test_debug_zip_bundles_all_debug_shots_and_redirects_to_it(self):
        first = self.write_debug_shot(
            f"{DAY}/08:30:00.shot.json.zst", {"a": 1}, FIRST_SHOT_TIME
        )
        second = self.write_debug_shot(
            "2026-10-08/23:00:00.purge.json.zst", {"b": 2}, SECOND_SHOT_TIME
        )
        stale_zip = Path(self.debug_path, "debug-2026-10-01-10:00:00.zip")
        stale_zip.write_bytes(b"old")

        response = self.fetch(f"{API_PREFIX}/history/debug.zip", follow_redirects=False)

        assert response.code == 302
        zips = sorted(p.name for p in self.debug_path.glob("*.zip"))
        assert len(zips) == 1
        assert zips[0].startswith("debug-") and zips[0] != stale_zip.name
        assert response.headers["Location"] == f"/api/v1/history/debug/{zips[0]}"
        with zipfile.ZipFile(Path(self.debug_path, zips[0])) as archive:
            assert sorted(archive.namelist()) == [
                "2026-10-08/23:00:00.purge.json.zst",
                f"{DAY}/08:30:00.shot.json.zst",
            ]
            assert archive.read(f"{DAY}/08:30:00.shot.json.zst") == first.read_bytes()
            assert archive.read("2026-10-08/23:00:00.purge.json.zst") == second.read_bytes()

        download = self.fetch(f"{API_PREFIX}/history/debug/{zips[0]}")
        assert download.code == 200
        assert download.body == Path(self.debug_path, zips[0]).read_bytes()
        assert download.headers["Content-Type"] == "application/zip"

    @pytest.mark.xfail(
        strict=True,
        reason="backend bug: ShotDebugManager.zipAllDebugShots (shot_debug_manager.py:315) "
        "opens the zip inside DEBUG_HISTORY_PATH without creating it, so GET "
        "/history/debug.zip on a machine without debug shots answers 500 "
        "(api/history.py:82-85)",
    )
    def test_debug_zip_on_fresh_machine_redirects_to_an_empty_archive(self):
        response = self.fetch(f"{API_PREFIX}/history/debug.zip", follow_redirects=False)

        assert response.code == 302
        zips = [p.name for p in self.debug_path.glob("*.zip")]
        assert len(zips) == 1
        assert response.headers["Location"] == f"/api/v1/history/debug/{zips[0]}"
        with zipfile.ZipFile(Path(self.debug_path, zips[0])) as archive:
            assert archive.namelist() == []
