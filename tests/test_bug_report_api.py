import asyncio
import json
import subprocess
from datetime import datetime
from pathlib import Path
from types import SimpleNamespace

import pytest
from sqlalchemy import create_engine, insert, select

from database_models import bug_reports, metadata
from shot_database import ShotDataBase


@pytest.fixture
def report_module(tmp_path, monkeypatch):
    import api.bug_report as bug_report

    debug_root = tmp_path.joinpath("history", "debug")
    draft_root = tmp_path.joinpath("reports", "draft")
    debug_root.mkdir(parents=True)
    draft_root.mkdir(parents=True)

    engine = create_engine(f"sqlite:///{tmp_path.joinpath('history.sqlite')}")
    metadata.create_all(engine)
    monkeypatch.setattr(ShotDataBase, "engine", engine)
    monkeypatch.setattr(bug_report, "DEBUG_HISTORY_ROOT", debug_root)
    monkeypatch.setattr(bug_report, "DRAFT_REPORTS_DIR", draft_root)
    return bug_report


def _debug_file(root: Path, day: str, name: str):
    path = root.joinpath(day, name)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"{day}/{name}", encoding="utf-8")
    return path


def _read_archive_report_info(bug_report, archive_path: Path):
    report_info, files, temp_dir = bug_report._read_tar_zstd(archive_path)
    try:
        return report_info, set(files.keys())
    finally:
        temp_dir.cleanup()


def _read_zstd_json(path: Path):
    result = subprocess.run(
        ["zstd", "-d", "-f", "-q", "-c", str(path)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=True,
    )
    return json.loads(result.stdout)


def test_select_debug_files_descending(report_module):
    _debug_file(report_module.DEBUG_HISTORY_ROOT, "2026-05-17", "08:00:00.shot.json.zst")
    _debug_file(report_module.DEBUG_HISTORY_ROOT, "2026-05-18", "09:00:00.shot.json.zst")
    _debug_file(report_module.DEBUG_HISTORY_ROOT, "2026-05-18", "10:00:00.shot.json.zst")

    selected, errors = report_module._select_debug_files(limit=2)

    assert [path.name for path in selected] == [
        "10:00:00.shot.json.zst",
        "09:00:00.shot.json.zst",
    ]
    assert errors == []


def test_fetch_report_files_uses_parent_debug_file_names(report_module, monkeypatch):
    debug_name = "2026-05-18/10:00:00.shot.json.zst"
    _debug_file(report_module.DEBUG_HISTORY_ROOT, "2026-05-18", "10:00:00.shot.json.zst")

    async def fake_machine_logs(start_time=None, end_time=None, cancellation=None):
        return "logs"

    async def fake_machine_status(cancellation=None):
        return '{"ok": true}'

    monkeypatch.setattr(report_module, "_get_machine_info", lambda: {"machine": "info"})
    monkeypatch.setattr(report_module, "_fetch_machine_logs", fake_machine_logs)
    monkeypatch.setattr(report_module, "_fetch_machine_status", fake_machine_status)

    draft_dir = report_module._draft_path("local-test-id")
    fetched = asyncio.run(report_module._fetch_report_files(draft_dir))

    assert fetched.automatic_debug_files == [debug_name]
    assert report_module._debug_archive_name(debug_name) in fetched.files
    assert fetched.machine_status is True
    assert (
        fetched.files[report_module._debug_archive_name(debug_name)].read_text(encoding="utf-8")
        == debug_name
    )
    assert (
        draft_dir.joinpath(report_module.MACHINE_STATUS_NAME).read_text(encoding="utf-8")
        == '{"ok": true}'
    )


def test_fetch_machine_logs_uses_emulated_response_without_watcher(report_module, monkeypatch):
    monkeypatch.setattr(report_module, "_machine_is_emulated", lambda: True)

    def fail_fetch(*args, **kwargs):
        raise AssertionError("Emulated machine logs should not call watcher")

    monkeypatch.setattr(report_module, "_fetch_watcher_text", fail_fetch)

    logs = asyncio.run(report_module._fetch_machine_logs(123, 456))

    assert "Emulated machine logs generated for bug report" in logs
    assert "start_time=123, end_time=456" in logs


def test_fetch_machine_logs_converts_range_to_watcher_hours(report_module, monkeypatch):
    captured = {}

    async def fake_fetch_watcher_text(url, timeout_seconds, cancellation=None):
        captured["url"] = url
        captured["timeout_seconds"] = timeout_seconds
        captured["cancellation"] = cancellation
        return "logs"

    monkeypatch.setattr(report_module, "_machine_is_emulated", lambda: False)
    monkeypatch.setattr(report_module, "_now_seconds", lambda: 100000)
    monkeypatch.setattr(report_module, "_fetch_watcher_text", fake_fetch_watcher_text)

    logs = asyncio.run(
        report_module._fetch_machine_logs(
            100000 - (24 * 60 * 60) - 1,
            100000 - (60 * 60) - 1,
        )
    )

    assert logs == "logs"
    assert captured["url"].endswith("&since=25&until=1")
    assert captured["timeout_seconds"] == 600
    assert captured["cancellation"] is None


def test_fetch_machine_status_uses_emulated_response_without_watcher(
    report_module, monkeypatch
):
    monkeypatch.setattr(report_module, "_machine_is_emulated", lambda: True)

    def fail_fetch(*args, **kwargs):
        raise AssertionError("Emulated machine status should not call watcher")

    monkeypatch.setattr(report_module, "_fetch_watcher_text", fail_fetch)

    status = json.loads(asyncio.run(report_module._fetch_machine_status()))

    assert status["emulated"] is True
    assert status["status"] == "ok"
    assert status["source"] == "meticulous-backend"


def test_fetch_watcher_text_uses_aiohttp_and_preserves_timeout_and_decoding(
    report_module, monkeypatch
):
    captured = {}

    class FakeResponse:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *exc_info):
            return None

        async def read(self):
            return "café".encode("utf-8") + b"\xff"

    class FakeSession:
        def __init__(self, timeout=None, raise_for_status=None):
            captured["timeout"] = timeout
            captured["raise_for_status"] = raise_for_status

        async def __aenter__(self):
            return self

        async def __aexit__(self, *exc_info):
            return None

        def get(self, url):
            captured["url"] = url
            return FakeResponse()

    monkeypatch.setattr(report_module.aiohttp, "ClientSession", FakeSession)

    text = asyncio.run(report_module._fetch_watcher_text("http://watcher/health/status", 120))

    assert captured["url"] == "http://watcher/health/status"
    assert captured["raise_for_status"] is True
    assert captured["timeout"].total == 120
    # Invalid utf-8 tail is replaced (U+FFFD), not raised, matching prior behavior.
    assert text == "café" + chr(0xFFFD)


