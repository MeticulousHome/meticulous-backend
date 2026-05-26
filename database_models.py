from sqlalchemy import (
    MetaData,
    Table,
    Column,
    Integer,
    Text,
    DateTime,
    JSON,
    ForeignKey,
    Float,
    Boolean,
)

metadata = MetaData(
    naming_convention={
        "ix": "ix_%(column_0_label)s",
        "uq": "uq_%(table_name)s_%(column_0_name)s",
        "ck": "ck_%(table_name)s_`%(constraint_name)s`",
        "fk": "fk_%(table_name)s_%(column_0_name)s_%(referred_table_name)s",
        "pk": "pk_%(table_name)s",
    }
)
fts_metadata = MetaData()

# Define all tables
profile = Table(
    "profile",
    metadata,
    Column("key", Integer, primary_key=True),
    Column("id", Text, nullable=False),
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

history = Table(
    "history",
    metadata,
    Column("id", Integer, primary_key=True),
    Column("uuid", Text),
    Column("file", Text, nullable=False),
    Column("time", DateTime, nullable=False),
    Column("profile_name", Text, nullable=False),
    Column("profile_id", Text, nullable=False),
    Column("profile_key", Integer, ForeignKey("profile.key"), nullable=False),
    Column("debug_file", Text, nullable=True),
)

shot_annotation = Table(
    "shot_annotation",
    metadata,
    Column("id", Integer, primary_key=True),
    Column("history_id", Integer, ForeignKey("history.id"), nullable=False, unique=True),
    Column("history_uuid", Text, ForeignKey("history.uuid"), nullable=False, unique=True),
)

shot_rating = Table(
    "shot_rating",
    metadata,
    Column("id", Integer, primary_key=True),
    Column(
        "annotation_id",
        Integer,
        ForeignKey("shot_annotation.id"),
        nullable=False,
        unique=True,
    ),
    Column("basic", Text, nullable=True),  # "like", "dislike", o null
)

bug_reports = Table(
    "bug_reports",
    metadata,
    Column("localID", Text, primary_key=True, nullable=False),
    Column("eventID", Text, nullable=True),
    Column("baseEventID", Text, nullable=True),
    Column("issueTime", Integer, nullable=False),
    Column("creationTime", Integer, nullable=False),
    Column("submissionTime", Integer, nullable=True),
    Column("description", Text, nullable=True),
    Column("multimedia", Integer, nullable=True),
    Column("machineID", Text, nullable=True),
    Column("logFiles", Text, nullable=True),
    Column("machineInfo", Boolean, nullable=True),
    Column("machineLogs", Boolean, nullable=True),
    Column("machineStatus", Boolean, nullable=True),
    Column("status", Text, nullable=False),
    Column("ticketNumber", Integer, nullable=True),
)

# FTS structure is defined here for reference
FTS_TABLES = {
    "profile_fts",
    "profile_fts_data",
    "profile_fts_idx",
    "profile_fts_content",
    "profile_fts_docsize",
    "profile_fts_config",
    "stage_fts",
    "stage_fts_data",
    "stage_fts_idx",
    "stage_fts_content",
    "stage_fts_docsize",
    "stage_fts_config",
}
