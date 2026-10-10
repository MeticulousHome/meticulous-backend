"""The history database's alembic migrations, as db_migration_updater runs them at boot.

Every machine carries its history.sqlite across OTA updates in both directions:
an update runs the new image's upgrades, and a rollback to an older image runs
the downgrade of each revision it does not know, from the scripts the newer
image backed up into USER_DB_MIGRATION_DIR. These tests run that code against
throwaway databases and a throwaway copy of alembic/.
"""

import shutil
from pathlib import Path

import pytest
from alembic.autogenerate import compare_metadata
from alembic.config import Config
from alembic.runtime.migration import MigrationContext
from alembic.script import ScriptDirectory
from sqlalchemy import create_engine, inspect, text

import config
import db_migration_updater
import shot_database
from database_models import FTS_TABLES, metadata
from shot_database import ShotDataBase

REPO_ALEMBIC = Path(db_migration_updater.BASE_DIR) / "alembic"
INITIAL_REVISION = "ebb6a77afd0e"


@pytest.fixture
def migration_env(tmp_path, monkeypatch):
    """Point db_migration_updater and alembic/env.py at throwaway paths."""
    history = tmp_path / "history"
    history.mkdir()
    alembic_dir = tmp_path / "alembic"
    shutil.copytree(REPO_ALEMBIC, alembic_dir, ignore=shutil.ignore_patterns("__pycache__"))
    backup_dir = tmp_path / "dbmigrations"
    database_url = f"sqlite:///{history / config.DATABASE_FILE}"

    # alembic/env.py imports HISTORY_PATH from config each time it runs.
    monkeypatch.setattr(config, "HISTORY_PATH", str(history))
    monkeypatch.setattr(db_migration_updater, "DATABASE_URL", database_url)
    monkeypatch.setattr(db_migration_updater, "ALEMBIC_DIR", str(alembic_dir))
    monkeypatch.setattr(db_migration_updater, "ALEMBIC_VER_DIR", str(alembic_dir / "versions"))
    monkeypatch.setattr(db_migration_updater, "USER_DB_MIGRATION_DIR", str(backup_dir))
    monkeypatch.setattr(shot_database, "HISTORY_PATH", str(history))
    monkeypatch.setattr(shot_database, "DATABASE_URL", database_url)
    for attribute in ("engine", "session", "profile_fts_table", "stage_fts_table"):
        monkeypatch.setattr(ShotDataBase, attribute, getattr(ShotDataBase, attribute))

    tables_before = set(metadata.tables)
    engine = create_engine(database_url)
    yield {
        "engine": engine,
        "alembic_dir": alembic_dir,
        "backup_dir": backup_dir,
    }
    engine.dispose()
    if ShotDataBase.engine is not None:
        ShotDataBase.engine.dispose()
    # ShotDataBase.init() reflects the FTS tables into the shared metadata,
    # where other tests' metadata.create_all() cannot render them.
    for name in set(metadata.tables) - tables_before:
        metadata.remove(metadata.tables[name])


def boot():
    """What back.run() does with the database, in its order.

    ShotManager.init() opens the database first and its connect hook switches
    the file to WAL, which the initial migration's checkpoint relies on.
    """
    ShotDataBase.init()
    db_migration_updater.update_db_migrations()


def alembic_config(alembic_dir: Path) -> Config:
    cfg = Config(db_migration_updater.ALEMBIC_CONFIG_FILE_PATH)
    cfg.set_main_option("script_location", str(alembic_dir))
    cfg.attributes["configure_logger"] = False
    return cfg


def revision_of(engine) -> str | None:
    with engine.connect() as connection:
        return MigrationContext.configure(connection).get_current_revision()


def schema_drift(engine) -> list:
    """Differences between the migrated database and database_models.metadata."""

    def include_object(obj, name, type_, reflected, compare_to):
        return not (type_ == "table" and name in FTS_TABLES)

    with engine.connect() as connection:
        context = MigrationContext.configure(
            connection,
            opts={"include_object": include_object, "render_as_batch": True},
        )
        return compare_metadata(context, metadata)


def upgrade_to(env, revision: str):
    from alembic import command

    command.upgrade(alembic_config(env["alembic_dir"]), revision)


def insert(engine, table: str, **values):
    columns = ", ".join(f'"{name}"' for name in values)
    params = ", ".join(f":{name}" for name in values)
    with engine.begin() as connection:
        connection.execute(text(f'INSERT INTO "{table}" ({columns}) VALUES ({params})'), values)


def test_the_single_head_is_the_revision_the_backend_requires():
    scripts = ScriptDirectory.from_config(alembic_config(REPO_ALEMBIC))
    assert scripts.get_heads() == [db_migration_updater.DB_VERSION_REQUIRED]


def test_every_revision_script_passes_the_backup_validation():
    # A script the boot-time validation rejects is never backed up, so a
    # rollback past it cannot downgrade.
    versions = REPO_ALEMBIC / "versions"
    for script in sorted(versions.glob("*.py")):
        assert db_migration_updater.is_valid_revision_script(str(versions), script.name), script


def test_a_new_database_is_migrated_to_the_schema_of_the_models(migration_env):
    boot()

    engine = migration_env["engine"]
    assert revision_of(engine) == db_migration_updater.DB_VERSION_REQUIRED
    assert schema_drift(engine) == []


