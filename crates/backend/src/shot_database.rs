//! `shot_database.py` and `pour_over_history.py`: the history database. The
//! queries are the `history` crate's; this opens it at boot and hands the
//! services and handlers a fallible view of it.
//!
//! Until the database opens every query fails, like the Python's
//! `ShotDataBase.engine` left at `None` after a failed `init()`.

use std::sync::OnceLock;

use serde_json::Value;

use history::search::SearchId;
use history::{HistoryDb, HistoryPaths, LocalZone, SearchOrder, SearchParams};

pub struct History {
    paths: HistoryPaths,
    db: OnceLock<HistoryDb>,
}

impl History {
    pub fn new(paths: HistoryPaths) -> History {
        History {
            paths,
            db: OnceLock::new(),
        }
    }

    /// `back.run()`: `ShotManager.init()` opens the database, then
    /// `update_db_migrations()` brings it to `DB_VERSION_REQUIRED`. Either
    /// failing is logged and the backend starts anyway.
    pub fn init(&self) {
        self.init_with(|_| {});
    }

    /// `init`, with the database adjusted before anything uses it (a test
    /// that shrinks the Pour Over limits, as the Python suite patches the
    /// module constants).
    pub fn init_with(&self, configure: impl FnOnce(&mut HistoryDb)) {
        match HistoryDb::open(self.paths.clone(), LocalZone::System) {
            Ok(mut db) => {
                configure(&mut db);
                let _ = self.db.set(db);
                tracing::info!("ShotManager initialized successfully");
            }
            Err(error) => {
                tracing::error!("Failed to initialize ShotManager: {error}");
                return;
            }
        }
        if let Err(error) = self
            .db()
            .and_then(|db| db.migrate().map_err(|e| e.to_string()))
        {
            tracing::error!("Failed to run database migrations: {error}");
        }
    }

    pub fn db(&self) -> Result<&HistoryDb, String> {
        self.db
            .get()
            .ok_or_else(|| "History database is not initialized".to_string())
    }

    fn with<T>(&self, f: impl FnOnce(&HistoryDb) -> history::Result<T>) -> Result<T, String> {
        f(self.db()?).map_err(|e| e.to_string())
    }

    /// `ShotDataBase.statistics()`.
    pub fn statistics(&self) -> Result<Value, String> {
        self.with(HistoryDb::statistics)
    }

    /// The newest shot (`ShotManager.getLastShot`'s query).
    pub fn last_shot(&self) -> Result<Option<Value>, String> {
        let params = SearchParams {
            sort: SearchOrder::Descending,
            max_results: 1,
            ..SearchParams::default()
        };
        Ok(self.search(&params)?.into_iter().next())
    }

    pub fn search(&self, params: &SearchParams) -> Result<Vec<Value>, String> {
        self.with(|db| db.search_history(params))
    }

    /// [`search`](Self::search) as JSON text in `style`, appended to `out`
    /// (see `HistoryDb::search_history_json`).
    pub fn search_json(
        &self,
        params: &SearchParams,
        style: pyfmt::JsonStyle,
        out: &mut String,
    ) -> Result<(), String> {
        self.with(|db| db.search_history_json(params, style, out))
    }

    pub fn insert_history(&self, shot: &Value) -> Result<i64, String> {
        self.with(|db| db.insert_history(shot))
    }

    pub fn link_debug_file(&self, history_id: i64, file: &str) -> Result<(), String> {
        self.with(|db| db.link_debug_file(history_id, file).map(|_| ()))
    }

    pub fn unlink_debug_file(&self, file: &str) -> Result<(), String> {
        self.with(|db| db.unlink_debug_file(file).map(|_| ()))
    }

    /// `find_manual_shot`'s query: the newest of these history ids or
    /// profile ids, with its data.
    pub fn search_manual_shots(&self, ids: &[String]) -> Result<Vec<Value>, String> {
        let params = SearchParams {
            ids: ids.iter().cloned().map(SearchId::Str).collect(),
            sort: SearchOrder::Descending,
            max_results: 1,
            dump_data: true,
            ..SearchParams::default()
        };
        self.search(&params)
    }
}