def test_fetch_watcher_text_cancels_active_task_on_disconnect(report_module, monkeypatch):
    started = asyncio.Event()

    async def slow_get_watcher_body(url, timeout_seconds):
        started.set()
        await asyncio.sleep(10)
        return b"too slow"

    monkeypatch.setattr(report_module, "_get_watcher_body", slow_get_watcher_body)

    async def run():
        cancellation = report_module.CollectionCancellation()
        fetch = asyncio.ensure_future(
            report_module._fetch_watcher_text("http://watcher/health/logs", 600, cancellation)
        )
        await started.wait()
        cancellation.cancel()
        with pytest.raises(asyncio.CancelledError):
            await fetch
        assert cancellation.active_task is None

    asyncio.run(run())


def test_fetch_report_files_raises_cancelled_at_next_boundary_after_disconnect(
    report_module, monkeypatch
):
    async def fail_machine_logs(start_time=None, end_time=None, cancellation=None):
        raise AssertionError("Machine logs must not be fetched after disconnect")

    monkeypatch.setattr(report_module, "_get_machine_info", lambda: {"machine": "info"})
    monkeypatch.setattr(report_module, "_fetch_machine_logs", fail_machine_logs)

    cancellation = report_module.CollectionCancellation()
    cancellation.disconnected = True

    draft_dir = report_module._draft_path("boundary-test-id")
    with pytest.raises(asyncio.CancelledError):
        asyncio.run(report_module._fetch_report_files(draft_dir, cancellation=cancellation))


def test_fetch_report_files_logs_stage_error_as_it_happens(report_module, monkeypatch):
    warnings = []

    async def failing_machine_logs(start_time=None, end_time=None, cancellation=None):
        raise RuntimeError("watcher unreachable")

    async def fake_machine_status(cancellation=None):
        return '{"ok": true}'

    async def no_incomplete_debug_shot(draft_dir):
        return None

    monkeypatch.setattr(report_module, "_get_machine_info", lambda: {"machine": "info"})
    monkeypatch.setattr(report_module, "_fetch_machine_logs", failing_machine_logs)
    monkeypatch.setattr(report_module, "_fetch_machine_status", fake_machine_status)
    monkeypatch.setattr(
        report_module, "_capture_incomplete_debug_shot", no_incomplete_debug_shot
    )
    monkeypatch.setattr(
        report_module.logger, "warning", lambda message, *a, **kw: warnings.append(message)
    )

    draft_dir = report_module._draft_path("logged-error-id")
    fetched = asyncio.run(report_module._fetch_report_files(draft_dir))

    # Logged at the point of failure, tagged with the localID, and the journal
    # copy says exactly what the bundle copy says.
    assert warnings == ["[logged-error-id] Failed to fetch machine logs: watcher unreachable"]
    assert fetched.errors[0] == "Failed to fetch machine logs: watcher unreachable"


def test_cancelled_stage_is_never_logged_as_a_collection_error(report_module, monkeypatch):
    """A disconnect must leave no trace, including in the journal.

    `CancelledError` is a `BaseException`, so it sails past every stage
    handler's `except Exception` without being recorded or logged. Widening
    one of those handlers would silently break that.
    """

    def fail_if_called(*args, **kwargs):
        raise AssertionError("A client disconnect must not be logged as a collection error")

    async def cancelled_machine_logs(start_time=None, end_time=None, cancellation=None):
        raise asyncio.CancelledError()

    async def no_incomplete_debug_shot(draft_dir):
        return None

    monkeypatch.setattr(report_module, "_get_machine_info", lambda: {"machine": "info"})
    monkeypatch.setattr(report_module, "_fetch_machine_logs", cancelled_machine_logs)
    monkeypatch.setattr(
        report_module, "_capture_incomplete_debug_shot", no_incomplete_debug_shot
    )
    monkeypatch.setattr(report_module.logger, "warning", fail_if_called)
    monkeypatch.setattr(report_module.logger, "info", fail_if_called)

    draft_dir = report_module._draft_path("cancelled-stage-id")
    with pytest.raises(asyncio.CancelledError):
        asyncio.run(report_module._fetch_report_files(draft_dir))


