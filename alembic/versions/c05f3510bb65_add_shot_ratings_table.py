"""Add shot ratings table

Revision ID: c05f3510bb65
Revises: c5aab5b65bdb
Create Date: 2024-12-30 18:11:44.793752

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision: str = "c05f3510bb65"
down_revision: Union[str, None] = "c5aab5b65bdb"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "shot_ratings",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("history_id", sa.Integer, nullable=False),
        sa.Column("rating", sa.String, nullable=False, server_default="unrated"),
        sa.ForeignKeyConstraint(["history_id"], ["history.id"], ondelete="CASCADE"),
        sa.CheckConstraint(
            "rating IN ('good', 'bad', 'unrated')", name="valid_rating_check"
        ),
    )


def downgrade() -> None:
    op.drop_table("shot_ratings")
