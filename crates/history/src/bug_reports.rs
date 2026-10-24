//! The `bug_reports` table queries of `api/bug_report.py`. The report module
//! (files, archives, the watcher) stays in the backend and calls these.

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, params_from_iter};
use serde_json::{Map, Value};

use crate::db::HistoryDb;
use crate::error::{Error, Result};
use crate::sqlvalue::{bind, to_json};

/// `REPORT_STATUS_QUEUED`.
pub const REPORT_STATUS_QUEUED: &str = "queued";

/// The columns, in `database_models.bug_reports` order.
const COLUMNS: [&str; 17] = [
    "localID",
    "eventID",
    "baseEventID",
    "issueTime",
    "creationTime",
    "submissionTime",
    "description",
    "multimedia",
    "machineID",
    "logFiles",
    "machineInfo",
    "machineLogs",
    "machineStatus",
    "status",
    "ticketNumber",
    "contactName",
    "contactEmail",
];

fn select_columns() -> String {
    COLUMNS
        .iter()
        .map(|c| format!("bug_reports.\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A `bug_reports` row as SQLAlchemy returns it: each column the Python
/// value (an int, a str, a bool for the `Boolean` columns, or `None`).
#[derive(Debug, Clone, PartialEq)]
pub struct BugReportRow {
    pub local_id: Value,
    pub event_id: Value,
    pub base_event_id: Value,
    pub issue_time: Value,
    pub creation_time: Value,
    pub submission_time: Value,
    pub description: Value,
    pub multimedia: Value,
    pub machine_id: Value,
    pub log_files: Value,
    pub machine_info: Value,
    pub machine_logs: Value,
    pub machine_status: Value,
    pub status: Value,
    pub ticket_number: Value,
    pub contact_name: Value,
    pub contact_email: Value,
}

/// SQLAlchemy's `Boolean` result processor: `bool(value)` or `None`.
fn boolean(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Bool(i != 0),
        ValueRef::Real(f) => Value::Bool(f != 0.0),
        ValueRef::Text(t) | ValueRef::Blob(t) => Value::Bool(!t.is_empty()),
    }
}

/// Python truthiness of a column value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

impl BugReportRow {
    fn from_row(row: &rusqlite::Row<'_>) -> Result<Self> {
        let get = |i: usize| -> Result<Value> { to_json(row.get_ref(i)?) };
        Ok(Self {
            local_id: get(0)?,
            event_id: get(1)?,
            base_event_id: get(2)?,
            issue_time: get(3)?,
            creation_time: get(4)?,
            submission_time: get(5)?,
            description: get(6)?,
            multimedia: get(7)?,
            machine_id: get(8)?,
            log_files: get(9)?,
            machine_info: boolean(row.get_ref(10)?),
            machine_logs: boolean(row.get_ref(11)?),
            machine_status: boolean(row.get_ref(12)?),
            status: get(13)?,
            ticket_number: get(14)?,
            contact_name: get(15)?,
            contact_email: get(16)?,
        })
    }

    pub fn status_str(&self) -> Option<&str> {
        self.status.as_str()
    }

    /// `_row_to_report_info`: the report as GET /reports lists it.
    pub fn to_report_info(&self) -> Value {
        let log_files: Vec<Value> = match &self.log_files {
            Value::String(s) => s
                .split(',')
                .filter(|name| !name.is_empty())
                .map(Value::from)
                .collect(),
            _ => Vec::new(),
        };
        pyfmt::object([
            ("description", self.description.clone()),
            ("dateAndTime", self.creation_time.clone()),
            ("issueTime", self.issue_time.clone()),
            (
                "attachments",
                pyfmt::object([
                    (
                        "debugFiles",
                        pyfmt::object([("automatic", Value::Array(log_files))]),
                    ),
                    ("machineInfo", Value::Bool(truthy(&self.machine_info))),
                    ("machineLogs", Value::Bool(truthy(&self.machine_logs))),
                    ("machineStatus", Value::Bool(truthy(&self.machine_status))),
                ]),
            ),
            ("multimedia", self.multimedia.clone()),
            ("machineID", self.machine_id.clone()),
            ("eventID", self.event_id.clone()),
            ("baseEventID", self.base_event_id.clone()),
            ("ticket", self.ticket_number.clone()),
            ("localID", self.local_id.clone()),
            ("status", self.status.clone()),
        ])
    }

    /// `_dispatch_payload`: the `upload_report` event body.
    pub fn dispatch_payload(&self) -> Value {
        pyfmt::object([
            ("localID", self.local_id.clone()),
            ("machineID", self.machine_id.clone()),
            ("ticket", self.ticket_number.clone()),
            ("issueTime", self.issue_time.clone()),
            ("requestTime", self.creation_time.clone()),
            ("description", self.description.clone()),
            ("name", self.contact_name.clone()),
            ("email", self.contact_email.clone()),
        ])
    }
}

/// The columns `sweep_reports` reads for every row.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepRow {
    pub local_id: Value,
    pub status: Value,
    pub creation_time: Value,
    pub submission_time: Value,
}

/// Columns to set (`_update_report_db(local_id, values)` and the inserts).
/// `None` leaves a column alone; `Some(Value::Null)` stores NULL. Values are
/// bound as Python binds them; the `Boolean` columns go through SQLAlchemy's
/// strict boolean processor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BugReportValues {
    pub local_id: Option<Value>,
    pub event_id: Option<Value>,
    pub base_event_id: Option<Value>,
    pub issue_time: Option<Value>,
    pub creation_time: Option<Value>,
    pub submission_time: Option<Value>,
    pub description: Option<Value>,
    pub multimedia: Option<Value>,
    pub machine_id: Option<Value>,
    pub log_files: Option<Value>,
    pub machine_info: Option<Value>,
    pub machine_logs: Option<Value>,
    pub machine_status: Option<Value>,
    pub status: Option<Value>,
    pub ticket_number: Option<Value>,
    pub contact_name: Option<Value>,
    pub contact_email: Option<Value>,
}

