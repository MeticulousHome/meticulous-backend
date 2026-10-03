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
    # ShotDataBase.session is only populated by ShotDataBase.init(), which
    # this fixture deliberately avoids calling (it wires the engine directly
    # instead). ReportsCreateHandler.post() calls ShotDataBase.statistics(),
    # so every test that drives post() through this fixture needs it stubbed;
    # doing it here once covers all of them instead of per test.
    monkeypatch.setattr(ShotDataBase, "statistics", lambda: {})
    monkeypatch.setattr(bug_report, "DEBUG_HISTORY_ROOT", debug_root)
    monkeypatch.setattr(bug_report, "REPORTS_DIR", draft_root.parent)
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

    async def fake_fetch_watcher_text(url, timeout_seconds, cancellation=None, max_bytes=None):
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

    async def slow_get_watcher_body(url, timeout_seconds, max_bytes=None):
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
    assert response["content"][0]["status"] == "draft"
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
    local_id = "018f0a2b-1234-7abc-8def-0123456789ab"
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
    assert handler.headers["Content-Disposition"] == f'attachment; filename="{local_id}.zstd"'
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


def test_repeat_submit_after_finalization_is_a_successful_noop(report_module):
    local_id = "018f0a2b-1234-7abc-8def-0123456789ab"
    with ShotDataBase.engine.begin() as connection:
        connection.execute(
            insert(bug_reports).values(
                localID=local_id,
                issueTime=1,
                creationTime=1,
                machineInfo=False,
                machineLogs=False,
                machineStatus=False,
                eventID="first-event",
                submissionTime=2,
                status="submitted",
            )
        )

    assert (
        report_module._mark_report_submitted(
            local_id, "retry-event", 3, ticket_provided=False, ticket=None
        )
        is True
    )
    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).one()
    assert row.status == "submitted"
    assert row.eventID == "first-event"
    assert row.submissionTime == 2


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
    local_id = {
        "directory": "018f0a2b-1234-7abc-8def-0123456789ab",
        "archive": "018f0a2b-1234-7abc-8def-0123456789ac",
        "both": "018f0a2b-1234-7abc-8def-0123456789ad",
    }[representation]
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
    local_id = "018f0a2b-1234-7abc-8def-0123456789ab"
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

    asyncio.run(
        report_module.ReportDraftHandler.delete(handler, "018f0a2b-1234-7abc-8def-0123456789ab")
    )

    assert handler.status == 404
    assert handler.response == {
        "error": "Unknown localID",
        "description": "",
        "data": {"code": "UNKNOWN_LOCAL_ID"},
    }


def test_delete_draft_rejects_request_body(report_module):
    handler = _DeleteDraftHandler(body=b"{}")

    asyncio.run(
        report_module.ReportDraftHandler.delete(handler, "018f0a2b-1234-7abc-8def-0123456789ab")
    )

    assert handler.status == 400
    assert handler.response == {
        "error": "Delete report draft request must not contain a body",
        "description": "",
        "data": {"code": "INVALID_BODY"},
    }


def test_local_id_validation_and_paths_reject_traversal(report_module):
    valid = "018f0a2b-1234-7abc-8def-0123456789ab"
    assert report_module._validate_local_id(valid) == valid
    for value in ("../history", "..", "A18f0a2b-1234-7abc-8def-0123456789ab", "short"):
        with pytest.raises(report_module.ReportRequestError) as exc:
            report_module._validate_local_id(value)
        assert exc.value.data["code"] == "INVALID_LOCAL_ID"
    with pytest.raises(report_module.ReportRequestError):
        report_module._draft_path("../history")


def test_submit_body_contract_validation(report_module):
    local_id = "018f0a2b-1234-7abc-8def-0123456789ab"
    parsed = report_module._parse_submit_body(
        json.dumps(
            {"localID": local_id, "eventID": "event", "submissionTime": 3, "ticket": None}
        ).encode()
    )
    assert parsed == (local_id, "event", 3, True, None)
    for body in (
        {"localID": local_id, "eventID": ""},
        {"localID": local_id, "eventID": "event", "submissionTime": True},
        {"localID": local_id, "eventID": "event", "ticket": "12"},
        {"localID": local_id, "eventID": "event", "extra": 1},
    ):
        with pytest.raises(report_module.ReportRequestError) as exc:
            report_module._parse_submit_body(json.dumps(body).encode())
        assert exc.value.data["code"] == "INVALID_BODY"


def test_write_tar_zstd_cleans_atomic_temp_on_failure(report_module, monkeypatch):
    draft_dir = report_module._draft_path("draft")
    draft_dir.mkdir()
    draft_dir.joinpath("file.txt").write_text("data")
    output = report_module.DRAFT_REPORTS_DIR.joinpath("draft.zstd")

    monkeypatch.setattr(
        report_module.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(returncode=1, stderr="failed"),
    )
    with pytest.raises(RuntimeError):
        report_module._write_tar_zstd_from_draft(output, draft_dir)
    assert not output.exists()
    assert not output.with_name("draft.zstd.tmp").exists()


