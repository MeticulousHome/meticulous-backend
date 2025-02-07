from alembic import command
from alembic.config import Config
from alembic.runtime.migration import MigrationContext
from alembic.script import ScriptDirectory
from sqlalchemy import create_engine
from log import MeticulousLogger
from shot_database import DATABASE_URL
import os

logger = MeticulousLogger.getLogger(__name__)

# MIGRATION_VERSION_STABLE = "ebb6a77afd0e" -> Revision ID
MIGRATION_VERSION_STABLE = ""  # Empty string means use latest version (head)


def update_db_migrations():
    """Update database schema to target version using Alembic migrations."""
    try:
        logger.info("Starting database migration process")

        base_dir = os.path.dirname(os.path.abspath(__file__))
        alembic_cfg = Config(os.path.join(base_dir, "alembic.ini"))
        alembic_cfg.set_main_option(
            "script_location", os.path.join(base_dir, "alembic")
        )
        alembic_cfg.attributes["configure_logger"] = False

        script = ScriptDirectory.from_config(alembic_cfg)

        engine = create_engine(DATABASE_URL)
        with engine.connect() as connection:
            current_rev = MigrationContext.configure(connection).get_current_revision()

        target_rev = MIGRATION_VERSION_STABLE or "head"
        logger.info(f"Target revision set to: {target_rev}")

        if current_rev is None:
            logger.warning("No current revision found - performing initial migration")
            command.upgrade(alembic_cfg, target_rev)
            logger.info("Initial migration completed successfully")
            return

        if current_rev == target_rev:
            logger.info("Database is already at target version")
            return

        current_head = script.get_current_head()

        # If no specific version configured, use head
        if not MIGRATION_VERSION_STABLE:
            if current_rev == current_head:
                logger.info("Database is already at latest version")
                return
            else:
                logger.info(f"Starting upgrade from {current_rev} to head")
                command.upgrade(alembic_cfg, "head")
                logger.info("Database upgrade completed successfully")
        else:
            # If specific version configured
            if current_rev == current_head and target_rev != "head":
                logger.info(f"Starting downgrade from {current_rev} to {target_rev}")
                command.downgrade(alembic_cfg, target_rev)
                logger.info("Database downgrade completed successfully")
            else:
                logger.info(f"Starting upgrade from {current_rev} to {target_rev}")
                command.upgrade(alembic_cfg, target_rev)
                logger.info("Database upgrade completed successfully")

    except Exception as e:
        logger.error(f"Database migration failed: {str(e)}", exc_info=True)
        raise

    finally:
        logger.info("Migration process finished")
