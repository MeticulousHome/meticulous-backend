"""create 'debug_file' column to history table to link the shot with its debug file

Revision ID: 470a6d3b0f44
Revises: 1a598cd3ace3
Create Date: 2025-07-15 13:07:03.844816

"""

from typing import Sequence, Union

import sqlalchemy as sa

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "470a6d3b0f44"
down_revision: Union[str, None] = "1a598cd3ace3"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    with op.batch_alter_table("history", schema=None) as batch_op:
        batch_op.add_column(sa.Column("debug_file", sa.Text(), nullable=True))


def downgrade() -> None:
    with op.batch_alter_table("history", schema=None) as batch_op:
        batch_op.drop_column("debug_file")