def test_short_debug_file_count_logs_at_info_not_warning(report_module, monkeypatch):
    """Every machine that has not brewed 10 shots reports a short count on
    every single report, so it must not reach the warning channel."""
    infos = []

    def fail_if_called(*args, **kwargs):
        raise AssertionError("An expected short debug-file count must not warn")

    async def fake_machine_logs(start_time=None, end_time=None, cancellation=None):
        return "logs"

    async def fake_machine_status(cancellation=None):
        return '{"ok": true}'

    async def no_incomplete_debug_shot(draft_dir):
        return None

    monkeypatch.setattr(report_module, "_get_machine_info", lambda: {"machine": "info"})
    monkeypatch.setattr(report_module, "_fetch_machine_logs", fake_machine_logs)
    monkeypatch.setattr(report_module, "_fetch_machine_status", fake_machine_status)
    monkeypatch.setattr(
        report_module, "_capture_incomplete_debug_shot", no_incomplete_debug_shot
    )
    monkeypatch.setattr(report_module.logger, "warning", fail_if_called)
    monkeypatch.setattr(
        report_module.logger, "info", lambda message, *a, **kw: infos.append(message)
    )

    draft_dir = report_module._draft_path("few-shots-id")
    fetched = asyncio.run(report_module._fetch_report_files(draft_dir))

    assert infos == ["[few-shots-id] Only found 0 debug files while reporting; requested 10."]
    assert "Only found 0 debug files while reporting; requested 10." in fetched.errors


def test_fetch_report_files_includes_active_incomplete_debug_shot_first(
    report_module, monkeypatch
):
    for index in range(10):
        _debug_file(
            report_module.DEBUG_HISTORY_ROOT,
            "2026-05-18",
            f"10:00:0{index}.shot.json.zst",
        )

    incomplete_name = "2026-05-18/11:00:00.shot_incomplete.json.zst"

    async def fake_machine_logs(start_time=None, end_time=None, cancellation=None):
        return "logs"

    async def fake_machine_status(cancellation=None):
        return '{"ok": true}'

    async def fake_capture_incomplete_debug_shot(draft_dir):
        path = draft_dir.joinpath(report_module.DEBUG_ARCHIVE_DIR, incomplete_name)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("active", encoding="utf-8")
        return incomplete_name

    monkeypatch.setattr(report_module, "_get_machine_info", lambda: {"machine": "info"})
    monkeypatch.setattr(report_module, "_fetch_machine_logs", fake_machine_logs)
    monkeypatch.setattr(report_module, "_fetch_machine_status", fake_machine_status)
    monkeypatch.setattr(
        report_module,
        "_capture_incomplete_debug_shot",
        fake_capture_incomplete_debug_shot,
    )

    draft_dir = report_module._draft_path("local-test-id")
    fetched = asyncio.run(report_module._fetch_report_files(draft_dir))

    assert len(fetched.automatic_debug_files) == report_module.MAX_DEBUG_SHOTS
    assert fetched.automatic_debug_files[0] == incomplete_name
    assert report_module._debug_archive_name(incomplete_name) in fetched.files
    assert (
        report_module._debug_archive_name("2026-05-18/10:00:00.shot.json.zst")
        not in fetched.files
    )


def test_select_debug_files_prioritizes_range_then_older_history(report_module):
    for name in (
        "09:00:00.shot.json.zst",
        "10:00:00.shot.json.zst",
        "11:00:00.shot.json.zst",
        "12:00:00.shot.json.zst",
        "13:00:00.shot.json.zst",
    ):
        _debug_file(report_module.DEBUG_HISTORY_ROOT, "2026-05-18", name)

    start = int(datetime(2026, 5, 18, 10, 0, 0).timestamp())
    end = int(datetime(2026, 5, 18, 12, 0, 0).timestamp())
    selected, errors = report_module._select_debug_files(
        limit=4, start_time=start, end_time=end
    )

    assert [path.name for path in selected] == [
        "12:00:00.shot.json.zst",
        "11:00:00.shot.json.zst",
        "10:00:00.shot.json.zst",
        "09:00:00.shot.json.zst",
    ]
    assert errors == []


def test_fetch_report_files_skips_active_debug_shot_for_historical_range(
    report_module, monkeypatch
):
    async def fail_capture(_draft_dir):
        raise AssertionError("Historical reports must not capture the active debug shot")

    async def fake_machine_logs(*_args, **_kwargs):
        return "logs"

    async def fake_machine_status(cancellation=None):
        return "status"

    monkeypatch.setattr(report_module, "_get_machine_info", lambda: {"machine": "info"})
    monkeypatch.setattr(report_module, "_fetch_machine_logs", fake_machine_logs)
    monkeypatch.setattr(report_module, "_fetch_machine_status", fake_machine_status)
    monkeypatch.setattr(report_module, "_capture_incomplete_debug_shot", fail_capture)

    fetched = asyncio.run(
        report_module._fetch_report_files(
            report_module._draft_path("historic"),
            start_time=1,
            end_time=2,
            capture_active_debug_shot=False,
        )
    )

    assert fetched.automatic_debug_files == []


