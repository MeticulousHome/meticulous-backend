from logging.config import fileConfig
import os
from pathlib import Path

from sqlalchemy import engine_from_config
from sqlalchemy import pool

from alembic import context

from database_models import metadata, FTS_TABLES
from shot_database import HISTORY_PATH, DATABASE_FILE

# this is the Alembic Config object, which provides
# access to the values within the .ini file in use.
config = context.config

db_path = Path(HISTORY_PATH).joinpath(DATABASE_FILE).resolve()
config.set_main_option('sqlalchemy.url', f"sqlite:///{db_path}")


if config.config_file_name is not None:
    fileConfig(config.config_file_name)

# add your model's MetaData object here
# for 'autogenerate' support
target_metadata = metadata

# other values from the config, defined by the needs of env.py,
# can be acquired:
# my_important_option = config.get_main_option("my_important_option")
# ... etc.

def exclude_tables_from_config(config_):
    """Exclude FTS tables from migrations since they're managed by SQLite."""
    tables = config_.get("tables", None)
    if tables is not None:
        tables = tables.split(",")
    return tables

def include_object(object, name, type_, reflected, compare_to):
    """Filter objects from migrations."""
    # Skip FTS tables as they're managed by SQLite
    if type_ == "table" and name in FTS_TABLES:
        return False
    return True

def run_migrations_offline() -> None:
    """Run migrations in 'offline' mode.

    This configures the context with just a URL
    and not an Engine, though an Engine is acceptable
    here as well.  By skipping the Engine creation
    we don't even need a DBAPI to be available.

    Calls to context.execute() here emit the given string to the
    script output.
    """
    url = config.get_main_option("sqlalchemy.url")
    context.configure(
        url=url,
        target_metadata=target_metadata,
        literal_binds=True,
        dialect_opts={"paramstyle": "named"},
        include_object=include_object,
    )

    with context.begin_transaction():
        context.run_migrations()


def run_migrations_online() -> None:
    """Run migrations in 'online' mode.

    In this scenario we need to create an Engine
    and associate a connection with the context.
    """
    # Create directory if it doesn't exist
    os.makedirs(HISTORY_PATH, exist_ok=True)

    connectable = engine_from_config(
        config.get_section(config.config_ini_section, {}),
        prefix="sqlalchemy.",
        poolclass=pool.NullPool,
    )

    with connectable.connect() as connection:
        context.configure(
            connection=connection,
            target_metadata=target_metadata,
            include_object=include_object,
        )

        with context.begin_transaction():
            context.run_migrations()


if context.is_offline_mode():
    run_migrations_offline()
else:
    run_migrations_online()