/// SQLAlchemy's `Boolean` bind processor: `None`, `True`/`False` and the
/// ints and floats equal to them; anything else raises.
fn bind_boolean(value: &Value) -> Result<SqlValue> {
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::Bool(b) => Ok(SqlValue::Integer(i64::from(*b))),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f == 0.0 || f == 1.0 => Ok(SqlValue::Integer(f as i64)),
            _ => Err(Error::InvalidValue(format!(
                "Value {n} is not None, True, or False"
            ))),
        },
        other => Err(Error::InvalidValue(format!("Not a boolean value: {other}"))),
    }
}

impl BugReportValues {
    /// The set columns with their bound values, in table order.
    fn bound(&self) -> Result<Vec<(&'static str, SqlValue)>> {
        let fields: [(&'static str, &Option<Value>, bool); 17] = [
            ("localID", &self.local_id, false),
            ("eventID", &self.event_id, false),
            ("baseEventID", &self.base_event_id, false),
            ("issueTime", &self.issue_time, false),
            ("creationTime", &self.creation_time, false),
            ("submissionTime", &self.submission_time, false),
            ("description", &self.description, false),
            ("multimedia", &self.multimedia, false),
            ("machineID", &self.machine_id, false),
            ("logFiles", &self.log_files, false),
            ("machineInfo", &self.machine_info, true),
            ("machineLogs", &self.machine_logs, true),
            ("machineStatus", &self.machine_status, true),
            ("status", &self.status, false),
            ("ticketNumber", &self.ticket_number, false),
            ("contactName", &self.contact_name, false),
            ("contactEmail", &self.contact_email, false),
        ];
        let mut bound = Vec::new();
        for (column, value, boolean) in fields {
            if let Some(value) = value {
                let value = if boolean {
                    bind_boolean(value)?
                } else {
                    bind(value)?
                };
                bound.push((column, value));
            }
        }
        Ok(bound)
    }
}

/// `_dedupe`: the first occurrence of each value, in order.
fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for value in values {
        if !result.contains(&value) {
            result.push(value);
        }
    }
    result
}

/// Python's `str(value)` of a JSON-decoded value.
fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => i.to_string(),
            (None, Some(f)) => pyfmt::repr_f64(f),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// `_attachments_to_log_files`: the `logFiles` column of a report's
