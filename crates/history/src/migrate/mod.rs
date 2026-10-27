//! `db_migration_updater.update_db_migrations()`: bring the history database
//! to [`DB_VERSION_REQUIRED`] the way alembic does, writing the same
//! `alembic_version` table and the same schema, so a machine can roll back to
//! the Python image and let its alembic carry on from there.
//!
//! The revision scripts directory (`USER_DB_MIGRATION_DIR`,
//! `/meticulous-user/.dbmigrations`): the Python updater copies its revision
//! scripts there at every boot, and an older image that finds the database at
//! a revision it does not ship copies the missing scripts back from there to
//! run their `downgrade()`. Rust cannot run Python, so it never reads that
//! directory, but it keeps filling it: before migrating it writes the Python
//! script of every revision it knows (byte-identical copies of
//! `alembic/versions/`, embedded in this crate), under the same file names and
//! with the same "only if missing" rule as `backup_new_scripts()`. Whatever
//! revision Rust leaves the database at, the Python image a machine rolls back
//! to finds the script it needs to downgrade. A database at a revision Rust
//! does not know (a newer Python image) is left untouched and reported, as the
//! Python updater does when no script for it can be found.

mod batch;
mod revisions;
mod sql;

use std::fs;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use crate::error::{Error, Result};
use revisions::Ops;
pub use revisions::{INITIAL_PRAGMAS, REVISIONS, Revision};
use sql::{StepRunner, run};

/// `db_migration_updater.DB_VERSION_REQUIRED`.
pub const DB_VERSION_REQUIRED: &str = "c4d2e9f1a7b3";

/// The revision chain, oldest first (`ScriptDirectory.walk_revisions()` reversed).
pub fn revision_ids() -> Vec<&'static str> {
    REVISIONS.iter().map(|r| r.id).collect()
}

/// What [`update_db_migrations`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// The database had no revision (new, or created before alembic).
    Initialized,
    AlreadyCurrent,
    Upgraded {
        from: String,
    },
    Downgraded {
        from: String,
    },
}

/// `MigrationContext.get_current_revision()`.
pub fn current_revision(conn: &Connection) -> Result<Option<String>> {
    let exists: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='alembic_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Ok(None);
    }
    let mut statement =
        conn.prepare("SELECT alembic_version.version_num \nFROM alembic_version")?;
    let versions: Vec<String> = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    match versions.len() {
        0 => Ok(None),
        1 => Ok(versions.into_iter().next()),
        _ => Err(Error::Migration {
            revision: versions.join(", "),
            source: Box::new(Error::InvalidValue(
                "Version table 'alembic_version' has more than one head present".into(),
            )),
        }),
    }
}

/// `backup_new_scripts()`: copy every known revision script that is missing
/// (by file name) into the backup directory. Returns the names written.
pub fn backup_scripts(backup_dir: &Path) -> Result<Vec<String>> {
    fs::create_dir_all(backup_dir)?;
    let mut written = Vec::new();
    for revision in &REVISIONS {
        let destination = backup_dir.join(revision.script.file_name);
        if destination.exists() {
            continue;
        }
        // Copy failures are logged and skipped, like shutil.copy's in Python.
        match fs::write(&destination, revision.script.contents) {
            Ok(()) => written.push(revision.script.file_name.to_string()),
            Err(error) => tracing::warn!(
                "failed to copy {} for backup: {error}",
                revision.script.file_name
            ),
        }
    }
    if written.is_empty() {
        tracing::info!("no new DB migration files to backup");
    }
    Ok(written)
}

fn ensure_version_table(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("PRAGMA main.table_info(\"alembic_version\")")?;
    let has_columns = statement.query([])?.next()?.is_some();
    if !has_columns {
        run(
            conn,
            "\nCREATE TABLE alembic_version (\n\tversion_num VARCHAR(32) NOT NULL, \n\tCONSTRAINT alembic_version_pkc PRIMARY KEY (version_num)\n)\n\n",
        )?;
    }
    Ok(())
}

fn position(id: &str) -> Option<usize> {
    REVISIONS.iter().position(|r| r.id == id)
}

