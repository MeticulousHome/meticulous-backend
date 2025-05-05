"""Add history_uuid to annotations table

Revision ID: ed8465b1592c
Revises: 0bdd1c635e7a
Create Date: 2025-05-05 17:22:10.924676

"""

from typing import Sequence, Union

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "ed8465b1592c"
down_revision: Union[str, None] = "0bdd1c635e7a"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

HISTORY_UUID_CONSTRAINT_FOREIGN = "fk_history_uuid"


def upgrade() -> None:
    with op.batch_alter_table("shot_annotation", schema=None) as batch_op:
        batch_op.create_foreign_key(
            HISTORY_UUID_CONSTRAINT_FOREIGN, "history", ["history_uuid"], ["uuid"]
        )


def downgrade() -> None:
    with op.batch_alter_table("shot_annotation", schema=None) as batch_op:
        batch_op.drop_constraint(HISTORY_UUID_CONSTRAINT_FOREIGN, type_="foreignkey")
