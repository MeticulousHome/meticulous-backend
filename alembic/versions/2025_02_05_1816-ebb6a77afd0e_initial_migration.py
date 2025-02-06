"""initial_migration

Revision ID: ebb6a77afd0e
Revises:
Create Date: 2025-02-05 18:16:32.626023

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy import inspect

# revision identifiers, used by Alembic.
revision: str = "ebb6a77afd0e"
down_revision: Union[str, None] = None
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def table_exists(table_name):
    """Helper function to check if a table exists"""
    inspector = inspect(op.get_bind())
    return table_name in inspector.get_table_names()


def upgrade() -> None:
    if not table_exists("profile"):
        op.create_table(
            "profile",
            sa.Column("key", sa.Integer(), nullable=False),
            sa.Column("id", sa.Text(), nullable=False),
            sa.Column("author", sa.Text(), nullable=True),
            sa.Column("author_id", sa.Text(), nullable=True),
            sa.Column("display", sa.JSON(), nullable=True),
            sa.Column("final_weight", sa.Integer(), nullable=True),
            sa.Column("last_changed", sa.Float(), nullable=True),
            sa.Column("name", sa.Text(), nullable=True),
            sa.Column("temperature", sa.Integer(), nullable=True),
            sa.Column("stages", sa.JSON(), nullable=True),
            sa.Column("variables", sa.JSON(), nullable=True),
            sa.Column("previous_authors", sa.JSON(), nullable=True),
            sa.PrimaryKeyConstraint("key"),
        )

    if not table_exists("history"):
        op.create_table(
            "history",
            sa.Column("id", sa.Integer(), nullable=False),
            sa.Column("uuid", sa.Text(), nullable=True),
            sa.Column("file", sa.Text(), nullable=False),
            sa.Column("time", sa.DateTime(), nullable=False),
            sa.Column("profile_name", sa.Text(), nullable=False),
            sa.Column("profile_id", sa.Text(), nullable=False),
            sa.Column("profile_key", sa.Integer(), nullable=False),
            sa.ForeignKeyConstraint(
                ["profile_key"],
                ["profile.key"],
            ),
            sa.PrimaryKeyConstraint("id"),
        )

    op.execute(
        """
        CREATE VIRTUAL TABLE IF NOT EXISTS profile_fts
        USING fts5(
            profile_key,
            profile_id,
            name
        )
    """
    )

    op.execute(
        """
        CREATE VIRTUAL TABLE IF NOT EXISTS stage_fts
        USING fts5(
            profile_key,
            profile_id,
            profile_name,
            stage_key,
            stage_name
        )
    """
    )

    op.execute("PRAGMA auto_vacuum=full")
    op.execute("PRAGMA journal_mode=WAL")
    op.execute("PRAGMA synchronous=EXTRA")
    op.execute("PRAGMA journal_size_limit = 1048576")
    op.execute("PRAGMA wal_checkpoint(TRUNCATE)")


def downgrade() -> None:
    pass