def test_incomplete_debug_shot_snapshot_keeps_active_state(tmp_path):
    from shot_debug_manager import ShotDebugManager

    class FakeDebugShot:
        startTime = 1780297200.0
        profile = {"name": "profile"}
        profile_name = "profile"
        nodeJSON = {}
        shottype = "shot"

        def to_json(self):
            return {
                "time": self.startTime,
                "type": self.shottype,
                "profile_name": self.profile_name,
                "profile": self.profile,
                "nodeJSON": self.nodeJSON,
                "data": [{"shot": {"pressure": 1}}],
                "logs": [],
            }

    original_current_data = ShotDebugManager._current_data
    active_debug_shot = FakeDebugShot()
    ShotDebugManager._current_data = active_debug_shot
    try:
        relative_name = ShotDebugManager.write_current_incomplete_debug_shot(tmp_path)
        active_state_kept = ShotDebugManager._current_data is active_debug_shot
    finally:
        ShotDebugManager._current_data = original_current_data

    expected_prefix = datetime.fromtimestamp(active_debug_shot.startTime).strftime(
        "%Y-%m-%d/%H:%M:%S"
    )
    assert relative_name == f"{expected_prefix}.shot_incomplete.json.zst"
    assert active_state_kept is True
    payload = _read_zstd_json(tmp_path.joinpath(relative_name))
    assert payload["type"] == "shot"
    assert payload["data"] == [{"shot": {"pressure": 1}}]


def test_fiql_filter_ignores_invalid_fields_and_rejects_empty(report_module):
    valid_condition, invalid = report_module._parse_fiql(
        "unknown==x;status==draft,creationTime=gt=10"
    )
    empty_condition, empty_invalid = report_module._parse_fiql("unknown==x")

    assert valid_condition is not None
    assert invalid is False
    assert empty_condition is None
    assert empty_invalid is True


def test_draft_patch_rejects_date_and_issue_times(report_module):
    with pytest.raises(PermissionError):
        report_module._validate_draft_patch({"dateAndTime": 2})
    with pytest.raises(PermissionError):
        report_module._validate_draft_patch({"issueTime": 2})


def test_draft_patch_adds_user_debug_file_to_draft_directory(report_module):
    user_file = _debug_file(
        report_module.DEBUG_HISTORY_ROOT, "2026-05-18", "14:00:00.user.json.zst"
    )
    user_file_name = report_module._safe_archive_name(user_file)

    local_id = "local-test-id"
    draft_dir = report_module._draft_path(local_id)
    draft_dir.mkdir(parents=True)
    report_module._write_draft_report_info(
        draft_dir,
        {
            "description": None,
            "dateAndTime": 1,
            "attachments": {
                "debugFiles": {
                    "automatic": [],
                    "user": [],
                },
                "machineInfo": True,
                "machineLogs": True,
                "machineStatus": True,
            },
            "multimedia": None,
            "machineID": "machine",
            "eventID": None,
            "baseEventID": None,
            "ticket": None,
            "localID": local_id,
        },
    )
    with ShotDataBase.engine.begin() as connection:
        connection.execute(
            insert(bug_reports).values(
                localID=local_id,
                issueTime=1,
                creationTime=1,
                logFiles=None,
                machineInfo=True,
                machineLogs=True,
                machineStatus=True,
                status="draft",
            )
        )

    updated = asyncio.run(
        report_module._apply_draft_patch(
            local_id, {"attachments": {"debugFiles": {"user": [user_file_name]}}}
        )
    )
    draft_files = set(report_module._draft_files(draft_dir).keys())

    assert updated["attachments"]["debugFiles"]["user"] == [user_file_name]
    assert report_module._debug_archive_name(user_file_name) in draft_files
    assert (
        draft_dir.joinpath(report_module._debug_archive_name(user_file_name)).read_text(
            encoding="utf-8"
        )
        == f"2026-05-18/{user_file.name}"
    )

    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()
    assert row.logFiles == user_file_name


def test_draft_directory_can_be_compressed(report_module):
    local_id = "local-test-id"
    draft_dir = report_module._draft_path(local_id)
    draft_dir.mkdir(parents=True)
    draft_dir.joinpath(report_module.MACHINE_STATUS_NAME).write_text(
        '{"ok": true}', encoding="utf-8"
    )
    report_module._write_draft_report_info(
        draft_dir,
        {
            "description": None,
            "dateAndTime": 1,
            "attachments": {"machineStatus": True},
            "multimedia": None,
            "machineID": "machine",
            "eventID": None,
            "baseEventID": None,
            "ticket": None,
            "localID": local_id,
        },
    )
    archive_path = report_module.DRAFT_REPORTS_DIR.joinpath("out.zstd")

    report_module._write_tar_zstd_from_draft(archive_path, draft_dir)
    report_info, archived_names = _read_archive_report_info(report_module, archive_path)

    assert report_info["localID"] == local_id
    assert report_module.MACHINE_STATUS_NAME in archived_names


def test_create_report_returns_machine_id_matching_report_info(report_module, monkeypatch):
    calls = []

    async def fake_fetch_report_files(draft_dir, *args, **kwargs):
        calls.append((args, kwargs))
        draft_dir.mkdir(parents=True, exist_ok=True)
        machine_status = draft_dir.joinpath(report_module.MACHINE_STATUS_NAME)
        machine_status.write_text('{"ok": true}', encoding="utf-8")
        return report_module.FetchResult(
            files={report_module.MACHINE_STATUS_NAME: machine_status},
            machine_status=True,
        )

    class FakeHandler:
        request = SimpleNamespace(body=b"")
        _cancellation = report_module.CollectionCancellation()

        def write(self, body):
            self.body = body

    monkeypatch.setattr(report_module, "_new_local_id", lambda: "local-test-id")
    monkeypatch.setattr(report_module, "_now_seconds", lambda: 1)
    monkeypatch.setattr(report_module, "_fetch_report_files", fake_fetch_report_files)
    monkeypatch.setattr(ShotDataBase, "statistics", lambda: {})
    monkeypatch.setattr(
        report_module,
        "MeticulousConfig",
        {
            report_module.CONFIG_SYSTEM: {
                report_module.MACHINE_SERIAL_NUMBER: "machine-test-id",
            },
        },
    )
    monkeypatch.setitem(
        report_module.MeticulousConfig[report_module.CONFIG_SYSTEM],
        report_module.MACHINE_SERIAL_NUMBER,
        "machine-test-id",
    )

    handler = FakeHandler()
    asyncio.run(report_module.ReportsCreateHandler.post(handler))
    report_info = report_module._read_draft_report_info(
        report_module._draft_path("local-test-id")
    )

    assert handler.body == {"localID": "local-test-id", "machineID": "machine-test-id"}
    assert report_info["machineID"] == handler.body["machineID"]
    assert report_info["dateAndTime"] == 1
    assert report_info["issueTime"] == 1
    assert len(calls) == 1
    call_args, call_kwargs = calls[0]
    assert call_args == (None, None)
    assert call_kwargs["capture_active_debug_shot"] is True
    assert call_kwargs["cancellation"] is handler._cancellation
    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()
    assert row.localID == "local-test-id"
    assert row.machineID == "machine-test-id"
    assert row.machineStatus is True
    assert row.creationTime == 1
    assert row.issueTime == 1


