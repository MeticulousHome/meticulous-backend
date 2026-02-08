import json
import os
import sqlite3
import threading
import uuid
from datetime import datetime
from enum import Enum
from pathlib import Path
from typing import List, Optional

import pytz
import zstandard as zstd
from pydantic import BaseModel, Field
from sqlalchemy import (
    asc,
    create_engine,
    delete,
    desc,
    distinct,
    func,
    insert,
    or_,
    select,
    text,
    Table,
)
from sqlalchemy import event as sqlEvent
from sqlalchemy.orm import sessionmaker
from sqlalchemy import update
from database_models import (
    metadata,
    Profile,
    History,
    ShotAnnotation,
    ShotRating,
)

from log import MeticulousLogger

HISTORY_PATH = os.getenv("HISTORY_PATH", "/meticulous-user/history")
DATABASE_FILE = "history.sqlite"
ABSOLUTE_DATABASE_FILE = Path(HISTORY_PATH).joinpath(DATABASE_FILE).resolve()
DATABASE_URL = f"sqlite:///{ABSOLUTE_DATABASE_FILE}"

logger = MeticulousLogger.getLogger(__name__)


class SearchOrder(str, Enum):
    ascending = "asc"
    descending = "desc"


class SearchOrderBy(str, Enum):
    date = ("date",)
    profile = ("profile",)


class SearchParams(BaseModel):
    query: Optional[str] = None
    ids: List[str | int] = Field(default_factory=list)
    start_date: Optional[float] = None
    end_date: Optional[float] = None
    order_by: List[SearchOrderBy] = [SearchOrderBy.date]
    sort: SearchOrder = SearchOrder.descending
    max_results: int = 20
    dump_data: bool = True


