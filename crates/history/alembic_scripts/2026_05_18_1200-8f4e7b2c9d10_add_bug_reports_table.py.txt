"""add bug reports table

Revision ID: 8f4e7b2c9d10
Revises: 470a6d3b0f44
Create Date: 2026-05-18 12:00:00.000000

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

# revision identifiers, used by Alembic.
revision: str = "8f4e7b2c9d10"
down_revision: Union[str, None] = "470a6d3b0f44"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "bug_reports",
        sa.Column("localID", sa.Text(), nullable=False),
        sa.Column("eventID", sa.Text(), nullable=True),
        sa.Column("baseEventID", sa.Text(), nullable=True),
        sa.Column("issueTime", sa.Integer(), nullable=False),
        sa.Column("creationTime", sa.Integer(), nullable=False),
        sa.Column("submissionTime", sa.Integer(), nullable=True),
        sa.Column("description", sa.Text(), nullable=True),
        sa.Column("multimedia", sa.Integer(), nullable=True),
        sa.Column("machineID", sa.Text(), nullable=True),
        sa.Column("logFiles", sa.Text(), nullable=True),
        sa.Column("machineInfo", sa.Boolean(), nullable=True),
        sa.Column("machineLogs", sa.Boolean(), nullable=True),
        sa.Column("machineStatus", sa.Boolean(), nullable=True),
        sa.Column("status", sa.Text(), nullable=False),
        sa.Column("ticketNumber", sa.Integer(), nullable=True),
        sa.PrimaryKeyConstraint("localID"),
    )
    op.create_index("ix_bug_reports_creationTime", "bug_reports", ["creationTime"])
    op.create_index("ix_bug_reports_status", "bug_reports", ["status"])


def downgrade() -> None:
    op.drop_index("ix_bug_reports_status", table_name="bug_reports")
    op.drop_index("ix_bug_reports_creationTime", table_name="bug_reports")
    op.drop_table("bug_reports")
