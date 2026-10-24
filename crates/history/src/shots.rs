//! `shot_database.ShotDataBase`: the espresso shot history.

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, params_from_iter};
use serde_json::{Map, Value};

use crate::db::HistoryDb;
use crate::error::{Error, Result};
use crate::files;
use crate::search::{SearchOrder, SearchOrderBy, SearchParams};
use crate::sqlvalue::{bind, json_column, to_json};

fn key_error(key: &str) -> Error {
    Error::InvalidValue(format!("KeyError: '{key}'"))
}

fn get<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Value> {
    object.get(key).ok_or_else(|| key_error(key))
}

/// `json.dumps(value)`, as SQLAlchemy's `JSON` type serialises a column.
fn dumps(value: &Value) -> String {
    pyfmt::dumps_default(value)
}

/// A `WHERE column = ?` term, or `column IS NULL` when the value is `None`
/// (SQLAlchemy turns `column == None` into `IS NULL`).
fn equals(column: &str, value: &Value, args: &mut Vec<SqlValue>) -> Result<String> {
    if value.is_null() {
        Ok(format!("{column} IS NULL"))
    } else {
        args.push(bind(value)?);
        Ok(format!("{column} = ?"))
    }
}

const SEARCH_COLUMNS: &str = "history.id AS history_id, history.uuid AS history_uuid, history.file AS history_file, history.time AS history_time, history.profile_name AS history_profile_name, history.profile_id AS history_profile_id, history.profile_key AS history_profile_key, history.debug_file AS history_debug_file, profile.\"key\" AS profile_key, profile.id AS profile_id, profile.author AS profile_author, profile.author_id AS profile_author_id, profile.display AS profile_display, profile.final_weight AS profile_final_weight, profile.last_changed AS profile_last_changed, profile.name AS profile_name, profile.temperature AS profile_temperature, profile.stages AS profile_stages, profile.variables AS profile_variables, profile.previous_authors AS profile_previous_authors";

/// Column positions in [`SEARCH_COLUMNS`].
mod col {
    pub const ID: usize = 0;
    pub const UUID: usize = 1;
    pub const FILE: usize = 2;
    pub const TIME: usize = 3;
    pub const NAME: usize = 4;
    pub const DEBUG_FILE: usize = 7;
    pub const PROFILE_KEY: usize = 8;
    pub const PROFILE_ID: usize = 9;
    pub const AUTHOR: usize = 10;
    pub const AUTHOR_ID: usize = 11;
    pub const DISPLAY: usize = 12;
    pub const FINAL_WEIGHT: usize = 13;
    pub const LAST_CHANGED: usize = 14;
    pub const PROFILE_NAME: usize = 15;
    pub const TEMPERATURE: usize = 16;
    pub const STAGES: usize = 17;
    pub const VARIABLES: usize = 18;
    pub const PREVIOUS_AUTHORS: usize = 19;
    pub const COUNT: usize = 20;
}

impl HistoryDb {
    /// `ShotDataBase.profile_exists`: the key of a stored profile equal to
    /// this one in every column (JSON columns compared with `json_extract`).
    fn profile_exists(conn: &Connection, profile: &Map<String, Value>) -> Result<Option<i64>> {
        let empty_list = Value::Array(Vec::new());
        let empty_object = Value::Object(Map::new());
        let zero = Value::from(0);
        let stages = dumps(profile.get("stages").unwrap_or(&empty_list));
        let variables = dumps(profile.get("variables").unwrap_or(&empty_list));
        let previous_authors = dumps(profile.get("previous_authors").unwrap_or(&empty_list));
        let display = dumps(profile.get("display").unwrap_or(&empty_object));

        let mut args = Vec::new();
        let mut terms = vec![
            equals("profile.id", get(profile, "id")?, &mut args)?,
            equals("profile.author", get(profile, "author")?, &mut args)?,
            equals("profile.author_id", get(profile, "author_id")?, &mut args)?,
        ];
        terms.push("json_extract(profile.display, ?) = json_extract(?, ?)".into());
        args.extend([
            SqlValue::Text("$".into()),
            SqlValue::Text(display),
            SqlValue::Text("$".into()),
        ]);
        terms.push(equals(
            "profile.final_weight",
            get(profile, "final_weight")?,
            &mut args,
        )?);
        terms.push(equals(
            "profile.last_changed",
            profile.get("last_changed").unwrap_or(&zero),
            &mut args,
        )?);
        terms.push(equals("profile.name", get(profile, "name")?, &mut args)?);
        terms.push(equals(
            "profile.temperature",
            get(profile, "temperature")?,
            &mut args,
        )?);
        for (column, text) in [
            ("stages", stages),
            ("variables", variables),
            ("previous_authors", previous_authors),
        ] {
            terms.push(format!(
                "json_extract(profile.{column}, ?) = json_extract(?, ?)"
            ));
            args.extend([
                SqlValue::Text("$".into()),
                SqlValue::Text(text),
                SqlValue::Text("$".into()),
            ]);
        }
        let sql = format!(
            "SELECT profile.\"key\" \nFROM profile \nWHERE {}",
            terms.join(" AND ")
        );
        Ok(conn
            .query_row(&sql, params_from_iter(args), |row| row.get(0))
            .optional()?)
    }

