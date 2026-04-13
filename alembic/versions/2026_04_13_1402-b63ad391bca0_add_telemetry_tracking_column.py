"""add telemetry tracking column

Revision ID: b63ad391bca0
Revises: 470a6d3b0f44
Create Date: 2026-04-13 14:02:32.642544

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision: str = "b63ad391bca0"
down_revision: Union[str, None] = "470a6d3b0f44"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    with op.batch_alter_table("history", schema=None) as operation:
        operation.add_column(sa.Column("debug_file_sent", sa.Text(), nullable=True))


def downgrade() -> None:
    with op.batch_alter_table("history", schema=None) as operation:
        operation.drop_column("debug_file_sent")