def test_create_report_with_historical_issue_time_persists_metadata_and_range(
    report_module, monkeypatch
):
    now = 200000
    issue_time = now - (25 * 60 * 60)
    calls = []

    async def fake_fetch_report_files(draft_dir, *args, **kwargs):
        calls.append((args, kwargs))
        draft_dir.mkdir(parents=True, exist_ok=True)
        return report_module.FetchResult()

    class FakeHandler:
        request = SimpleNamespace(body=json.dumps({"issueTime": issue_time}).encode())
        _cancellation = report_module.CollectionCancellation()

        def write(self, body):
            self.body = body

    monkeypatch.setattr(report_module, "_new_local_id", lambda: "historical-id")
    monkeypatch.setattr(report_module, "_now_seconds", lambda: now)
    monkeypatch.setattr(report_module, "_fetch_report_files", fake_fetch_report_files)
    monkeypatch.setitem(
        report_module.MeticulousConfig[report_module.CONFIG_SYSTEM],
        report_module.MACHINE_SERIAL_NUMBER,
        "machine-test-id",
    )

    handler = FakeHandler()
    asyncio.run(report_module.ReportsCreateHandler.post(handler))

    report_info = report_module._read_draft_report_info(
        report_module._draft_path("historical-id")
    )
    listed = report_module._list_report_page(page=0, size=1)["content"][0]
    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()

    assert len(calls) == 1
    call_args, call_kwargs = calls[0]
    assert call_args == (issue_time - (12 * 60 * 60), issue_time + (12 * 60 * 60))
    assert call_kwargs["capture_active_debug_shot"] is False
    assert call_kwargs["cancellation"] is handler._cancellation
    assert report_info["dateAndTime"] == now
    assert report_info["issueTime"] == issue_time
    assert row.creationTime == now
    assert row.issueTime == issue_time
    assert listed["dateAndTime"] == now
    assert listed["issueTime"] == issue_time


def test_create_report_uses_trailing_range_for_recent_and_future_issue_times(report_module):
    now = 200000

    assert report_module._collection_range(now - 1, now) == (
        now - (24 * 60 * 60),
        now,
    )
    assert report_module._collection_range(now + 1, now) == (
        now - (24 * 60 * 60),
        now,
    )
    assert report_module._collection_range(now - (12 * 60 * 60), now) == (
        now - (24 * 60 * 60),
        now,
    )


def test_create_report_with_recent_issue_time_keeps_active_shot_eligible(
    report_module, monkeypatch
):
    now = 200000
    issue_time = now - 1
    calls = []

    async def fake_fetch_report_files(draft_dir, *args, **kwargs):
        calls.append((args, kwargs))
        draft_dir.mkdir(parents=True, exist_ok=True)
        return report_module.FetchResult()

    class FakeHandler:
        request = SimpleNamespace(body=json.dumps({"issueTime": issue_time}).encode())
        _cancellation = report_module.CollectionCancellation()

        def write(self, body):
            self.body = body

    monkeypatch.setattr(report_module, "_new_local_id", lambda: "recent-id")
    monkeypatch.setattr(report_module, "_now_seconds", lambda: now)
    monkeypatch.setattr(report_module, "_fetch_report_files", fake_fetch_report_files)
    monkeypatch.setitem(
        report_module.MeticulousConfig[report_module.CONFIG_SYSTEM],
        report_module.MACHINE_SERIAL_NUMBER,
        "machine-test-id",
    )

    handler = FakeHandler()
    asyncio.run(report_module.ReportsCreateHandler.post(handler))

    assert len(calls) == 1
    call_args, call_kwargs = calls[0]
    assert call_args == (now - (24 * 60 * 60), now)
    assert call_kwargs["capture_active_debug_shot"] is True
    assert call_kwargs["cancellation"] is handler._cancellation