class ShotDataBase:
    engine = None
    metadata = metadata
    Session = None
    stage_fts_table = None
    profile_fts_table = None

    @staticmethod
    def setup_engine():
        ShotDataBase.engine = create_engine(
            DATABASE_URL, echo=False, connect_args={"check_same_thread": False}
        )

        @sqlEvent.listens_for(ShotDataBase.engine, "connect")
        def setupDatabase(dbapi_connection, _connection_record):
            # Configure the DB for most safety
            dbapi_connection.execute("PRAGMA auto_vacuum=full;")
            dbapi_connection.execute("PRAGMA journal_mode=WAL;")
            dbapi_connection.execute("PRAGMA synchronous=EXTRA;")
            maxJournalBytes = 1024 * 1024 * 1  # 1MB
            dbapi_connection.execute(f"PRAGMA journal_size_limit = {maxJournalBytes};")
            dbapi_connection.execute("PRAGMA wal_checkpoint(TRUNCATE);")

        sqlEvent.listen(ShotDataBase.engine, "connect", setupDatabase)
        ShotDataBase.Session = sessionmaker(bind=ShotDataBase.engine)

    @staticmethod
    def init():
        os.makedirs(HISTORY_PATH, exist_ok=True)
        ShotDataBase.setup_engine()

        # Validate database integrity
        try:
            with ShotDataBase.engine.connect() as connection:
                result = connection.execute(text("PRAGMA integrity_check")).fetchone()
                if result and result[0] == "ok":
                    logger.info("Database integrity check passed")
                else:
                    logger.error("Database integrity check failed: %s", result)
                    ShotDataBase.rebuild_database()
        except Exception as e:
            logger.error("Database integrity check failed with exception: %s", e)
            ShotDataBase.rebuild_database()

        # Ensure FTS tables are created
        with ShotDataBase.engine.connect() as connection:
            connection.execute(
                text(
                    "CREATE VIRTUAL TABLE IF NOT EXISTS profile_fts USING fts5(profile_key, profile_id, name)"
                )
            )
            connection.execute(
                text(
                    "CREATE VIRTUAL TABLE IF NOT EXISTS stage_fts USING fts5(profile_key, profile_id, profile_name, stage_key, stage_name)"
                )
            )
            # Register FTS tables with SQLAlchemy
            ShotDataBase.profile_fts_table = Table(
                "profile_fts",
                ShotDataBase.metadata,
                autoload_with=ShotDataBase.engine,
            )
            ShotDataBase.stage_fts_table = Table(
                "stage_fts",
                ShotDataBase.metadata,
                autoload_with=ShotDataBase.engine,
            )

        # Log number of history entries
        try:
            with ShotDataBase.Session() as session:
                count = session.execute(
                    select(func.count()).select_from(History)
                ).scalar() or 0
                logger.info("Number of history entries in database: %d", count)
        except Exception as e:
            logger.error("Failed to count history entries: %s", e)

    @staticmethod
    def rebuild_database():
        try:
            if ShotDataBase.engine is not None:
                ShotDataBase.engine.dispose()

            if os.path.exists(ABSOLUTE_DATABASE_FILE):
                os.remove(ABSOLUTE_DATABASE_FILE)
                logger.info("Database file deleted successfully.")

            ShotDataBase.setup_engine()
            logger.info("Database engine recreated after rebuild")
        except sqlite3.DatabaseError as e:
            logger.error("Failed to completely rebuild the database: %s", e)
        except OSError as e:
            logger.error("Failed to delete the database file: %s", e)

        rescan_thread = threading.Thread(
            target=ShotDataBase.rescan_shots,
            name="DBRescan",
            daemon=True,
        )
        rescan_thread.start()

    @staticmethod
    def rescan_shots():
        from shot_manager import SHOT_PATH

        logger.info("Starting rescan of shot files from disk")
        shot_path = Path(SHOT_PATH)
        if not shot_path.exists():
            logger.info("Shot path does not exist, nothing to rescan")
            return

        count = 0
        errors = 0
        for shot_file in sorted(shot_path.rglob("*.shot.json.zst")):
            try:
                relative_path = shot_file.relative_to(shot_path)
                with open(shot_file, "rb") as f:
                    decompressor = zstd.ZstdDecompressor()
                    content = json.loads(decompressor.stream_reader(f).read())

                entry = {
                    "id": content.get("id", str(uuid.uuid4())),
                    "file": str(relative_path),
                    "time": content["time"],
                    "profile_name": content["profile_name"],
                    "profile": content.get("profile"),
                }
                ShotDataBase.insert_history(entry)
                count += 1
            except Exception as e:
                errors += 1
                logger.error("Failed to rescan shot file %s: %s", shot_file, e)

        logger.info("Rescan complete: %d shots ingested, %d errors", count, errors)

    @staticmethod
    def profile_exists(profile_data):
        stages_json = json.dumps(profile_data.get("stages", []))
        variables_json = json.dumps(profile_data.get("variables", []))
        previous_authors_json = json.dumps(profile_data.get("previous_authors", []))
        display_json = json.dumps(profile_data.get("previous_authors", []))

        query = (
            select(Profile.key)
            .where(Profile.id == profile_data["id"])
            .where(Profile.author == profile_data["author"])
            .where(Profile.author_id == profile_data["author_id"])
            .where(
                func.json_extract(Profile.display, "$")
                == func.json_extract(display_json, "$")
            )
            .where(Profile.final_weight == profile_data["final_weight"])
            .where(Profile.last_changed == profile_data.get("last_changed", 0))
            .where(Profile.name == profile_data["name"])
            .where(Profile.temperature == profile_data["temperature"])
            .where(
                func.json_extract(Profile.stages, "$")
                == func.json_extract(stages_json, "$")
            )
            .where(
                func.json_extract(Profile.variables, "$")
                == func.json_extract(variables_json, "$")
            )
            .where(
                func.json_extract(Profile.previous_authors, "$")
                == func.json_extract(previous_authors_json, "$")
            )
        )

        with ShotDataBase.Session() as session:
            return session.execute(query).fetchone()

    @staticmethod
    def insert_profile(profile_data):
        if profile_data is None:
            return -1

        existing_profile = ShotDataBase.profile_exists(profile_data)

        if existing_profile:
            logger.debug(
                f"Profile with id {profile_data['id']}, name {profile_data['name']}, and author_id {profile_data['author_id']} already exists."
            )
            return existing_profile[0]

        with ShotDataBase.Session() as session:
            with session.begin():
                profile_obj = Profile(
                    id=profile_data["id"],
                    author=profile_data["author"],
                    author_id=profile_data["author_id"],
                    display=profile_data["display"],
                    final_weight=profile_data["final_weight"],
                    last_changed=profile_data.get("last_changed", 0),
                    name=profile_data["name"],
                    temperature=profile_data["temperature"],
                    stages=profile_data.get("stages", []),
                    variables=profile_data.get("variables", []),
                    previous_authors=profile_data.get("previous_authors", []),
                )
                session.add(profile_obj)
                session.flush()
                profile_key = profile_obj.key

                # Insert into profile FTS table
                session.execute(
                    insert(ShotDataBase.profile_fts_table).values(
                        profile_key=profile_key,
                        profile_id=profile_data["id"],
                        name=profile_data["name"],
                    )
                )

                # Insert stages into stage_fts
                for stage in profile_data["stages"]:
                    session.execute(
                        insert(ShotDataBase.stage_fts_table).values(
                            profile_key=profile_key,
                            profile_id=profile_data["id"],
                            profile_name=profile_data["name"],
                            stage_key=stage["key"],
                            stage_name=stage["name"],
                        )
                    )

                return profile_key

    @staticmethod
    def history_exists(entry):
        with ShotDataBase.Session() as session:
            return session.execute(
                select(History.id).where(History.file == entry["file"])
            ).fetchone()

    @staticmethod
    def insert_history(entry):
        if "id" not in entry:
            entry["id"] = str(uuid.uuid4())
        existing_history = ShotDataBase.history_exists(entry)

        if existing_history:
            logger.debug(f"History entry with file {entry['file']} already exists.")
            return existing_history[0]

        profile_data = entry.get("profile")
        profile_key = ShotDataBase.insert_profile(profile_data)

        with ShotDataBase.Session() as session:
            with session.begin():
                # Convert to UTC
                time_obj = datetime.fromtimestamp(entry["time"])
                time_obj = pytz.timezone("UTC").localize(time_obj)

                history_obj = History(
                    uuid=entry["id"],
                    file=entry.get("file"),
                    time=time_obj,
                    profile_name=entry["profile_name"],
                    profile_id=profile_data["id"],
                    profile_key=profile_key,
                )
                session.add(history_obj)
                session.flush()
                return history_obj.id

    @staticmethod
    def link_debug_file(history_shot_id, debug_filename):
        with ShotDataBase.Session() as session:
            with session.begin():
                history_obj = session.get(History, history_shot_id)
                if history_obj:
                    history_obj.debug_file = debug_filename
                    logger.info(f"debug file linked to history id {history_shot_id}")
                else:
                    logger.warning(f"history id {history_shot_id} not found")

    @staticmethod
    def unlink_debug_file(file_relative_path):
        with ShotDataBase.Session() as session:
            with session.begin():
                result = session.execute(
                    update(History)
                    .where(History.debug_file == file_relative_path)
                    .values(debug_file=None)
                )
                if result.rowcount == 0:
                    logger.warning("no columns affected, check relative file path")
                else:
                    logger.info(
                        f"debug file unlinked, affected rows: {result.rowcount}"
                    )

    @staticmethod
    def delete_shot(shot_id):
        with ShotDataBase.Session() as session:
            with session.begin():
                history_obj = session.get(History, shot_id)
                if not history_obj:
                    return

                profile_key = history_obj.profile_key
                session.delete(history_obj)
                session.flush()

                # Check if this profile is now orphaned
                has_other = session.execute(
                    select(History.id).where(History.profile_key == profile_key).limit(1)
                ).fetchone()

                if not has_other:
                    profile_obj = session.get(Profile, profile_key)
                    if profile_obj:
                        session.delete(profile_obj)

                    # Delete from profile_fts
                    session.execute(
                        delete(ShotDataBase.profile_fts_table).where(
                            ShotDataBase.profile_fts_table.c.profile_key == profile_key
                        )
                    )

                    # Delete stages from stage_fts
                    session.execute(
                        delete(ShotDataBase.stage_fts_table).where(
                            ShotDataBase.stage_fts_table.c.profile_key == profile_key
                        )
                    )

    @staticmethod
    def search_history(params: SearchParams):
        history_table = History.__table__
        profile_table = Profile.__table__

        stmt = (
            select(
                *[c.label(f"history_{c.name}") for c in history_table.c],
                *[c.label(f"profile_{c.name}") for c in profile_table.c],
            )
            .distinct()
            .select_from(
                history_table.join(
                    profile_table,
                    history_table.c.profile_key == profile_table.c.key,
                )
            )
            .outerjoin(
                ShotDataBase.profile_fts_table,
                profile_table.c.key == ShotDataBase.profile_fts_table.c.profile_key,
            )
            .outerjoin(
                ShotDataBase.stage_fts_table,
                profile_table.c.key == ShotDataBase.stage_fts_table.c.profile_key,
            )
        )

        if params.query:
            stmt = stmt.where(
                or_(
                    ShotDataBase.profile_fts_table.c.name.like(f"%{params.query}%"),
                    ShotDataBase.stage_fts_table.c.stage_name.like(
                        f"%{params.query}%"
                    ),
                )
            )

        if params.ids:
            stmt = stmt.where(
                or_(
                    history_table.c.id.in_(params.ids),
                    history_table.c.uuid.in_(params.ids),
                    profile_table.c.id.in_(params.ids),
                )
            )

        if params.start_date:
            start_datetime = datetime.fromtimestamp(params.start_date)
            start_datetime = pytz.timezone("UTC").localize(start_datetime)
            stmt = stmt.where(history_table.c.time >= start_datetime)

        if params.end_date:
            end_datetime = datetime.fromtimestamp(params.end_date)
            end_datetime = pytz.timezone("UTC").localize(end_datetime)
            stmt = stmt.where(history_table.c.time <= end_datetime)

        order_by = []
        for ordering in params.order_by:
            if ordering == SearchOrderBy.date:
                order_by.append(history_table.c.time)
            elif ordering == SearchOrderBy.profile:
                order_by.append(history_table.c.profile_name)

        order_by.append(history_table.c.id)

        if params.sort == SearchOrder.ascending:
            stmt = stmt.order_by(*[asc(order_column) for order_column in order_by])
        else:
            stmt = stmt.order_by(*[desc(order_column) for order_column in order_by])

        if params.max_results > 0:
            stmt = stmt.limit(params.max_results)

        with ShotDataBase.Session() as session:
            results = session.execute(stmt)
            parsed_results = []
            for row in results:
                row_dict = dict(row._mapping)
                data = None
                file_entry = row_dict.pop("history_file")

                if params.dump_data:
                    from shot_manager import SHOT_PATH

                    data_file = Path(SHOT_PATH).joinpath(file_entry)
                    with open(data_file, "rb") as compressed_file:
                        decompressor = zstd.ZstdDecompressor()
                        decompressed_content = decompressor.stream_reader(
                            compressed_file
                        )
                        file_contents = json.loads(decompressed_content.read())
                        data = file_contents.get("data")

                profile = {
                    "id": row_dict.pop("profile_id"),
                    "db_key": row_dict.pop("profile_key"),
                    "author": row_dict.pop("profile_author"),
                    "author_id": row_dict.pop("profile_author_id"),
                    "display": row_dict.pop("profile_display"),
                    "final_weight": row_dict.pop("profile_final_weight"),
                    "last_changed": row_dict.pop("profile_last_changed"),
                    "name": row_dict.pop("profile_name"),
                    "temperature": row_dict.pop("profile_temperature"),
                    "stages": row_dict.pop("profile_stages"),
                    "variables": row_dict.pop("profile_variables"),
                    "previous_authors": row_dict.pop("profile_previous_authors"),
                }
                history = {
                    "id": row_dict.pop("history_uuid"),
                    "db_key": row_dict.pop("history_id"),
                    "time": datetime.timestamp(row_dict.pop("history_time")),
                    "file": file_entry,
                    "debug_file": row_dict.pop("history_debug_file", None),
                    "name": row_dict.pop("history_profile_name"),
                    "data": data,
                    "profile": profile,
                }

                parsed_results.append(history)

            logger.info(f"shot database query returned {len(parsed_results)} results")
            return parsed_results

    @staticmethod
    def autocomplete_profile_name(prefix):
        with ShotDataBase.Session() as session:
            if not prefix:
                stmt = (
                    select(History.profile_name)
                    .distinct()
                    .group_by(History.profile_name)
                    .order_by(func.count().desc())
                )
                results = session.execute(stmt).fetchall()
                return [{"profile": result[0], "type": "profile"} for result in results]

            stmt_profile = (
                select(ShotDataBase.profile_fts_table.c.name.label("name"))
                .distinct()
                .where(ShotDataBase.profile_fts_table.c.name.like(f"%{prefix}%"))
            )

            stmt_stage = (
                select(
                    ShotDataBase.stage_fts_table.c.profile_name,
                    ShotDataBase.stage_fts_table.c.stage_name,
                )
                .distinct()
                .where(ShotDataBase.stage_fts_table.c.stage_name.like(f"%{prefix}%"))
            )

            profile_results = session.execute(stmt_profile).fetchall()
            stage_results = session.execute(stmt_stage).fetchall()

            results = [
                {"profile": res.name, "type": "profile"} for res in profile_results
            ]
            for result in stage_results:
                results.append(
                    {
                        "profile": result.profile_name,
                        "type": "stage",
                        "name": result.stage_name,
                    }
                )

            return results

    @staticmethod
    def statistics():
        with ShotDataBase.Session() as session:
            stmt = (
                select(
                    History.profile_name.label("profile_name"),
                    func.count(History.profile_name).label("profile_count"),
                    func.count(distinct(History.profile_id)).label(
                        "profile_versions"
                    ),
                )
                .group_by(History.profile_name)
                .order_by(func.count("profile_count").desc())
            )
            results = session.execute(stmt)
            total_shots = 0
            parsed_results = []
            for row in results:
                row_dict = dict(row._mapping)
                profile = {
                    "name": row_dict.pop("profile_name"),
                    "count": row_dict.pop("profile_count"),
                    "profileVersions": row_dict.pop("profile_versions"),
                }
                total_shots += profile["count"]

                parsed_results.append(profile)
            return {"totalSavedShots": total_shots, "byProfile": parsed_results}

    @staticmethod
    def rate_shot(history_uuid: str, rating: Optional[str]) -> bool:

        if rating not in ("like", "dislike", None):
            logger.error(
                f"Invalid rating: {rating}. Must be 'like', 'dislike', or None."
            )
            return False

        try:
            with ShotDataBase.Session() as session:
                with session.begin():
                    history_obj = session.execute(
                        select(History).where(History.uuid == history_uuid)
                    ).scalar_one_or_none()

                    if not history_obj:
                        logger.error(f"Shot with ID {history_uuid} does not exist")
                        return False

                    annotation_obj = session.execute(
                        select(ShotAnnotation).where(
                            ShotAnnotation.history_uuid == history_uuid
                        )
                    ).scalar_one_or_none()

                    if not annotation_obj:
                        annotation_obj = ShotAnnotation(
                            history_id=history_obj.id,
                            history_uuid=history_uuid,
                        )
                        session.add(annotation_obj)
                        session.flush()

                    existing_rating = session.execute(
                        select(ShotRating).where(
                            ShotRating.annotation_id == annotation_obj.id
                        )
                    ).scalar_one_or_none()

                    if rating is None:
                        if existing_rating:
                            session.delete(existing_rating)
                    else:
                        if existing_rating:
                            existing_rating.basic = rating
                        else:
                            session.add(
                                ShotRating(
                                    annotation_id=annotation_obj.id,
                                    basic=rating,
                                )
                            )

                    return True
        except Exception as e:
            logger.error(f"Error rating shot: {e}")
            return False

    @staticmethod
    def get_shot_rating(history_uuid: str) -> Optional[str]:
        try:
            with ShotDataBase.Session() as session:
                result = session.execute(
                    select(ShotRating.basic)
                    .join(ShotAnnotation, ShotAnnotation.id == ShotRating.annotation_id)
                    .where(ShotAnnotation.history_uuid == history_uuid)
                ).fetchone()

                if result:
                    return result[0]
                return None
        except Exception as e:
            logger.error(f"Error getting shot rating: {e}")
            return None
