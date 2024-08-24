import json
import os
import sqlite3
import uuid
from datetime import datetime
from enum import Enum
from pathlib import Path
from typing import List, Optional

import pytz
import zstandard as zstd
from pydantic import BaseModel, Field
from sqlalchemy import (
    JSON,
    Column,
    DateTime,
    Float,
    ForeignKey,
    Integer,
    MetaData,
    String,
    Table,
    Text,
    asc,
    create_engine,
    delete,
    desc,
    distinct,
)
from sqlalchemy import event as sqlEvent
from sqlalchemy import func, insert, or_, select, text
from sqlalchemy.orm import sessionmaker

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
    ids: List[int] = Field(default_factory=list)
    start_date: Optional[float] = None
    end_date: Optional[float] = None
    order_by: List[SearchOrderBy] = [SearchOrderBy.date]
    sort: SearchOrder = SearchOrder.descending
    max_results: int = 20
    dump_data: bool = True


class ShotDataBase:
    engine = None
    metadata = MetaData()
    session = None

    # Define tables
    profile_table = Table(
        "profile",
        metadata,
        Column("key", Integer, primary_key=True, autoincrement=True),
        Column("id", String, nullable=False),
        Column("author", Text),
        Column("author_id", Text),
        Column("display", JSON),
        Column("final_weight", Integer),
        Column("last_changed", Float),
        Column("name", Text),
        Column("temperature", Integer),
        Column("stages", JSON),
        Column("variables", JSON),
        Column("previous_authors", JSON),
    )

    history_table = Table(
        "history",
        metadata,
        Column("id", Integer, primary_key=True, autoincrement=True),
        Column("uuid", Text, nullable=True),
        Column("file", Text, nullable=False),
        Column("time", DateTime, nullable=False),
        Column("profile_name", Text, nullable=False),
        Column("profile_id", String, nullable=False),
        Column("profile_key", Integer, ForeignKey("profile.key"), nullable=False),
    )

    stage_fts_table = None
    profile_fts_table = None

    @staticmethod
    def init():

        os.makedirs(HISTORY_PATH, exist_ok=True)
        # Initialize database connection
        ShotDataBase.engine = create_engine(
            DATABASE_URL, echo=True, connect_args={"check_same_thread": False}
        )

        @sqlEvent.listens_for(ShotDataBase.engine, "connect")
        def setupDatabase(dbapi_connection, _connection_record):
            # Connfigure the DB for most safety
            dbapi_connection.execute("PRAGMA auto_vacuum=full;")
            dbapi_connection.execute("PRAGMA journal_mode=WAL;")
            dbapi_connection.execute("PRAGMA synchronous=EXTRA;")
            maxJournalBytes = 1024 * 1024 * 1  # 1MB
            dbapi_connection.execute(f"PRAGMA journal_size_limit = {maxJournalBytes};")
            dbapi_connection.execute("PRAGMA wal_checkpoint(TRUNCATE);")

        sqlEvent.listen(ShotDataBase.engine, "connect", setupDatabase)
        ShotDataBase.session = sessionmaker(bind=ShotDataBase.engine)

        try:
            # Ensure tables are created
            ShotDataBase.metadata.create_all(ShotDataBase.engine)

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

        except sqlite3.DatabaseError as e:
            logger.error("Database error: %s", e)
            ShotDataBase.handle_error(e)

    @staticmethod
    def handle_error(e):
        if "database disk image is malformed" in str(e):
            logger.error("Database corrupted, reinitializing by deleteing...")
            ShotDataBase.delete_and_rebuild()
        elif "unable to open database file" in str(e):
            logger.error(
                "Cannot open database file, attempting to delete and completely rebuild the ShotDataBase..."
            )
            ShotDataBase.delete_and_rebuild()
        else:
            logger.error("Unhandled database error: %s", e)

    @staticmethod
    def delete_and_rebuild():
        try:
            # Close the ShotDataBase.engine connection before deleting the file
            ShotDataBase.engine.dispose()

            # Delete the database file
            if os.path.exists(DATABASE_FILE):
                os.remove(DATABASE_FILE)
                logger.info("Database file deleted successfully.")

            # Recreate the entire database
            ShotDataBase.init()
        except sqlite3.DatabaseError as e:
            logger.error("Failed to completely rebuild the database: %s", e)
        except OSError as e:
            logger.error("Failed to delete the database file: %s", e)

    @staticmethod
    def profile_exists(profile_data):
        stages_json = json.dumps(profile_data["stages"])
        variables_json = json.dumps(profile_data["variables"])
        previous_authors_json = json.dumps(profile_data["previous_authors"])
        display_json = json.dumps(profile_data["previous_authors"])

        query = (
            select(ShotDataBase.profile_table.c.key)
            .where(ShotDataBase.profile_table.c.id == profile_data["id"])
            .where(ShotDataBase.profile_table.c.author == profile_data["author"])
            .where(ShotDataBase.profile_table.c.author_id == profile_data["author_id"])
            .where(
                func.json_extract(ShotDataBase.profile_table.c.display, "$")
                == func.json_extract(display_json, "$")
            )
            .where(
                ShotDataBase.profile_table.c.final_weight
                == profile_data["final_weight"]
            )
            .where(
                ShotDataBase.profile_table.c.last_changed
                == profile_data.get("last_changed", 0)
            )
            .where(ShotDataBase.profile_table.c.name == profile_data["name"])
            .where(
                ShotDataBase.profile_table.c.temperature == profile_data["temperature"]
            )
            .where(
                func.json_extract(ShotDataBase.profile_table.c.stages, "$")
                == func.json_extract(stages_json, "$")
            )
            .where(
                func.json_extract(ShotDataBase.profile_table.c.variables, "$")
                == func.json_extract(variables_json, "$")
            )
            .where(
                func.json_extract(ShotDataBase.profile_table.c.previous_authors, "$")
                == func.json_extract(previous_authors_json, "$")
            )
        )

        with ShotDataBase.engine.connect() as connection:
            existing_profile = connection.execute(query).fetchone()
            return existing_profile

    @staticmethod
    def insert_profile(profile_data):
        existing_profile = ShotDataBase.profile_exists(profile_data)

        if existing_profile:
            logger.debug(
                f"Profile with id {profile_data['id']}, name {profile_data['name']}, and author_id {profile_data['author_id']} already exists."
            )
            return existing_profile[0]
        with ShotDataBase.engine.connect() as connection:
            with connection.begin():
                ins_stmt = insert(ShotDataBase.profile_table).values(
                    id=profile_data["id"],
                    author=profile_data["author"],
                    author_id=profile_data["author_id"],
                    display=profile_data["display"],
                    final_weight=profile_data["final_weight"],
                    last_changed=profile_data.get("last_changed", 0),
                    name=profile_data["name"],
                    temperature=profile_data["temperature"],
                    stages=profile_data["stages"],
                    variables=profile_data["variables"],
                    previous_authors=profile_data["previous_authors"],
                )
                result = connection.execute(ins_stmt)
                profile_key = result.inserted_primary_key[0]

                # Insert into profile FTS table
                fts_ins_stmt = insert(ShotDataBase.profile_fts_table).values(
                    profile_key=profile_key,
                    profile_id=profile_data["id"],
                    name=profile_data["name"],
                )
                connection.execute(fts_ins_stmt)

                # Insert stages into stage_fts
                for stage in profile_data["stages"]:
                    stage_fts_ins_stmt = insert(ShotDataBase.stage_fts_table).values(
                        profile_key=profile_key,
                        profile_id=profile_data["id"],
                        profile_name=profile_data["name"],
                        stage_key=stage["key"],
                        stage_name=stage["name"],
                    )
                    connection.execute(stage_fts_ins_stmt)
                return profile_key

    @staticmethod
    def history_exists(entry):
        query = select(ShotDataBase.history_table.c.id).where(
            ShotDataBase.history_table.c.file == entry["file"]
        )

        with ShotDataBase.engine.connect() as connection:
            existing_history = connection.execute(query).fetchone()
            return existing_history

    @staticmethod
    def insert_history(entry):
        if "id" not in entry:
            entry["id"] = str(uuid.uuid4())
        existing_history = ShotDataBase.history_exists(entry)

        if existing_history:
            logger.debug(f"History entry with file {entry['file']} already exists.")
            return existing_history[0]

        profile_data = entry["profile"]
        profile_key = ShotDataBase.insert_profile(profile_data)
        with ShotDataBase.engine.connect() as connection:
            with connection.begin():

                # Convert to UTC
                time_obj = datetime.fromtimestamp(entry["time"])
                time_obj = pytz.timezone("UTC").localize(time_obj)

                ins_stmt = insert(ShotDataBase.history_table).values(
                    uuid=entry["id"],
                    file=entry.get("file"),
                    time=time_obj,
                    profile_name=entry["profile_name"],
                    profile_id=profile_data["id"],
                    profile_key=profile_key,
                )
                connection.execute(ins_stmt)

    @staticmethod
    def delete_shot(shot_id):
        with ShotDataBase.engine.connect() as connection:
            with connection.begin():
                # Delete from history
                del_stmt = delete(ShotDataBase.history_table).where(
                    ShotDataBase.history_table.c.id == shot_id
                )
                connection.execute(del_stmt)

                # Get the profile_key of the deleted shot
                profile_key_stmt = select(
                    [ShotDataBase.history_table.c.profile_key]
                ).where(ShotDataBase.history_table.c.id == shot_id)
                connection.execute(profile_key_stmt).fetchone()

                # Check for orphaned profiles
                orphaned_profiles_stmt = select(
                    [ShotDataBase.profile_table.c.key]
                ).where(
                    ~ShotDataBase.profile_table.c.key.in_(
                        select([ShotDataBase.history_table.c.profile_key])
                    )
                )
                orphaned_profiles = connection.execute(
                    orphaned_profiles_stmt
                ).fetchall()
                for orphan in orphaned_profiles:
                    del_profile_stmt = delete(ShotDataBase.profile_table).where(
                        ShotDataBase.profile_table.c.key == orphan[0]
                    )
                    connection.execute(del_profile_stmt)

                    # Delete from profile_fts
                    del_profile_fts_stmt = delete(ShotDataBase.profile_fts_table).where(
                        ShotDataBase.profile_fts_table.c.key == orphan[0]
                    )
                    connection.execute(del_profile_fts_stmt)

                    # Delete stages from stage_fts
                    del_stage_fts_stmt = delete(ShotDataBase.stage_fts_table).where(
                        ShotDataBase.stage_fts_table.c.profile_key == orphan[0]
                    )
                    connection.execute(del_stage_fts_stmt)

    @staticmethod
    def search_history(params: SearchParams):
        stmt = (
            select(
                *[c.label(f"history_{c.name}") for c in ShotDataBase.history_table.c],
                *[c.label(f"profile_{c.name}") for c in ShotDataBase.profile_table.c],
            )
            .distinct()
            .select_from(
                ShotDataBase.history_table.join(
                    ShotDataBase.profile_table,
                    ShotDataBase.history_table.c.profile_key
                    == ShotDataBase.profile_table.c.key,
                )
            )
            .outerjoin(
                ShotDataBase.profile_fts_table,
                ShotDataBase.profile_table.c.key
                == ShotDataBase.profile_fts_table.c.profile_key,
            )
            .outerjoin(
                ShotDataBase.stage_fts_table,
                ShotDataBase.profile_table.c.key
                == ShotDataBase.stage_fts_table.c.profile_key,
            )
        )

        if params.query:
            stmt = stmt.where(
                or_(
                    ShotDataBase.profile_fts_table.c.name.like(f"%{params.query}%"),
                    ShotDataBase.stage_fts_table.c.stage_name.like(f"%{params.query}%"),
                )
            )

        if params.ids:
            stmt = stmt.where(
                or_(
                    ShotDataBase.history_table.c.id.in_(params.ids),
                    ShotDataBase.history_table.c.uuid.in_(params.ids),
                )
            )

        if params.start_date:
            start_datetime = datetime.fromtimestamp(params.start_date)
            start_datetime = pytz.timezone("UTC").localize(start_datetime)
            stmt = stmt.where(ShotDataBase.history_table.c.time >= start_datetime)

        if params.end_date:
            end_datetime = datetime.fromtimestamp(params.end_date)
            end_datetime = pytz.timezone("UTC").localize(end_datetime)
            stmt = stmt.where(ShotDataBase.history_table.c.time <= end_datetime)

        order_by = []
        for ordering in params.order_by:
            if ordering == SearchOrderBy.date:
                order_by.append(ShotDataBase.history_table.c.time)
            elif ordering == SearchOrderBy.profile:
                order_by.append(ShotDataBase.history_table.c.profile_name)

        order_by.append(ShotDataBase.history_table.c.id)

        if params.sort == SearchOrder.ascending:
            stmt = stmt.order_by(*[asc(order_column) for order_column in order_by])
        else:
            stmt = stmt.order_by(*[desc(order_column) for order_column in order_by])

        if params.max_results > 0:
            stmt = stmt.limit(params.max_results)

        with ShotDataBase.engine.connect() as connection:
            results = connection.execute(stmt)
            parsed_results = []
            for row in results:
                row_dict = dict(row._mapping)
                data = None
                if params.dump_data:
                    from shot_manager import SHOT_PATH

                    file_entry = row_dict.pop("history_file")
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
                    "name": row_dict.pop("history_profile_name"),
                    "data": data,
                    "profile": profile,
                }

                parsed_results.append(history)

            logger.info(f"shot database query returned {len(parsed_results)} results")
            return parsed_results

    @staticmethod
    def autocomplete_profile_name(prefix):
        with ShotDataBase.session() as session:
            if not prefix:
                stmt = (
                    select(ShotDataBase.history_table.c.profile_name)
                    .distinct()
                    .group_by(ShotDataBase.history_table.c.profile_name)
                    .order_by(func.count().desc())
                )
                results = session.execute(stmt).fetchall()
                return [{"profile": result[0], "type": "profile"} for result in results]

            # Update queries to use LIKE instead of MATCH for partial matching
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
        with ShotDataBase.session() as session:
            stmt = (
                select(
                    ShotDataBase.history_table.c.profile_name.label("profile_name"),
                    func.count(ShotDataBase.history_table.c.profile_name).label(
                        "profile_count"
                    ),
                    func.count(distinct(ShotDataBase.history_table.c.profile_id)).label(
                        "profile_versions"
                    ),
                )
                .group_by(
                    ShotDataBase.history_table.c.profile_name,
                )
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