def test_create_report_cancelled_mid_collection_leaves_no_trace(report_module, monkeypatch):
    async def cancelling_fetch_report_files(draft_dir, *args, cancellation=None, **kwargs):
        draft_dir.mkdir(parents=True, exist_ok=True)
        draft_dir.joinpath("partial.txt").write_text("partial", encoding="utf-8")
        raise asyncio.CancelledError()

    def fail_if_called(*args, **kwargs):
        raise AssertionError("Cancellation must not be treated as an error")

    class FakeHandler:
        request = SimpleNamespace(body=b"")
        _cancellation = report_module.CollectionCancellation()
        wrote = False
        status = None

        def write(self, body):
            self.wrote = True
            self.body = body

        def set_status(self, status):
            self.status = status

    monkeypatch.setattr(report_module, "_new_local_id", lambda: "cancelled-id")
    monkeypatch.setattr(report_module, "_fetch_report_files", cancelling_fetch_report_files)
    monkeypatch.setattr(report_module.logger, "exception", fail_if_called)
    monkeypatch.setattr(report_module.logger, "error", fail_if_called)
    monkeypatch.setattr(report_module, "_api_error", fail_if_called)
    monkeypatch.setitem(
        report_module.MeticulousConfig[report_module.CONFIG_SYSTEM],
        report_module.MACHINE_SERIAL_NUMBER,
        "machine-test-id",
    )

    handler = FakeHandler()
    asyncio.run(report_module.ReportsCreateHandler.post(handler))

    draft_dir = report_module._draft_path("cancelled-id")
    assert not draft_dir.exists()
    assert handler.wrote is False
    assert handler.status is None
    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()
    assert row is None


def test_create_report_cancelled_during_debug_file_copy_never_inserts_row(
    report_module, monkeypatch
):
    for index in range(3):
        _debug_file(
            report_module.DEBUG_HISTORY_ROOT, "2026-05-18", f"10:00:0{index}.shot.json.zst"
        )

    async def fake_machine_logs(start_time=None, end_time=None, cancellation=None):
        return "logs"

    async def fake_machine_status(cancellation=None):
        return '{"ok": true}'

    cancellation = report_module.CollectionCancellation()
    real_copy_draft_file = report_module._copy_draft_file
    copy_calls = []

    def cancel_after_first_copy(draft_dir, archive_name, source_path):
        copied = real_copy_draft_file(draft_dir, archive_name, source_path)
        copy_calls.append(archive_name)
        # Simulate the client disconnecting while the first file was copying:
        # the collection must stop before the *next* copy, not this one.
        cancellation.disconnected = True
        return copied

    class FakeHandler:
        request = SimpleNamespace(body=b"")
        _cancellation = cancellation
        wrote = False

        def write(self, body):
            self.wrote = True
            self.body = body

    monkeypatch.setattr(report_module, "_get_machine_info", lambda: {"machine": "info"})
    monkeypatch.setattr(report_module, "_fetch_machine_logs", fake_machine_logs)
    monkeypatch.setattr(report_module, "_fetch_machine_status", fake_machine_status)
    monkeypatch.setattr(report_module, "_copy_draft_file", cancel_after_first_copy)
    monkeypatch.setattr(report_module, "_new_local_id", lambda: "mid-copy-cancel-id")
    monkeypatch.setitem(
        report_module.MeticulousConfig[report_module.CONFIG_SYSTEM],
        report_module.MACHINE_SERIAL_NUMBER,
        "machine-test-id",
    )

    handler = FakeHandler()
    asyncio.run(report_module.ReportsCreateHandler.post(handler))

    assert handler.wrote is False
    draft_dir = report_module._draft_path("mid-copy-cancel-id")
    assert not draft_dir.exists()
    assert len(copy_calls) == 1
    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()
    assert row is None


def test_create_report_records_watcher_failure_but_still_returns_draft_info(
    report_module, monkeypatch
):
    async def failing_machine_logs(start_time=None, end_time=None, cancellation=None):
        raise RuntimeError("watcher unreachable")

    async def fake_machine_status(cancellation=None):
        return '{"ok": true}'

    monkeypatch.setattr(report_module, "_get_machine_info", lambda: {"machine": "info"})
    monkeypatch.setattr(report_module, "_fetch_machine_logs", failing_machine_logs)
    monkeypatch.setattr(report_module, "_fetch_machine_status", fake_machine_status)
    monkeypatch.setattr(report_module, "_new_local_id", lambda: "watcher-fail-id")
    monkeypatch.setattr(report_module, "_now_seconds", lambda: 1)
    monkeypatch.setitem(
        report_module.MeticulousConfig[report_module.CONFIG_SYSTEM],
        report_module.MACHINE_SERIAL_NUMBER,
        "machine-test-id",
    )

    class FakeHandler:
        request = SimpleNamespace(body=b"")
        _cancellation = report_module.CollectionCancellation()

        def write(self, body):
            self.body = body

    handler = FakeHandler()
    asyncio.run(report_module.ReportsCreateHandler.post(handler))

    assert handler.body == {"localID": "watcher-fail-id", "machineID": "machine-test-id"}
    draft_dir = report_module._draft_path("watcher-fail-id")
    report_log = draft_dir.joinpath(report_module.REPORT_LOG_NAME).read_text(encoding="utf-8")
    assert "Failed to fetch machine logs: watcher unreachable" in report_log
    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()
    assert row.localID == "watcher-fail-id"
    assert row.machineLogs is False
    assert row.machineStatus is True


def test_create_report_request_requires_only_integer_issue_time(report_module):
    assert report_module._create_report_issue_time(b"") is None
    assert report_module._create_report_issue_time(b'{"issueTime": 123}') == 123
    for body in (b"{}", b'{"issueTime": true}', b'{"issueTime": 1.5}', b"[]"):
        with pytest.raises(ValueError):
            report_module._create_report_issue_time(body)