/// attachments (user then automatic debug files, deduplicated, comma joined).
pub fn attachments_to_log_files(attachments: Option<&Value>) -> Value {
    let Some(attachments) = attachments.filter(|a| truthy(a)) else {
        return Value::Null;
    };
    let debug_files = attachments
        .get("debugFiles")
        .filter(|d| truthy(d))
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let mut names = Vec::new();
    for key in ["user", "automatic"] {
        if let Some(Value::Array(list)) = debug_files.get(key) {
            names.extend(list.iter().filter(|n| truthy(n)).map(py_str));
        }
    }
    let unique = dedupe(names);
    if unique.is_empty() {
        Value::Null
    } else {
        Value::String(unique.join(","))
    }
}

/// `_collected_db_values`: the columns a collected draft writes.
pub fn collected_db_values(report_info: &Value) -> Result<BugReportValues> {
    let info = report_info
        .as_object()
        .ok_or_else(|| Error::InvalidValue("report info must be an object".into()))?;
    let attachments = info.get("attachments").filter(|a| truthy(a));
    let get = |key: &str| info.get(key).cloned().unwrap_or(Value::Null);
    let attachment = |key: &str| {
        attachments
            .and_then(|a| a.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    };
    Ok(BugReportValues {
        issue_time: Some(
            info.get("issueTime")
                .cloned()
                .ok_or_else(|| Error::InvalidValue("KeyError: 'issueTime'".into()))?,
        ),
        description: Some(get("description")),
        multimedia: Some(get("multimedia")),
        machine_id: Some(get("machineID")),
        log_files: Some(attachments_to_log_files(attachments)),
        machine_info: Some(attachment("machineInfo")),
        machine_logs: Some(attachment("machineLogs")),
        machine_status: Some(attachment("machineStatus")),
        ticket_number: Some(get("ticket")),
        status: Some(Value::from("draft")),
        ..BugReportValues::default()
    })
}

/// `_draft_db_values`: the attachment columns a draft patch rewrites.
pub fn draft_db_values(attachments: &Value) -> BugReportValues {
    let attachment = |key: &str| attachments.get(key).cloned().unwrap_or(Value::Null);
    BugReportValues {
        log_files: Some(attachments_to_log_files(Some(attachments))),
        machine_logs: Some(attachment("machineLogs")),
        machine_info: Some(attachment("machineInfo")),
        machine_status: Some(attachment("machineStatus")),
        ..BugReportValues::default()
    }
}

/// A report the mobile app dispatched (`_parse_dispatch_body`'s result).
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedReport {
    pub local_id: String,
    pub ticket: Value,
    pub issue_time: Option<i64>,
    pub description: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// A FIQL value after `_coerce_filter_value`.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterValue {
    Null,
    Int(i64),
    Bool(bool),
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    Eq,
    Like,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilterCondition {
    pub column: &'static str,
    pub op: FilterOp,
    pub value: FilterValue,
}

/// `_parse_fiql`'s condition: OR of AND groups.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportFilter {
    pub groups: Vec<Vec<FilterCondition>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnType {
    Integer,
    Boolean,
    Text,
}

fn column(name: &str) -> Option<(&'static str, ColumnType)> {
    let column = *COLUMNS.iter().find(|c| **c == name)?;
    let kind = match column {
        "issueTime" | "creationTime" | "submissionTime" | "multimedia" | "ticketNumber" => {
            ColumnType::Integer
        }
        "machineInfo" | "machineLogs" | "machineStatus" => ColumnType::Boolean,
        _ => ColumnType::Text,
    };
    Some((column, kind))
}

/// `urllib.parse.unquote_plus`.
fn unquote_plus(text: &str) -> String {
    pyfmt::unquote(&text.replace('+', " "))
}

/// `_parse_fiql(filter_text)`: `(condition, invalid_filter)`. What raises in
/// Python (an integer column compared with a non-integer, an ordering against
/// `null` or a boolean; the handler answers 500) is an error here.
pub fn parse_fiql(filter_text: Option<&str>) -> Result<(Option<ReportFilter>, bool)> {
    let Some(text) = filter_text.filter(|t| !t.is_empty()) else {
        return Ok((None, false));
    };
    static EXPRESSION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^([A-Za-z][A-Za-z0-9_]*)(==|!=|=gt=|=ge=|=lt=|=le=)(.*)$")
            .expect("static regex is valid")
    });
    let mut groups = Vec::new();
    for or_part in text.split(',').filter(|p| !p.is_empty()) {
        let mut conditions = Vec::new();
        for expression in or_part.split(';').filter(|p| !p.is_empty()) {
            // `re.match(...$)`: `.` stops at a newline, `$` may match before
            // a trailing one.
            let candidate = expression.strip_suffix('\n').unwrap_or(expression);
            let Some(captures) = EXPRESSION.captures(candidate) else {
                continue;
            };
            if captures[3].contains('\n') {
                continue;
            }
            let Some((column, kind)) = column(&captures[1]) else {
                continue;
            };
            let raw = unquote_plus(&captures[3]);
            let value = if raw.to_lowercase() == "null" {
                FilterValue::Null
            } else {
                match kind {
                    ColumnType::Boolean => FilterValue::Bool(matches!(
                        raw.to_lowercase().as_str(),
                        "true" | "1" | "yes"
                    )),
                    ColumnType::Integer => {
                        FilterValue::Int(pyfmt::py_int(&raw).ok_or_else(|| {
                            Error::InvalidValue(format!(
                                "invalid literal for int() with base 10: '{raw}'"
                            ))
                        })?)
                    }
                    ColumnType::Text => FilterValue::Text(raw),
                }
            };
            let op = match &captures[2] {
                "==" => match &value {
                    FilterValue::Text(t) if t.contains('*') || t.contains('?') => FilterOp::Like,
                    _ => FilterOp::Eq,
                },
                "!=" => FilterOp::Ne,
                "=gt=" => FilterOp::Gt,
                "=ge=" => FilterOp::Ge,
                "=lt=" => FilterOp::Lt,
                _ => FilterOp::Le,
            };
            // SQLAlchemy refuses `<`, `>`, ... against None/True/False
            // (ArgumentError; the handler answers 500).
            if matches!(
                op,
                FilterOp::Gt | FilterOp::Ge | FilterOp::Lt | FilterOp::Le
            ) && matches!(value, FilterValue::Null | FilterValue::Bool(_))
            {
                return Err(Error::InvalidValue(
                    "Only '=', '!=', 'is_()', 'is_not()', 'is_distinct_from()', 'is_not_distinct_from()' operators can be used with None/True/False".into(),
                ));
            }
            conditions.push(FilterCondition { column, op, value });
        }
        if !conditions.is_empty() {
            groups.push(conditions);
        }
    }
    if groups.is_empty() {
        return Ok((None, true));
    }
    Ok((Some(ReportFilter { groups }), false))
}