def test_find_debug_file_rejects_globs(report_module):
    _debug_file(report_module.DEBUG_HISTORY_ROOT, "2026-05-18", "10:00:00.shot.json.zst")
    assert report_module._find_debug_file("*") is None
    assert report_module._find_debug_file("**/*") is None
    assert report_module._find_debug_file("../x.json.zst") is None
    assert report_module._find_debug_file("10:00:00.shot.json.zst") is not None


def test_preflight_has_ordered_blockers(report_module, monkeypatch):
    class Handler:
        def get_query_arguments(self, name):
            assert name == "probe"
            return ["http://invalid", "https://unreachable"]

        def write(self, body):
            self.body = body

    async def fake_probe(url):
        return {"reachable": False, "status": None, "latencyMs": None, "error": "INVALID_PROBE"}

    monkeypatch.setattr(report_module, "_probe_url", fake_probe)
    monkeypatch.setattr(report_module, "_disk_free_bytes", lambda: 0)
    monkeypatch.setitem(
        report_module.MeticulousConfig[report_module.CONFIG_SYSTEM],
        report_module.MACHINE_SERIAL_NUMBER,
        None,
    )
    handler = Handler()
    asyncio.run(report_module.ReportsPreflightHandler.get(handler))
    assert handler.body["blockers"] == [
        "NO_SERIAL_NUMBER",
        "INSUFFICIENT_DISK_SPACE",
        "NETWORK_UNREACHABLE",
    ]
    assert handler.body["network"]["http://invalid"]["error"] == "INVALID_PROBE"


def test_upload_diagnostics_survive_report_archive(report_module, tmp_path, monkeypatch):
    monkeypatch.setenv("CONFIG_PATH", str(tmp_path))
    source = tmp_path / "community-upload" / "diagnostics.json"
    source.parent.mkdir()
    source.write_text(
        json.dumps({"schemaVersion": 1, "pendingCount": 7, "privateSeed": "SECRET"})
    )

    async def logs(*args):
        return "[CommunityUpload] upload_resumed"

    async def status(*args):
        return "{}"

    monkeypatch.setattr(report_module, "_get_machine_info", lambda: {})
    monkeypatch.setattr(report_module, "_fetch_machine_logs", logs)
    monkeypatch.setattr(report_module, "_fetch_machine_status", status)
    draft = report_module._draft_path("upload-diagnostics-test")
    fetched = asyncio.run(
        report_module._fetch_report_files(draft, capture_active_debug_shot=False)
    )
    name = report_module.community_upload_diagnostics.NAME
    assert name in fetched.files
    (draft / report_module.REPORT_INFO_NAME).write_text("{}")
    archive = tmp_path / "report.tar.zst"
    report_module._write_tar_zstd_from_draft(archive, draft)
    _, files, extracted = report_module._read_tar_zstd(archive)
    try:
        payload = files[name].read_text()
        assert "SECRET" not in payload
        assert json.loads(payload)["pendingCount"] == 7
        assert "upload_resumed" in files[report_module.MACHINE_LOGS_NAME].read_text()
    finally:
        extracted.cleanup()


# Reports handed over by the mobile app: request (mint a localID), dispatch (store the
# ticket and contact details as a `queued` row), then create by localID (collect).

MACHINE_SERIAL = "machine-test-id"
QUEUED_ID = "018f0a2b-1234-7abc-8def-0123456789ab"
SECOND_ID = "018f0a2b-1234-7abc-8def-0123456789ac"
THIRD_ID = "018f0a2b-1234-7abc-8def-0123456789ad"
DISPATCHED_AT = 1_700_000_000
HOUR = 60 * 60
AUTOMATIC_DEBUG_FILE = "2026-05-18/10:00:00.shot.json.zst"


class _FakeSio:
    def __init__(self):
        self.calls = []

    async def emit(self, event, data):
        self.calls.append((event, data))


class _FailingSio:
    async def emit(self, event, data):
        raise RuntimeError("socket down")


class _ApiHandler:
    """Stands in for a Tornado handler and records what the API wrote."""

    def __init__(self, report_module, body=b""):
        self.request = SimpleNamespace(body=body)
        self._cancellation = report_module.CollectionCancellation()
        self.status = None
        self.response = None
        self.writes = 0

    def set_status(self, status):
        self.status = status

    def write(self, body):
        self.writes += 1
        self.response = body