def test_draft_patch_persists_ticket_and_multimedia_in_db_and_report_info(
    report_module,
):
    local_id = "local-test-id"
    draft_dir = report_module._draft_path(local_id)
    draft_dir.mkdir(parents=True)
    report_module._write_draft_report_info(
        draft_dir,
        {
            "description": None,
            "dateAndTime": 1,
            "attachments": {
                "debugFiles": {"automatic": [], "user": []},
                "machineInfo": True,
                "machineLogs": True,
                "machineStatus": True,
            },
            "multimedia": None,
            "machineID": "machine",
            "eventID": None,
            "baseEventID": None,
            "ticket": None,
            "localID": local_id,
        },
    )
    with ShotDataBase.engine.begin() as connection:
        connection.execute(
            insert(bug_reports).values(
                localID=local_id,
                issueTime=1,
                creationTime=1,
                logFiles=None,
                machineInfo=True,
                machineLogs=True,
                machineStatus=True,
                status="draft",
            )
        )

    patch = {"ticket": 1234, "multimedia": 2}
    report_module._validate_draft_patch(patch)
    updated = asyncio.run(report_module._apply_draft_patch(local_id, patch))

    assert updated["ticket"] == 1234
    assert updated["multimedia"] == 2
    archived_info = report_module._read_draft_report_info(draft_dir)
    assert archived_info["ticket"] == 1234
    assert archived_info["multimedia"] == 2
    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()
    assert row.ticketNumber == 1234
    assert row.multimedia == 2


def test_list_report_page_returns_newest_first_with_machine_id(report_module):
    with ShotDataBase.engine.begin() as connection:
        connection.execute(
            insert(bug_reports),
            [
                {
                    "localID": "older-id",
                    "issueTime": 1,
                    "creationTime": 1,
                    "machineID": "older-machine",
                    "machineInfo": False,
                    "machineLogs": False,
                    "machineStatus": False,
                    "status": "draft",
                },
                {
                    "localID": "newer-id",
                    "issueTime": 2,
                    "creationTime": 2,
                    "machineID": "newer-machine",
                    "machineInfo": True,
                    "machineLogs": True,
                    "machineStatus": True,
                    "status": "draft",
                },
            ],
        )

    response = report_module._list_report_page(page=0, size=1)

    assert response["content"][0]["localID"] == "newer-id"
    assert response["content"][0]["machineID"] == "newer-machine"
    assert response["content"][0]["dateAndTime"] == 2
    assert response["content"][0]["issueTime"] == 2
    assert response["hasMore"] is True


def test_submit_update_persists_db_and_report_info(report_module):
    local_id = "submit-id"
    draft_dir = report_module._draft_path(local_id)
    draft_dir.mkdir(parents=True)
    report_module._write_draft_report_info(
        draft_dir,
        {
            "description": None,
            "dateAndTime": 1,
            "attachments": {
                "debugFiles": {"automatic": [], "user": []},
                "machineInfo": False,
                "machineLogs": False,
                "machineStatus": False,
            },
            "multimedia": 1,
            "machineID": "machine",
            "eventID": None,
            "baseEventID": None,
            "ticket": None,
            "localID": local_id,
        },
    )
    with ShotDataBase.engine.begin() as connection:
        connection.execute(
            insert(bug_reports).values(
                localID=local_id,
                issueTime=1,
                creationTime=1,
                machineInfo=False,
                machineLogs=False,
                machineStatus=False,
                status="draft",
            )
        )

    updated = report_module._mark_report_submitted(
        local_id, "event-1", 3, ticket_provided=True, ticket=42
    )

    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()
    archived_info, _archived_names = _read_archive_report_info(
        report_module, report_module._finalized_draft_path(local_id)
    )
    assert updated is True
    assert not draft_dir.exists()
    assert report_module._finalized_draft_path(local_id).exists()
    assert row.eventID == "event-1"
    assert row.ticketNumber == 42
    assert row.submissionTime == 3
    assert row.status == "submitted"
    assert archived_info["eventID"] == "event-1"
    assert archived_info["ticket"] == 42
    assert archived_info["multimedia"] == 1


def test_get_draft_returns_finalized_archive_without_recompressing(report_module, monkeypatch):
    local_id = "submit-id"
    draft_dir = report_module._draft_path(local_id)
    draft_dir.mkdir(parents=True)
    draft_dir.joinpath(report_module.MACHINE_STATUS_NAME).write_text(
        '{"ok": true}', encoding="utf-8"
    )
    report_module._write_draft_report_info(
        draft_dir,
        {
            "description": None,
            "dateAndTime": 1,
            "attachments": {"machineStatus": True},
            "multimedia": None,
            "machineID": "machine",
            "eventID": "event-1",
            "baseEventID": None,
            "ticket": 42,
            "localID": local_id,
        },
    )
    report_module._finalize_draft_archive(local_id)
    finalized_archive_path = report_module._finalized_draft_path(local_id)
    finalized_archive_bytes = finalized_archive_path.read_bytes()

    def fail_recompression(*args, **kwargs):
        raise AssertionError("Finalized archive should be streamed without recompressing")

    class FakeHandler:
        def __init__(self):
            self.headers = {}
            self.body = b""

        def set_header(self, name, value):
            self.headers[name] = value

        def write(self, body):
            self.body += body

    monkeypatch.setattr(report_module, "_write_tar_zstd_from_draft", fail_recompression)

    handler = FakeHandler()
    asyncio.run(report_module.ReportDraftHandler.get(handler, local_id))

    assert not draft_dir.exists()
    assert handler.headers["Content-Type"] == "application/octet-stream"
    assert handler.headers["Content-Disposition"] == 'attachment; filename="submit-id.zstd"'
    assert handler.body == finalized_archive_bytes