    fn insert_profile_with(conn: &mut Connection, profile: Option<&Value>) -> Result<i64> {
        let Some(profile) = profile.filter(|p| !p.is_null()) else {
            return Ok(-1);
        };
        let Some(profile) = profile.as_object() else {
            return Err(Error::InvalidValue(
                "the profile must be a JSON object".into(),
            ));
        };
        if let Some(existing) = Self::profile_exists(conn, profile)? {
            let shown = |key: &str| profile.get(key).map(|v| v.to_string()).unwrap_or_default();
            let (id, name, author_id) = (shown("id"), shown("name"), shown("author_id"));
            tracing::debug!(
                "Profile with id {id}, name {name}, and author_id {author_id} already exists."
            );
            return Ok(existing);
        }
        let empty_list = Value::Array(Vec::new());
        let zero = Value::from(0);
        let id = get(profile, "id")?;
        let name = get(profile, "name")?;
        let values = [
            bind(id)?,
            bind(get(profile, "author")?)?,
            bind(get(profile, "author_id")?)?,
            SqlValue::Text(dumps(get(profile, "display")?)),
            bind(get(profile, "final_weight")?)?,
            bind(profile.get("last_changed").unwrap_or(&zero))?,
            bind(name)?,
            bind(get(profile, "temperature")?)?,
            SqlValue::Text(dumps(profile.get("stages").unwrap_or(&empty_list))),
            SqlValue::Text(dumps(profile.get("variables").unwrap_or(&empty_list))),
            SqlValue::Text(dumps(
                profile.get("previous_authors").unwrap_or(&empty_list),
            )),
        ];
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO profile (id, author, author_id, display, final_weight, last_changed, name, temperature, stages, variables, previous_authors) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params_from_iter(values),
        )?;
        let profile_key = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO profile_fts (profile_key, profile_id, name) VALUES (?, ?, ?)",
            params_from_iter([SqlValue::Integer(profile_key), bind(id)?, bind(name)?]),
        )?;
        // `for stage in profile_data["stages"]`: a missing or malformed stage
        // list raises after the profile row and rolls the insert back.
        let stages = get(profile, "stages")?;
        let stages = stages.as_array().ok_or_else(|| {
            Error::InvalidValue("TypeError: the profile stages are not a list".into())
        })?;
        for stage in stages {
            let stage = stage.as_object().ok_or_else(|| {
                Error::InvalidValue("TypeError: a profile stage is not an object".into())
            })?;
            tx.execute(
                "INSERT INTO stage_fts (profile_key, profile_id, profile_name, stage_key, stage_name) VALUES (?, ?, ?, ?, ?)",
                params_from_iter([
                    SqlValue::Integer(profile_key),
                    bind(id)?,
                    bind(name)?,
                    bind(get(stage, "key")?)?,
                    bind(get(stage, "name")?)?,
                ]),
            )?;
        }
        tx.commit()?;
        Ok(profile_key)
    }

    /// `ShotDataBase.insert_profile`: store a profile (and its FTS rows) unless
    /// an identical one exists; returns its key, or -1 for no profile.
    pub fn insert_profile(&self, profile: Option<&Value>) -> Result<i64> {
        let mut conn = self.connection();
        Self::insert_profile_with(&mut conn, profile)
    }

    /// `ShotDataBase.insert_history`: store a shot (the dict `Shot.to_json()`
    /// returns, with `file` set) unless one with the same file exists; returns
    /// the history row id.
    pub fn insert_history(&self, entry: &Value) -> Result<i64> {
        let entry = entry
            .as_object()
            .ok_or_else(|| Error::InvalidValue("the history entry must be an object".into()))?;
        let mut conn = self.connection();
        let file = get(entry, "file")?;
        let mut args = Vec::new();
        let condition = equals("history.file", file, &mut args)?;
        let existing: Option<i64> = conn
            .query_row(
                &format!("SELECT history.id \nFROM history \nWHERE {condition}"),
                params_from_iter(args),
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            tracing::debug!("History entry with file {file} already exists.");
            return Ok(existing);
        }
        let uuid = match entry.get("id") {
            Some(id) => id.clone(),
            None => Value::String(uuid::Uuid::new_v4().to_string()),
        };
        let profile = entry.get("profile");
        let profile_key = Self::insert_profile_with(&mut conn, profile)?;

        let time = match get(entry, "time")? {
            Value::Number(n) => match n.as_i64() {
                Some(i) => self.zone.from_timestamp_int(i),
                None => self.zone.from_timestamp(n.as_f64().unwrap_or(f64::NAN))?,
            },
            Value::Bool(b) => self.zone.from_timestamp_int(i64::from(*b)),
            _ => {
                return Err(Error::InvalidValue(
                    "TypeError: 'time' must be a number".into(),
                ));
            }
        };
        let profile_id = match profile {
            Some(Value::Object(profile)) => get(profile, "id")?,
            _ => {
                return Err(Error::InvalidValue(
                    "TypeError: 'NoneType' object is not subscriptable".into(),
                ));
            }
        };
        let values = [
            bind(&uuid)?,
            bind(file)?,
            SqlValue::Text(time.to_sqlite()?),
            bind(get(entry, "profile_name")?)?,
            bind(profile_id)?,
            SqlValue::Integer(profile_key),
        ];
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO history (uuid, file, time, profile_name, profile_id, profile_key) VALUES (?, ?, ?, ?, ?, ?)",
            params_from_iter(values),
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    /// `ShotDataBase.link_debug_file`: returns the number of rows updated.
    pub fn link_debug_file(&self, history_id: i64, debug_file: &str) -> Result<usize> {
        let conn = self.connection();
        let rows = conn.execute(
            "UPDATE history SET debug_file=? WHERE history.id = ?",
            rusqlite::params![debug_file, history_id],
        )?;
        tracing::info!("debug file linked, affected rows: {{{rows}}}");
        Ok(rows)
    }

    /// `ShotDataBase.unlink_debug_file`: returns the number of rows updated.
    pub fn unlink_debug_file(&self, debug_file: &str) -> Result<usize> {
        let conn = self.connection();
        let rows = conn.execute(
            "UPDATE history SET debug_file=? WHERE history.debug_file = ?",
            rusqlite::params![Option::<String>::None, debug_file],
        )?;
        if rows == 0 {
            tracing::warn!("no columns affected, check relative file path");
        } else {
            tracing::info!("debug file linked, affected rows: {{{rows}}}");
        }
        Ok(rows)
    }

    /// `ShotDataBase.delete_shot`: delete a history row, then every profile
    /// no shot uses any more with its FTS rows.
    pub fn delete_shot(&self, shot_id: i64) -> Result<()> {
        let mut conn = self.connection();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM history WHERE history.id = ?", [shot_id])?;
        let orphans: Vec<i64> = {
            let mut statement = tx.prepare(
                "SELECT profile.\"key\" \nFROM profile \nWHERE (profile.\"key\" NOT IN (SELECT history.profile_key \nFROM history))",
            )?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        for orphan in orphans {
            tx.execute("DELETE FROM profile WHERE profile.\"key\" = ?", [orphan])?;
            tx.execute(
                "DELETE FROM profile_fts WHERE profile_fts.profile_key = ?",
                [orphan],
            )?;
            tx.execute(
                "DELETE FROM stage_fts WHERE stage_fts.profile_key = ?",
                [orphan],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// `ShotDataBase.search_history`: the shots matching `params`, each as the
    /// dict the history API returns, with the shot file's `data` and
    /// `push_to_brew_time` when `dump_data` is set. A shot whose file cannot
    /// be read is left out (after the LIMIT, as in Python).
    pub fn search_history(&self, params: &SearchParams) -> Result<Vec<Value>> {
        let mut args: Vec<SqlValue> = Vec::new();
        let mut conditions: Vec<String> = Vec::new();
        if let Some(query) = params.query.as_deref().filter(|q| !q.is_empty()) {
            conditions.push("(profile_fts.name LIKE ? OR stage_fts.stage_name LIKE ?)".to_string());
            args.push(SqlValue::Text(format!("%{query}%")));
            args.push(SqlValue::Text(format!("%{query}%")));
        }
        if !params.ids.is_empty() {
            let marks = vec!["?"; params.ids.len()].join(", ");
            conditions.push(format!(
                "(history.id IN ({marks}) OR history.uuid IN ({marks}) OR profile.id IN ({marks}))"
            ));
            for _ in 0..3 {
                args.extend(params.ids.iter().map(|id| id.to_sql()));
            }
        }
        // `if params.start_date:` 0.0 means no bound; NaN is truthy and fails.
        if let Some(start) = params.start_date.filter(|d| *d != 0.0) {
            conditions.push("history.time >= ?".into());
            args.push(SqlValue::Text(
                self.zone.from_timestamp(start)?.to_sqlite()?,
            ));
        }
        if let Some(end) = params.end_date.filter(|d| *d != 0.0) {
            conditions.push("history.time <= ?".into());
            args.push(SqlValue::Text(self.zone.from_timestamp(end)?.to_sqlite()?));
        }
        let direction = match params.sort {
            SearchOrder::Ascending => "ASC",
            SearchOrder::Descending => "DESC",
        };
        let mut order: Vec<String> = params
            .order_by
            .iter()
            .map(|o| match o {
                SearchOrderBy::Date => format!("history.time {direction}"),
                SearchOrderBy::Profile => format!("history.profile_name {direction}"),
            })
            .collect();
        order.push(format!("history.id {direction}"));

        let mut sql = format!(
            "SELECT DISTINCT {SEARCH_COLUMNS} \nFROM history JOIN profile ON history.profile_key = profile.\"key\" LEFT OUTER JOIN profile_fts ON profile.\"key\" = profile_fts.profile_key LEFT OUTER JOIN stage_fts ON profile.\"key\" = stage_fts.profile_key"
        );
        if !conditions.is_empty() {
            sql.push_str(" \nWHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY ");
        sql.push_str(&order.join(", "));
        if params.max_results > 0 {
            sql.push_str("\n LIMIT ? OFFSET ?");
            args.push(SqlValue::Integer(params.max_results));
            args.push(SqlValue::Integer(0));
        }

        let rows: Vec<Vec<SqlValue>> = {
            let conn = self.connection();
            let mut statement = conn.prepare(&sql)?;
            let mut rows = statement.query(params_from_iter(args))?;
            let mut collected = Vec::new();
            while let Some(row) = rows.next()? {
                let mut values = Vec::with_capacity(col::COUNT);
                for index in 0..col::COUNT {
                    values.push(row.get::<_, SqlValue>(index)?);
                }
                collected.push(values);
            }
            collected
        };

        let mut results = Vec::new();
        for row in rows {
            let value = |index: usize| to_json(rusqlite::types::ValueRef::from(&row[index]));
            let json = |index: usize| json_column(rusqlite::types::ValueRef::from(&row[index]));
            // SQLAlchemy converts every column (JSON, DATETIME) when the row is
            // read, before the shot file is opened: a bad value fails the
            // search even for a row whose file is missing.
            let file_entry = value(col::FILE)?;
            let time = match &row[col::TIME] {
                SqlValue::Text(text) => {
                    let stored = crate::localtime::parse_stored(text)?;
                    Value::from(self.zone.stored_timestamp(stored))
                }
                SqlValue::Null => {
                    return Err(Error::InvalidValue(
                        "TypeError: descriptor 'timestamp' requires a 'datetime.datetime' object but received a 'NoneType'".into(),
                    ));
                }
                other => {
                    return Err(Error::InvalidValue(format!(
                        "Couldn't parse datetime string '{other:?}' - value is not a string."
                    )));
                }
            };
            let profile = pyfmt::object([
                ("id", value(col::PROFILE_ID)?),
                ("db_key", value(col::PROFILE_KEY)?),
                ("author", value(col::AUTHOR)?),
                ("author_id", value(col::AUTHOR_ID)?),
                ("display", json(col::DISPLAY)?),
                ("final_weight", value(col::FINAL_WEIGHT)?),
                ("last_changed", value(col::LAST_CHANGED)?),
                ("name", value(col::PROFILE_NAME)?),
                ("temperature", value(col::TEMPERATURE)?),
                ("stages", json(col::STAGES)?),
                ("variables", json(col::VARIABLES)?),
                ("previous_authors", json(col::PREVIOUS_AUTHORS)?),
            ]);
            let mut data = Value::Null;
            let mut push_to_brew_time = Value::Null;
            if params.dump_data {
                let relative = match &file_entry {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                match files::read_shot_json(&self.shot_path(&relative)) {
                    Ok(Value::Object(mut contents)) => {
                        data = contents.remove("data").unwrap_or(Value::Null);
                        push_to_brew_time =
                            contents.remove("push_to_brew_time").unwrap_or(Value::Null);
                    }
                    Ok(_) => {
                        tracing::error!(
                            "Failed to read shot file {relative}: 'list' object has no attribute 'get'"
                        );
                        continue;
                    }
                    Err(error) => {
                        tracing::error!("Failed to read shot file {relative}: {error}");
                        continue;
                    }
                }
            }
            results.push(pyfmt::object([
                ("id", value(col::UUID)?),
                ("db_key", value(col::ID)?),
                ("time", time),
                ("file", file_entry),
                ("debug_file", value(col::DEBUG_FILE)?),
                ("name", value(col::NAME)?),
                ("data", data),
                ("push_to_brew_time", push_to_brew_time),
                ("profile", profile),
            ]));
        }
        tracing::info!("shot database query returned {} results", results.len());
        Ok(results)
    }

    /// `ShotDataBase.list_history_files`: `[{"file": ...}]` after the cursor,
    /// ordered by file, at most `max_results` clamped to 1..=200.
    pub fn list_history_files(&self, after: Option<&str>, max_results: i64) -> Result<Vec<Value>> {
        let limit = max_results.clamp(1, 200);
        let conn = self.connection();
        let mut args: Vec<SqlValue> = Vec::new();
        let mut sql = String::from("SELECT history.file \nFROM history");
        if let Some(after) = after.filter(|a| !a.is_empty()) {
            sql.push_str(" \nWHERE history.file > ?");
            args.push(SqlValue::Text(after.to_string()));
        }
        sql.push_str(" ORDER BY history.file ASC\n LIMIT ? OFFSET ?");
        args.push(SqlValue::Integer(limit));
        args.push(SqlValue::Integer(0));
        let mut statement = conn.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(args))?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            result.push(pyfmt::object([("file", to_json(row.get_ref(0)?)?)]));
        }
        Ok(result)
    }

    /// `ShotDataBase.autocomplete_profile_name`.
    pub fn autocomplete_profile_name(&self, prefix: &str) -> Result<Vec<Value>> {
        let conn = self.connection();
        let mut result = Vec::new();
        if prefix.is_empty() {
            let mut statement = conn.prepare(
                "SELECT DISTINCT history.profile_name \nFROM history GROUP BY history.profile_name ORDER BY count(*) DESC",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                result.push(pyfmt::object([
                    ("profile", to_json(row.get_ref(0)?)?),
                    ("type", Value::from("profile")),
                ]));
            }
            return Ok(result);
        }
        let pattern = format!("%{prefix}%");
        {
            let mut statement = conn.prepare(
                "SELECT DISTINCT profile_fts.name AS name \nFROM profile_fts \nWHERE profile_fts.name LIKE ?",
            )?;
            let mut rows = statement.query([&pattern])?;
            while let Some(row) = rows.next()? {
                result.push(pyfmt::object([
                    ("profile", to_json(row.get_ref(0)?)?),
                    ("type", Value::from("profile")),
                ]));
            }
        }
        let mut statement = conn.prepare(
            "SELECT DISTINCT stage_fts.profile_name, stage_fts.stage_name \nFROM stage_fts \nWHERE stage_fts.stage_name LIKE ?",
        )?;
        let mut rows = statement.query([&pattern])?;
        while let Some(row) = rows.next()? {
            result.push(pyfmt::object([
                ("profile", to_json(row.get_ref(0)?)?),
                ("type", Value::from("stage")),
                ("name", to_json(row.get_ref(1)?)?),
            ]));
        }
        Ok(result)
    }

    /// `ShotDataBase.statistics`:
    /// `{"totalSavedShots": n, "byProfile": [{"name", "count", "profileVersions"}]}`.
    pub fn statistics(&self) -> Result<Value> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT history.profile_name AS profile_name, count(history.profile_name) AS profile_count, count(DISTINCT history.profile_id) AS profile_versions \nFROM history GROUP BY history.profile_name ORDER BY count(?) DESC",
        )?;
        let mut rows = statement.query(["profile_count"])?;
        let mut total: i64 = 0;
        let mut by_profile = Vec::new();
        while let Some(row) = rows.next()? {
            let count: i64 = row.get(1)?;
            total += count;
            by_profile.push(pyfmt::object([
                ("name", to_json(row.get_ref(0)?)?),
                ("count", Value::from(count)),
                ("profileVersions", Value::from(row.get::<_, i64>(2)?)),
            ]));
        }
        Ok(pyfmt::object([
            ("totalSavedShots", Value::from(total)),
            ("byProfile", Value::Array(by_profile)),
        ]))
    }

    fn rate_shot_inner(&self, history_uuid: &str, rating: Option<&str>) -> Result<bool> {
        let mut conn = self.connection();
        let tx = conn.transaction()?;
        let shot: Option<i64> = tx
            .query_row(
                "SELECT history.id \nFROM history \nWHERE history.uuid = ?",
                [history_uuid],
                |row| row.get(0),
            )
            .optional()?;
        let Some(history_id) = shot else {
            tracing::error!("Shot with ID {history_uuid} does not exist");
            tx.commit()?;
            return Ok(false);
        };
        let annotation: Option<i64> = tx
            .query_row(
                "SELECT shot_annotation.id \nFROM shot_annotation \nWHERE shot_annotation.history_uuid = ?",
                [history_uuid],
                |row| row.get(0),
            )
            .optional()?;
        let annotation_id = match annotation {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO shot_annotation (history_id, history_uuid) VALUES (?, ?)",
                    rusqlite::params![history_id, history_uuid],
                )?;
                tx.last_insert_rowid()
            }
        };
        let existing: Option<i64> = tx
            .query_row(
                "SELECT shot_rating.id \nFROM shot_rating \nWHERE shot_rating.annotation_id = ?",
                [annotation_id],
                |row| row.get(0),
            )
            .optional()?;
        match (rating, existing) {
            (None, Some(_)) => {
                tx.execute(
                    "DELETE FROM shot_rating WHERE shot_rating.annotation_id = ?",
                    [annotation_id],
                )?;
            }
            (None, None) => {}
            (Some(rating), Some(_)) => {
                tx.execute(
                    "UPDATE shot_rating SET basic=? WHERE shot_rating.annotation_id = ?",
                    rusqlite::params![rating, annotation_id],
                )?;
            }
            (Some(rating), None) => {
                tx.execute(
                    "INSERT INTO shot_rating (annotation_id, basic) VALUES (?, ?)",
                    rusqlite::params![annotation_id, rating],
                )?;
            }
        }
        tx.commit()?;
        Ok(true)
    }

    /// `ShotDataBase.rate_shot`: `"like"`, `"dislike"` or `None` (clear) for
    /// the shot with this uuid. `false` for an invalid rating, an unknown
    /// shot or any database error.
    ///
    /// PARITY: clearing the rating of a shot without an annotation still
    /// creates its (empty) annotation row.
    pub fn rate_shot(&self, history_uuid: &str, rating: Option<&str>) -> bool {
        if !matches!(rating, None | Some("like") | Some("dislike")) {
            tracing::error!(
                "Invalid rating: {:?}. Must be 'like', 'dislike', or None.",
                rating
            );
            return false;
        }
        match self.rate_shot_inner(history_uuid, rating) {
            Ok(result) => result,
            Err(error) => {
                tracing::error!("Error rating shot: {error}");
                false
            }
        }
    }

    /// `ShotDataBase.get_shot_rating`: the rating of the shot with this uuid,
    /// `None` when unrated, unknown, or on a database error.
    pub fn get_shot_rating(&self, history_uuid: &str) -> Option<String> {
        let conn = self.connection();
        let result: rusqlite::Result<Option<Option<String>>> = conn
            .query_row(
                "SELECT shot_rating.basic \nFROM shot_annotation LEFT OUTER JOIN shot_rating ON shot_annotation.id = shot_rating.annotation_id \nWHERE shot_annotation.history_uuid = ?",
                [history_uuid],
                |row| row.get(0),
            )
            .optional();
        match result {
            Ok(rating) => rating.flatten(),
            Err(error) => {
                tracing::error!("Error getting shot rating: {error}");
                None
            }
        }
    }
}
