from alembic import command, util
from alembic.config import Config
from alembic.runtime.migration import MigrationContext
from alembic.script import ScriptDirectory
from sqlalchemy import create_engine
from log import MeticulousLogger
from shot_database import DATABASE_URL
import os
import shutil

from collections.abc import Sequence

logger = MeticulousLogger.getLogger(__name__)

DB_VERSION_REQUIRED = "1a598cd3ace3"

USER_DB_MIGRATION_DIR = "/meticulous-user/.dbmigrations"
BASE_DIR = os.path.dirname(os.path.abspath(__file__))
ALEMBIC_CONFIG_FILE_PATH = os.path.join(BASE_DIR, "alembic.ini")
ALEMBIC_DIR = os.path.join(BASE_DIR, "alembic")
ALEMBIC_VER_DIR = os.path.join(ALEMBIC_DIR, "versions")


REQUIRED_MOD_ATTRS = {
    "revision": str,
    "down_revision": str,
    "branch_labels": str | Sequence,
    "depends_on": str | Sequence,
    "upgrade": "function",
    "downgrade": "function",
}

VALID_SCRIPT_EXT = [".py", ".pyc", ".pyo"]


def retrieve_scripts(backup_dir: os.path) -> list[str]:
    """
    Looks into the migration scripts backup directory `backup_dir`
    for valid migration scripts that are not present in the alembic versions directory
    """

    if not os.path.exists(backup_dir):
        logger.warning(f"{backup_dir} does not exist")
        return []

    # Get all valid files list in backup dir and versions dir
    backup_files = os.listdir(USER_DB_MIGRATION_DIR)
    version_files = os.listdir(ALEMBIC_VER_DIR)

    files_to_retrieve = [
        filename for filename in backup_files if filename not in version_files
    ]

    if len(files_to_retrieve) == 0:
        logger.info("no missing DB migration files to retrieve")
        return []

    retrieved_files = []
    for file in files_to_retrieve:
        if not is_valid_revision_script(USER_DB_MIGRATION_DIR, file):
            continue
        full_script_path = os.path.join(USER_DB_MIGRATION_DIR, file)
        try:
            shutil.copy(full_script_path, ALEMBIC_VER_DIR)
            retrieved_files.append(file)
        except Exception as e:
            logger.warning(f"Failed to copy {file} for retrieval: {e}")
    return retrieved_files


def is_valid_revision_script(dir_path: str, filename: str) -> bool:
    """
    Checks if the script `filename` at `dirpath` is a valid migration script, for it to be a
    valid script, it must be a python file with the following attributes present

    {
        "revision": str,
        "down_revision": str | NoneType,
        "branch_labels": str | Sequence[str] | NoneType,
        "depends_on": str | Sequence[str] | NoneType,
        "upgrade": callable,
        "downgrade": callable,
    }
    """

    full_path: str = os.path.join(dir_path, filename)
    _, ext = os.path.splitext(filename)

    if (
        not os.path.exists(full_path)
        or not os.path.isfile(full_path)
        or ext not in VALID_SCRIPT_EXT
    ):
        logger.warning(f"Error on file {full_path}: Not a valid file")
        return False

    script_module = util.load_python_file(dir_path, filename)
    if script_module is None:
        logger.warning(f"[{full_path} script failed to load")
        return False

    for attr, expected_type in REQUIRED_MOD_ATTRS.items():

        if not hasattr(script_module, attr):
            logger.warning(f"Invalid script at {full_path} ")
            logger.warning(f"Missing [{attr} : {expected_type}]")
            # return None
            return False
        value = getattr(script_module, attr)

        # this attributes cannot be None
        if value is None:
            if attr in ["revision"]:
                return False
            else:
                continue

        if expected_type == "function":
            if not callable(value) or (value.__name__ == "<lambda>"):
                logger.warning(f"Invalid script at {full_path}")
                logger.warning(f"Invalid attribute. Expected [{attr} : function]")
                return False
        elif not isinstance(value, expected_type):
            logger.warning(f"Invalid script at {full_path}")
            logger.warning(f"Invalid attribute [{attr} : {expected_type}]")
            return False
        # further check for Sequences
        if isinstance(value, Sequence):
            if not all(isinstance(x, str) for x in value):
                return False
    return True


