//! Shared helpers of the history integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use history::{HistoryDb, HistoryPaths, LocalZone};
use rusqlite::Connection;
use rusqlite::types::ValueRef;
use serde_json::{Map, Value, json};

pub const REVISIONS: [&str; 7] = [
    "ebb6a77afd0e",
    "0bdd1c635e7a",
    "1a598cd3ace3",
    "470a6d3b0f44",
    "8f4e7b2c9d10",
    "b71d84a4c2ef",
    "c4d2e9f1a7b3",
];
pub const HEAD: &str = "c4d2e9f1a7b3";

pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

pub fn load_json(name: &str) -> Value {
    let text = std::fs::read_to_string(data_dir().join(name)).expect("reference file exists");
    pyfmt::loads_lenient(&text).expect("reference file is JSON")
}

/// One machine: a history directory and a script backup directory.
pub struct Env {
    pub dir: tempfile::TempDir,
}

impl Env {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temporary directory"),
        }
    }

    pub fn paths(&self) -> HistoryPaths {
        HistoryPaths::new(
            self.dir.path().join("history"),
            self.dir.path().join("dbmigrations"),
        )
    }

    pub fn db_file(&self) -> PathBuf {
        self.paths().database_file()
    }

    /// `ShotDataBase.init()`.
    pub fn open(&self) -> HistoryDb {
        HistoryDb::open(self.paths(), LocalZone::Fixed(0)).expect("database opens")
    }

    pub fn open_in(&self, zone: LocalZone) -> HistoryDb {
        HistoryDb::open(self.paths(), zone).expect("database opens")
    }

    /// `back.run()`: open, then migrate; the error text if it failed.
    pub fn boot(&self) -> Option<String> {
        let db = self.open();
        db.migrate().err().map(|e| e.to_string())
    }

    pub fn connect(&self) -> Connection {
        std::fs::create_dir_all(self.paths().history_dir).unwrap();
        let conn = Connection::open(self.db_file()).expect("database connects");
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn
    }
}

/// The rows of `sample_rows.json` that exist at `revision`.
pub fn sample_rows(revision: &str, ratings: bool) -> Vec<(String, Map<String, Value>)> {
    let index = REVISIONS.iter().position(|r| *r == revision).unwrap();
    let rows = load_json("sample_rows.json");
    let mut result = Vec::new();
    for entry in rows.as_array().unwrap() {
        let since = REVISIONS
            .iter()
            .position(|r| *r == entry["since"].as_str().unwrap())
            .unwrap();
        let until = REVISIONS
            .iter()
            .position(|r| *r == entry.get("until").and_then(Value::as_str).unwrap_or(HEAD))
            .unwrap();
        let is_rating = entry
            .get("ratings")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if since <= index && index <= until && (ratings || !is_rating) {
            result.push((
                entry["table"].as_str().unwrap().to_string(),
                entry["row"].as_object().unwrap().clone(),
            ));
        }
    }
    result
}

pub fn bind(value: &Value) -> rusqlite::types::Value {
    match value {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(b) => rusqlite::types::Value::Integer(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => rusqlite::types::Value::Integer(i),
            None => rusqlite::types::Value::Real(n.as_f64().unwrap()),
        },
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        other => rusqlite::types::Value::Text(other.to_string()),
    }
}

pub fn insert_row(conn: &Connection, table: &str, row: &Map<String, Value>) {
    let columns: Vec<String> = row.keys().map(|k| format!("\"{k}\"")).collect();
    let marks = vec!["?"; row.len()].join(", ");
    conn.execute(
        &format!(
            "INSERT INTO \"{table}\" ({}) VALUES ({marks})",
            columns.join(", ")
        ),
        rusqlite::params_from_iter(row.values().map(bind)),
    )
    .unwrap_or_else(|e| panic!("insert into {table}: {e}"));
}

pub fn insert_sample(env: &Env, revision: &str, ratings: bool) {
    let conn = env.connect();
    for (table, row) in sample_rows(revision, ratings) {
        insert_row(&conn, &table, &row);
    }
}

fn cell(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => json!(["null", null]),
        ValueRef::Integer(i) => json!(["integer", i]),
        ValueRef::Real(f) => json!(["real", pyfmt::repr_f64(f)]),
        ValueRef::Text(t) => json!(["text", String::from_utf8_lossy(t)]),
        ValueRef::Blob(b) => {
            let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
            json!(["blob", hex])
        }
    }
}

const FTS_SHADOW_SUFFIXES: [&str; 5] = ["_data", "_idx", "_content", "_docsize", "_config"];