def test_boot_backs_up_every_revision_script(migration_env):
    boot()

    shipped = {path.name for path in (migration_env["alembic_dir"] / "versions").glob("*.py")}
    backed_up = {path.name for path in migration_env["backup_dir"].glob("*.py")}
    assert backed_up == shipped


def test_history_from_the_first_release_survives_the_upgrade(migration_env):
    engine = migration_env["engine"]
    ShotDataBase.init()
    upgrade_to(migration_env, INITIAL_REVISION)
    insert(
        engine,
        "profile",
        key=1,
        id="05051ed3-9996-43e8-9da6-963f2b31d481",
        name="Italian limbus",
        stages="[]",
        variables="[]",
        previous_authors="[]",
        display="{}",
    )
    insert(
        engine,
        "history",
        id=1,
        uuid="3f8c4d8e-35a4-4f0c-9b1f-0b5f8a1e9c11",
        file="2025-01-30/09:15:00.shot.json.zst",
        time="2025-01-30 09:15:00.000000",
        profile_name="Italian limbus",
        profile_id="05051ed3-9996-43e8-9da6-963f2b31d481",
        profile_key=1,
    )

    boot()

    assert revision_of(engine) == db_migration_updater.DB_VERSION_REQUIRED
    assert schema_drift(engine) == []
    with engine.connect() as connection:
        rows = connection.execute(text("SELECT uuid, file, profile_name FROM history")).all()
    assert rows == [
        (
            "3f8c4d8e-35a4-4f0c-9b1f-0b5f8a1e9c11",
            "2025-01-30/09:15:00.shot.json.zst",
            "Italian limbus",
        )
    ]


@pytest.mark.xfail(
    strict=True,
    raises=Exception,
    reason="backend bug: 1a598cd3ace3 adds shot_annotation.history_uuid NOT NULL with no "
    "value for existing rows, so a database at 0bdd1c635e7a with a shot rating fails to "
    "migrate and back.run() carries on with the old schema. Reached by machines on a "
    "Feb-May 2025 image, and by any rollback below 1a598cd3ace3 followed by an update.",
)
def test_shot_ratings_from_before_history_uuid_survive_the_upgrade(migration_env):
    """1a598cd3ace3 adds shot_annotation.history_uuid NOT NULL to a table that may have rows."""
    engine = migration_env["engine"]
    ShotDataBase.init()
    upgrade_to(migration_env, "0bdd1c635e7a")
    insert(engine, "profile", key=1, id="05051ed3-9996-43e8-9da6-963f2b31d481", name="Italian")
    insert(
        engine,
        "history",
        id=1,
        uuid="3f8c4d8e-35a4-4f0c-9b1f-0b5f8a1e9c11",
        file="2025-03-01/08:00:00.shot.json.zst",
        time="2025-03-01 08:00:00.000000",
        profile_name="Italian",
        profile_id="05051ed3-9996-43e8-9da6-963f2b31d481",
        profile_key=1,
    )
    insert(engine, "shot_annotation", id=1, history_id=1)
    insert(engine, "shot_rating", id=1, annotation_id=1, basic="like")

    boot()

    assert revision_of(engine) == db_migration_updater.DB_VERSION_REQUIRED
    with engine.connect() as connection:
        ratings = connection.execute(
            text(
                "SELECT shot_annotation.history_uuid, shot_rating.basic FROM shot_rating "
                "JOIN shot_annotation ON shot_annotation.id = shot_rating.annotation_id"
            )
        ).all()
    assert ratings == [("3f8c4d8e-35a4-4f0c-9b1f-0b5f8a1e9c11", "like")]


def test_each_revision_downgrades_and_upgrades_again(migration_env):
    from alembic import command

    cfg = alembic_config(migration_env["alembic_dir"])
    engine = migration_env["engine"]
    scripts = ScriptDirectory.from_config(cfg)
    revisions = [rev for rev in scripts.walk_revisions() if rev.down_revision is not None]

    ShotDataBase.init()
    command.upgrade(cfg, "head")
    for revision in revisions:  # newest first
        command.downgrade(cfg, revision.down_revision)
        assert revision_of(engine) == revision.down_revision
    command.upgrade(cfg, "head")

    assert revision_of(engine) == db_migration_updater.DB_VERSION_REQUIRED
    assert schema_drift(engine) == []


def test_a_rollback_downgrades_with_the_script_the_newer_image_backed_up(migration_env):
    """Boot the newest backend, then an image that predates its last revision."""
    engine = migration_env["engine"]
    versions = migration_env["alembic_dir"] / "versions"
    newest = db_migration_updater.DB_VERSION_REQUIRED

    boot()
    assert revision_of(engine) == newest

    newest_script = next(versions.glob(f"*{newest}*.py"))
    previous = ScriptDirectory.from_config(
        alembic_config(migration_env["alembic_dir"])
    ).get_revision(newest)
    # The older image ships neither the newest script nor the requirement for it.
    newest_script.unlink()
    db_migration_updater.DB_VERSION_REQUIRED = previous.down_revision
    try:
        boot()
    finally:
        db_migration_updater.DB_VERSION_REQUIRED = newest

    assert revision_of(engine) == previous.down_revision
    columns = {column["name"] for column in inspect(engine).get_columns("bug_reports")}
    assert not {"contactName", "contactEmail"} & columns
