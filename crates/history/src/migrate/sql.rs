//! How SQLAlchemy names things in SQLite DDL, and how pysqlite runs it.

use rusqlite::Connection;

use crate::error::Result;

/// `SQLiteIdentifierPreparer.reserved_words` (SQLAlchemy 2.0).
const RESERVED_WORDS: &[&str] = &[
    "add",
    "after",
    "all",
    "alter",
    "analyze",
    "and",
    "as",
    "asc",
    "attach",
    "autoincrement",
    "before",
    "begin",
    "between",
    "by",
    "cascade",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "commit",
    "conflict",
    "constraint",
    "create",
    "cross",
    "current_date",
    "current_time",
    "current_timestamp",
    "database",
    "default",
    "deferrable",
    "deferred",
    "delete",
    "desc",
    "detach",
    "distinct",
    "drop",
    "each",
    "else",
    "end",
    "escape",
    "except",
    "exclusive",
    "exists",
    "explain",
    "false",
    "fail",
    "for",
    "foreign",
    "from",
    "full",
    "glob",
    "group",
    "having",
    "if",
    "ignore",
    "immediate",
    "in",
    "index",
    "indexed",
    "initially",
    "inner",
    "insert",
    "instead",
    "intersect",
    "into",
    "is",
    "isnull",
    "join",
    "key",
    "left",
    "like",
    "limit",
    "match",
    "natural",
    "not",
    "notnull",
    "null",
    "of",
    "offset",
    "on",
    "or",
    "order",
    "outer",
    "plan",
    "pragma",
    "primary",
    "query",
    "raise",
    "references",
    "reindex",
    "rename",
    "replace",
    "restrict",
    "right",
    "rollback",
    "row",
    "select",
    "set",
    "table",
    "temp",
    "temporary",
    "then",
    "to",
    "transaction",
    "trigger",
    "true",
    "union",
    "unique",
    "update",
    "using",
    "vacuum",
    "values",
    "view",
    "virtual",
    "when",
    "where",
];

/// `IdentifierPreparer._requires_quotes`.
fn requires_quotes(name: &str) -> bool {
    let lower = name.to_lowercase();
    let first_illegal = name
        .chars()
        .next()
        .is_none_or(|c| c.is_ascii_digit() || c == '$');
    let legal = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    RESERVED_WORDS.contains(&lower.as_str()) || first_illegal || !legal || lower != name
}

/// `IdentifierPreparer.quote`: the name as SQLAlchemy writes it into SQL.
pub fn quote(name: &str) -> String {
    if requires_quotes(name) {
        format!("\"{}\"", name.replace('"', "\"\""))
    } else {
        name.to_string()
    }
}

/// A statement pysqlite (Python's sqlite3, legacy transaction control) opens a
/// transaction for: it issues `BEGIN` before INSERT, UPDATE, DELETE and
/// REPLACE only, so DDL before the first of them runs in autocommit mode.
fn is_dml(sql: &str) -> bool {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, tail)| tail);
        } else {
            break;
        }
    }
    let keyword: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase();
    matches!(keyword.as_str(), "insert" | "update" | "delete" | "replace")
}

/// Run one statement to completion, ignoring any rows it returns (PRAGMAs).
pub fn run(conn: &Connection, sql: &str) -> Result<()> {
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

/// Executes the statements of one alembic migration step the way alembic does
/// on pysqlite: each step is its own transaction, which pysqlite only opens at
/// the step's first DML statement. A failing step rolls back to that point;
/// DDL it ran before its first DML stays.
pub struct StepRunner<'c> {
    conn: &'c Connection,
    in_transaction: bool,
}

impl<'c> StepRunner<'c> {
    pub fn new(conn: &'c Connection) -> Self {
        Self {
            conn,
            in_transaction: false,
        }
    }

    pub fn conn(&self) -> &'c Connection {
        self.conn
    }

    pub fn exec(&mut self, sql: &str) -> Result<()> {
        if is_dml(sql) && !self.in_transaction && self.conn.is_autocommit() {
            self.conn.execute_batch("BEGIN")?;
            self.in_transaction = true;
        }
        run(self.conn, sql)
    }

    pub fn commit(&mut self) -> Result<()> {
        if self.in_transaction {
            self.in_transaction = false;
            self.conn.execute_batch("COMMIT")?;
        }
        Ok(())
    }

    pub fn rollback(&mut self) {
        if self.in_transaction {
            self.in_transaction = false;
            if let Err(error) = self.conn.execute_batch("ROLLBACK") {
                tracing::warn!("rollback of a failed migration step failed: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names SQLAlchemy quotes in the reference schemas (tests/data/migrations.json).
    #[test]
    fn quotes_like_sqlalchemy() {
        assert_eq!(quote("key"), "\"key\"");
        assert_eq!(quote("localID"), "\"localID\"");
        assert_eq!(
            quote("ix_bug_reports_creationTime"),
            "\"ix_bug_reports_creationTime\""
        );
        assert_eq!(quote("history"), "history");
        assert_eq!(quote("time"), "time");
        assert_eq!(quote("file"), "file");
        assert_eq!(
            quote("_alembic_tmp_shot_annotation"),
            "_alembic_tmp_shot_annotation"
        );
        assert_eq!(quote("1abc"), "\"1abc\"");
        assert_eq!(quote("a b"), "\"a b\"");
    }

    #[test]
    fn dml_detection_matches_pysqlite() {
        assert!(is_dml("INSERT INTO x VALUES (1)"));
        assert!(is_dml("\n  update x set a=1"));
        assert!(is_dml("-- c\nDELETE FROM x"));
        assert!(is_dml("/* c */ REPLACE INTO x VALUES (1)"));
        assert!(!is_dml("\nCREATE TABLE x (a)"));
        assert!(!is_dml("PRAGMA journal_mode=WAL"));
        assert!(!is_dml("SELECT 1"));
        assert!(!is_dml("ALTER TABLE x RENAME TO y"));
    }
}