/// Run one migration step in its own transaction, as alembic does on SQLite.
fn run_step(
    conn: &Connection,
    revision: &Revision,
    apply: fn(&mut Ops<'_, '_>) -> Result<()>,
    version_sql: &str,
) -> Result<()> {
    let mut runner = StepRunner::new(conn);
    let result = (|| {
        let mut ops = Ops { run: &mut runner };
        apply(&mut ops)?;
        ops.run.exec(version_sql)?;
        if conn.changes() != 1 {
            return Err(Error::InvalidValue(format!(
                "Online migration expected to match one row when updating the version to {}",
                revision.id
            )));
        }
        Ok(())
    })();
    match result {
        Ok(()) => runner.commit(),
        Err(error) => {
            runner.rollback();
            Err(Error::Migration {
                revision: revision.id.to_string(),
                source: Box::new(error),
            })
        }
    }
}

/// `command.upgrade(cfg, target)` from `current`.
fn upgrade(conn: &Connection, current: Option<&str>, target: &str) -> Result<()> {
    ensure_version_table(conn)?;
    let start = match current {
        Some(id) => position(id).map(|i| i + 1).unwrap_or(REVISIONS.len()),
        None => 0,
    };
    let end = position(target).ok_or_else(|| Error::UnknownRevision {
        current: current.unwrap_or("None").to_string(),
        target: target.to_string(),
    })?;
    for (index, revision) in REVISIONS.iter().enumerate().take(end + 1).skip(start) {
        tracing::info!("Running upgrade {:?} -> {}", revision.down, revision.id);
        let version_sql = match (index == start, current, revision.down) {
            (true, None, _) | (_, _, None) => format!(
                "INSERT INTO alembic_version (version_num) VALUES ('{}') RETURNING version_num",
                revision.id
            ),
            (_, _, Some(down)) => format!(
                "UPDATE alembic_version SET version_num='{}' WHERE alembic_version.version_num = '{down}'",
                revision.id
            ),
        };
        run_step(conn, revision, revision.upgrade, &version_sql)?;
    }
    Ok(())
}

/// `command.downgrade(cfg, target)` from `current`.
fn downgrade(conn: &Connection, current: &str, target: &str) -> Result<()> {
    let (Some(from), Some(to)) = (position(current), position(target)) else {
        return Err(Error::UnknownRevision {
            current: current.to_string(),
            target: target.to_string(),
        });
    };
    for revision in REVISIONS[to + 1..=from].iter().rev() {
        let down = revision.down.unwrap_or_default();
        tracing::info!("Running downgrade {} -> {down}", revision.id);
        let version_sql = format!(
            "UPDATE alembic_version SET version_num='{down}' WHERE alembic_version.version_num = '{}'",
            revision.id
        );
        run_step(conn, revision, revision.downgrade, &version_sql)?;
    }
    Ok(())
}

/// Upgrade (or downgrade) to an explicit revision, like `alembic upgrade
/// <rev>` / `alembic downgrade <rev>`. The tests use it to build databases
/// at older revisions.
pub fn migrate_to(conn: &Connection, target: &str) -> Result<MigrationOutcome> {
    let current = current_revision(conn)?;
    match current.as_deref() {
        None => {
            upgrade(conn, None, target)?;
            Ok(MigrationOutcome::Initialized)
        }
        Some(current) if current == target => Ok(MigrationOutcome::AlreadyCurrent),
        Some(current) => {
            let (Some(from), Some(to)) = (position(current), position(target)) else {
                return Err(Error::UnknownRevision {
                    current: current.to_string(),
                    target: target.to_string(),
                });
            };
            if from > to {
                downgrade(conn, current, target)?;
                Ok(MigrationOutcome::Downgraded {
                    from: current.to_string(),
                })
            } else {
                upgrade(conn, Some(current), target)?;
                Ok(MigrationOutcome::Upgraded {
                    from: current.to_string(),
                })
            }
        }
    }
}

/// `update_db_migrations()`: back up the revision scripts, then bring the
/// database to [`DB_VERSION_REQUIRED`]. Each step commits on its own, so a
/// failure leaves the database at the last step that succeeded.
pub fn update_db_migrations(conn: &Connection, backup_dir: &Path) -> Result<MigrationOutcome> {
    tracing::info!("Starting database migration process");
    let result = (|| {
        let current = current_revision(conn)?;
        tracing::info!("current DB revision: {current:?}");
        backup_scripts(backup_dir)?;
        migrate_to(conn, DB_VERSION_REQUIRED)
    })();
    if let Err(error) = &result {
        tracing::error!("Database migration failed: {error}");
    }
    tracing::info!("Migration process finished");
    result
}
