import hashlib
import json
import os
import shutil
import uuid
from datetime import timezone
from pathlib import Path
from typing import Any, Literal

import zstandard as zstd
from pydantic import AwareDatetime, BaseModel, ConfigDict, Field, model_validator
from sqlalchemy import asc, delete, desc, insert, select
from sqlalchemy.exc import IntegrityError

from config import POUR_OVER_PATH
from database_models import brew_history
from log import MeticulousLogger
from shot_database import ShotDataBase

logger = MeticulousLogger.getLogger(__name__)

MAX_POUR_OVER_DURATION_MS = 10 * 60 * 1000
MAX_POUR_OVER_SAMPLES = MAX_POUR_OVER_DURATION_MS // 200 + 1
MAX_POUR_OVER_RECORD_BYTES = 2 * 1024 * 1024
MAX_POUR_OVER_HISTORY_RECORDS = 1_000
MAX_POUR_OVER_HISTORY_BYTES = 128 * 1024 * 1024
MIN_POUR_OVER_FREE_BYTES = 128 * 1024 * 1024


class PourOverHistoryConflictError(Exception):
    pass


class PourOverHistoryRecordTooLargeError(Exception):
    pass


def read_compressed_record(path: Path) -> bytes:
    with path.open("rb") as compressed:
        raw = (
            zstd.ZstdDecompressor()
            .stream_reader(compressed)
            .read(MAX_POUR_OVER_RECORD_BYTES + 1)
        )
    if len(raw) > MAX_POUR_OVER_RECORD_BYTES:
        raise PourOverHistoryRecordTooLargeError(
            "Pour Over history entry exceeds the decompressed size limit"
        )
    return raw


class PourOverContractModel(BaseModel):
    model_config = ConfigDict(extra="allow", allow_inf_nan=False)


class FreePourSample(PourOverContractModel):
    t: float = Field(ge=0, le=MAX_POUR_OVER_DURATION_MS)
    w: float = Field(ge=0)
    f: float = Field(ge=0)
    p: int = Field(ge=0)


class FreePourPour(PourOverContractModel):
    number: int = Field(gt=0)
    startTimeMs: float = Field(ge=0, le=MAX_POUR_OVER_DURATION_MS)
    endTimeMs: float = Field(ge=0, le=MAX_POUR_OVER_DURATION_MS)
    startWeightG: float = Field(ge=0)
    endWeightG: float = Field(ge=0)
    waterG: float = Field(ge=0)
    averageFlowGps: float = Field(ge=0)
    peakFlowGps: float = Field(ge=0)

    @model_validator(mode="after")
    def validate_range(self):
        if self.endTimeMs < self.startTimeMs:
            raise ValueError("Pour end time cannot precede its start time")
        return self


class PourOverPourTarget(PourOverContractModel):
    number: int = Field(gt=0)
    startTimeMs: float = Field(ge=0, le=MAX_POUR_OVER_DURATION_MS)
    stopWeightG: float = Field(ge=0)
    flowGps: float | None = Field(default=None, ge=0)
    flowRangeGps: tuple[float, float] | None = None

    @model_validator(mode="after")
    def validate_flow_range(self):
        if self.flowRangeGps is not None:
            low, high = self.flowRangeGps
            if low < 0 or high < 0 or low > high:
                raise ValueError("Flow range must be non-negative and ordered")
        return self


class PourOverRecipe(PourOverContractModel):
    profileId: str | None
    profileName: str = Field(min_length=1, max_length=512)
    doseG: float = Field(gt=0)
    temperatureC: float | None = Field(default=None, gt=0)
    targetWaterG: float | None = Field(gt=0)
    targetDurationMs: float | None = Field(default=None, gt=0, le=MAX_POUR_OVER_DURATION_MS)
    pourTargets: list[PourOverPourTarget] = Field(max_length=128)


