"""Add history_uuid to annotations table

Revision ID: ed8465b1592c
Revises: 0bdd1c635e7a
Create Date: 2025-05-05 17:22:10.924676

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision: str = "ed8465b1592c"
down_revision: Union[str, None] = "0bdd1c635e7a"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "shot_annotation",
        sa.Column(
            "history_uuid",
            sa.Integer,
            sa.ForeignKey("history.uuid"),
            nullable=False,
            unique=True,
        ),
    )


def downgrade() -> None:
    op.drop_column("shot_annotation", "history_uuid")
