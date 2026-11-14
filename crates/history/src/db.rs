use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;

use crate::error::Result;
use crate::localtime::LocalZone;
use crate::migrate::{self, MigrationOutcome};
use crate::pour_over::PourOverLimits;

/// `config.DATABASE_FILE`.
pub const DATABASE_FILE: &str = "history.sqlite";

/// The FTS tables as `ShotDataBase.init()` creates them (one line each).
pub const PROFILE_FTS_DDL: &str =
    "CREATE VIRTUAL TABLE IF NOT EXISTS profile_fts USING fts5(profile_key, profile_id, name)";
pub const STAGE_FTS_DDL: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS stage_fts USING fts5(profile_key, profile_id, profile_name, stage_key, stage_name)";

/// The connect hook of `ShotDataBase.init()`: "Configure the DB for most safety".
const CONNECT_PRAGMAS: [&str; 5] = [
    "PRAGMA auto_vacuum=full;",
    "PRAGMA journal_mode=WAL;",
    "PRAGMA synchronous=EXTRA;",
    "PRAGMA journal_size_limit = 1048576;",
    "PRAGMA wal_checkpoint(TRUNCATE);",
];

/// Where the history lives (`config.py`).
#[derive(Debug, Clone)]
pub struct HistoryPaths {
    /// `HISTORY_PATH` (`/meticulous-user/history`): the database, `shots/`
    /// and `pour-over/`.
    pub history_dir: PathBuf,
    /// `USER_DB_MIGRATION_DIR` (`/meticulous-user/.dbmigrations`).
    pub migration_backup_dir: PathBuf,
}

impl HistoryPaths {
    pub fn new(history_dir: impl Into<PathBuf>, migration_backup_dir: impl Into<PathBuf>) -> Self {
        Self {
            history_dir: history_dir.into(),
            migration_backup_dir: migration_backup_dir.into(),
        }
    }

    /// `ABSOLUTE_DATABASE_FILE`.
    pub fn database_file(&self) -> PathBuf {
        self.history_dir.join(DATABASE_FILE)
    }

    /// `SHOT_PATH`.
    pub fn shot_dir(&self) -> PathBuf {
        self.history_dir.join("shots")
    }

    /// `POUR_OVER_PATH`.
    pub fn pour_over_dir(&self) -> PathBuf {
        self.history_dir.join("pour-over")
    }
}

/// The history database (`ShotDataBase`, `PourOverHistoryManager` and the
/// bug report queries share it). One connection, used from one thread at a
/// time; every method is synchronous.
pub struct HistoryDb {
    conn: Mutex<Connection>,
    pub(crate) paths: HistoryPaths,
    pub(crate) zone: LocalZone,
    pub(crate) pour_over_limits: PourOverLimits,
}

impl std::fmt::Debug for HistoryDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryDb")
            .field("paths", &self.paths)
            .field("zone", &self.zone)
            .finish_non_exhaustive()
    }
}

fn run_pragma(conn: &Connection, sql: &str) -> Result<()> {
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

impl HistoryDb {
    /// `ShotDataBase.init()`: create `HISTORY_PATH`, open the database with
    /// the connect pragmas and make sure the FTS tables exist.
    ///
    /// PARITY: Python's `handle_error()` (delete and rebuild a corrupt
    /// database) never runs, because SQLAlchemy wraps the sqlite3 error it
    /// catches; a corrupt file is an error here too and is left in place.
    pub fn open(paths: HistoryPaths, zone: LocalZone) -> Result<Self> {
        fs::create_dir_all(&paths.history_dir)?;
        let conn = Connection::open(paths.database_file())?;
        // The bundled SQLite enforces foreign keys by default; Python's
        // sqlite3 does not, and the migrations (alembic's table copies) and
        // deletes rely on that.
        conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
        for pragma in CONNECT_PRAGMAS {
            run_pragma(&conn, pragma)?;
        }
        conn.execute_batch(PROFILE_FTS_DDL)?;
        conn.execute_batch(STAGE_FTS_DDL)?;
        Ok(Self {
            conn: Mutex::new(conn),
            paths,
            zone,
            pour_over_limits: PourOverLimits::default(),
        })
    }

    /// `db_migration_updater.update_db_migrations()`.
    pub fn migrate(&self) -> Result<MigrationOutcome> {
        let conn = self.connection();
        migrate::update_db_migrations(&conn, &self.paths.migration_backup_dir)
    }

    /// `alembic upgrade|downgrade <revision>` (no script backup).
    pub fn migrate_to(&self, revision: &str) -> Result<MigrationOutcome> {
        let conn = self.connection();
        migrate::migrate_to(&conn, revision)
    }

    /// The alembic revision the database is at.
    pub fn current_revision(&self) -> Result<Option<String>> {
        let conn = self.connection();
        migrate::current_revision(&conn)
    }

    pub fn paths(&self) -> &HistoryPaths {
        &self.paths
    }

    pub fn zone(&self) -> &LocalZone {
        &self.zone
    }

    /// The Pour Over history quotas (Python's module constants).
    pub fn set_pour_over_limits(&mut self, limits: PourOverLimits) {
        self.pour_over_limits = limits;
    }

    pub fn pour_over_limits(&self) -> &PourOverLimits {
        &self.pour_over_limits
    }

    /// Direct access to the connection, for callers that need a query this
    /// crate does not offer, and for tests.
    pub fn with_connection<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.connection();
        f(&conn)
    }

    pub(crate) fn connection(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves the connection usable: every
        // write is a transaction that SQLite rolls back on its own.
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn shot_path(&self, relative: &str) -> PathBuf {
        self.paths.shot_dir().join(relative)
    }

    pub(crate) fn pour_over_path(&self, relative: &Path) -> PathBuf {
        self.paths.pour_over_dir().join(relative)
    }
}