class PourOverMeasurements(PourOverContractModel):
    emptyServerG: float | None = Field(default=None, ge=0)
    serverBaselineG: float | None = None
    brewerG: float | None = Field(default=None, ge=0)
    setupG: float = Field(ge=0)
    doseG: float | None = Field(default=None, gt=0)
    waterTemperatureC: float | None = Field(default=None, gt=0)
    waterPouredG: float = Field(ge=0)
    beverageG: float | None = Field(ge=0)
    retainedG: float | None = Field(ge=0)
    durationMs: float = Field(ge=0, le=MAX_POUR_OVER_DURATION_MS)
    status: Literal["measured", "skipped"]

    @model_validator(mode="after")
    def validate_server_baseline(self):
        if self.emptyServerG is None and self.serverBaselineG is None:
            raise ValueError("A server baseline measurement is required")
        return self


class PourOverSync(PourOverContractModel):
    status: Literal["pending", "uploaded", "failed"]
    attempts: int = Field(ge=0)
    uploadedAt: AwareDatetime | None = None


class PourOverSession(PourOverContractModel):
    schemaVersion: Literal[1, 2, 3, 4]
    id: str = Field(min_length=1, max_length=256)
    brewType: Literal["pour_over"]
    mode: Literal["free_pour", "profile"]
    name: str = Field(min_length=1, max_length=512)
    source: Literal["dial"]
    startedAt: AwareDatetime
    completedAt: AwareDatetime
    recipe: PourOverRecipe
    measurements: PourOverMeasurements
    pours: list[FreePourPour] = Field(max_length=128)
    samples: list[FreePourSample] = Field(min_length=1, max_length=MAX_POUR_OVER_SAMPLES)
    completion: Literal["brewer_removed", "dial_fallback"]
    sync: PourOverSync

    @model_validator(mode="after")
    def validate_timestamps(self):
        if self.completedAt < self.startedAt:
            raise ValueError("Completion time cannot precede brew start time")
        return self


