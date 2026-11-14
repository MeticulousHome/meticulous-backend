"""Read a history the Rust store wrote, with the Python code (crates/history tests).

    read_rust_written.py <history_dir> <backup_dir> <queries.json> <answers.json>

Answers the queries in queries.json with ShotDataBase, PourOverHistoryManager,
the bug report functions and alembic, then rolls the database back one
revision the way an older Python image does: with the newest revision script
missing from its alembic/ and only the copy the Rust store left in the backup
directory.
"""

import asyncio
import io
import json
import sys
from pathlib import Path
from typing import cast

from pyenv import Env

import db_migration_updater
import pour_over_history
from alembic import command
from alembic.runtime.migration import MigrationContext
from api.history import ZstdHistoryHandler
from pour_over_history import PourOverHistoryManager
from shot_database import SearchParams, ShotDataBase
from sqlalchemy import create_engine, inspect


class FakeHandler:
    def __init__(self):
        self.headers = {}
        self.body = b""

    def set_header(self, key, value):
        self.headers[key] = value

    def write(self, chunk):
        self.body += chunk

    def finish(self):
        pass


def revision(env):
    engine = create_engine(f"sqlite:///{env.db_file}")
    with engine.connect() as connection:
        current = MigrationContext.configure(connection).get_current_revision()
    engine.dispose()
    return current


def main():
    import api.bug_report as bug_report

    history, backup, queries_path, answers_path = (Path(a) for a in sys.argv[1:5])
    queries = json.loads(queries_path.read_text(encoding="utf-8"))
    env = Env("read-rust-written", history=history, backup=backup)
    pour_over_history.POUR_OVER_PATH = history / "pour-over"

    answers = {"revision_before": revision(env)}
    cfg = env.alembic_config()
    cfg.stdout = io.StringIO()
    command.current(cfg)
    answers["alembic_current"] = cfg.stdout.getvalue().strip()

    # The Python boot on this database: nothing to migrate.
    answers["boot_error"] = env.boot()
    answers["revision_after_boot"] = revision(env)

    answers["search"] = [
        ShotDataBase.search_history(SearchParams(**params)) for params in queries["search"]
    ]
    answers["statistics"] = ShotDataBase.statistics()
    answers["autocomplete"] = [
        ShotDataBase.autocomplete_profile_name(prefix) for prefix in queries["autocomplete"]
    ]
    answers["ratings"] = [ShotDataBase.get_shot_rating(uuid) for uuid in queries["ratings"]]
    answers["upload_index"] = ShotDataBase.list_history_files(None, 200)
    served = []
    for file in queries["served"]:
        handler = FakeHandler()
        asyncio.run(
            ZstdHistoryHandler.serve_decompressed_file(
                cast(ZstdHistoryHandler, handler), str(history / "shots" / file)
            )
        )
        served.append(handler.body.decode())
    answers["served"] = served
    answers["pour_over_search"] = PourOverHistoryManager.search()
    answers["pour_over_latest"] = PourOverHistoryManager.latest()
    records = []
    for row in PourOverHistoryManager.search():
        records.append(
            json.loads(
                pour_over_history.read_compressed_record(history / "pour-over" / row["file"])
            )
        )
    answers["pour_over_records"] = records
    answers["bug_report_page"] = bug_report._list_report_page(0, 10)
    answers["dispatch"] = [
        bug_report._dispatch_payload(r) for r in bug_report._queued_report_rows()
    ]
    env.close()

    # Roll back to the image before the newest revision.
    newest = db_migration_updater.DB_VERSION_REQUIRED
    script = next((env.alembic_dir / "versions").glob(f"*{newest}*.py"))
    script.unlink()
    previous = "b71d84a4c2ef"
    db_migration_updater.DB_VERSION_REQUIRED = previous
    try:
        answers["rollback_error"] = env.boot()
    finally:
        db_migration_updater.DB_VERSION_REQUIRED = newest
    answers["revision_after_rollback"] = revision(env)
    engine = create_engine(f"sqlite:///{env.db_file}")
    answers["bug_report_columns_after_rollback"] = [
        column["name"] for column in inspect(engine).get_columns("bug_reports")
    ]
    engine.dispose()
    env.close()

    answers_path.write_text(json.dumps(answers, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
