//! Alembic's `batch_alter_table` on SQLite: reflect the table the way
//! SQLAlchemy 2.0 does, apply the operations, and either ALTER in place or
//! recreate the table ("move and copy") with the DDL SQLAlchemy would render.
//!
//! The reflection follows `sqlalchemy/dialects/sqlite/base.py` (`get_columns`,
//! `get_pk_constraint`, `get_foreign_keys`, `get_unique_constraints`,
//! `get_check_constraints`, `get_indexes`) including its regular expressions,
//! so tables written by older images (unnamed constraints, pre-alembic
//! schemas) come out as alembic makes them.

use std::sync::LazyLock;

use regex::Regex;
use rusqlite::{Connection, OptionalExtension};

use super::sql::{StepRunner, quote};
use crate::error::{Error, Result};

/// A column as `batch_op.add_column(sa.Column(...))` declares it.
#[derive(Debug, Clone)]
pub struct NewColumn {
    pub name: &'static str,
    pub type_sql: &'static str,
    pub nullable: bool,
}

/// The operations the revisions use inside `batch_alter_table`.
#[derive(Debug, Clone)]
pub enum BatchOp {
    AddColumn(NewColumn),
    DropColumn(&'static str),
    /// `create_unique_constraint(None, cols)`: named by the naming convention.
    CreateUnique {
        columns: Vec<&'static str>,
    },
    /// `create_foreign_key(None, referent, local, remote)`.
    CreateForeignKey {
        referent: &'static str,
        local: Vec<&'static str>,
        remote: Vec<&'static str>,
    },
    DropConstraint(&'static str),
}

#[derive(Debug, Clone)]
struct Column {
    name: String,
    type_sql: String,
    nullable: bool,
    default: Option<String>,
    /// Whether this column existed before the batch (it is copied over).
    transfer: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum ConstraintKind {
    PrimaryKey(Vec<String>),
    ForeignKey {
        columns: Vec<String>,
        referred_table: String,
        referred_columns: Vec<String>,
        ondelete: Option<String>,
        onupdate: Option<String>,
        deferrable: Option<bool>,
        initially: Option<String>,
    },
    Unique(Vec<String>),
    Check(String),
}

#[derive(Debug, Clone)]
struct Constraint {
    name: Option<String>,
    kind: ConstraintKind,
}

impl Constraint {
    fn columns(&self) -> Vec<&str> {
        match &self.kind {
            ConstraintKind::PrimaryKey(columns)
            | ConstraintKind::Unique(columns)
            | ConstraintKind::ForeignKey { columns, .. } => {
                columns.iter().map(String::as_str).collect()
            }
            ConstraintKind::Check(_) => Vec::new(),
        }
    }

    /// The constraint as SQLAlchemy's DDL compiler renders it inside CREATE TABLE.
    fn render(&self) -> Option<String> {
        let prefix = match &self.name {
            Some(name) => format!("CONSTRAINT {} ", quote(name)),
            None => String::new(),
        };
        let join = |columns: &[String]| {
            columns
                .iter()
                .map(|c| quote(c))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match &self.kind {
            ConstraintKind::PrimaryKey(columns) => {
                if columns.is_empty() {
                    None
                } else {
                    Some(format!("{prefix}PRIMARY KEY ({})", join(columns)))
                }
            }
            ConstraintKind::Unique(columns) => Some(format!("{prefix}UNIQUE ({})", join(columns))),
            ConstraintKind::Check(sqltext) => Some(format!("{prefix}CHECK ({sqltext})")),
            ConstraintKind::ForeignKey {
                columns,
                referred_table,
                referred_columns,
                ondelete,
                onupdate,
                deferrable,
                initially,
            } => {
                let mut text = format!(
                    "{prefix}FOREIGN KEY({}) REFERENCES {} ({})",
                    join(columns),
                    quote(referred_table),
                    join(referred_columns)
                );
                if let Some(ondelete) = ondelete {
                    text.push_str(&format!(" ON DELETE {ondelete}"));
                }
                if let Some(onupdate) = onupdate {
                    text.push_str(&format!(" ON UPDATE {onupdate}"));
                }
                match deferrable {
                    Some(true) => text.push_str(" DEFERRABLE"),
                    Some(false) => text.push_str(" NOT DEFERRABLE"),
                    None => {}
                }
                if let Some(initially) = initially {
                    text.push_str(&format!(" INITIALLY {initially}"));
                }
                Some(text)
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Index {
    name: String,
    columns: Vec<String>,
    unique: bool,
    where_clause: Option<String>,
}

/// A table as `Table(name, MetaData(), autoload_with=...)` reflects it.
#[derive(Debug, Clone)]
struct Reflected {
    columns: Vec<Column>,
    constraints: Vec<Constraint>,
    indexes: Vec<Index>,
}

fn table_sql(conn: &Connection, table: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT sql FROM  (SELECT * FROM sqlite_master UNION ALL   SELECT * FROM sqlite_temp_master) WHERE name = ? AND type in ('table', 'view')",
            [table],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(table_sql(conn, table)?.is_some())
}

/// `SQLiteDialect._resolve_type_affinity` followed by the SQLite type
/// compiler: the type a reflected column is written back with.
fn resolve_type(declared: &str) -> Result<String> {
    static TYPE_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([\w ]+)(\(.*?\))?").expect("static regex is valid"));
    static ARG_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d+)").expect("static regex is valid"));
    let upper = declared.to_uppercase();
    let (name, args) = match TYPE_RE.captures(&upper) {
        Some(captures) => (
            captures.get(1).map_or("", |m| m.as_str()).to_string(),
            captures.get(2).map(|m| {
                ARG_RE
                    .find_iter(m.as_str())
                    .map(|n| n.as_str().to_string())
                    .collect::<Vec<_>>()
            }),
        ),
        None => (String::new(), None),
    };
    let args = args.unwrap_or_default();
    // (rendered name, how many arguments the type takes and renders)
    let (rendered, takes): (&str, usize) = match name.as_str() {
        "BIGINT" => ("BIGINT", 0),
        "BLOB" => ("BLOB", 1),
        "BOOL" | "BOOLEAN" => ("BOOLEAN", 0),
        "CHAR" => ("CHAR", 1),
        "DATE" | "DATE_CHAR" => ("DATE", 0),
        "DATETIME" | "DATETIME_CHAR" => ("DATETIME", 0),
        "DOUBLE" => ("DOUBLE", 0),
        "DECIMAL" => ("DECIMAL", 2),
        "FLOAT" => ("FLOAT", 1),
        "INT" | "INTEGER" => ("INTEGER", 0),
        "JSON" => ("JSON", 0),
        "NUMERIC" => ("NUMERIC", 2),
        "REAL" => ("REAL", 0),
        "SMALLINT" => ("SMALLINT", 0),
        "TEXT" => ("TEXT", 1),
        "TIME" | "TIME_CHAR" => ("TIME", 0),
        "TIMESTAMP" => ("TIMESTAMP", 0),
        "VARCHAR" => ("VARCHAR", 1),
        "NVARCHAR" => ("NVARCHAR", 1),
        "NCHAR" => ("NCHAR", 1),
        other if other.contains("INT") => ("INTEGER", 0),
        other if other.contains("CHAR") || other.contains("CLOB") || other.contains("TEXT") => {
            ("TEXT", 1)
        }
        other if other.contains("BLOB") || other.is_empty() => {
            return Err(Error::Batch(format!(
                "Can't generate DDL for NullType(); did you forget to specify a type on this Column? ({declared})"
            )));
        }
        other if other.contains("REAL") || other.contains("FLOA") || other.contains("DOUB") => {
            ("REAL", 0)
        }
        _ => ("NUMERIC", 2),
    };
    let used: Vec<&String> = args.iter().take(takes).collect();
    if used.is_empty() {
        Ok(rendered.to_string())
    } else {
        let joined = used
            .iter()
            .map(|a| a.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!("{rendered}({joined})"))
    }
}

/// `SQLiteImpl._guess_if_default_is_unparenthesized_sql_expr` as applied by
/// alembic's column reflection hook.
fn reflect_default(default: Option<String>) -> Option<String> {
    static DIGIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[0-9\.]$").expect("static regex is valid"));
    static QUOTED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^'.+'$").expect("static regex is valid"));
    static PARENS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\(.+\)$").expect("static regex is valid"));
    let default = default?;
    if default.is_empty()
        || DIGIT.is_match(&default)
        || QUOTED.is_match(&default)
        || PARENS.is_match(&default)
    {
        Some(default)
    } else {
        Some(format!("({default})"))
    }
}

struct RawColumn {
    name: String,
    declared: String,
    notnull: bool,
    default: Option<String>,
    pk: i64,
}

fn raw_columns(conn: &Connection, table: &str) -> Result<Vec<RawColumn>> {
    let mut statement = conn.prepare(&format!("PRAGMA main.table_xinfo({})", sql_string(table)))?;
    let rows = statement.query_map([], |row| {
        Ok((
            RawColumn {
                name: row.get(1)?,
                declared: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                notnull: row.get::<_, i64>(3)? != 0,
                default: row.get::<_, Option<String>>(4)?,
                pk: row.get(5)?,
            },
            row.get::<_, i64>(6)?,
        ))
    })?;
    let mut columns = Vec::new();
    for row in rows {
        let (column, hidden) = row?;
        if hidden != 1 {
            columns.push(column);
        }
    }
    Ok(columns)
}

/// `"name"` as a pragma argument, the way SQLAlchemy quotes it there.
fn sql_string(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn pk_constraint(conn: &Connection, table: &str) -> Result<(Option<String>, Vec<String>)> {
    static PK_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)CONSTRAINT (\w+) PRIMARY KEY").expect("static regex is valid")
    });
    let name =
        table_sql(conn, table)?.and_then(|sql| PK_RE.captures(&sql).map(|c| c[1].to_string()));
    let mut columns: Vec<(i64, String)> = raw_columns(conn, table)?
        .into_iter()
        .filter(|c| c.pk > 0)
        .map(|c| (c.pk, c.name))
        .collect();
    columns.sort_by_key(|(pk, _)| *pk);
    Ok((name, columns.into_iter().map(|(_, name)| name).collect()))
}

fn find_cols_in_sig(sig: &str) -> Vec<String> {
    static COLS_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)(?:"(.+?)")|([a-z0-9_]+)"#).expect("static regex is valid")
    });
    COLS_RE
        .captures_iter(sig)
        .map(|c| {
            c.get(1)
                .or_else(|| c.get(2))
                .map_or(String::new(), |m| m.as_str().to_string())
        })
        .collect()
}

#[derive(Debug, Clone)]
struct ReflectedFk {
    name: Option<String>,
    columns: Vec<String>,
    referred_table: String,
    referred_columns: Vec<String>,
    ondelete: Option<String>,
    onupdate: Option<String>,
    deferrable: Option<bool>,
    initially: Option<String>,
}

type FkSig = (Vec<String>, String, Vec<String>);

/// `SQLiteDialect.get_foreign_keys`.
fn foreign_keys(conn: &Connection, table: &str) -> Result<Vec<ReflectedFk>> {
    static FK_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"(?i)(?:CONSTRAINT (\w+) +)?",
            r"FOREIGN KEY *\( *(.+?) *\) +",
            r#"REFERENCES +(?:(?:"(.+?)")|([a-z0-9_]+)) *\( *((?:(?:"[^"]+"|[a-z0-9_]+) *(?:, *)?)+)\) *"#,
            r"((?:ON (?:DELETE|UPDATE) (?:SET NULL|SET DEFAULT|CASCADE|RESTRICT|NO ACTION) *)*)",
            r"((?:NOT +)?DEFERRABLE)?",
            r"(?: +INITIALLY +(DEFERRED|IMMEDIATE))?",
        ))
        .expect("static regex is valid")
    });
    static ON_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r" *\bON\b *").expect("static regex is valid"));

    let mut statement = conn.prepare(&format!(
        "PRAGMA main.foreign_key_list({})",
        sql_string(table)
    ))?;
    let rows: Vec<(i64, String, String, Option<String>)> = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })?
        .collect::<Result<_, _>>()?;
    let mut fks: Vec<(i64, ReflectedFk)> = Vec::new();
    for (id, referred_table, local, remote) in rows {
        let referred_columns = if remote.is_none() {
            if table_exists(conn, &referred_table)? {
                pk_constraint(conn, &referred_table)?.1
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let index = match fks.iter().position(|(existing, _)| *existing == id) {
            Some(index) => index,
            None => {
                fks.push((
                    id,
                    ReflectedFk {
                        name: None,
                        columns: Vec::new(),
                        referred_table,
                        referred_columns,
                        ondelete: None,
                        onupdate: None,
                        deferrable: None,
                        initially: None,
                    },
                ));
                fks.len() - 1
            }
        };
        let fk = &mut fks[index].1;
        fk.columns.push(local);
        if let Some(remote) = remote {
            fk.referred_columns.push(remote);
        }
    }

    // keys_by_signature: a dict, so a later FK with the same signature replaces
    // an earlier one in place.
    let mut by_signature: Vec<(FkSig, ReflectedFk)> = Vec::new();
    for (_, fk) in fks {
        let sig = (
            fk.columns.clone(),
            fk.referred_table.clone(),
            fk.referred_columns.clone(),
        );
        match by_signature
            .iter_mut()
            .find(|(existing, _)| *existing == sig)
        {
            Some(entry) => entry.1 = fk,
            None => by_signature.push((sig, fk)),
        }
    }

    let mut result = Vec::new();
    if let Some(sql) = table_sql(conn, table)? {
        for captures in FK_RE.captures_iter(&sql) {
            let group = |i: usize| captures.get(i).map(|m| m.as_str().to_string());
            let constrained = find_cols_in_sig(&group(2).unwrap_or_default());
            let referred_columns = match group(5) {
                Some(columns) if !columns.is_empty() => find_cols_in_sig(&columns),
                _ => constrained.clone(),
            };
            let referred_name = group(3).or_else(|| group(4)).unwrap_or_default();
            let mut ondelete = None;
            let mut onupdate = None;
            let clauses = group(6).unwrap_or_default().to_uppercase();
            for token in ON_RE.split(&clauses) {
                if let Some(rest) = token.strip_prefix("DELETE") {
                    let rest = rest.trim();
                    if !rest.is_empty() && rest != "NO ACTION" {
                        ondelete = Some(rest.to_string());
                    }
                } else if let Some(rest) = token.strip_prefix("UPDATE") {
                    let rest = rest.trim();
                    if !rest.is_empty() && rest != "NO ACTION" {
                        onupdate = Some(rest.to_string());
                    }
                }
            }
            let deferrable = group(7).map(|d| !d.to_uppercase().contains("NOT"));
            let initially = group(8).map(|i| i.to_uppercase());
            let sig = (constrained, referred_name, referred_columns);
            let Some(position) = by_signature
                .iter()
                .position(|(existing, _)| *existing == sig)
            else {
                tracing::debug!(
                    "SQL-parsed foreign key constraint {sig:?} could not be located in PRAGMA foreign_keys for table {table}"
                );
                continue;
            };
            let (_, mut fk) = by_signature.remove(position);
            fk.name = group(1);
            fk.ondelete = ondelete;
            fk.onupdate = onupdate;
            fk.deferrable = deferrable;
            fk.initially = initially;
            result.push(fk);
        }
    }
    result.extend(by_signature.into_iter().map(|(_, fk)| fk));
    Ok(result)
}

struct RawIndex {
    name: String,
    unique: bool,
    partial: bool,
}

/// `SQLiteDialect.get_indexes`, sorted by name; `None` columns (expression
/// indexes) drop the index, as SQLAlchemy does with a warning.
fn indexes(conn: &Connection, table: &str, include_auto: bool) -> Result<Vec<Index>> {
    static WHERE_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)\)\s+where\s+(.+)").expect("static regex is valid"));
    let mut statement = conn.prepare(&format!("PRAGMA main.index_list({})", sql_string(table)))?;
    let raw: Vec<RawIndex> = statement
        .query_map([], |row| {
            Ok(RawIndex {
                name: row.get(1)?,
                unique: row.get::<_, i64>(2)? != 0,
                partial: row.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut result = Vec::new();
    'indexes: for index in raw {
        if !include_auto && index.name.starts_with("sqlite_autoindex") {
            continue;
        }
        let mut where_clause = None;
        if index.partial {
            let sql: Option<String> = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE name = ? AND type = 'index'",
                    [&index.name],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            where_clause = sql.and_then(|sql| WHERE_RE.captures(&sql).map(|c| c[1].to_string()));
        }
        let mut info = conn.prepare(&format!(
            "PRAGMA main.index_info({})",
            sql_string(&index.name)
        ))?;
        let names: Vec<Option<String>> = info
            .query_map([], |row| row.get::<_, Option<String>>(2))?
            .collect::<Result<_, _>>()?;
        let mut columns = Vec::new();
        for name in names {
            match name {
                Some(name) => columns.push(name),
                None => {
                    tracing::debug!(
                        "skipped unsupported reflection of expression-based index {}",
                        index.name
                    );
                    continue 'indexes;
                }
            }
        }
        result.push(Index {
            name: index.name,
            columns,
            unique: index.unique,
            where_clause,
        });
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

/// `SQLiteDialect.get_unique_constraints`.
fn unique_constraints(
    conn: &Connection,
    table: &str,
) -> Result<Vec<(Option<String>, Vec<String>)>> {
    static UNIQUE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)(?:CONSTRAINT "?(.+?)"? +)?UNIQUE *\((.+?)\)"#)
            .expect("static regex is valid")
    });
    static INLINE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r#"(?i)(?:(".+?")|(?:[\[`])?([a-z0-9_]+)(?:[\]`])?)[\t ]"#,
            r"+[a-z0-9_ ]+?[\t ]+UNIQUE"
        ))
        .expect("static regex is valid")
    });
    let mut auto_by_sig: Vec<(Vec<String>, String)> = Vec::new();
    for index in indexes(conn, table, true)? {
        if !index.name.starts_with("sqlite_autoindex") {
            continue;
        }
        match auto_by_sig
            .iter_mut()
            .find(|(sig, _)| *sig == index.columns)
        {
            Some(entry) => entry.1 = index.name,
            None => auto_by_sig.push((index.columns, index.name)),
        }
    }
    let Some(sql) = table_sql(conn, table)? else {
        return Ok(Vec::new());
    };
    let mut parsed: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for captures in UNIQUE_RE.captures_iter(&sql) {
        parsed.push((
            captures.get(1).map(|m| m.as_str().to_string()),
            find_cols_in_sig(&captures[2]),
        ));
    }
    for captures in INLINE_RE.captures_iter(&sql) {
        let sig = captures
            .get(1)
            .or_else(|| captures.get(2))
            .map_or("", |m| m.as_str());
        parsed.push((None, find_cols_in_sig(sig)));
    }
    let mut result = Vec::new();
    for (name, columns) in parsed {
        if let Some(position) = auto_by_sig.iter().position(|(sig, _)| *sig == columns) {
            auto_by_sig.remove(position);
            result.push((name, columns));
        }
    }
    Ok(result)
}