class _Collector:
    """Replaces `_fetch_report_files` and records how the create handler called it."""

    def __init__(self, report_module):
        self.report_module = report_module
        self.calls = []

    async def __call__(self, draft_dir, *args, **kwargs):
        self.calls.append((args, kwargs))
        draft_dir.mkdir(parents=True, exist_ok=True)
        machine_status = draft_dir.joinpath(self.report_module.MACHINE_STATUS_NAME)
        machine_status.write_text('{"ok": true}', encoding="utf-8")
        return self.report_module.FetchResult(
            files={self.report_module.MACHINE_STATUS_NAME: machine_status},
            automatic_debug_files=[AUTOMATIC_DEBUG_FILE],
            machine_status=True,
        )


@pytest.fixture
def machine_serial(report_module, monkeypatch):
    monkeypatch.setitem(
        report_module.MeticulousConfig[report_module.CONFIG_SYSTEM],
        report_module.MACHINE_SERIAL_NUMBER,
        MACHINE_SERIAL,
    )
    return MACHINE_SERIAL


@pytest.fixture
def clock(report_module, monkeypatch):
    state = SimpleNamespace(now=DISPATCHED_AT)
    monkeypatch.setattr(report_module, "_now_seconds", lambda: state.now)
    return state


@pytest.fixture
def fake_sio(report_module, monkeypatch):
    sio = _FakeSio()
    monkeypatch.setattr(report_module, "_sio", sio)
    return sio


@pytest.fixture
def collector(report_module, monkeypatch):
    fake = _Collector(report_module)
    monkeypatch.setattr(report_module, "_fetch_report_files", fake)
    return fake


def _all_report_rows():
    with ShotDataBase.engine.connect() as connection:
        return connection.execute(
            select(bug_reports).order_by(bug_reports.c.creationTime, bug_reports.c.localID)
        ).all()


def _report_row(local_id: str):
    with ShotDataBase.engine.connect() as connection:
        return connection.execute(
            select(bug_reports).where(bug_reports.c.localID == local_id)
        ).first()


def _insert_report_row(
    local_id: str, status: str, creation_time: int = DISPATCHED_AT, **values
):
    row = {
        "localID": local_id,
        "issueTime": creation_time,
        "creationTime": creation_time,
        "status": status,
        **values,
    }
    with ShotDataBase.engine.begin() as connection:
        connection.execute(insert(bug_reports).values(**row))


def _dispatch_body(local_id: str = QUEUED_ID, **fields):
    return {"localID": local_id, "ticket": 4242, **fields}


def _post_dispatch(report_module, body):
    handler = _ApiHandler(report_module, json.dumps(body).encode())
    asyncio.run(report_module.ReportsDispatchHandler.post(handler))
    return handler


def _post_create(report_module, body):
    handler = _ApiHandler(report_module, json.dumps(body).encode())
    asyncio.run(report_module.ReportsCreateHandler.post(handler))
    return handler


def _assert_api_error(handler, status: int, code: str):
    assert handler.status == status
    assert handler.response["data"]["code"] == code


def test_dispatch_routes_are_registered(report_module):
    from api.api import API, APIVersion

    routes = API._versions[APIVersion.V1]

    assert routes["/reports/request"][0] is report_module.ReportsRequestHandler
    assert routes["/reports/dispatch"][0] is report_module.ReportsDispatchHandler


@pytest.mark.parametrize(
    "body, expected",
    [
        (b"", (None, None)),
        (b'{"issueTime": 123}', (123, None)),
        (b'{"issueTime": 0}', (0, None)),
        (json.dumps({"localID": QUEUED_ID}).encode(), (None, QUEUED_ID)),
    ],
)
def test_parse_create_body_accepts_each_documented_shape(report_module, body, expected):
    assert report_module._parse_create_body(body) == expected
    assert report_module._create_report_issue_time(body) == expected[0]


@pytest.mark.parametrize(
    "body",
    [
        b"{}",
        json.dumps({"issueTime": 1, "localID": QUEUED_ID}).encode(),
        json.dumps({"localID": QUEUED_ID, "extra": 1}).encode(),
        b'{"issueTime": 1, "extra": 1}',
        b'{"extra": 1}',
        b"[]",
        b'"text"',
        b"{",
        b'{"issueTime": true}',
        b'{"issueTime": 1.5}',
        b'{"issueTime": "1"}',
        b'{"issueTime": null}',
    ],
)
def test_parse_create_body_rejects_everything_else(report_module, body):
    with pytest.raises(ValueError):
        report_module._parse_create_body(body)
    with pytest.raises(ValueError):
        report_module._create_report_issue_time(body)


@pytest.mark.parametrize("local_id", ["not-a-uuid", "../history", QUEUED_ID.upper(), 123, None])
def test_parse_create_body_rejects_malformed_local_id(report_module, local_id):
    with pytest.raises(report_module.ReportRequestError) as exc:
        report_module._parse_create_body(json.dumps({"localID": local_id}).encode())

    assert exc.value.status == 400
    assert exc.value.data["code"] == "INVALID_LOCAL_ID"


