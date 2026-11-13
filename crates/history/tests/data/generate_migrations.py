"""Reference databases for crates/history's migration engine, made by alembic.

Writes migrations.json: for every scenario, the resulting sqlite_master, the
rows of every table, the alembic revision and the error alembic raised, if any.
The Rust tests replay each scenario with the Rust engine and compare.

    uv run python crates/history/tests/data/generate_migrations.py
"""

import json

from pyenv import HEAD, REVISIONS, DATA_DIR, Env, dump, write_json

SAMPLE_ROWS = json.loads((DATA_DIR / "sample_rows.json").read_text(encoding="utf-8"))


def rows_for(revision: str, ratings: bool = True):
    index = REVISIONS.index(revision)
    for entry in SAMPLE_ROWS:
        since = REVISIONS.index(entry["since"])
        until = REVISIONS.index(entry.get("until", HEAD))
        if since <= index <= until and (ratings or not entry.get("ratings", False)):
            yield entry["table"], entry["row"]


def insert_sample(env: Env, revision: str, ratings: bool = True):
    connection = env.connect()
    try:
        for table, row in rows_for(revision, ratings):
            columns = ", ".join(f'"{name}"' for name in row)
            marks = ", ".join("?" for _ in row)
            connection.execute(
                f'INSERT INTO "{table}" ({columns}) VALUES ({marks})', list(row.values())
            )
        connection.commit()
    finally:
        connection.close()


# The schema the Python backend created before alembic existed (shot_database.py
# at c7162db^, Dec 2024 - Feb 2025): metadata.create_all() of these tables.
def pre_alembic_metadata(ratings_table: bool):
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
    )

    old = MetaData()
    Table(
        "profile",
        old,
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
    Table(
        "history",
        old,
        Column("id", Integer, primary_key=True, autoincrement=True),
        Column("uuid", Text, nullable=True),
        Column("file", Text, nullable=False),
        Column("time", DateTime, nullable=False),
        Column("profile_name", Text, nullable=False),
        Column("profile_id", String, nullable=False),
        Column("profile_key", Integer, ForeignKey("profile.key"), nullable=False),
    )
    if ratings_table:
        Table(
            "shot_ratings",
            old,
            Column("id", Integer, primary_key=True, autoincrement=True),
            Column(
                "history_id",
                Integer,
                ForeignKey("history.id", ondelete="CASCADE"),
                nullable=False,
            ),
            Column("rating", String, nullable=False, server_default="unrated"),
        )
    return old


def pre_alembic(env: Env, ratings_table: bool):
    from sqlalchemy import create_engine

    engine = create_engine(f"sqlite:///{env.db_file}")
    pre_alembic_metadata(ratings_table).create_all(engine)
    engine.dispose()
    connection = env.connect()
    try:
        connection.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS profile_fts USING fts5(profile_key, profile_id, name)"
        )
        connection.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS stage_fts USING fts5(profile_key, profile_id, profile_name, stage_key, stage_name)"
        )
        connection.commit()
    finally:
        connection.close()
    insert_sample(env, "ebb6a77afd0e")
    if ratings_table:
        connection = env.connect()
        connection.execute(
            "INSERT INTO shot_ratings (id, history_id, rating) VALUES (1, 1, 'good')"
        )
        connection.commit()
        connection.close()