/// The same dump as `pyenv.dump()` in tests/data.
pub fn dump(db_file: &Path) -> Value {
    let conn = Connection::open(db_file).unwrap();
    let mut statement = conn
        .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name")
        .unwrap();
    let master: Vec<(String, String, String, Option<String>)> = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut rows = Map::new();
    for (kind, name, _, _) in &master {
        if kind != "table"
            || name.starts_with("sqlite_")
            || FTS_SHADOW_SUFFIXES.iter().any(|s| name.ends_with(s))
        {
            continue;
        }
        let query = if name == "alembic_version" {
            "SELECT * FROM \"alembic_version\"".to_string()
        } else {
            format!("SELECT rowid, * FROM \"{name}\" ORDER BY rowid")
        };
        let mut statement = conn.prepare(&query).unwrap();
        let columns: Vec<String> = statement
            .column_names()
            .iter()
            .map(|c| c.to_string())
            .collect();
        let count = columns.len();
        let mut result = Vec::new();
        let mut cursor = statement.query([]).unwrap();
        while let Some(row) = cursor.next().unwrap() {
            let cells: Vec<Value> = (0..count).map(|i| cell(row.get_ref(i).unwrap())).collect();
            result.push(Value::Array(cells));
        }
        rows.insert(name.clone(), json!({"columns": columns, "rows": result}));
    }
    let auto_vacuum: i64 = conn
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .unwrap();
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    json!({
        "master": master.iter().map(|(kind, name, tbl, sql)| json!({"type": kind, "name": name, "tbl_name": tbl, "sql": sql})).collect::<Vec<_>>(),
        "rows": rows,
        "auto_vacuum": auto_vacuum,
        "journal_mode": journal_mode,
    })
}

/// A CREATE TABLE with its constraint lines sorted: alembic's batch mode
/// orders constraints by iterating a Python set, so their order varies.
pub fn normalise_sql(sql: &str) -> String {
    if !sql.starts_with("CREATE TABLE") {
        return sql.to_string();
    }
    let (Some(open), Some(close)) = (sql.find('('), sql.rfind(')')) else {
        return sql.to_string();
    };
    let (head, body, tail) = (&sql[..open], &sql[open + 1..close], &sql[close + 1..]);
    if !body.contains('\n') {
        return sql.to_string();
    }
    let mut columns = Vec::new();
    let mut constraints = Vec::new();
    for line in body.split('\n') {
        let line = line.trim_start_matches('\t').trim_end();
        let line = line.strip_suffix(',').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let is_constraint = [
            "CONSTRAINT ",
            "PRIMARY KEY",
            "FOREIGN KEY",
            "UNIQUE ",
            "CHECK ",
        ]
        .iter()
        .any(|p| line.starts_with(p));
        if is_constraint {
            constraints.push(line.to_string());
        } else {
            columns.push(line.to_string());
        }
    }
    constraints.sort();
    format!(
        "{head}(columns: {} | constraints: {}){tail}",
        columns.join(" ; "),
        constraints.join(" ; ")
    )
}

/// The comparable form of a dump: normalised schema, rows and pragmas.
pub fn comparable(dump: &Value) -> Value {
    let master: Vec<Value> = dump["master"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            json!({
                "type": m["type"],
                "name": m["name"],
                "tbl_name": m["tbl_name"],
                "sql": m["sql"].as_str().map(normalise_sql),
            })
        })
        .collect();
    json!({
        "master": master,
        "rows": dump["rows"],
        "auto_vacuum": dump["auto_vacuum"],
        "journal_mode": dump["journal_mode"],
    })
}

/// Assert two dumps are equal, with a readable difference.
pub fn assert_same_database(name: &str, expected: &Value, actual: &Value) {
    let expected = comparable(expected);
    let actual = comparable(actual);
    if expected == actual {
        return;
    }
    let e_master = expected["master"].as_array().unwrap();
    let a_master = actual["master"].as_array().unwrap();
    for entry in e_master {
        if !a_master.contains(entry) {
            eprintln!("[{name}] missing or different: {entry}");
        }
    }
    for entry in a_master {
        if !e_master.contains(entry) {
            eprintln!("[{name}] unexpected: {entry}");
        }
    }
    for (table, rows) in expected["rows"].as_object().unwrap() {
        if actual["rows"].get(table) != Some(rows) {
            eprintln!(
                "[{name}] rows of {table}: expected {rows}, got {:?}",
                actual["rows"].get(table)
            );
        }
    }
    for key in ["auto_vacuum", "journal_mode"] {
        if expected[key] != actual[key] {
            eprintln!(
                "[{name}] {key}: expected {}, got {}",
                expected[key], actual[key]
            );
        }
    }
    panic!("[{name}] the Rust database differs from the Python reference");
}
