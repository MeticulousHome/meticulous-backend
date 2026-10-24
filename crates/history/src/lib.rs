//! The history store of the Meticulous backend: the SQLite database
//! (`history.sqlite`) with the espresso shot history, Pour Over brew history
//! and bug reports, its alembic-compatible migrations, and the zstd JSON
//! files the history points at.
//!
//! Synchronous by design: the backend calls it from `spawn_blocking`. Paths
//! come in as arguments; nothing here is global.
//!
//! Python reference: `database_models.py`, `alembic/versions/`,
//! `db_migration_updater.py`, `shot_database.py`, `pour_over_history.py`,
//! the `bug_reports` queries of `api/bug_report.py`, and the shot file access
//! of `api/history.py` and `shot_manager.py`.

pub mod bug_reports;
mod db;
mod error;
pub mod files;
pub mod localtime;
pub mod migrate;
pub mod pour_over;
mod pydantic;
pub mod search;
mod shots;
mod sqlvalue;

pub use db::{HistoryDb, HistoryPaths};
pub use error::{Error, Result};
pub use localtime::LocalZone;
pub use migrate::{DB_VERSION_REQUIRED, MigrationOutcome};
pub use search::{SearchOrder, SearchOrderBy, SearchParams};

#[cfg(test)]
mod tests {
    /// The backend shares one store between `spawn_blocking` tasks.
    #[test]
    fn the_store_can_be_shared_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<crate::HistoryDb>();
    }
}
