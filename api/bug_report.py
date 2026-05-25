import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
import time
import uuid_utils as uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import unquote_plus
from hostname import HostnameManager

import tornado.httpclient
from sqlalchemy import and_, desc, func, insert, or_, select, update

from config import DATABASE_URL, DEBUG_HISTORY_PATH
from database_models import bug_reports
from log import MeticulousLogger
from shot_database import ShotDataBase

from .api import API, APIVersion
from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)

REPORTS_DIR = Path(os.getenv("REPORTS_DIR", "/meticulous-user/reports"))
DRAFT_REPORTS_DIR = Path(os.getenv("DRAFT_REPORTS_DIR", str(REPORTS_DIR.joinpath("draft"))))
DEBUG_HISTORY_ROOT = Path(DEBUG_HISTORY_PATH)
WATCHER_LOGS_URL = os.getenv("WATCHER_LOGS_URL", "http://localhost/health/logs?filter=*")
WATCHER_STATUS_URL = os.getenv("WATCHER_STATUS_URL", "http://localhost/health/status")
REPORT_INFO_NAME = "report_info.json"
REPORT_LOG_NAME = "logs_while_reporting.txt"
MACHINE_INFO_NAME = "machine_info.json"
MACHINE_LOGS_NAME = "machine_logs.txt"
MACHINE_STATUS_NAME = "machine_status.json"
DEBUG_ARCHIVE_DIR = "debug"
ALLOWED_DRAFT_UPDATE_KEYS = {"description", "dateAndTime", "baseEventID", "attachments"}


@dataclass
class FetchResult:
    files: dict[str, Path] = field(default_factory=dict)
    errors: list[str] = field(default_factory=list)
    automatic_debug_files: list[str] = field(default_factory=list)
    machine_info: bool = False
    machine_logs: bool = False
    machine_status: bool = False


def _ensure_database_initialized():
    if ShotDataBase.engine is None:
        from sqlalchemy import create_engine

        ShotDataBase.engine = create_engine(
            DATABASE_URL, echo=False, connect_args={"check_same_thread": False}
        )


def _api_error(handler: BaseHandler, status_code: int, error: str, context: dict | None = None):
    handler.set_status(status_code)
    body = {"error": error, "description": ""}
    if context is not None:
        if isinstance(context, dict):
            body["data"] = context
        else:
            body["description"] = f"{context}"
    handler.write(body)


def _now_seconds() -> int:
    return int(time.time())


def _get_machine_info() -> dict[str, Any]:
    from .machine import get_machine_info

    return get_machine_info()


def _new_local_id() -> str:
    return str(uuid.uuid7())


def _json_loads_body(body: bytes) -> dict[str, Any]:
    try:
        data = json.loads(body.decode("utf-8") if body else "{}")
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"Invalid JSON: {exc}") from exc
    if not isinstance(data, dict):
        raise ValueError("Request body must be a JSON object")
    return data


def _draft_path(local_id: str) -> Path:
    return DRAFT_REPORTS_DIR.joinpath(local_id)


def _safe_draft_file_path(draft_dir: Path, archive_name: str) -> Path:
    target = draft_dir.joinpath(archive_name).resolve()
    try:
        target.relative_to(draft_dir.resolve())
    except ValueError as exc:
        raise RuntimeError(f"Unsafe draft member: {archive_name}") from exc
    return target


def _safe_archive_name(path: Path) -> str:
    return f"{path.parent.name}/{path.name}"


def _debug_archive_name(file_name: str) -> str:
    return f"{DEBUG_ARCHIVE_DIR}/{file_name}"


def _dedupe(values: list[str]) -> list[str]:
    seen = set()
    result = []
    for value in values:
        if value not in seen:
            seen.add(value)
            result.append(value)
    return result


def _attachments_to_log_files(attachments: dict[str, Any] | None) -> str | None:
    if not attachments:
        return None
    debug_files = attachments.get("debugFiles") or {}
    names = []
    names.extend(debug_files.get("user") or [])
    names.extend(debug_files.get("automatic") or [])
    unique_names = _dedupe([str(name) for name in names if name])
    return ",".join(unique_names) if unique_names else None