impl ReportFilter {
    fn to_sql(&self, args: &mut Vec<SqlValue>) -> String {
        let groups: Vec<String> = self
            .groups
            .iter()
            .map(|group| {
                let terms: Vec<String> = group
                    .iter()
                    .map(|condition| {
                        let column = format!("bug_reports.\"{}\"", condition.column);
                        let value = match &condition.value {
                            FilterValue::Null => None,
                            FilterValue::Int(i) => Some(SqlValue::Integer(*i)),
                            FilterValue::Bool(b) => Some(SqlValue::Integer(i64::from(*b))),
                            FilterValue::Text(t) => Some(SqlValue::Text(t.clone())),
                        };
                        match (condition.op, value) {
                            (FilterOp::Eq, None) => format!("{column} IS NULL"),
                            (FilterOp::Ne, None) => format!("{column} IS NOT NULL"),
                            (FilterOp::Like, Some(SqlValue::Text(text))) => {
                                let pattern = text
                                    .replace('%', "\\%")
                                    .replace('_', "\\_")
                                    .replace('*', "%")
                                    .replace('?', "_");
                                args.push(SqlValue::Text(pattern));
                                format!("{column} LIKE ? ESCAPE '\\'")
                            }
                            (op, value) => {
                                let symbol = match op {
                                    FilterOp::Eq | FilterOp::Like => "=",
                                    FilterOp::Ne => "!=",
                                    FilterOp::Gt => ">",
                                    FilterOp::Ge => ">=",
                                    FilterOp::Lt => "<",
                                    FilterOp::Le => "<=",
                                };
                                match value {
                                    Some(value) => {
                                        args.push(value);
                                        format!("{column} {symbol} ?")
                                    }
                                    None => format!("{column} {symbol} NULL"),
                                }
                            }
                        }
                    })
                    .collect();
                format!("({})", terms.join(" AND "))
            })
            .collect();
        groups.join(" OR ")
    }
}