class PourOverHistoryManager:
    @staticmethod
    def _require_engine():
        if ShotDataBase.engine is None:
            raise RuntimeError("History database is not initialized")
        return ShotDataBase.engine

    @staticmethod
    def _timestamp_to_file_path(session: PourOverSession) -> Path:
        started = session.startedAt.astimezone(timezone.utc)
        folder = started.strftime("%Y-%m-%d")
        digest = hashlib.sha256(session.id.encode("utf-8")).hexdigest()[:12]
        timestamp = started.strftime("%H-%M-%S-%f")[:12]
        return Path(folder).joinpath(f"{timestamp}-{digest}.pour-over.json.zst")

    @staticmethod
    def _row_to_metadata(row) -> dict[str, Any]:
        started = row.time
        completed = row.completed_time
        if started.tzinfo is None:
            started = started.replace(tzinfo=timezone.utc)
        if completed.tzinfo is None:
            completed = completed.replace(tzinfo=timezone.utc)
        return {
            "db_key": row.id,
            "id": row.uuid,
            "brewType": row.brew_type,
            "mode": row.mode,
            "file": row.file,
            "time": started.timestamp(),
            "startedAt": started.isoformat().replace("+00:00", "Z"),
            "completedAt": completed.isoformat().replace("+00:00", "Z"),
            "name": row.name,
            "schemaVersion": row.schema_version,
        }

    @staticmethod
    def _find_by_id(session_id: str):
        engine = PourOverHistoryManager._require_engine()
        with engine.connect() as connection:
            return connection.execute(
                select(brew_history).where(brew_history.c.uuid == session_id)
            ).fetchone()

    @staticmethod
    def _history_rows():
        engine = PourOverHistoryManager._require_engine()
        statement = (
            select(brew_history)
            .where(brew_history.c.brew_type == "pour_over")
            .order_by(asc(brew_history.c.time), asc(brew_history.c.id))
        )
        with engine.connect() as connection:
            return connection.execute(statement).fetchall()

    @staticmethod
    def _available_history_bytes() -> int:
        path = POUR_OVER_PATH if POUR_OVER_PATH.exists() else POUR_OVER_PATH.parent
        path.mkdir(parents=True, exist_ok=True)
        return shutil.disk_usage(path).free

    @staticmethod
    def _remove_history_row(row) -> int:
        engine = PourOverHistoryManager._require_engine()
        record_path = POUR_OVER_PATH.joinpath(row.file)
        try:
            record_size = record_path.stat().st_size
        except OSError:
            record_size = 0
        with engine.begin() as connection:
            connection.execute(delete(brew_history).where(brew_history.c.id == row.id))
        try:
            record_path.unlink(missing_ok=True)
        except OSError as error:
            logger.warning(
                "Could not remove pruned Pour Over history file %s: %s",
                record_path,
                type(error).__name__,
            )
        logger.info("Pruned Pour Over history entry %s", row.uuid)
        return record_size

    @staticmethod
    def _ensure_storage_capacity(incoming_bytes: int, additional_records: int) -> None:
        if incoming_bytes > MAX_POUR_OVER_HISTORY_BYTES:
            raise OSError("Pour Over record exceeds the history storage quota")

        rows = PourOverHistoryManager._history_rows()
        sizes = []
        for row in rows:
            try:
                sizes.append(POUR_OVER_PATH.joinpath(row.file).stat().st_size)
            except OSError:
                sizes.append(0)
        total_bytes = sum(sizes)

        # Prune oldest entries only to enforce this store's own quotas (record
        # count and total stored bytes), and never the most recent record. Low
        # free disk space is usually caused by unrelated data, so deleting Pour
        # Over history could wipe it without recovering any meaningful space.
        while len(rows) > 1 and (
            len(rows) + additional_records > MAX_POUR_OVER_HISTORY_RECORDS
            or total_bytes + incoming_bytes > MAX_POUR_OVER_HISTORY_BYTES
        ):
            row = rows.pop(0)
            expected_size = sizes.pop(0)
            removed_size = PourOverHistoryManager._remove_history_row(row)
            total_bytes = max(0, total_bytes - max(expected_size, removed_size))

        if (
            len(rows) + additional_records > MAX_POUR_OVER_HISTORY_RECORDS
            or total_bytes + incoming_bytes > MAX_POUR_OVER_HISTORY_BYTES
        ):
            raise OSError("Pour Over history quota cannot accommodate this record")

        free_bytes = PourOverHistoryManager._available_history_bytes()
        if free_bytes < MIN_POUR_OVER_FREE_BYTES + incoming_bytes:
            raise OSError(
                "Insufficient free disk space for Pour Over history; "
                "existing records were preserved"
            )

    @staticmethod
    def _write_record(
        relative_path: Path,
        payload: dict[str, Any],
        *,
        additional_records: int,
    ) -> None:
        destination = POUR_OVER_PATH.joinpath(relative_path)
        destination.parent.mkdir(parents=True, exist_ok=True)
        raw = json.dumps(
            payload,
            ensure_ascii=False,
            allow_nan=False,
            separators=(",", ":"),
        ).encode("utf-8")
        if len(raw) > MAX_POUR_OVER_RECORD_BYTES:
            raise PourOverHistoryRecordTooLargeError(
                "Pour Over history entry exceeds the size limit"
            )
        compressed = zstd.ZstdCompressor(level=10).compress(raw)
        PourOverHistoryManager._ensure_storage_capacity(len(compressed), additional_records)
        temporary = destination.parent.joinpath(f".{destination.name}.{uuid.uuid4()}.tmp")
        try:
            with temporary.open("xb") as output:
                output.write(compressed)
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary, destination)
        finally:
            if temporary.exists():
                temporary.unlink()

    @staticmethod
    def _read_record(relative_path: Path) -> dict[str, Any]:
        raw = read_compressed_record(POUR_OVER_PATH.joinpath(relative_path))
        return json.loads(raw)

    @staticmethod
    def _comparable_payload(payload: dict[str, Any]) -> dict[str, Any]:
        # The "sync" block (status/attempts/uploadedAt) mutates between upload
        # attempts, so it must not participate in conflict detection: the Dial
        # re-POSTing the same session after an upload attempt is idempotent,
        # not a conflict. The block is still stored as part of the record.
        return {key: value for key, value in payload.items() if key != "sync"}

    @staticmethod
    def _validate_existing_record(row, payload: dict[str, Any]) -> None:
        existing_path = POUR_OVER_PATH.joinpath(row.file)
        if not existing_path.is_file():
            PourOverHistoryManager._write_record(Path(row.file), payload, additional_records=0)
            logger.warning("Repaired missing Pour Over history file for session %s", row.uuid)
            return
        existing_payload = PourOverHistoryManager._read_record(Path(row.file))
        if PourOverHistoryManager._comparable_payload(
            existing_payload
        ) != PourOverHistoryManager._comparable_payload(payload):
            raise PourOverHistoryConflictError(
                "A different Pour Over record already uses this session id"
            )

    @staticmethod
    def save(session: PourOverSession) -> tuple[dict[str, Any], bool]:
        engine = PourOverHistoryManager._require_engine()
        payload = session.model_dump(mode="json")
        existing = PourOverHistoryManager._find_by_id(session.id)
        if existing is not None:
            PourOverHistoryManager._validate_existing_record(existing, payload)
            return PourOverHistoryManager._row_to_metadata(existing), False

        relative_path = PourOverHistoryManager._timestamp_to_file_path(session)
        PourOverHistoryManager._write_record(relative_path, payload, additional_records=1)
        try:
            with engine.begin() as connection:
                result = connection.execute(
                    insert(brew_history).values(
                        uuid=session.id,
                        brew_type=session.brewType,
                        mode=session.mode,
                        file=str(relative_path),
                        time=session.startedAt.astimezone(timezone.utc),
                        completed_time=session.completedAt.astimezone(timezone.utc),
                        name=session.name,
                        schema_version=session.schemaVersion,
                    )
                )
                db_key = result.inserted_primary_key[0]
        except IntegrityError:
            existing = PourOverHistoryManager._find_by_id(session.id)
            if existing is None:
                POUR_OVER_PATH.joinpath(relative_path).unlink(missing_ok=True)
                raise
            if str(relative_path) != existing.file:
                POUR_OVER_PATH.joinpath(relative_path).unlink(missing_ok=True)
            PourOverHistoryManager._validate_existing_record(existing, payload)
            return PourOverHistoryManager._row_to_metadata(existing), False
        except Exception:
            POUR_OVER_PATH.joinpath(relative_path).unlink(missing_ok=True)
            raise

        logger.info("Saved Pour Over session %s in brew history with id %s", session.id, db_key)
        return {
            "db_key": db_key,
            "id": session.id,
            "brewType": session.brewType,
            "mode": session.mode,
            "file": str(relative_path),
            "time": session.startedAt.timestamp(),
            "startedAt": session.startedAt.isoformat(),
            "completedAt": session.completedAt.isoformat(),
            "name": session.name,
            "schemaVersion": session.schemaVersion,
        }, True

    @staticmethod
    def search(
        *,
        max_results: int = 50,
        descending: bool = True,
        after: str | None = None,
        mode: Literal["free_pour", "profile"] | None = None,
    ) -> list[dict[str, Any]]:
        engine = PourOverHistoryManager._require_engine()
        statement = select(brew_history).where(brew_history.c.brew_type == "pour_over")
        if after:
            statement = statement.where(brew_history.c.file > after)
        if mode:
            statement = statement.where(brew_history.c.mode == mode)
        order_by = desc if descending else asc
        statement = statement.order_by(
            order_by(brew_history.c.file), order_by(brew_history.c.id)
        ).limit(max_results)
        with engine.connect() as connection:
            rows = connection.execute(statement).fetchall()
        return [PourOverHistoryManager._row_to_metadata(row) for row in rows]

    @staticmethod
    def latest() -> dict[str, Any] | None:
        results = PourOverHistoryManager.search(max_results=1, descending=True)
        return results[0] if results else None