def _row_to_report_info(row) -> dict[str, Any]:
    log_files = [name for name in (row.logFiles or "").split(",") if name]
    return {
        "description": row.description,
        "dateAndTime": row.issueTime,
        "attachments": {
            "debugFiles": {"automatic": log_files},
            "machineInfo": bool(row.machineInfo),
            "machineLogs": bool(row.machineLogs),
            "machineStatus": bool(row.machineStatus),
        },
        "multimedia": None,
        "machineID": None,
        "eventID": row.eventID,
        "baseEventID": row.baseEventID,
        "ticket": row.ticketNumber,
        "localID": row.localID,
    }


def _read_tar_zstd(
    archive_path: Path,
) -> tuple[dict[str, Any], dict[str, Path], tempfile.TemporaryDirectory]:
    temp_dir_obj = tempfile.TemporaryDirectory()
    temp_path = Path(temp_dir_obj.name)
    tar_path = temp_path.joinpath("draft.tar")
    with tar_path.open("wb") as tar_handle:
        result = subprocess.run(
            ["zstd", "-d", "-f", "-q", "-c", str(archive_path)],
            stdout=tar_handle,
            stderr=subprocess.PIPE,
            text=False,
            check=False,
        )
    if result.returncode != 0:
        temp_dir_obj.cleanup()
        stderr = result.stderr.decode("utf-8", errors="replace") if result.stderr else ""
        raise RuntimeError(stderr or "zstd decompression failed")

    extract_dir = temp_path.joinpath("extract")
    extract_dir.mkdir()
    with tarfile.open(tar_path, "r") as archive:
        for member in archive.getmembers():
            target = extract_dir.joinpath(member.name).resolve()
            try:
                target.relative_to(extract_dir.resolve())
            except ValueError:
                raise RuntimeError(f"Unsafe archive member: {member.name}")
        archive.extractall(extract_dir)

    report_info_path = extract_dir.joinpath(REPORT_INFO_NAME)
    report_info = json.loads(report_info_path.read_text(encoding="utf-8"))
    files = {
        str(path.relative_to(extract_dir)): path
        for path in extract_dir.rglob("*")
        if path.is_file() and path.name != REPORT_INFO_NAME
    }
    return report_info, files, temp_dir_obj


def _write_tar_zstd_from_draft(output_path: Path, draft_dir: Path):
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temp_dir:
        tar_path = Path(temp_dir).joinpath("draft.tar")

        with tarfile.open(tar_path, "w") as archive:
            for source_path in sorted(draft_dir.rglob("*")):
                if source_path.is_file():
                    archive.add(source_path, arcname=str(source_path.relative_to(draft_dir)))

        result = subprocess.run(
            ["zstd", "-10", "-f", "-q", "-o", str(output_path), str(tar_path)],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            raise RuntimeError(result.stderr or "zstd compression failed")


def _read_draft_report_info(draft_dir: Path) -> dict[str, Any]:
    return json.loads(draft_dir.joinpath(REPORT_INFO_NAME).read_text(encoding="utf-8"))


def _write_draft_report_info(draft_dir: Path, report_info: dict[str, Any]):
    draft_dir.joinpath(REPORT_INFO_NAME).write_text(
        json.dumps(report_info, ensure_ascii=False), encoding="utf-8"
    )


def _draft_files(draft_dir: Path) -> dict[str, Path]:
    return {
        str(path.relative_to(draft_dir)): path
        for path in draft_dir.rglob("*")
        if path.is_file() and path.name != REPORT_INFO_NAME
    }


def _append_reporting_log_to_dir(draft_dir: Path, errors: list[str]):
    if not errors:
        return
    log_path = draft_dir.joinpath(REPORT_LOG_NAME)
    with log_path.open("a", encoding="utf-8") as handle:
        for error in errors:
            handle.write(f"{datetime.now(timezone.utc).isoformat()} {error}\n")


def _copy_draft_file(draft_dir: Path, archive_name: str, source_path: Path) -> Path:
    target = _safe_draft_file_path(draft_dir, archive_name)
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source_path, target)
    return target