def test_request_handler_mints_local_id_and_stores_nothing(report_module, machine_serial):
    first = _ApiHandler(report_module)
    second = _ApiHandler(report_module)

    asyncio.run(report_module.ReportsRequestHandler.post(first))
    asyncio.run(report_module.ReportsRequestHandler.post(second))

    for handler in (first, second):
        assert handler.status is None
        assert set(handler.response) == {"localID", "machineID"}
        assert report_module.LOCAL_ID_RE.fullmatch(handler.response["localID"])
        assert handler.response["machineID"] == machine_serial
    assert first.response["localID"] != second.response["localID"]
    assert _all_report_rows() == []
    assert list(report_module.DRAFT_REPORTS_DIR.iterdir()) == []


@pytest.mark.parametrize(
    "body", [b"{}", b"garbage", json.dumps({"localID": QUEUED_ID}).encode()]
)
def test_request_handler_rejects_any_body(report_module, machine_serial, body):
    handler = _ApiHandler(report_module, body)

    asyncio.run(report_module.ReportsRequestHandler.post(handler))

    _assert_api_error(handler, 400, "INVALID_BODY")
    assert _all_report_rows() == []


def test_parse_dispatch_body_accepts_a_full_body(report_module):
    body = {
        "localID": QUEUED_ID,
        "ticket": 42,
        "issueTime": 1_700_000_000,
        "description": "Espresso is slow",
        "name": "Ada",
        "email": "ada@example.com",
    }

    assert report_module._parse_dispatch_body(json.dumps(body).encode()) == body


def test_parse_dispatch_body_fills_missing_optionals_with_none(report_module):
    parsed = report_module._parse_dispatch_body(
        json.dumps({"localID": QUEUED_ID, "ticket": 42}).encode()
    )

    assert parsed == {
        "localID": QUEUED_ID,
        "ticket": 42,
        "issueTime": None,
        "description": None,
        "name": None,
        "email": None,
    }


def test_parse_dispatch_body_trims_text_fields(report_module):
    parsed = report_module._parse_dispatch_body(
        json.dumps(
            {
                "localID": QUEUED_ID,
                "ticket": 42,
                "description": "  Espresso is slow \n",
                "name": "  Ada ",
                "email": " ada@example.com ",
            }
        ).encode()
    )

    assert parsed["description"] == "Espresso is slow"
    assert parsed["name"] == "Ada"
    assert parsed["email"] == "ada@example.com"


def test_parse_dispatch_body_maps_empty_text_fields_to_none(report_module):
    parsed = report_module._parse_dispatch_body(
        json.dumps(
            {
                "localID": QUEUED_ID,
                "ticket": 42,
                "issueTime": None,
                "description": "   ",
                "name": "",
                "email": None,
            }
        ).encode()
    )

    assert parsed["issueTime"] is None
    assert parsed["description"] is None
    assert parsed["name"] is None
    assert parsed["email"] is None


def test_parse_dispatch_body_accepts_text_at_the_limits(report_module):
    parsed = report_module._parse_dispatch_body(
        json.dumps(
            {
                "localID": QUEUED_ID,
                "ticket": 42,
                "description": "d" * 10_000,
                "name": "n" * 200,
                "email": "e" * 249 + "@b.cd",
            }
        ).encode()
    )

    assert len(parsed["description"]) == 10_000
    assert len(parsed["name"]) == 200
    assert len(parsed["email"]) == 254


@pytest.mark.parametrize(
    "body, status, code",
    [
        ({"localID": QUEUED_ID, "ticket": 42, "email": "not-an-email"}, 400, "INVALID_BODY"),
        (
            {"localID": QUEUED_ID, "ticket": 42, "email": "e" * 250 + "@b.cd"},
            400,
            "INVALID_BODY",
        ),
        ({"localID": QUEUED_ID, "ticket": 42, "email": 7}, 400, "INVALID_BODY"),
        ({"localID": QUEUED_ID, "ticket": True}, 400, "INVALID_BODY"),
        ({"localID": QUEUED_ID, "ticket": "42"}, 400, "INVALID_BODY"),
        ({"localID": QUEUED_ID, "ticket": 4.5}, 400, "INVALID_BODY"),
        ({"localID": QUEUED_ID, "ticket": None}, 400, "INVALID_BODY"),
        ({"localID": QUEUED_ID}, 400, "INVALID_BODY"),
        ({"localID": QUEUED_ID, "ticket": 42, "issueTime": True}, 400, "INVALID_BODY"),
        ({"localID": QUEUED_ID, "ticket": 42, "issueTime": "1"}, 400, "INVALID_BODY"),
        ({"localID": QUEUED_ID, "ticket": 42, "issueTime": 1.5}, 400, "INVALID_BODY"),
        ({"localID": QUEUED_ID, "ticket": 42, "unknown": 1}, 400, "INVALID_BODY"),
        (
            {"localID": QUEUED_ID, "ticket": 42, "description": "d" * 10_001},
            400,
            "INVALID_BODY",
        ),
        ({"localID": QUEUED_ID, "ticket": 42, "description": 5}, 400, "INVALID_BODY"),
        ({"localID": QUEUED_ID, "ticket": 42, "name": "n" * 201}, 400, "INVALID_BODY"),
        ({"localID": "not-a-uuid", "ticket": 42}, 400, "INVALID_LOCAL_ID"),
        ({"localID": QUEUED_ID.upper(), "ticket": 42}, 400, "INVALID_LOCAL_ID"),
        ({"localID": None, "ticket": 42}, 400, "INVALID_LOCAL_ID"),
        ({"ticket": 42}, 400, "INVALID_LOCAL_ID"),
    ],
)
def test_parse_dispatch_body_rejects_invalid_bodies(report_module, body, status, code):
    with pytest.raises(report_module.ReportRequestError) as exc:
        report_module._parse_dispatch_body(json.dumps(body).encode())

    assert exc.value.status == status
    assert exc.value.data["code"] == code


