import asyncio
from pathlib import Path

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


def test_submit_db_update(report_module):
    with ShotDataBase.engine.begin() as connection:
        connection.execute(
            insert(bug_reports).values(
                localID="submit-id",
                issueTime=1,
                creationTime=1,
                machineInfo=False,
                machineLogs=False,
                machineStatus=False,
                status="draft",
            )
        )

    updated = report_module._update_report_db(
        "submit-id",
        {
            "eventID": "event-1",
            "ticketNumber": 42,
            "submissionTime": 3,
            "status": "submitted",
        },
    )

    with ShotDataBase.engine.connect() as connection:
        row = connection.execute(select(bug_reports)).first()
    assert updated is True
    assert row.eventID == "event-1"
    assert row.ticketNumber == 42
    assert row.submissionTime == 3
    assert row.status == "submitted"