def _remove_draft_file(draft_dir: Path, archive_name: str):
    target = _safe_draft_file_path(draft_dir, archive_name)
    if target.exists() and target.is_file():
        target.unlink()


def _find_debug_file(file_name: str) -> Path | None:
    candidate = DEBUG_HISTORY_ROOT.joinpath(file_name)
    if candidate.exists() and candidate.is_file():
        return candidate
    matches = list(DEBUG_HISTORY_ROOT.rglob(file_name))
    return matches[0] if matches else None


def _select_debug_files(
    limit: int = 10, reference_time: int | None = None
) -> tuple[list[Path], list[str]]:
    debug_root = DEBUG_HISTORY_ROOT
    if not debug_root.exists():
        return [], [f"Debug history directory does not exist: {debug_root}"]

    selected = []
    errors = []
    if reference_time is None:
        directories = sorted(
            [path for path in debug_root.iterdir() if path.is_dir()],
            key=lambda path: path.name,
            reverse=True,
        )
        for directory in directories:
            for path in sorted(directory.iterdir(), key=lambda item: item.name, reverse=True):
                if path.is_file():
                    selected.append(path)
                    if len(selected) >= limit:
                        return selected, errors
    else:
        ref_dt = datetime.fromtimestamp(reference_time)
        if (
            ref_dt.hour == 0
            and ref_dt.minute == 0
            and ref_dt.second == 0
            and ref_dt.microsecond == 0
        ):
            date_dir = debug_root.joinpath(ref_dt.strftime("%Y-%m-%d"))
            candidates = (
                sorted(date_dir.iterdir(), key=lambda item: item.name, reverse=True)
                if date_dir.exists()
                else []
            )
            selected.extend([path for path in candidates if path.is_file()][:limit])
        else:
            end_ts = min(_now_seconds(), reference_time + (3 * 60 * 60))
            start_ts = end_ts - (24 * 60 * 60)
            for path in debug_root.rglob("*"):
                if not path.is_file():
                    continue
                try:
                    day = path.parent.name
                    clock = path.name.split(".", 1)[0]
                    file_dt = datetime.strptime(f"{day} {clock}", "%Y-%m-%d %H:%M:%S")
                    file_ts = int(file_dt.timestamp())
                except ValueError:
                    continue
                if start_ts <= file_ts <= end_ts:
                    selected.append(path)
            selected.sort(key=lambda item: (item.parent.name, item.name), reverse=True)
            selected = selected[:limit]

    if len(selected) < limit:
        errors.append(
            f"Only found {len(selected)} debug files while reporting; requested {limit}."
        )
    return selected, errors