@pytest.mark.parametrize("body", [b"[]", b"{", b'"text"'])
def test_parse_dispatch_body_rejects_non_object_json(report_module, body):
    with pytest.raises(report_module.ReportRequestError) as exc:
        report_module._parse_dispatch_body(body)

    assert exc.value.status == 400
    assert exc.value.data["code"] == "INVALID_BODY"


def test_dispatch_post_queues_the_report_and_emits_upload_report(
    report_module, machine_serial, clock, fake_sio
):
    issue_time = DISPATCHED_AT - 600

    handler = _post_dispatch(
        report_module,
        _dispatch_body(
            issueTime=issue_time,
            description="Grinder jammed",
            name="Ada",
            email="ada@example.com",
        ),
    )

    expected = {
        "localID": QUEUED_ID,
        "machineID": machine_serial,
        "ticket": 4242,
        "issueTime": issue_time,
        "requestTime": DISPATCHED_AT,
        "description": "Grinder jammed",
        "name": "Ada",
        "email": "ada@example.com",
    }
    assert handler.status == 202
    assert handler.response == expected
    assert fake_sio.calls == [(report_module.UPLOAD_REPORT_EVENT, expected)]
    assert report_module.UPLOAD_REPORT_EVENT == "upload_report"
    (row,) = _all_report_rows()
    assert row.localID == QUEUED_ID
    assert row.status == "queued"
    assert row.ticketNumber == 4242
    assert row.issueTime == issue_time
    assert row.creationTime == DISPATCHED_AT
    assert row.submissionTime is None
    assert row.description == "Grinder jammed"
    assert row.machineID == machine_serial
    assert row.contactName == "Ada"
    assert row.contactEmail == "ada@example.com"
    assert row.eventID is None
    assert row.logFiles is None
    assert row.machineStatus is None


def test_dispatch_post_defaults_issue_time_to_now_and_contact_to_none(
    report_module, machine_serial, clock, fake_sio
):
    handler = _post_dispatch(report_module, _dispatch_body())

    assert handler.status == 202
    assert handler.response == {
        "localID": QUEUED_ID,
        "machineID": machine_serial,
        "ticket": 4242,
        "issueTime": DISPATCHED_AT,
        "requestTime": DISPATCHED_AT,
        "description": None,
        "name": None,
        "email": None,
    }
    assert fake_sio.calls == [("upload_report", handler.response)]
    (row,) = _all_report_rows()
    assert row.issueTime == DISPATCHED_AT
    assert row.creationTime == DISPATCHED_AT
    assert row.contactName is None
    assert row.contactEmail is None


def test_dispatch_post_with_the_same_local_id_is_a_conflict(
    report_module, machine_serial, clock, fake_sio
):
    first = _post_dispatch(report_module, _dispatch_body(description="first"))
    clock.now += 60

    second = _post_dispatch(
        report_module, _dispatch_body(ticket=9999, description="second", name="Eve")
    )

    assert first.status == 202
    _assert_api_error(second, 409, "DUPLICATE_LOCAL_ID")
    assert len(fake_sio.calls) == 1
    (row,) = _all_report_rows()
    assert row.ticketNumber == 4242
    assert row.description == "first"
    assert row.contactName is None
    assert row.creationTime == DISPATCHED_AT


@pytest.mark.parametrize("status", ["draft", "submitted"])
def test_dispatch_post_conflicts_with_a_report_in_any_status(
    report_module, machine_serial, clock, fake_sio, status
):
    _insert_report_row(QUEUED_ID, status, machineID=machine_serial)

    handler = _post_dispatch(report_module, _dispatch_body())

    _assert_api_error(handler, 409, "DUPLICATE_LOCAL_ID")
    assert fake_sio.calls == []
    (row,) = _all_report_rows()
    assert row.status == status


