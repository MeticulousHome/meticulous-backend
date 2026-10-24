"""Shared setup for the scripts that produce crates/history's reference data.

The scripts import the Python backend (config, shot_database, alembic env) the
way tests/conftest.py does: every path the backend writes points into a
throwaway directory before config.py is imported. Run them from anywhere with
the repository's virtualenv, e.g. ``uv run python crates/history/tests/data/<script>.py``.
"""

import json
import os
import shutil
import sqlite3
import sys
import tempfile
from pathlib import Path

DATA_DIR = Path(__file__).resolve().parent
# The Python backend these scripts import: a checkout of the last release
# that shipped it (the Rust backend replaced it in this repository).
REPO_ROOT = Path(os.environ.get("PYTHON_BACKEND_DIR", DATA_DIR.parents[3]))
WORK = Path(tempfile.mkdtemp(prefix="history-reference-"))

for _key in ("CONFIG_PATH", "LOG_PATH", "ALARMS_PATH", "USER_SOUNDS", "DEBUG_HISTORY_PATH"):
    os.environ.setdefault(_key, str(WORK / _key.lower()))
os.environ.setdefault("HISTORY_PATH", str(WORK / "default-history"))
os.environ.setdefault("USER_DB_MIGRATION_DIR", str(WORK / "default-dbmigrations"))
os.environ.setdefault("REDACTION_KEY_PATH", str(WORK / ".redaction_key"))
os.environ.setdefault("SMOKE_VALIDATION_STATE_FILE", str(WORK / "smoke.json"))
sys.dont_write_bytecode = True
sys.path.insert(0, str(REPO_ROOT))
os.chdir(REPO_ROOT)

import logging  # noqa: E402

import config  # noqa: E402
import db_migration_updater  # noqa: E402
import shot_database  # noqa: E402
from database_models import metadata  # noqa: E402
from shot_database import ShotDataBase  # noqa: E402

logging.disable(logging.CRITICAL)

REVISIONS = [
    "ebb6a77afd0e",
    "0bdd1c635e7a",
    "1a598cd3ace3",
    "470a6d3b0f44",
    "8f4e7b2c9d10",
    "b71d84a4c2ef",
    "c4d2e9f1a7b3",
]
HEAD = REVISIONS[-1]
FTS_SHADOW_SUFFIXES = ("_data", "_idx", "_content", "_docsize", "_config")


class Env:
    """One machine: a history directory, a backup directory and a copy of alembic/."""

    def __init__(self, name: str, history: Path | None = None, backup: Path | None = None):
        self.root = WORK / name
        if self.root.exists():
            shutil.rmtree(self.root)
        self.root.mkdir(parents=True)
        self.history = history or self.root / "history"
        self.history.mkdir(parents=True, exist_ok=True)
        self.backup = backup or self.root / "dbmigrations"
        self.alembic_dir = self.root / "alembic"
        shutil.copytree(
            REPO_ROOT / "alembic",
            self.alembic_dir,
            ignore=shutil.ignore_patterns("__pycache__"),
        )
        self.db_file = self.history / config.DATABASE_FILE
        url = f"sqlite:///{self.db_file}"
        config.HISTORY_PATH = str(self.history)
        db_migration_updater.DATABASE_URL = url
        db_migration_updater.ALEMBIC_DIR = str(self.alembic_dir)
        db_migration_updater.ALEMBIC_VER_DIR = str(self.alembic_dir / "versions")
        db_migration_updater.USER_DB_MIGRATION_DIR = str(self.backup)
        db_migration_updater.DB_VERSION_REQUIRED = HEAD
        shot_database.HISTORY_PATH = str(self.history)
        shot_database.DATABASE_URL = url
        shot_database.ABSOLUTE_DATABASE_FILE = self.db_file
        shot_database.SHOT_PATH = self.history / "shots"
        if ShotDataBase.engine is not None:
            ShotDataBase.engine.dispose()
        ShotDataBase.engine = None
        ShotDataBase.session = None
        ShotDataBase.profile_fts_table = None
        ShotDataBase.stage_fts_table = None
        for table in ("profile_fts", "stage_fts"):
            if table in metadata.tables:
                metadata.remove(metadata.tables[table])

    def alembic_config(self):
        from alembic.config import Config

        cfg = Config(db_migration_updater.ALEMBIC_CONFIG_FILE_PATH)
        cfg.set_main_option("script_location", str(self.alembic_dir))
        cfg.attributes["configure_logger"] = False
        return cfg

    def init(self):
        ShotDataBase.init()

    def boot(self):
        """back.run(): ShotManager.init() opens the database, then the migration."""
        ShotDataBase.init()
        try:
            db_migration_updater.update_db_migrations()
        except Exception as error:  # noqa: BLE001 - the reference records it
            return f"{type(error).__name__}: {error}"
        return None

    def upgrade(self, revision):
        from alembic import command

        command.upgrade(self.alembic_config(), revision)

    def downgrade(self, revision):
        from alembic import command

        try:
            command.downgrade(self.alembic_config(), revision)
        except Exception as error:  # noqa: BLE001
            return f"{type(error).__name__}: {error}"
        return None

    def connect(self):
        return sqlite3.connect(self.db_file)

    def close(self):
        if ShotDataBase.engine is not None:
            ShotDataBase.engine.dispose()