def legacy_annotations(env: Env, ratings: bool):
    """0bdd1c635e7a as the Feb-May 2025 images ran it, before the naming convention."""
    from sqlalchemy import (
        Column,
        ForeignKeyConstraint,
        Integer,
        MetaData,
        PrimaryKeyConstraint,
        Table,
        Text,
        UniqueConstraint,
        create_engine,
    )

    env.init()
    env.upgrade("ebb6a77afd0e")
    old = MetaData()
    Table("history", old, Column("id", Integer), Column("uuid", Text))
    annotation = Table(
        "shot_annotation",
        old,
        Column("id", Integer, nullable=False),
        Column("history_id", Integer, nullable=False),
        ForeignKeyConstraint(["history_id"], ["history.id"]),
        PrimaryKeyConstraint("id"),
        UniqueConstraint("history_id"),
    )
    Table("shot_annotation_ref", old)
    rating = Table(
        "shot_rating",
        old,
        Column("id", Integer, nullable=False),
        Column("annotation_id", Integer, nullable=False),
        Column("basic", Text, nullable=True),
        ForeignKeyConstraint(["annotation_id"], ["shot_annotation.id"]),
        PrimaryKeyConstraint("id"),
        UniqueConstraint("annotation_id"),
    )
    engine = create_engine(f"sqlite:///{env.db_file}")
    annotation.create(engine)
    rating.create(engine)
    engine.dispose()
    connection = env.connect()
    connection.execute("UPDATE alembic_version SET version_num = '0bdd1c635e7a'")
    connection.commit()
    connection.close()
    insert_sample(env, "0bdd1c635e7a", ratings)


def main():
    reference = {"revisions": REVISIONS, "scenarios": {}}
    scenarios = reference["scenarios"]

    def record(name, env, error=None, **extra):
        env.close()
        result = {"error": error, **extra, **dump(env.db_file)}
        scenarios[name] = result

    # A new machine boots: ShotDataBase.init() then update_db_migrations().
    env = Env("boot-new")
    error = env.boot()
    record(
        "boot_new",
        env,
        error,
        backup=sorted(p.name for p in env.backup.iterdir()),
    )

    # The database at each revision, as the alembic chain builds it after init().
    for revision in REVISIONS:
        env = Env(f"fresh-{revision}")
        env.init()
        env.upgrade(revision)
        record(f"fresh/{revision}", env)

    # Back down from the head to each revision (a rollback to an older image).
    for revision in REVISIONS[:-1]:
        env = Env(f"downgrade-{revision}")
        env.init()
        env.upgrade(HEAD)
        error = env.downgrade(revision)
        record(f"downgrade/{revision}", env, error)

    # A machine at each older revision with history in it boots the newest image.
    for revision in REVISIONS[:-1]:
        variants = [True, False] if revision == "0bdd1c635e7a" else [True]
        for ratings in variants:
            name = f"data_upgrade/{revision}" + ("" if ratings else "/no_ratings")
            env = Env(name.replace("/", "-"))
            env.init()
            env.upgrade(revision)
            insert_sample(env, revision, ratings)
            error = env.boot()
            record(name, env, error)

    # A machine at the head with history rolls back to each older revision, and
    # then updates again.
    for revision in REVISIONS[:-1]:
        env = Env(f"data-downgrade-{revision}")
        env.init()
        env.upgrade(HEAD)
        insert_sample(env, HEAD)
        error = env.downgrade(revision)
        record(f"data_downgrade/{revision}", env, error)
        error = env.boot()
        record(f"data_roundtrip/{revision}", env, error)

    # A machine whose database predates alembic.
    for ratings_table in (False, True):
        name = "pre_alembic" + ("/shot_ratings" if ratings_table else "")
        env = Env(name.replace("/", "-"))
        pre_alembic(env, ratings_table)
        error = env.boot()
        record(name, env, error)

    # Feb-May 2025 machines: shot_annotation without constraint names.
    for ratings in (False, True):
        name = "legacy_0bdd1c635e7a" + ("/ratings" if ratings else "")
        env = Env(name.replace("/", "-"))
        legacy_annotations(env, ratings)
        error = env.boot()
        record(name, env, error)

    # A pre-alembic database brought to the head, then rolled back.
    for revision in ("1a598cd3ace3", "ebb6a77afd0e"):
        env = Env(f"pre-alembic-downgrade-{revision}")
        pre_alembic(env, False)
        env.boot()
        error = env.downgrade(revision)
        record(f"pre_alembic_downgrade/{revision}", env, error)

    write_json("migrations.json", reference)


if __name__ == "__main__":
    main()