def test_dispatch_post_without_a_socket_server_still_queues_the_report(
    report_module, machine_serial, clock, monkeypatch
):
    monkeypatch.setattr(report_module, "_sio", None)

    handler = _post_dispatch(report_module, _dispatch_body())

    assert handler.status == 202
    assert handler.response["localID"] == QUEUED_ID
    (row,) = _all_report_rows()
    assert row.status == "queued"


def test_dispatch_post_survives_a_failing_socket_emit(
    report_module, machine_serial, clock, monkeypatch
):
    monkeypatch.setattr(report_module, "_sio", _FailingSio())

    handler = _post_dispatch(report_module, _dispatch_body())

    assert handler.status == 202
    (row,) = _all_report_rows()
    assert row.status == "queued"


def test_init_socket_registers_the_server_used_for_emits(
    report_module, machine_serial, clock, monkeypatch
):
    monkeypatch.setattr(report_module, "_sio", None)
    sio = _FakeSio()

    report_module.init_socket(sio)
    _post_dispatch(report_module, _dispatch_body())

    assert report_module._sio is sio
    assert [event for event, _ in sio.calls] == ["upload_report"]


@pytest.mark.parametrize(
    "body",
    [
        {"localID": QUEUED_ID},
        {"localID": QUEUED_ID, "ticket": 42, "email": "not-an-email"},
        {"localID": "nope", "ticket": 42},
    ],
)
def test_dispatch_post_rejects_invalid_bodies_without_side_effects(
    report_module, machine_serial, clock, fake_sio, body
):
    handler = _post_dispatch(report_module, body)

    assert handler.status == 400
    assert handler.response["data"]["code"] in {"INVALID_BODY", "INVALID_LOCAL_ID"}
    assert fake_sio.calls == []
    assert _all_report_rows() == []


def test_dispatch_get_lists_only_queued_reports_oldest_first(report_module, machine_serial):
    # Inserted out of order; ties on creationTime fall back to the localID.
    _insert_report_row(
        SECOND_ID,
        "queued",
        DISPATCHED_AT + 10,
        issueTime=DISPATCHED_AT + 5,
        ticketNumber=2,
        machineID=machine_serial,
        description="later",
    )
    _insert_report_row(
        "018f0a2b-1234-7abc-8def-0123456789ae",
        "draft",
        DISPATCHED_AT - 5,
        ticketNumber=3,
        contactName="Draft",
    )
    _insert_report_row(
        THIRD_ID,
        "queued",
        DISPATCHED_AT,
        ticketNumber=4,
        machineID=machine_serial,
        contactName="Ada",
        contactEmail="ada@example.com",
    )
    _insert_report_row(
        "018f0a2b-1234-7abc-8def-0123456789af",
        "submitted",
        DISPATCHED_AT - 10,
        ticketNumber=5,
        contactName="Done",
    )
    _insert_report_row(
        QUEUED_ID,
        "queued",
        DISPATCHED_AT,
        issueTime=DISPATCHED_AT - 600,
        ticketNumber=1,
        machineID=machine_serial,
    )
    handler = _ApiHandler(report_module)

    asyncio.run(report_module.ReportsDispatchHandler.get(handler))

    assert handler.status is None
    assert handler.response == {
        "content": [
            {
                "localID": QUEUED_ID,
                "machineID": machine_serial,
                "ticket": 1,
                "issueTime": DISPATCHED_AT - 600,
                "requestTime": DISPATCHED_AT,
                "description": None,
                "name": None,
                "email": None,
            },
            {
                "localID": THIRD_ID,
                "machineID": machine_serial,
                "ticket": 4,
                "issueTime": DISPATCHED_AT,
                "requestTime": DISPATCHED_AT,
                "description": None,
                "name": "Ada",
                "email": "ada@example.com",
            },
            {
                "localID": SECOND_ID,
                "machineID": machine_serial,
                "ticket": 2,
                "issueTime": DISPATCHED_AT + 5,
                "requestTime": DISPATCHED_AT + 10,
                "description": "later",
                "name": None,
                "email": None,
            },
        ]
    }


def test_dispatch_get_is_empty_without_queued_reports(report_module):
    _insert_report_row(QUEUED_ID, "draft")
    handler = _ApiHandler(report_module)

    asyncio.run(report_module.ReportsDispatchHandler.get(handler))

    assert handler.response == {"content": []}


