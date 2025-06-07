"""Add history_uuid to shot annotation

Revision ID: 1a598cd3ace3
Revises: 0bdd1c635e7a
Create Date: 2025-05-05 22:00:42.393323

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision: str = "1a598cd3ace3"
down_revision: Union[str, None] = "0bdd1c635e7a"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:

    # Get the table constraints up to date with the new naming convention
    with op.batch_alter_table("shot_annotation", schema=None) as batch_op:
        batch_op.create_unique_constraint(None, ["history_id"])
        batch_op.create_foreign_key(None, "history", ["history_id"], ["id"])

    with op.batch_alter_table("shot_rating", schema=None) as batch_op:
        batch_op.create_unique_constraint(None, ["annotation_id"])

    # Add history_uuid to shot_annotation
    with op.batch_alter_table("shot_annotation", schema=None) as batch_op:
        batch_op.add_column(sa.Column("history_uuid", sa.Text(), nullable=False))
        batch_op.create_unique_constraint(None, ["history_uuid"])
        batch_op.create_foreign_key(None, "history", ["history_uuid"], ["uuid"])


def downgrade() -> None:

    with op.batch_alter_table("shot_annotation", schema=None) as batch_op:

        batch_op.drop_constraint("fk_shot_annotation_history_id_history")
        batch_op.drop_constraint("uq_shot_annotation_history_id")
        batch_op.drop_constraint("fk_shot_annotation_history_uuid_history")
        batch_op.drop_constraint("uq_shot_annotation_history_uuid")
        batch_op.drop_column("history_uuid")

    # For shot_rating, if you upgraded it, you should also downgrade it.
    with op.batch_alter_table("shot_rating", schema=None) as batch_op:
        batch_op.drop_constraint("fk_shot_rating_annotation_id_shot_annotation")
        batch_op.drop_constraint("uq_shot_rating_annotation_id")
