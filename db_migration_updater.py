from alembic import command, util
from alembic.config import Config
from alembic.runtime.migration import MigrationContext
from alembic.script import ScriptDirectory
from alembic.script.revision import ResolutionError
from sqlalchemy import create_engine
from log import MeticulousLogger
from shot_database import DATABASE_URL
import os
import shutil

logger = MeticulousLogger.getLogger(__name__)

# MIGRATION_VERSION_STABLE = "ebb6a77afd0e" -> Revision ID
MIGRATION_VERSION_STABLE = ""  # Empty string means use latest version (head)

USER_DB_MIGRATION_DIR = "/meticulous-user/.dbmigrations"

BASE_DIR = os.path.dirname(os.path.abspath(__file__))
ALEMBIC_CONFIG_FILE_PATH = os.path.join(BASE_DIR, "alembic.ini")
ALEMBIC_DIR = os.path.join(BASE_DIR, "alembic")


def retrieve_revision_script(dir: os.path, revision_id: str) -> tuple[str | None]:
    if os.path.exists(dir):
        files = os.listdir(dir)
        for script in files:
            _, ext = os.path.splitext(script)
            full_script_path = os.path.join(dir, script)
            if ext in [".py", ".pyc", ".pyo"] and os.path.isfile(full_script_path):
                # script_module = load_module_py(revision_id, full_script_path)
                script_module = util.load_python_file(dir, script)
                if script_module is None:
                    continue
                if hasattr(script_module, "revision") and hasattr(
                    script_module, "down_revision"
                ):
                    if script_module.revision == revision_id:
                        shutil.copy(
                            full_script_path, os.path.join(ALEMBIC_DIR, "versions")
                        )
                        return str(script), str(script_module.down_revision)
        return None, "ENOFOUND"
    return None, "ENODIR"


def clear_retrieved_scripts(files: list[str]):
    versions_dir = os.path.join(ALEMBIC_DIR, "versions")
    logger.info("\n==== removing extra migration scripts ====\n\n")

    if files.__len__() == 0:
        logger.info("no scripts retrieved")
        return

    for file in files:
        file_abs_path = os.path.join(versions_dir, file)
        if os.path.isfile(file_abs_path):
            logger.info(f" * {file_abs_path}")
            os.remove(file_abs_path)


def update_db_migrations():
    """Update database schema to target version using Alembic migrations."""

    retrieved_ids: list[str] = []
    retrieved_files: list[str] = []
    current_rev = ""
    try:
        logger.info("Starting database migration process")

        alembic_cfg = Config(ALEMBIC_CONFIG_FILE_PATH)
        alembic_cfg.set_main_option("script_location", ALEMBIC_DIR)
        alembic_cfg.attributes["configure_logger"] = False

        script = ScriptDirectory.from_config(alembic_cfg)

        engine = create_engine(DATABASE_URL)
        with engine.connect() as connection:
            current_rev = MigrationContext.configure(connection).get_current_revision()
            logger.info(f"current DB revision: {current_rev}")

        # the target revision is the latest migration path provided revision in the folder of version
        target_rev = script.get_current_head()

        logger.info(f"Target revision set to: {target_rev}")

        # update the migration scripts in the user space
        versions_path = os.path.join(BASE_DIR, "alembic", "versions")
        migration_scripts = os.listdir(versions_path)

        os.makedirs(USER_DB_MIGRATION_DIR, exist_ok=True)

        for filename in migration_scripts:
            full_filename = os.path.join(versions_path, filename)
            if os.path.isfile(full_filename):
                shutil.copy(full_filename, USER_DB_MIGRATION_DIR)

        if current_rev == target_rev:
            logger.info("Database is already at target version")
            return
        # validate the current revision is in the revision list, if it is not, we are facing a downgrade, else an upgrade
        try:
            # if it is we are going to update it
            if current_rev is not None:
                logger.info("looking for current revision in script dir")
                script.get_revision(
                    current_rev
                )  # raises exception if does not found the revision
                # TODO: Look for the revision that revises the current rev
            logger.info(f"Starting upgrade from {current_rev} to {target_rev} (head)")
            command.upgrade(alembic_cfg, "head")
            logger.info("Database upgrade completed successfully")

        except util.CommandError as ce:
            if not isinstance(ce.__cause__, ResolutionError):
                raise ce
            # if it is not, we need to get the downgrade path from the user space
            # aka all the migration files from head in the scripts dir to the current version

            # check the current version migration file is saved in user space. If so, retrieve them
            # until reach the "head" version
            logger.info(
                "revision not found in script dir, retrieving downgrade path from backup"
            )

            next_id = current_rev

            while True:
                if next_id in retrieved_ids:
                    raise Exception("circular path found while downgrading")
                retrieved_file, aux_id = retrieve_revision_script(
                    USER_DB_MIGRATION_DIR, next_id
                )
                if retrieved_file is None:
                    error = (
                        "backup dir not found"
                        if aux_id == "ENODIR"
                        else "current revision migration script not found in backup dir"
                    )
                    raise Exception(
                        f"error retrieving revision {next_id} script: {aux_id}. Downgrade path broken: {error}"
                    )

                retrieved_files.append(retrieved_file)
                retrieved_ids.append(next_id)

                if aux_id is None:
                    raise Exception(
                        f"error retrieving downgrade path from {current_rev} to {target_rev}. Base reached"
                    )

                next_id = aux_id
                if next_id == target_rev:
                    break

            # do the downgrade
            logger.info(f"Starting downgrade from {current_rev} to {target_rev}")
            command.downgrade(alembic_cfg, target_rev)
            logger.info("Database downgrade completed successfully")

    except Exception as e:
        logger.error(f"Database migration failed: {str(e)}", exc_info=True)
        raise

    finally:
        # delete the retrieved files. If not deleted the next backend start will find them
        # and perform an upgrade on the db that might not be compatible with the backend
        clear_retrieved_scripts(retrieved_files)
        logger.info("Migration process finished")
