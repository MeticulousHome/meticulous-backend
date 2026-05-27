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

    async def fake_machine_logs(reference_time=None):
        return "logs"

    async def fake_machine_status():
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

    async def fake_machine_logs(reference_time=None):
        return "logs"

    async def fake_machine_status():
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


def test_draft_patch_preserves_user_file_when_date_changes(report_module, monkeypatch):
    old_auto = _debug_file(
        report_module.DEBUG_HISTORY_ROOT, "2026-05-17", "11:00:00.old.json.zst"
    )
    old_user = _debug_file(
        report_module.DEBUG_HISTORY_ROOT, "2026-05-17", "12:00:00.user.json.zst"
    )
    new_auto = _debug_file(
        report_module.DEBUG_HISTORY_ROOT, "2026-05-18", "13:00:00.new.json.zst"
    )
    old_auto_name = report_module._safe_archive_name(old_auto)
    old_user_name = report_module._safe_archive_name(old_user)
    new_auto_name = report_module._safe_archive_name(new_auto)

    async def fake_machine_logs(reference_time=None):
        return "new logs"

    monkeypatch.setattr(report_module, "_fetch_machine_logs", fake_machine_logs)
    monkeypatch.setattr(
        report_module,
        "_select_debug_files",
        lambda limit=10, reference_time=None: ([new_auto], []),
    )

    local_id = "local-test-id"
    draft_dir = report_module._draft_path(local_id)
    draft_dir.mkdir(parents=True)
    report_info = {
        "description": None,
        "dateAndTime": 1,
        "attachments": {
            "debugFiles": {
                "automatic": [old_auto_name, old_user_name],
                "user": [old_user_name],
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
    }
    report_module._copy_draft_file(
        draft_dir, report_module._debug_archive_name(old_auto_name), old_auto
    )
    report_module._copy_draft_file(
        draft_dir, report_module._debug_archive_name(old_user_name), old_user
    )
    report_module._write_draft_report_info(draft_dir, report_info)
    with ShotDataBase.engine.begin() as connection:
        connection.execute(
            insert(bug_reports).values(
                localID=local_id,
                issueTime=1,
                creationTime=1,
                logFiles=f"{old_auto_name},{old_user_name}",
                machineInfo=True,
                machineLogs=True,
                machineStatus=True,
                status="draft",
            )
        )

    updated = asyncio.run(report_module._apply_draft_patch(local_id, {"dateAndTime": 2}))
    archived_info = report_module._read_draft_report_info(draft_dir)
    archived_names = set(report_module._draft_files(draft_dir).keys())

    assert updated["attachments"]["debugFiles"]["automatic"] == [new_auto_name]
    assert report_module._debug_archive_name(old_auto_name) not in archived_names
    assert report_module._debug_archive_name(old_user_name) in archived_names
    assert report_module._debug_archive_name(new_auto_name) in archived_names
    assert archived_info["dateAndTime"] == 2

    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()
    assert row.issueTime == 2
    assert old_user_name in row.logFiles
    assert new_auto_name in row.logFiles


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
    async def fake_fetch_report_files(draft_dir):
        draft_dir.mkdir(parents=True, exist_ok=True)
        machine_status = draft_dir.joinpath(report_module.MACHINE_STATUS_NAME)
        machine_status.write_text('{"ok": true}', encoding="utf-8")
        return report_module.FetchResult(
            files={report_module.MACHINE_STATUS_NAME: machine_status},
            machine_status=True,
        )

    class FakeHandler:
        request = SimpleNamespace(body=b"")

        def write(self, body):
            self.body = body

    monkeypatch.setattr(report_module, "_new_local_id", lambda: "local-test-id")
    monkeypatch.setattr(report_module, "_now_seconds", lambda: 1)
    monkeypatch.setattr(report_module, "_fetch_report_files", fake_fetch_report_files)
    monkeypatch.setattr(
        report_module.HostnameManager, "generateHostname", lambda: "machine-test-id"
    )

    handler = FakeHandler()
    asyncio.run(report_module.ReportsCreateHandler.post(handler))
    report_info = report_module._read_draft_report_info(
        report_module._draft_path("local-test-id")
    )

    assert handler.body == {"localID": "local-test-id", "machineID": "machine-test-id"}
    assert report_info["machineID"] == handler.body["machineID"]
    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()
    assert row.localID == "local-test-id"
    assert row.machineID == "machine-test-id"
    assert row.machineStatus is True


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
    archived_info = report_module._read_draft_report_info(draft_dir)
    assert updated is True
    assert row.eventID == "event-1"
    assert row.ticketNumber == 42
    assert row.submissionTime == 3
    assert row.status == "submitted"
    assert archived_info["eventID"] == "event-1"
    assert archived_info["ticket"] == 42
    assert archived_info["multimedia"] == 1


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
    archive_path = report_module.DRAFT_REPORTS_DIR.joinpath("out.zstd")
    report_module._write_tar_zstd_from_draft(archive_path, draft_dir)

    archived_info, archived_names = _read_archive_report_info(report_module, archive_path)
    assert report_module.REPORT_INFO_NAME not in archived_names
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
    archived_info = report_module._read_draft_report_info(draft_dir)
    assert updated is True
    assert row.eventID == "event-1"
    assert row.ticketNumber == 42
    assert archived_info["eventID"] == "event-1"
    assert archived_info["ticket"] == 42