def test_compressed_draft_contains_latest_report_info_after_updates(report_module):
    local_id = "submit-id"
    draft_dir = report_module._draft_path(local_id)
    draft_dir.mkdir(parents=True)
    report_module._write_draft_report_info(
        draft_dir,
        {
            "description": None,
            "dateAndTime": 1,
            "attachments": {
                "debugFiles": {"automatic": [], "user": []},
                "machineInfo": False,
                "machineLogs": False,
                "machineStatus": False,
            },
            "multimedia": None,
            "machineID": "machine",
            "eventID": None,
            "baseEventID": None,
            "ticket": None,
            "localID": local_id,
        },
    )
    with ShotDataBase.engine.begin() as connection:
        connection.execute(
            insert(bug_reports).values(
                localID=local_id,
                issueTime=1,
                creationTime=1,
                machineInfo=False,
                machineLogs=False,
                machineStatus=False,
                status="draft",
            )
        )

    asyncio.run(report_module._apply_draft_patch(local_id, {"ticket": 42, "multimedia": 2}))
    report_module._mark_report_submitted(
        local_id, "event-1", 3, ticket_provided=True, ticket=42
    )

    archived_info, archived_names = _read_archive_report_info(
        report_module, report_module._finalized_draft_path(local_id)
    )
    assert report_module.REPORT_INFO_NAME not in archived_names
    assert not draft_dir.exists()
    assert archived_info["eventID"] == "event-1"
    assert archived_info["ticket"] == 42
    assert archived_info["multimedia"] == 2


def test_submit_without_ticket_preserves_existing_ticket(report_module):
    local_id = "submit-id"
    draft_dir = report_module._draft_path(local_id)
    draft_dir.mkdir(parents=True)
    report_module._write_draft_report_info(
        draft_dir,
        {
            "description": None,
            "dateAndTime": 1,
            "attachments": {
                "debugFiles": {"automatic": [], "user": []},
                "machineInfo": False,
                "machineLogs": False,
                "machineStatus": False,
            },
            "multimedia": None,
            "machineID": "machine",
            "eventID": None,
            "baseEventID": None,
            "ticket": 42,
            "localID": local_id,
        },
    )
    with ShotDataBase.engine.begin() as connection:
        connection.execute(
            insert(bug_reports).values(
                localID=local_id,
                issueTime=1,
                creationTime=1,
                machineInfo=False,
                machineLogs=False,
                machineStatus=False,
                ticketNumber=42,
                status="draft",
            )
        )

    updated = report_module._mark_report_submitted(
        local_id, "event-1", 3, ticket_provided=False, ticket=None
    )

    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()
    archived_info, _archived_names = _read_archive_report_info(
        report_module, report_module._finalized_draft_path(local_id)
    )
    assert updated is True
    assert not draft_dir.exists()
    assert row.eventID == "event-1"
    assert row.ticketNumber == 42
    assert archived_info["eventID"] == "event-1"
    assert archived_info["ticket"] == 42


class _DeleteDraftHandler:
    def __init__(self, body=b""):
        self.request = SimpleNamespace(body=body)
        self.status = None
        self.response = None
        self.finished = False

    def set_status(self, status):
        self.status = status

    def write(self, body):
        self.response = body

    def finish(self):
        self.finished = True


def _insert_deletable_report(report_module, local_id: str, status: str = "draft"):
    with ShotDataBase.engine.begin() as connection:
        connection.execute(
            insert(bug_reports).values(
                localID=local_id,
                issueTime=1,
                creationTime=1,
                machineInfo=False,
                machineLogs=False,
                machineStatus=False,
                status=status,
            )
        )


@pytest.mark.parametrize("representation", ["directory", "archive", "both"])
def test_delete_draft_removes_all_report_representations_and_db_row(
    report_module, representation
):
    local_id = f"delete-{representation}"
    draft_dir = report_module._draft_path(local_id)
    archive_path = report_module._finalized_draft_path(local_id)
    if representation in {"directory", "both"}:
        draft_dir.mkdir()
        draft_dir.joinpath("report.txt").write_text("draft", encoding="utf-8")
    if representation in {"archive", "both"}:
        archive_path.write_bytes(b"archive")
    _insert_deletable_report(report_module, local_id)

    handler = _DeleteDraftHandler()
    asyncio.run(report_module.ReportDraftHandler.delete(handler, local_id))

    assert handler.status == 204
    assert handler.finished is True
    assert not draft_dir.exists()
    assert not archive_path.exists()
    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(
            select(bug_reports).where(bug_reports.c.localID == local_id)
        ).first()
    assert row is None


def test_delete_draft_allows_submitted_report(report_module):
    local_id = "submitted-report"
    archive_path = report_module._finalized_draft_path(local_id)
    archive_path.write_bytes(b"archive")
    _insert_deletable_report(report_module, local_id, status="submitted")

    handler = _DeleteDraftHandler()
    asyncio.run(report_module.ReportDraftHandler.delete(handler, local_id))

    assert handler.status == 204
    assert not archive_path.exists()
    assert report_module._get_report_row(local_id) is None


def test_delete_draft_returns_not_found_for_unknown_local_id(report_module):
    handler = _DeleteDraftHandler()

    asyncio.run(report_module.ReportDraftHandler.delete(handler, "unknown-id"))

    assert handler.status == 404
    assert handler.response == {"error": "Unknown localID", "description": ""}


def test_delete_draft_rejects_request_body(report_module):
    handler = _DeleteDraftHandler(body=b"{}")

    asyncio.run(report_module.ReportDraftHandler.delete(handler, "unknown-id"))

    assert handler.status == 400
    assert handler.response == {
        "error": "Delete report draft request must not contain a body",
        "description": "",
    }
