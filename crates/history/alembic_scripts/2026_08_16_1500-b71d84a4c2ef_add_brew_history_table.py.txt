"""add first-class brew history table

Revision ID: b71d84a4c2ef
Revises: 8f4e7b2c9d10
Create Date: 2026-08-16 15:00:00.000000

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "b71d84a4c2ef"
down_revision: Union[str, None] = "8f4e7b2c9d10"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "brew_history",
        sa.Column("id", sa.Integer(), nullable=False),
        sa.Column("uuid", sa.Text(), nullable=False),
        sa.Column("brew_type", sa.Text(), nullable=False),
        sa.Column("mode", sa.Text(), nullable=False),
        sa.Column("file", sa.Text(), nullable=False),
        sa.Column("time", sa.DateTime(), nullable=False),
        sa.Column("completed_time", sa.DateTime(), nullable=False),
        sa.Column("name", sa.Text(), nullable=False),
        sa.Column("schema_version", sa.Integer(), nullable=False),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("file"),
        sa.UniqueConstraint("uuid"),
    )
    op.create_index("ix_brew_history_completed_time", "brew_history", ["completed_time"])
    op.create_index("ix_brew_history_brew_type", "brew_history", ["brew_type"])


def downgrade() -> None:
    op.drop_index("ix_brew_history_brew_type", table_name="brew_history")
    op.drop_index("ix_brew_history_completed_time", table_name="brew_history")
    op.drop_table("brew_history")