def cell(value):
    if isinstance(value, bytes):
        return ["blob", value.hex()]
    if value is None:
        return ["null", None]
    if isinstance(value, bool):
        raise TypeError("sqlite3 never returns bool")
    if isinstance(value, int):
        return ["integer", value]
    if isinstance(value, float):
        return ["real", repr(value)]
    return ["text", value]


def dump(db_file: Path) -> dict:
    """Everything a schema or data comparison needs, in a stable order."""
    connection = sqlite3.connect(db_file)
    try:
        master = connection.execute(
            "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name"
        ).fetchall()
        tables = [
            name
            for kind, name, _tbl, sql in master
            if kind == "table"
            and not name.startswith("sqlite_")
            and not name.endswith(FTS_SHADOW_SUFFIXES)
        ]
        rows = {}
        for table in tables:
            query = f'SELECT rowid, * FROM "{table}" ORDER BY rowid'
            if table == "alembic_version":
                query = 'SELECT * FROM "alembic_version"'
            cursor = connection.execute(query)
            columns = [d[0] for d in cursor.description]
            rows[table] = {
                "columns": columns,
                "rows": [[cell(v) for v in row] for row in cursor.fetchall()],
            }
        return {
            "master": [
                {"type": kind, "name": name, "tbl_name": tbl, "sql": sql}
                for kind, name, tbl, sql in master
            ],
            "rows": rows,
            "auto_vacuum": connection.execute("PRAGMA auto_vacuum").fetchone()[0],
            "journal_mode": connection.execute("PRAGMA journal_mode").fetchone()[0],
        }
    finally:
        connection.close()


def _line(value) -> str:
    return json.dumps(value, separators=(",", ":"))


def _block(value) -> str:
    """One entry of a list or dict per line."""
    if isinstance(value, dict):
        lines = [f"{json.dumps(key)}: {_line(item)}" for key, item in value.items()]
        return "{\n" + ",\n".join(lines) + "\n}"
    if isinstance(value, list):
        return "[\n" + ",\n".join(_line(item) for item in value) + "\n]"
    return _line(value)


def compact(value) -> str:
    """The reference files: each top-level key on its own lines, one entry per line."""
    if isinstance(value, dict):
        lines = [f"{json.dumps(key)}: {_block(item)}" for key, item in value.items()]
        return "{\n" + ",\n".join(lines) + "\n}"
    return _block(value)


def write_json(name: str, value):
    path = DATA_DIR / name
    path.write_text(compact(value) + "\n", encoding="utf-8")
    print(f"wrote {path.relative_to(REPO_ROOT)}")