def test_create_by_local_id_collects_around_the_dispatch_and_promotes_the_row(
    report_module, machine_serial, clock, fake_sio, collector, monkeypatch
):
    issue_time = DISPATCHED_AT - 600
    _post_dispatch(
        report_module,
        _dispatch_body(
            issueTime=issue_time,
            description="Grinder jammed",
            name="Ada",
            email="ada@example.com",
        ),
    )
    queued = _report_row(QUEUED_ID)

    def fail_new_local_id():
        raise AssertionError("Create by localID must reuse the dispatched localID")

    monkeypatch.setattr(report_module, "_new_local_id", fail_new_local_id)
    # More than 12 hours after the issue time: the historical range applies.
    clock.now = issue_time + 13 * HOUR
    handler = _post_create(report_module, {"localID": QUEUED_ID})

    assert handler.status is None
    assert handler.response == {"localID": QUEUED_ID, "machineID": machine_serial}
    assert len(collector.calls) == 1
    call_args, call_kwargs = collector.calls[0]
    assert call_args == report_module._collection_range(queued.issueTime, clock.now)
    assert call_args == (issue_time - 12 * HOUR, issue_time + 12 * HOUR)
    assert call_kwargs["capture_active_debug_shot"] is False
    assert call_kwargs["cancellation"] is handler._cancellation

    report_info = report_module._read_draft_report_info(report_module._draft_path(QUEUED_ID))
    assert report_info["localID"] == QUEUED_ID
    assert report_info["description"] == "Grinder jammed"
    assert report_info["ticket"] == 4242
    assert report_info["dateAndTime"] == queued.creationTime == DISPATCHED_AT
    assert report_info["issueTime"] == queued.issueTime == issue_time
    assert report_info["machineID"] == machine_serial

    (row,) = _all_report_rows()
    assert row.localID == QUEUED_ID
    assert row.status == "draft"
    assert row.ticketNumber == 4242
    assert row.contactName == "Ada"
    assert row.contactEmail == "ada@example.com"
    assert row.creationTime == DISPATCHED_AT
    assert row.issueTime == issue_time
    assert row.description == "Grinder jammed"
    assert row.machineID == machine_serial
    assert row.machineStatus is True
    assert row.logFiles == AUTOMATIC_DEBUG_FILE


def test_create_by_local_id_shortly_after_the_dispatch_uses_the_trailing_range(
    report_module, machine_serial, clock, fake_sio, collector
):
    _post_dispatch(report_module, _dispatch_body())
    clock.now = DISPATCHED_AT + 60

    handler = _post_create(report_module, {"localID": QUEUED_ID})

    assert handler.response == {"localID": QUEUED_ID, "machineID": machine_serial}
    call_args, call_kwargs = collector.calls[0]
    assert call_args == (clock.now - 24 * HOUR, clock.now)
    assert call_kwargs["capture_active_debug_shot"] is True
    assert _report_row(QUEUED_ID).status == "draft"


def test_create_by_local_id_picked_up_late_still_covers_the_dispatch_time(
    report_module, machine_serial, clock, fake_sio, collector
):
    # No issueTime in the dispatch: the issue time is the dispatch time, even when the
    # dial only gets around to collecting hours later (for example after a restart).
    _post_dispatch(report_module, _dispatch_body())
    clock.now = DISPATCHED_AT + 20 * HOUR

    _post_create(report_module, {"localID": QUEUED_ID})

    call_args, call_kwargs = collector.calls[0]
    assert call_args == (DISPATCHED_AT - 12 * HOUR, DISPATCHED_AT + 12 * HOUR)
    assert call_kwargs["capture_active_debug_shot"] is False
    row = _report_row(QUEUED_ID)
    assert row.issueTime == DISPATCHED_AT
    assert row.creationTime == DISPATCHED_AT


def test_create_by_local_id_of_a_collected_draft_answers_without_collecting(
    report_module, machine_serial, clock, collector
):
    _insert_report_row(QUEUED_ID, "draft", machineID=machine_serial, ticketNumber=4242)
    draft_dir = report_module._draft_path(QUEUED_ID)
    draft_dir.mkdir()
    marker = draft_dir.joinpath("kept.txt")
    marker.write_text("kept", encoding="utf-8")

    handler = _post_create(report_module, {"localID": QUEUED_ID})

    assert handler.status is None
    assert handler.response == {"localID": QUEUED_ID, "machineID": machine_serial}
    assert collector.calls == []
    assert marker.read_text(encoding="utf-8") == "kept"
    (row,) = _all_report_rows()
    assert row.status == "draft"
    assert row.ticketNumber == 4242


def test_create_by_unknown_local_id_is_not_found(
    report_module, machine_serial, clock, collector
):
    handler = _post_create(report_module, {"localID": QUEUED_ID})

    _assert_api_error(handler, 404, "UNKNOWN_LOCAL_ID")
    assert collector.calls == []
    assert _all_report_rows() == []
    assert list(report_module.DRAFT_REPORTS_DIR.iterdir()) == []