async def _fetch_machine_logs(reference_time: int | None = None) -> str:
    url = WATCHER_LOGS_URL
    if reference_time is not None:
        ceiling = min(_now_seconds(), reference_time + (3 * 60 * 60))
        start_hours = max(0, int((time.time() - (ceiling - 24 * 60 * 60) + 3599) // 3600))
        end_hours = max(0, int((time.time() - ceiling) // 3600))
        separator = "&" if "?" in url else "?"
        url = f"{url}{separator}since={start_hours}&until={end_hours}"
    client = tornado.httpclient.AsyncHTTPClient()
    response = await client.fetch(url, request_timeout=60)
    return response.body.decode("utf-8", errors="replace")


async def _fetch_machine_status() -> str:
    client = tornado.httpclient.AsyncHTTPClient()
    response = await client.fetch(WATCHER_STATUS_URL, request_timeout=60)
    return response.body.decode("utf-8", errors="replace")


async def _fetch_report_files(
    draft_dir: Path, reference_time: int | None = None
) -> FetchResult:
    result = FetchResult()
    draft_dir.mkdir(parents=True, exist_ok=True)

    try:
        machine_info_path = draft_dir.joinpath(MACHINE_INFO_NAME)
        machine_info_path.write_text(
            json.dumps(_get_machine_info(), ensure_ascii=False), encoding="utf-8"
        )
        result.files[MACHINE_INFO_NAME] = machine_info_path
        result.machine_info = True
    except Exception as exc:
        result.errors.append(f"Failed to fetch machine info: {exc}")

    try:
        machine_logs_path = draft_dir.joinpath(MACHINE_LOGS_NAME)
        machine_logs_path.write_text(
            await _fetch_machine_logs(reference_time), encoding="utf-8"
        )
        result.files[MACHINE_LOGS_NAME] = machine_logs_path
        result.machine_logs = True
    except Exception as exc:
        result.errors.append(f"Failed to fetch machine logs: {exc}")

    try:
        machine_status_path = draft_dir.joinpath(MACHINE_STATUS_NAME)
        machine_status_path.write_text(await _fetch_machine_status(), encoding="utf-8")
        result.files[MACHINE_STATUS_NAME] = machine_status_path
        result.machine_status = True
    except Exception as exc:
        result.errors.append(f"Failed to fetch machine status: {exc}")

    debug_files, debug_errors = _select_debug_files(reference_time=reference_time)
    result.errors.extend(debug_errors)
    for path in debug_files:
        archive_name = _debug_archive_name(_safe_archive_name(path))
        try:
            result.files[archive_name] = _copy_draft_file(draft_dir, archive_name, path)
            result.automatic_debug_files.append(_safe_archive_name(path))
        except Exception as exc:
            result.errors.append(f"Failed to copy debug file {path}: {exc}")

    _append_reporting_log_to_dir(draft_dir, result.errors)
    if draft_dir.joinpath(REPORT_LOG_NAME).exists():
        result.files[REPORT_LOG_NAME] = draft_dir.joinpath(REPORT_LOG_NAME)
    return result


def _insert_report(report_info: dict[str, Any]):
    _ensure_database_initialized()
    attachments = report_info.get("attachments") or {}
    with ShotDataBase.engine.connect() as connection:
        with connection.begin():
            connection.execute(
                insert(bug_reports).values(
                    localID=report_info["localID"],
                    eventID=None,
                    baseEventID=None,
                    issueTime=report_info["dateAndTime"],
                    creationTime=report_info["dateAndTime"],
                    submissionTime=None,
                    description=None,
                    logFiles=_attachments_to_log_files(attachments),
                    machineInfo=attachments.get("machineInfo"),
                    machineLogs=attachments.get("machineLogs"),
                    machineStatus=attachments.get("machineStatus"),
                    status="draft",
                    ticketNumber=None,
                )
            )


def _update_report_db(local_id: str, values: dict[str, Any]) -> bool:
    _ensure_database_initialized()
    with ShotDataBase.engine.connect() as connection:
        with connection.begin():
            result = connection.execute(
                update(bug_reports).where(bug_reports.c.localID == local_id).values(**values)
            )
            return result.rowcount > 0


def _get_report_row(local_id: str):
    _ensure_database_initialized()
    with ShotDataBase.engine.connect() as connection:
        return connection.execute(
            select(bug_reports).where(bug_reports.c.localID == local_id)
        ).first()


def _validate_draft_patch(data: dict[str, Any]):
    invalid = set(data.keys()) - ALLOWED_DRAFT_UPDATE_KEYS
    if invalid:
        raise PermissionError(f"Forbidden update keys: {', '.join(sorted(invalid))}")
    if "attachments" in data:
        attachments = data["attachments"]
        debug_files = attachments.get("debugFiles") if isinstance(attachments, dict) else None
        user_files = debug_files.get("user") if isinstance(debug_files, dict) else None
        if not isinstance(attachments, dict) or not isinstance(debug_files, dict):
            raise PermissionError("Only attachments.debugFiles.user can be updated")
        if set(attachments.keys()) - {"debugFiles"} or set(debug_files.keys()) - {"user"}:
            raise PermissionError("Only attachments.debugFiles.user can be updated")
        if user_files is not None and not isinstance(user_files, list):
            raise ValueError("attachments.debugFiles.user must be a list")


def _apply_scalar_draft_patch(
    report_info: dict[str, Any], db_values: dict[str, Any], patch: dict[str, Any]
):
    if "description" in patch:
        report_info["description"] = patch["description"]
        db_values["description"] = patch["description"]

    if "baseEventID" in patch:
        report_info["baseEventID"] = patch["baseEventID"]
        db_values["baseEventID"] = patch["baseEventID"]


async def _apply_draft_time_patch(
    report_info: dict[str, Any],
    files: dict[str, Path],
    draft_dir: Path,
    attachments: dict[str, Any],
    debug_files: dict[str, Any],
    user_files: list[str],
    db_values: dict[str, Any],
    patch: dict[str, Any],
) -> list[str]:
    if "dateAndTime" not in patch:
        return []
    if patch["dateAndTime"] is None:
        raise ValueError("dateAndTime cannot be null")

    issue_time = int(patch["dateAndTime"])
    report_info["dateAndTime"] = issue_time
    db_values["issueTime"] = issue_time

    preserve_user_names = set(user_files)
    for name in list(debug_files.setdefault("automatic", [])):
        if name not in preserve_user_names:
            archive_name = _debug_archive_name(name)
            files.pop(archive_name, None)
            _remove_draft_file(draft_dir, archive_name)

    errors = []
    try:
        machine_logs_path = draft_dir.joinpath(MACHINE_LOGS_NAME)
        machine_logs_path.write_text(
            await _fetch_machine_logs(reference_time=issue_time), encoding="utf-8"
        )
        files[MACHINE_LOGS_NAME] = machine_logs_path
        attachments["machineLogs"] = True
    except Exception as exc:
        _remove_draft_file(draft_dir, MACHINE_LOGS_NAME)
        files.pop(MACHINE_LOGS_NAME, None)
        attachments["machineLogs"] = False
        errors.append(f"Failed to fetch machine logs: {exc}")

    selected_debug_files, debug_errors = _select_debug_files(reference_time=issue_time)
    errors.extend(debug_errors)
    copied_debug_files = []
    for path in selected_debug_files:
        name = _safe_archive_name(path)
        archive_name = _debug_archive_name(name)
        try:
            files[archive_name] = _copy_draft_file(draft_dir, archive_name, path)
            copied_debug_files.append(name)
        except Exception as exc:
            errors.append(f"Failed to copy debug file {path}: {exc}")

    debug_files["automatic"] = copied_debug_files
    return errors


def _apply_user_debug_file_patch(
    files: dict[str, Path],
    draft_dir: Path,
    debug_files: dict[str, Any],
    user_files: list[str],
    patch: dict[str, Any],
) -> list[str]:
    if "attachments" not in patch:
        return []

    log_errors = []
    incoming = patch.get("attachments", {}).get("debugFiles", {}).get("user", [])
    for name in incoming:
        name = str(name)
        if name in user_files:
            continue
        source_path = _find_debug_file(name)
        if source_path is None:
            log_errors.append(f"User selected debug file not found: {name}")
            continue
        archive_name = _debug_archive_name(name)
        try:
            files.setdefault(
                archive_name, _copy_draft_file(draft_dir, archive_name, source_path)
            )
        except Exception as exc:
            log_errors.append(f"Failed to copy user selected debug file {name}: {exc}")
            continue
        user_files.append(name)

    debug_files["user"] = _dedupe(user_files)
    return log_errors


def _draft_db_values(attachments: dict[str, Any]) -> dict[str, Any]:
    return {
        "logFiles": _attachments_to_log_files(attachments),
        "machineLogs": attachments.get("machineLogs"),
        "machineInfo": attachments.get("machineInfo"),
        "machineStatus": attachments.get("machineStatus"),
    }


async def _apply_draft_patch(local_id: str, patch: dict[str, Any]) -> dict[str, Any]:
    draft_dir = _draft_path(local_id)
    if not draft_dir.exists() or not draft_dir.is_dir():
        raise FileNotFoundError(local_id)

    report_info = _read_draft_report_info(draft_dir)
    files = _draft_files(draft_dir)
    attachments = report_info.setdefault("attachments", {})
    debug_files = attachments.setdefault("debugFiles", {})
    debug_files.setdefault("automatic", [])
    user_files = debug_files.setdefault("user", [])
    db_values = {}
    log_errors = []

    _apply_scalar_draft_patch(report_info, db_values, patch)
    log_errors.extend(
        await _apply_draft_time_patch(
            report_info,
            files,
            draft_dir,
            attachments,
            debug_files,
            user_files,
            db_values,
            patch,
        )
    )
    log_errors.extend(
        _apply_user_debug_file_patch(files, draft_dir, debug_files, user_files, patch)
    )

    if log_errors:
        _append_reporting_log_to_dir(draft_dir, log_errors)

    db_values.update(_draft_db_values(attachments))
    _write_draft_report_info(draft_dir, report_info)
    if db_values:
        _update_report_db(local_id, db_values)
    return report_info


def _column_for_filter(name: str):
    return bug_reports.c.get(name)


def _coerce_filter_value(column, value: str):
    if value.lower() == "null":
        return None
    if str(column.type).upper() in {"INTEGER", "BOOLEAN"}:
        if str(column.type).upper() == "BOOLEAN":
            return value.lower() in {"true", "1", "yes"}
        return int(value)
    return value


def _parse_fiql(filter_text: str):
    if not filter_text:
        return None, False
    valid_parts = []
    or_parts = [part for part in filter_text.split(",") if part]
    for or_part in or_parts:
        and_conditions = []
        for expression in [part for part in or_part.split(";") if part]:
            match = re.match(
                r"^([A-Za-z][A-Za-z0-9_]*)(==|!=|=gt=|=ge=|=lt=|=le=)(.*)$",
                expression,
            )
            if not match:
                continue
            field, operator, raw_value = match.groups()
            column = _column_for_filter(field)
            if column is None:
                continue
            value = _coerce_filter_value(column, unquote_plus(raw_value))
            if operator == "==":
                if isinstance(value, str) and ("*" in value or "?" in value):
                    pattern = value.replace("%", "\\%").replace("_", "\\_")
                    pattern = pattern.replace("*", "%").replace("?", "_")
                    and_conditions.append(column.like(pattern, escape="\\"))
                else:
                    and_conditions.append(column == value)
            elif operator == "!=":
                and_conditions.append(column != value)
            elif operator == "=gt=":
                and_conditions.append(column > value)
            elif operator == "=ge=":
                and_conditions.append(column >= value)
            elif operator == "=lt=":
                and_conditions.append(column < value)
            elif operator == "=le=":
                and_conditions.append(column <= value)
        if and_conditions:
            valid_parts.append(and_(*and_conditions))
    if not valid_parts:
        return None, True
    return or_(*valid_parts), False


class ReportsCreateHandler(BaseHandler):
    async def post(self):
        if self.request.body:
            _api_error(self, 400, "Request body must be empty")
            return
        local_id = _new_local_id()
        now = _now_seconds()
        draft_dir = _draft_path(local_id)
        try:
            fetched = await _fetch_report_files(draft_dir)
            attachments = {
                "debugFiles": {"automatic": fetched.automatic_debug_files},
                "machineInfo": fetched.machine_info,
                "machineLogs": fetched.machine_logs,
                "machineStatus": fetched.machine_status,
            }
            report_info = {
                "description": None,
                "dateAndTime": now,
                "attachments": attachments,
                "multimedia": None,
                "machineID": HostnameManager.generateHostname(),
                "eventID": None,
                "baseEventID": None,
                "ticket": None,
                "localID": local_id,
            }
            _write_draft_report_info(draft_dir, report_info)
            _insert_report(report_info)
            self.write({"localID": local_id})
        except Exception as exc:
            shutil.rmtree(draft_dir, ignore_errors=True)
            logger.exception("Failed to create bug report draft")
            _api_error(self, 500, "Failed to create report draft", {"message": str(exc)})


class ReportDraftHandler(BaseHandler):
    async def get(self, local_id: str):
        draft_dir = _draft_path(local_id)
        if not draft_dir.exists() or not draft_dir.is_dir():
            _api_error(self, 404, "Unknown localID")
            return
        self.set_header("Content-Type", "application/octet-stream")
        self.set_header("Content-Disposition", f'attachment; filename="{local_id}.zstd"')
        try:
            with tempfile.TemporaryDirectory() as temp_dir:
                archive_path = Path(temp_dir).joinpath(f"{local_id}.zstd")
                _write_tar_zstd_from_draft(archive_path, draft_dir)
                with archive_path.open("rb") as handle:
                    self.write(handle.read())
        except Exception as exc:
            logger.exception("Failed to read bug report draft")
            _api_error(self, 500, "Failed to read report draft", {"message": str(exc)})

    async def put(self, local_id: str):
        try:
            data = _json_loads_body(self.request.body)
            _validate_draft_patch(data)
        except PermissionError as exc:
            _api_error(self, 403, str(exc))
            return
        except ValueError as exc:
            _api_error(self, 400, str(exc))
            return

        if _get_report_row(local_id) is None:
            _api_error(self, 404, "Unknown localID")
            return

        try:
            report_info = await _apply_draft_patch(local_id, data)
            self.write(report_info)
        except FileNotFoundError:
            _api_error(self, 404, "Unknown localID")
        except ValueError as exc:
            _api_error(self, 400, str(exc))
        except Exception as exc:
            logger.exception("Failed to update bug report draft")
            _api_error(self, 500, "Failed to update report draft", {"message": str(exc)})


class ReportsListHandler(BaseHandler):
    async def get(self):
        try:
            page = int(self.get_query_argument("page"))
            size = int(self.get_query_argument("size"))
            if page < 0 or size <= 0:
                raise ValueError("page must be >= 0 and size must be > 0")
        except Exception as exc:
            _api_error(self, 400, f"Invalid pagination params: {exc}")
            return

        filter_text = self.get_query_argument("filter", None)
        condition, invalid_filter = _parse_fiql(filter_text)
        if invalid_filter:
            _api_error(self, 400, "Filter contains no valid fields")
            return

        _ensure_database_initialized()
        query = select(bug_reports).order_by(desc(bug_reports.c.creationTime))
        count_query = select(func.count()).select_from(bug_reports)
        if condition is not None:
            query = query.where(condition)
            count_query = count_query.where(condition)
        query = query.limit(size).offset(page * size)

        with ShotDataBase.engine.connect() as connection:
            rows = connection.execute(query).fetchall()
            total = connection.execute(count_query).scalar_one()

        self.write(
            {
                "content": [_row_to_report_info(row) for row in rows],
                "size": size,
                "page": page,
                "hasMore": (page + 1) * size < total,
            }
        )


class ReportsSubmitHandler(BaseHandler):
    async def post(self):
        try:
            data = _json_loads_body(self.request.body)
            local_id = data["localID"]
            event_id = data["eventID"]
            submission_time = data.get("submissionTime", _now_seconds())
            ticket = data.get("ticket")
        except (KeyError, ValueError) as exc:
            _api_error(self, 400, f"Invalid submit body: {exc}")
            return

        updated = _update_report_db(
            local_id,
            {
                "eventID": event_id,
                "ticketNumber": ticket,
                "submissionTime": submission_time,
                "status": "submitted",
            },
        )
        if not updated:
            _api_error(self, 404, "Unknown localID")
            return
        self.set_status(200)
        self.finish()


API.register_handler(APIVersion.V1, r"/reports/create", ReportsCreateHandler)
API.register_handler(APIVersion.V1, r"/reports/draft/([^/]+)", ReportDraftHandler)
API.register_handler(APIVersion.V1, r"/reports/list", ReportsListHandler)
API.register_handler(APIVersion.V1, r"/reports/submit", ReportsSubmitHandler)
