"""add bug report contact columns

Revision ID: c4d2e9f1a7b3
Revises: b71d84a4c2ef
Create Date: 2026-10-01 12:00:00.000000

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

# revision identifiers, used by Alembic.
revision: str = "c4d2e9f1a7b3"
down_revision: Union[str, None] = "b71d84a4c2ef"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    with op.batch_alter_table("bug_reports") as batch_op:
        batch_op.add_column(sa.Column("contactName", sa.Text(), nullable=True))
        batch_op.add_column(sa.Column("contactEmail", sa.Text(), nullable=True))


def downgrade() -> None:
    with op.batch_alter_table("bug_reports") as batch_op:
        batch_op.drop_column("contactEmail")
        batch_op.drop_column("contactName")