def backup_new_scripts() -> list[str]:
    """
    Cleans the backup directory of invalid migration scripts to then copy all the
    valid migration scripts that are present in the `alembic/versions` directory and are
    missing in the backup directory
    """

    os.makedirs(USER_DB_MIGRATION_DIR, exist_ok=True)

    # Get all valid files list in backup dir and versions dir
    backup_files = os.listdir(USER_DB_MIGRATION_DIR)
    version_files = os.listdir(ALEMBIC_VER_DIR)

    # Validate and clean invalid scripts in backup folder
    for backup in backup_files:
        if os.path.isfile(
            os.path.join(USER_DB_MIGRATION_DIR, backup)
        ) and not is_valid_revision_script(USER_DB_MIGRATION_DIR, backup):
            logger.warning(f"removing invalid migration file {backup} from backup")
            full_path = os.path.join(USER_DB_MIGRATION_DIR, backup)
            os.remove(full_path)

    backup_files = os.listdir(USER_DB_MIGRATION_DIR)
    files_to_backup = [
        filename for filename in version_files if filename not in backup_files
    ]

    if len(files_to_backup) == 0:
        logger.info("no new DB migration files to backup")
        return

    backed_up_files = []
    for file in files_to_backup:
        if not is_valid_revision_script(ALEMBIC_VER_DIR, file):
            continue
        full_path = os.path.join(ALEMBIC_VER_DIR, file)
        try:
            shutil.copy(full_path, USER_DB_MIGRATION_DIR)
        except Exception as e:
            logger.warning(f"failed to copy {file} for backup: {e}")
    return backed_up_files


def update_db_migrations():
    """Update database schema to target version using Alembic migrations."""

    retrieved_files: list[str] = []
    current_rev = ""
    try:
        logger.info("Starting database migration process")

        alembic_cfg = Config(ALEMBIC_CONFIG_FILE_PATH)
        alembic_cfg.set_main_option("script_location", ALEMBIC_DIR)
        alembic_cfg.attributes["configure_logger"] = False

        SD = ScriptDirectory.from_config(alembic_cfg)

        engine = create_engine(DATABASE_URL)
        with engine.connect() as connection:
            current_rev = MigrationContext.configure(connection).get_current_revision()
            logger.info(f"current DB revision: {current_rev}")

        # the target revision is the required DB version for the backend
        target_rev = DB_VERSION_REQUIRED
        logger.info(f"Target revision set to: {target_rev}")

        # Backup new scripts
        backup_new_scripts()

        # Exit if no migration is needed
        if current_rev == target_rev:
            logger.info("Database is already at target version")
            return

        # Try the migration with current scripts in alembic revs version dir

        ordered_revisions = []
        walk_failed = False
        try:
            for rev in SD.walk_revisions():
                revision = rev.module.revision
                if (
                    revision in [target_rev, current_rev]
                    and revision not in ordered_revisions
                ):
                    ordered_revisions.append(revision)
                    if len(ordered_revisions) == 2:
                        break
        except Exception:
            walk_failed = True

        # if at least one script misses retrieve data from backup or the SD walk fails
        if walk_failed or len(ordered_revisions) < 2:

            missing_scripts = [
                script
                for script in [target_rev, current_rev]
                if script not in ordered_revisions
            ]
            logger.warning(
                f"missing migration script for revision{'s' if len(missing_scripts) > 1 else ''} [{' '.join(missing_scripts)}]"
            )
            retrieved_files = retrieve_scripts(USER_DB_MIGRATION_DIR)
            logger.info("Files retrieved:")
            for file in retrieved_files:
                logger.info(f" - {file}")
            # retry with the updated ScriptDirectory

            ordered_revisions = []

            SD = ScriptDirectory.from_config(alembic_cfg)

            # If the walk fails again, we just report it
            for rev in SD.walk_revisions():
                revision = rev.module.revision
                if (
                    revision in [target_rev, current_rev]
                    and revision not in ordered_revisions
                ):
                    ordered_revisions.append(revision)
                    if len(ordered_revisions) == 2:
                        break

            if len(ordered_revisions) < 2:
                raise Exception("Cannot retrieve missing scripts")

        is_downgrade: bool = (
            current_rev == ordered_revisions[0]
        )  # highest version against target version
        migration = "downgrade" if is_downgrade else "upgrade"

        logger.info(f"Starting {migration} from {current_rev} to {target_rev} (head)")
        if not is_downgrade:
            command.upgrade(alembic_cfg, target_rev)
        else:
            command.downgrade(alembic_cfg, target_rev)
            logger.info(f"Database {migration} completed successfully")

    except Exception as e:
        logger.error(f"Database migration failed: {str(e)}", exc_info=True)
        raise

    finally:
        logger.info("Migration process finished")