def test_create_by_submitted_local_id_is_a_conflict(
    report_module, machine_serial, clock, collector
):
    _insert_report_row(QUEUED_ID, "submitted", machineID=machine_serial)

    handler = _post_create(report_module, {"localID": QUEUED_ID})

    _assert_api_error(handler, 409, "DUPLICATE_LOCAL_ID")
    assert collector.calls == []
    assert _report_row(QUEUED_ID).status == "submitted"


def test_create_by_draft_local_id_without_a_directory_is_not_found(
    report_module, machine_serial, clock, collector
):
    _insert_report_row(QUEUED_ID, "draft", machineID=machine_serial)

    handler = _post_create(report_module, {"localID": QUEUED_ID})

    _assert_api_error(handler, 404, "UNKNOWN_LOCAL_ID")
    assert collector.calls == []
    assert _report_row(QUEUED_ID).status == "draft"
    assert not report_module._draft_path(QUEUED_ID).exists()


def test_create_rejects_issue_time_together_with_local_id(
    report_module, machine_serial, clock, fake_sio, collector
):
    _post_dispatch(report_module, _dispatch_body())

    handler = _post_create(report_module, {"localID": QUEUED_ID, "issueTime": 1})

    _assert_api_error(handler, 400, "INVALID_BODY")
    assert collector.calls == []
    assert _report_row(QUEUED_ID).status == "queued"


def test_create_rejects_malformed_local_id(report_module, machine_serial, clock, collector):
    handler = _post_create(report_module, {"localID": "../history"})

    _assert_api_error(handler, 400, "INVALID_LOCAL_ID")
    assert collector.calls == []


def test_create_by_local_id_cancelled_mid_collection_keeps_the_row_queued(
    report_module, machine_serial, clock, fake_sio, collector, monkeypatch
):
    _post_dispatch(report_module, _dispatch_body(name="Ada", email="ada@example.com"))
    queued = _report_row(QUEUED_ID)

    async def cancelling_fetch_report_files(draft_dir, *args, **kwargs):
        draft_dir.mkdir(parents=True, exist_ok=True)
        draft_dir.joinpath("partial.txt").write_text("partial", encoding="utf-8")
        raise asyncio.CancelledError()

    def fail_if_called(*args, **kwargs):
        raise AssertionError("Cancellation must not be treated as an error")

    monkeypatch.setattr(report_module, "_fetch_report_files", cancelling_fetch_report_files)
    monkeypatch.setattr(report_module.logger, "exception", fail_if_called)
    monkeypatch.setattr(report_module.logger, "error", fail_if_called)
    monkeypatch.setattr(report_module, "_api_error", fail_if_called)
    clock.now += HOUR

    handler = _post_create(report_module, {"localID": QUEUED_ID})

    assert not report_module._draft_path(QUEUED_ID).exists()
    assert handler.writes == 0
    assert handler.status is None
    row = _report_row(QUEUED_ID)
    assert row.status == "queued"
    assert row == queued

    # The dial can simply try again: the report is still waiting to be collected.
    monkeypatch.setattr(report_module, "_fetch_report_files", collector)
    retry = _post_create(report_module, {"localID": QUEUED_ID})

    assert retry.response == {"localID": QUEUED_ID, "machineID": machine_serial}
    assert _report_row(QUEUED_ID).status == "draft"


def test_sweep_keeps_fresh_queued_reports_and_expires_stale_ones(report_module):
    now = DISPATCHED_AT
    max_age = report_module.DRAFT_MAX_AGE_SECONDS
    _insert_report_row(QUEUED_ID, "queued", now - 60)
    _insert_report_row(SECOND_ID, "queued", now - max_age)
    _insert_report_row(THIRD_ID, "queued", now - max_age - 1)

    stats = report_module.sweep_reports(now)

    assert stats == {"tmp": 0, "drafts": 0, "rows": 1, "archives": 0, "finalized": 0}
    assert {row.localID for row in _all_report_rows()} == {QUEUED_ID, SECOND_ID}


def test_sweep_still_deletes_draft_rows_without_files(report_module):
    now = DISPATCHED_AT
    _insert_report_row(QUEUED_ID, "queued", now - 60)
    _insert_report_row(SECOND_ID, "draft", now - 60)

    stats = report_module.sweep_reports(now)

    assert stats["rows"] == 1
    assert [row.localID for row in _all_report_rows()] == [QUEUED_ID]


def test_list_report_page_includes_queued_reports_with_their_ticket(
    report_module, machine_serial, clock, fake_sio
):
    _post_dispatch(report_module, _dispatch_body())

    (listed,) = report_module._list_report_page(page=0, size=10)["content"]

    assert listed["localID"] == QUEUED_ID
    assert listed["ticket"] == 4242
    assert listed["dateAndTime"] == DISPATCHED_AT
