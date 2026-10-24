use std::path::PathBuf;

/// Errors of the history store.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// A value the Python code would have rejected with an exception
    /// (`KeyError`, `TypeError`, `ValueError`) before touching the database.
    #[error("invalid value: {0}")]
    InvalidValue(String),
    /// The database is at a revision this engine has no script for.
    #[error("Cannot retrieve missing scripts (database at {current}, required {target})")]
    UnknownRevision { current: String, target: String },
    /// A migration step failed; the steps before it stay applied, as with alembic.
    #[error("migration {revision} failed: {source}")]
    Migration {
        revision: String,
        #[source]
        source: Box<Error>,
    },
    /// Alembic's batch mode refused an operation (`No such constraint`, ...).
    #[error("{0}")]
    Batch(String),
    #[error("the file {path} is not a readable zstd stream: {message}")]
    Zstd { path: PathBuf, message: String },
    /// `PourOverHistoryConflictError` (the handler answers 409).
    #[error("A different Pour Over record already uses this session id")]
    PourOverConflict,
    /// `PourOverHistoryRecordTooLargeError` (the handler answers 413 on save,
    /// 500 "Invalid history entry" when serving a file).
    #[error("{0}")]
    PourOverTooLarge(String),
    /// The `OSError`s of the Pour Over quotas (the handler answers 500).
    #[error("{0}")]
    Storage(String),
    /// `DuplicateLocalID` of the bug report dispatch (409).
    #[error("This localID already exists")]
    DuplicateLocalId,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
