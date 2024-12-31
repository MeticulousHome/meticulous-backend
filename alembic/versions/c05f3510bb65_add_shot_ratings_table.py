"""Add shot ratings table with data preservation

Revision ID: c05f3510bb65
Revises: c5aab5b65bdb
Create Date: 2024-12-30 18:11:44.793752

"""

from typing import Sequence, Union
import json
import os
from datetime import datetime
from pathlib import Path

from alembic import op
import sqlalchemy as sa
from sqlalchemy import inspect, text

# revision identifiers, used by Alembic.
revision: str = "c05f3510bb65"
down_revision: Union[str, None] = "c5aab5b65bdb"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def get_backup_directory():
    """Create and return backup directory path"""
    backup_dir = Path("/meticulous-user/history/backup")
    backup_dir.mkdir(parents=True, exist_ok=True)
    return backup_dir


def backup_shot_ratings():
    """Backup existing shot ratings data"""
    try:
        conn = op.get_bind()
        inspector = inspect(conn)

        # Check if the table exists
        if "shot_ratings" not in inspector.get_table_names():
            print("No shot_ratings table found to backup")
            return None

        # Query all existing data
        result = conn.execute(text("SELECT history_id, rating FROM shot_ratings"))
        data = [{"history_id": row[0], "rating": row[1]} for row in result]

        if not data:
            print("No data found to backup in shot_ratings")
            return None

        # Create backup file with timestamp
        backup_dir = get_backup_directory()
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        backup_file = backup_dir / f"shot_ratings_backup_{timestamp}.json"

        # Save backup data
        with open(backup_file, "w") as f:
            json.dump(data, f, indent=2)

        print(f"Successfully backed up {len(data)} ratings to {backup_file}")
        return backup_file
    except Exception as e:
        print(f"Error during backup: {str(e)}")
        return None


def restore_shot_ratings(backup_file):
    """Restore shot ratings from backup"""
    if not backup_file or not os.path.exists(backup_file):
        print("No backup file found to restore")
        return

    try:
        # Read backup data
        with open(backup_file, "r") as f:
            data = json.load(f)

        if not data:
            print("No data found in backup file")
            return

        print(f"Starting data restoration from: {backup_file}")
        print(f"Found {len(data)} ratings to restore")

        # Insert data
        conn = op.get_bind()
        restored_count = 0

        for item in data:
            try:
                conn.execute(
                    text(
                        "INSERT INTO shot_ratings (history_id, rating) VALUES (:history_id, :rating)"
                    ),
                    item,
                )
                restored_count += 1
            except Exception as e:
                print(f"Error restoring record {item}: {str(e)}")
                continue

        print(f"Successfully restored {restored_count} of {len(data)} ratings")

    except Exception as e:
        print(f"Error during restoration: {str(e)}")


def recreate_shot_ratings_table():
    """Drop and recreate the shot_ratings table"""
    conn = op.get_bind()

    print("Dropping existing shot_ratings table if exists...")
    conn.execute(text("DROP TABLE IF EXISTS shot_ratings"))

    print("Creating new shot_ratings table...")
    op.create_table(
        "shot_ratings",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("history_id", sa.Integer, nullable=False),
        sa.Column("rating", sa.String, nullable=False, server_default="unrated"),
        sa.ForeignKeyConstraint(["history_id"], ["history.id"], ondelete="CASCADE"),
        sa.CheckConstraint(
            'rating IN ("good", "bad", "unrated")', name="valid_rating_check"
        ),
    )
    print("Shot_ratings table created successfully")


def upgrade() -> None:
    print("Starting upgrade process...")

    # Check if we have a previous backup to restore
    backup_info_file = get_backup_directory() / "last_backup_file.txt"
    existing_backup = None

    if backup_info_file.exists():
        with open(backup_info_file, "r") as f:
            backup_path = f.read().strip()
            if os.path.exists(backup_path):
                existing_backup = backup_path
                print(f"Found existing backup at: {existing_backup}")

    if not existing_backup:
        # Only try new backup if we don't have an existing one
        backup_file = backup_shot_ratings()
        if backup_file:
            existing_backup = str(backup_file)
            with open(backup_info_file, "w") as f:
                f.write(str(backup_file))

    # Always recreate the table
    recreate_shot_ratings_table()

    # Restore data if we have any backup
    if existing_backup:
        restore_shot_ratings(existing_backup)

    print("Upgrade process completed")


def downgrade() -> None:
    print("Starting downgrade process...")

    # Backup before dropping
    backup_file = backup_shot_ratings()

    if backup_file:
        print(f"Data backed up to: {backup_file}")

        # Save backup path for future upgrades
        backup_info_file = get_backup_directory() / "last_backup_file.txt"
        with open(backup_info_file, "w") as f:
            f.write(str(backup_file))
    else:
        print("No data to back up")

    # Drop the table
    print("Dropping shot_ratings table...")
    conn = op.get_bind()
    conn.execute(text("DROP TABLE IF EXISTS shot_ratings"))
    print("Downgrade completed successfully")
