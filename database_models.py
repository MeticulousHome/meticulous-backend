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
)


metadata = MetaData()
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