/// `SQLiteDialect.get_check_constraints`.
fn check_constraints(conn: &Connection, table: &str) -> Result<Vec<(Option<String>, String)>> {
    static CHECK_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:CONSTRAINT (.+) +)?CHECK *\( *(.+) *\),? *")
            .expect("static regex is valid")
    });
    static QUOTES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"^"|"$"#).expect("static regex is valid"));
    let sql = table_sql(conn, table)?.unwrap_or_default();
    let mut checks: Vec<(Option<String>, String)> = CHECK_RE
        .captures_iter(&sql)
        .map(|c| {
            (
                c.get(1)
                    .map(|m| QUOTES.replace_all(m.as_str(), "").into_owned()),
                c[2].to_string(),
            )
        })
        .collect();
    checks.sort_by(|a, b| {
        let key = |c: &(Option<String>, String)| c.0.clone().unwrap_or_else(|| "~".to_string());
        key(a).cmp(&key(b))
    });
    Ok(checks)
}

fn reflect(conn: &Connection, table: &str) -> Result<Reflected> {
    let raw = raw_columns(conn, table)?;
    if raw.is_empty() {
        return Err(Error::Batch(format!("NoSuchTableError: {table}")));
    }
    let mut columns = Vec::new();
    for column in raw {
        columns.push(Column {
            type_sql: resolve_type(&column.declared)?,
            nullable: !column.notnull,
            default: reflect_default(column.default),
            name: column.name,
            transfer: true,
        });
    }
    let mut constraints = Vec::new();
    let (pk_name, pk_columns) = pk_constraint(conn, table)?;
    constraints.push(Constraint {
        name: pk_name,
        kind: ConstraintKind::PrimaryKey(pk_columns),
    });
    for fk in foreign_keys(conn, table)? {
        // Inspector.reflect_table reflects the referred table too, and fails
        // when it does not exist.
        if !table_exists(conn, &fk.referred_table)? {
            return Err(Error::Batch(format!(
                "NoSuchTableError: {}",
                fk.referred_table
            )));
        }
        constraints.push(Constraint {
            name: fk.name,
            kind: ConstraintKind::ForeignKey {
                columns: fk.columns,
                referred_table: fk.referred_table,
                referred_columns: fk.referred_columns,
                ondelete: fk.ondelete,
                onupdate: fk.onupdate,
                deferrable: fk.deferrable,
                initially: fk.initially,
            },
        });
    }
    for (name, columns) in unique_constraints(conn, table)? {
        constraints.push(Constraint {
            name,
            kind: ConstraintKind::Unique(columns),
        });
    }
    for (name, sqltext) in check_constraints(conn, table)? {
        constraints.push(Constraint {
            name,
            kind: ConstraintKind::Check(sqltext),
        });
    }
    Ok(Reflected {
        columns,
        constraints,
        indexes: indexes(conn, table, false)?,
    })
}