impl HistoryDb {
    fn report_rows(conn: &Connection, sql: &str, args: Vec<SqlValue>) -> Result<Vec<BugReportRow>> {
        let mut statement = conn.prepare(sql)?;
        let mut rows = statement.query(params_from_iter(args))?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            result.push(BugReportRow::from_row(row)?);
        }
        Ok(result)
    }

    fn insert_report_values(conn: &Connection, values: &BugReportValues) -> Result<()> {
        let bound = values.bound()?;
        let columns: Vec<String> = bound.iter().map(|(c, _)| format!("\"{c}\"")).collect();
        let marks = vec!["?"; bound.len()].join(", ");
        conn.execute(
            &format!(
                "INSERT INTO bug_reports ({}) VALUES ({marks})",
                columns.join(", ")
            ),
            params_from_iter(bound.into_iter().map(|(_, v)| v)),
        )?;
        Ok(())
    }

    /// `_get_report_row`.
    pub fn get_report_row(&self, local_id: &str) -> Result<Option<BugReportRow>> {
        let conn = self.connection();
        Ok(Self::report_rows(
            &conn,
            &format!(
                "SELECT {} \nFROM bug_reports \nWHERE bug_reports.\"localID\" = ?",
                select_columns()
            ),
            vec![SqlValue::Text(local_id.to_string())],
        )?
        .into_iter()
        .next())
    }

    /// `_insert_report`: the row of a draft the dial just collected.
    pub fn insert_report(&self, report_info: &Value) -> Result<()> {
        let info = report_info
            .as_object()
            .ok_or_else(|| Error::InvalidValue("report info must be an object".into()))?;
        let mut values = collected_db_values(report_info)?;
        values.local_id = Some(
            info.get("localID")
                .cloned()
                .ok_or_else(|| Error::InvalidValue("KeyError: 'localID'".into()))?,
        );
        values.event_id = Some(Value::Null);
        values.base_event_id = Some(Value::Null);
        values.creation_time = Some(
            info.get("dateAndTime")
                .cloned()
                .ok_or_else(|| Error::InvalidValue("KeyError: 'dateAndTime'".into()))?,
        );
        values.submission_time = Some(Value::Null);
        values.contact_name = Some(Value::Null);
        values.contact_email = Some(Value::Null);
        let conn = self.connection();
        Self::insert_report_values(&conn, &values)
    }

    /// `_insert_queued_report`: store a mobile dispatch as a `queued` row and
    /// return it; [`Error::DuplicateLocalId`] when the localID exists in any
    /// status.
    pub fn insert_queued_report(
        &self,
        dispatch: &QueuedReport,
        now: i64,
        machine_id: &Value,
    ) -> Result<BugReportRow> {
        if self.get_report_row(&dispatch.local_id)?.is_some() {
            return Err(Error::DuplicateLocalId);
        }
        let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
        let values = BugReportValues {
            local_id: Some(Value::String(dispatch.local_id.clone())),
            event_id: Some(Value::Null),
            base_event_id: Some(Value::Null),
            issue_time: Some(Value::from(dispatch.issue_time.unwrap_or(now))),
            creation_time: Some(Value::from(now)),
            submission_time: Some(Value::Null),
            description: Some(text(&dispatch.description)),
            multimedia: Some(Value::Null),
            machine_id: Some(machine_id.clone()),
            log_files: Some(Value::Null),
            machine_info: Some(Value::Null),
            machine_logs: Some(Value::Null),
            machine_status: Some(Value::Null),
            status: Some(Value::from(REPORT_STATUS_QUEUED)),
            ticket_number: Some(dispatch.ticket.clone()),
            contact_name: Some(text(&dispatch.name)),
            contact_email: Some(text(&dispatch.email)),
        };
        {
            let conn = self.connection();
            Self::insert_report_values(&conn, &values)?;
        }
        self.get_report_row(&dispatch.local_id)?.ok_or_else(|| {
            Error::InvalidValue("the queued report vanished after its insert".into())
        })
    }

    /// `_queued_report_rows`: the dispatches waiting for the dial, oldest first.
    pub fn queued_report_rows(&self) -> Result<Vec<BugReportRow>> {
        let conn = self.connection();
        Self::report_rows(
            &conn,
            &format!(
                "SELECT {} \nFROM bug_reports \nWHERE bug_reports.status = ? ORDER BY bug_reports.\"creationTime\", bug_reports.\"localID\"",
                select_columns()
            ),
            vec![SqlValue::Text(REPORT_STATUS_QUEUED.into())],
        )
    }

    /// `_update_report_db`: whether a row was updated.
    pub fn update_report(&self, local_id: &str, values: &BugReportValues) -> Result<bool> {
        let bound = values.bound()?;
        if bound.is_empty() {
            return Err(Error::InvalidValue("no columns to update".into()));
        }
        let assignments: Vec<String> = bound.iter().map(|(c, _)| format!("\"{c}\"=?")).collect();
        let mut args: Vec<SqlValue> = bound.into_iter().map(|(_, v)| v).collect();
        args.push(SqlValue::Text(local_id.to_string()));
        let conn = self.connection();
        let rows = conn.execute(
            &format!(
                "UPDATE bug_reports SET {} WHERE bug_reports.\"localID\" = ?",
                assignments.join(", ")
            ),
            params_from_iter(args),
        )?;
        Ok(rows > 0)
    }

    /// The database part of `_delete_report`: delete the row.
    pub fn delete_report_row(&self, local_id: &str) -> Result<bool> {
        let conn = self.connection();
        let rows = conn.execute(
            "DELETE FROM bug_reports WHERE bug_reports.\"localID\" = ?",
            [local_id],
        )?;
        tracing::info!("Deleted bug report draft with localID: {local_id}, affected rows: {rows}");
        Ok(rows > 0)
    }

    /// `_list_report_page`: newest first, `{"content", "size", "page", "hasMore"}`.
    pub fn list_report_page(
        &self,
        page: i64,
        size: i64,
        filter: Option<&ReportFilter>,
    ) -> Result<Value> {
        let mut where_args = Vec::new();
        let condition = filter.map(|f| f.to_sql(&mut where_args));
        let where_clause = condition
            .map(|c| format!(" \nWHERE {c}"))
            .unwrap_or_default();
        let conn = self.connection();
        let mut args = where_args.clone();
        args.push(SqlValue::Integer(size));
        args.push(SqlValue::Integer(page * size));
        let rows = Self::report_rows(
            &conn,
            &format!(
                "SELECT {} \nFROM bug_reports{where_clause} ORDER BY bug_reports.\"creationTime\" DESC, bug_reports.\"localID\" DESC\n LIMIT ? OFFSET ?",
                select_columns()
            ),
            args,
        )?;
        let total: i64 = conn.query_row(
            &format!("SELECT count(*) AS count_1 \nFROM bug_reports{where_clause}"),
            params_from_iter(where_args),
            |row| row.get(0),
        )?;
        Ok(pyfmt::object([
            (
                "content",
                Value::Array(rows.iter().map(BugReportRow::to_report_info).collect()),
            ),
            ("size", Value::from(size)),
            ("page", Value::from(page)),
            ("hasMore", Value::Bool((page + 1) * size < total)),
        ]))
    }

    /// The rows `sweep_reports` walks.
    pub fn sweep_rows(&self) -> Result<Vec<SweepRow>> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT bug_reports.\"localID\", bug_reports.status, bug_reports.\"creationTime\", bug_reports.\"submissionTime\" \nFROM bug_reports",
        )?;
        let mut rows = statement.query([])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            result.push(SweepRow {
                local_id: to_json(row.get_ref(0)?)?,
                status: to_json(row.get_ref(1)?)?,
                creation_time: to_json(row.get_ref(2)?)?,
                submission_time: to_json(row.get_ref(3)?)?,
            });
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn log_files_from_attachments() {
        let attachments = json!({"debugFiles": {"user": ["b", "a"], "automatic": ["a", "c", ""]}});
        assert_eq!(attachments_to_log_files(Some(&attachments)), json!("b,a,c"));
        assert_eq!(attachments_to_log_files(Some(&json!({}))), Value::Null);
        assert_eq!(attachments_to_log_files(None), Value::Null);
    }

    #[test]
    fn strict_booleans() {
        assert_eq!(bind_boolean(&json!(true)).ok(), Some(SqlValue::Integer(1)));
        assert_eq!(bind_boolean(&json!(0)).ok(), Some(SqlValue::Integer(0)));
        assert!(bind_boolean(&json!(2)).is_err());
        assert!(bind_boolean(&json!("yes")).is_err());
    }
}