/// `get_column_specification` of SQLAlchemy's SQLite DDL compiler.
fn column_spec(name: &str, type_sql: &str, nullable: bool, default: Option<&str>) -> String {
    let mut spec = format!("{} {}", quote(name), type_sql);
    if let Some(default) = default {
        spec.push_str(&format!(" DEFAULT {default}"));
    }
    if !nullable {
        spec.push_str(" NOT NULL");
    }
    spec
}

fn convention_name(table: &str, op: &BatchOp) -> String {
    match op {
        BatchOp::CreateUnique { columns } => format!("uq_{table}_{}", columns[0]),
        BatchOp::CreateForeignKey {
            referent, local, ..
        } => format!("fk_{table}_{}_{referent}", local[0]),
        _ => String::new(),
    }
}

/// `requires_recreate_in_batch`: only plain ADD COLUMNs can be ALTERed in place.
fn requires_recreate(ops: &[BatchOp]) -> bool {
    ops.iter().any(|op| !matches!(op, BatchOp::AddColumn(_)))
}

/// Run one `with op.batch_alter_table(table) as batch_op:` block.
pub fn batch_alter_table(run: &mut StepRunner<'_>, table: &str, ops: &[BatchOp]) -> Result<()> {
    if !requires_recreate(ops) {
        for op in ops {
            if let BatchOp::AddColumn(column) = op {
                run.exec(&format!(
                    "ALTER TABLE {} ADD COLUMN {}",
                    quote(table),
                    column_spec(column.name, column.type_sql, column.nullable, None)
                ))?;
            }
        }
        return Ok(());
    }

    let reflected = reflect(run.conn(), table)?;
    let mut columns = reflected.columns;
    // ApplyBatchImpl._grab_table_elements: named constraints keyed by name, in
    // order; unnamed ones (and unnamed reflected CHECKs are skipped) apart.
    let mut named: Vec<(String, Constraint)> = Vec::new();
    let mut unnamed: Vec<Constraint> = Vec::new();
    for constraint in reflected.constraints {
        if let ConstraintKind::Check(_) = constraint.kind
            && constraint.name.is_none()
        {
            continue;
        }
        match constraint.name.clone() {
            Some(name) => match named.iter_mut().find(|(existing, _)| *existing == name) {
                Some(entry) => entry.1 = constraint,
                None => named.push((name, constraint)),
            },
            None => unnamed.push(constraint),
        }
    }
    let mut pk_dropped = false;

    for op in ops {
        match op {
            BatchOp::AddColumn(column) => columns.push(Column {
                name: column.name.to_string(),
                type_sql: column.type_sql.to_string(),
                nullable: column.nullable,
                default: None,
                transfer: false,
            }),
            BatchOp::DropColumn(name) => {
                let before = columns.len();
                columns.retain(|c| c.name != *name);
                if columns.len() == before {
                    return Err(Error::Batch(format!("KeyError: '{name}'")));
                }
                // A dropped primary key column leaves the primary key.
                for (_, constraint) in named.iter_mut() {
                    if let ConstraintKind::PrimaryKey(pk) = &mut constraint.kind {
                        pk.retain(|c| c != name);
                    }
                }
                for constraint in unnamed.iter_mut() {
                    if let ConstraintKind::PrimaryKey(pk) = &mut constraint.kind {
                        pk.retain(|c| c != name);
                    }
                }
            }
            BatchOp::CreateUnique { columns: unique } => {
                let name = convention_name(table, op);
                let constraint = Constraint {
                    name: Some(name.clone()),
                    kind: ConstraintKind::Unique(unique.iter().map(|c| c.to_string()).collect()),
                };
                match named.iter_mut().find(|(existing, _)| *existing == name) {
                    Some(entry) => entry.1 = constraint,
                    None => named.push((name, constraint)),
                }
            }
            BatchOp::CreateForeignKey {
                referent,
                local,
                remote,
            } => {
                let name = convention_name(table, op);
                let constraint = Constraint {
                    name: Some(name.clone()),
                    kind: ConstraintKind::ForeignKey {
                        columns: local.iter().map(|c| c.to_string()).collect(),
                        referred_table: referent.to_string(),
                        referred_columns: remote.iter().map(|c| c.to_string()).collect(),
                        ondelete: None,
                        onupdate: None,
                        deferrable: None,
                        initially: None,
                    },
                };
                match named.iter_mut().find(|(existing, _)| *existing == name) {
                    Some(entry) => entry.1 = constraint,
                    None => named.push((name, constraint)),
                }
            }
            BatchOp::DropConstraint(name) => {
                let Some(position) = named.iter().position(|(existing, _)| existing == name) else {
                    return Err(Error::Batch(format!("No such constraint: '{name}'")));
                };
                let (_, constraint) = named.remove(position);
                if let ConstraintKind::PrimaryKey(_) = constraint.kind {
                    pk_dropped = true;
                }
            }
        }
    }

    let temp = format!("_alembic_tmp_{table}");
    let temp: String = temp.chars().take(50).collect();
    let column_names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();

    // _transfer_elements_to_new_table: constraints whose columns all survive.
    let mut kept: Vec<Constraint> = named
        .into_iter()
        .map(|(_, c)| c)
        .chain(unnamed)
        .filter(|c| c.columns().iter().all(|col| column_names.contains(col)))
        .collect();
    // The primary key renders first (create_table_constraints); without a
    // reflected one, the columns' primary_key flags would build an unnamed one,
    // which never happens for the tables the revisions batch.
    if pk_dropped {
        kept.retain(|c| !matches!(c.kind, ConstraintKind::PrimaryKey(_)));
    }
    let pk_position = kept
        .iter()
        .position(|c| matches!(c.kind, ConstraintKind::PrimaryKey(_)));
    if let Some(position) = pk_position {
        let pk = kept.remove(position);
        kept.insert(0, pk);
    }

    let mut parts: Vec<String> = columns
        .iter()
        .map(|c| column_spec(&c.name, &c.type_sql, c.nullable, c.default.as_deref()))
        .collect();
    parts.extend(kept.iter().filter_map(Constraint::render));
    let create = format!(
        "\nCREATE TABLE {} (\n\t{}\n)\n\n",
        quote(&temp),
        parts.join(", \n\t")
    );
    run.exec(&create)?;

    let transfers: Vec<&Column> = columns.iter().filter(|c| c.transfer).collect();
    let insert = format!(
        "INSERT INTO {} ({}) SELECT {} \nFROM {}",
        quote(&temp),
        transfers
            .iter()
            .map(|c| quote(&c.name))
            .collect::<Vec<_>>()
            .join(", "),
        transfers
            .iter()
            .map(|c| format!("{}.{}", quote(table), quote(&c.name)))
            .collect::<Vec<_>>()
            .join(", "),
        quote(table)
    );
    let copied = run
        .exec(&insert)
        .and_then(|()| run.exec(&format!("\nDROP TABLE {}", quote(table))));
    if let Err(error) = copied {
        // ApplyBatchImpl._create drops the new table before re-raising.
        if let Err(drop_error) = run.exec(&format!("\nDROP TABLE {}", quote(&temp))) {
            tracing::warn!("could not drop {temp} after a failed batch copy: {drop_error}");
        }
        return Err(error);
    }
    run.exec(&format!(
        "ALTER TABLE {} RENAME TO {}",
        quote(&temp),
        quote(table)
    ))?;
    for index in reflected.indexes {
        if !index
            .columns
            .iter()
            .all(|c| column_names.contains(&c.as_str()))
        {
            continue;
        }
        let mut sql = format!(
            "CREATE {}INDEX {} ON {} ({})",
            if index.unique { "UNIQUE " } else { "" },
            quote(&index.name),
            quote(table),
            index
                .columns
                .iter()
                .map(|c| quote(c))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Some(where_clause) = index.where_clause {
            sql.push_str(&format!(" WHERE {where_clause}"));
        }
        run.exec(&sql)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `_resolve_type_affinity` for the declared types the history tables use.
    #[test]
    fn reflected_types_render_like_sqlalchemy() {
        let cases = [
            ("INTEGER", "INTEGER"),
            ("TEXT", "TEXT"),
            ("JSON", "JSON"),
            ("FLOAT", "FLOAT"),
            ("DATETIME", "DATETIME"),
            ("VARCHAR", "VARCHAR"),
            ("VARCHAR(32)", "VARCHAR(32)"),
            ("BOOLEAN", "BOOLEAN"),
            ("int", "INTEGER"),
            ("NUMERIC(10, 2)", "NUMERIC(10, 2)"),
            ("UNSIGNED BIG INT", "INTEGER"),
            ("DOUBLE PRECISION", "REAL"),
            ("VARCHAR2(10)", "TEXT(10)"),
            ("INTEGER(11)", "INTEGER"),
            ("WHATEVER", "NUMERIC"),
        ];
        for (declared, rendered) in cases {
            assert_eq!(
                resolve_type(declared).ok().as_deref(),
                Some(rendered),
                "{declared}"
            );
        }
        assert!(resolve_type("").is_err());
    }

    #[test]
    fn column_signature_parsing() {
        assert_eq!(find_cols_in_sig("\"key\""), vec!["key"]);
        assert_eq!(find_cols_in_sig("a, \"b c\""), vec!["a", "b c"]);
    }

    #[test]
    fn defaults_are_parenthesized_like_alembic() {
        assert_eq!(
            reflect_default(Some("'unrated'".into())).as_deref(),
            Some("'unrated'")
        );
        assert_eq!(reflect_default(Some("1".into())).as_deref(), Some("1"));
        assert_eq!(reflect_default(Some("10".into())).as_deref(), Some("(10)"));
        assert_eq!(
            reflect_default(Some("CURRENT_TIMESTAMP".into())).as_deref(),
            Some("(CURRENT_TIMESTAMP)")
        );
        assert_eq!(reflect_default(None), None);
    }
}
