//! The store's rows as Python's `sqlite3.Row` reads them, and this crate's errors.

use rusqlite::types::Value as Sql;
use rusqlite::{params_from_iter, Connection};
use serde_json::{Map, Value};

/// What these modules raise where Python raises.
#[derive(Debug)]
pub enum Error {
    /// A database error (`sqlite3.Error`).
    Sql(rusqlite::Error),
    /// Python's `ValueError(text)`.
    Value(String),
    /// Python's `KeyError(key)`.
    Key(String),
    /// A stored text that isn't JSON (`json.JSONDecodeError`).
    Json(String),
    /// ledger.py `BadTransition`.
    BadTransition(String),
    /// ledger.py `DuplicateUnresolved`: (message, intent id, state).
    Duplicate {
        message: String,
        intent_id: String,
        state: String,
    },
    /// The store crate's own error.
    Store(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Sql(e) => write!(f, "{e}"),
            Error::Value(m) | Error::Json(m) | Error::BadTransition(m) | Error::Store(m) => {
                f.write_str(m)
            }
            Error::Key(k) => write!(f, "{}", crate::py::repr_str(k)),
            Error::Duplicate { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Error {
        Error::Sql(e)
    }
}

impl From<ferb_store::Error> for Error {
    fn from(e: ferb_store::Error) -> Error {
        Error::Store(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A column's value as JSON (`None`, int, float, str).
pub fn to_json(value: Sql) -> Value {
    match value {
        Sql::Null => Value::Null,
        Sql::Integer(i) => Value::from(i),
        Sql::Real(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
        Sql::Text(t) => Value::String(t),
        Sql::Blob(b) => Value::Array(b.into_iter().map(Value::from).collect()),
    }
}

/// A JSON value as a parameter.
pub fn to_sql(value: &Value) -> Sql {
    match value {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Sql::Integer(i),
            None => Sql::Real(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => Sql::Text(s.clone()),
        other => Sql::Text(ferb_py::json::dumps(other)),
    }
}

/// Every row of a query, each as `dict(row)`.
pub fn rows(conn: &Connection, sql: &str, params: &[Sql]) -> Result<Vec<Map<String, Value>>> {
    let mut statement = conn.prepare(sql)?;
    let names: Vec<String> = statement.column_names().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    let mut found = statement.query(params_from_iter(params.iter()))?;
    while let Some(row) = found.next()? {
        let mut map = Map::new();
        for (i, name) in names.iter().enumerate() {
            map.insert(name.clone(), to_json(row.get::<_, Sql>(i)?));
        }
        out.push(map);
    }
    Ok(out)
}

/// The first row, if any.
pub fn row(conn: &Connection, sql: &str, params: &[Sql]) -> Result<Option<Map<String, Value>>> {
    Ok(rows(conn, sql, params)?.into_iter().next())
}

/// Run a statement; how many rows it changed.
pub fn exec(conn: &Connection, sql: &str, params: &[Sql]) -> Result<usize> {
    Ok(conn.execute(sql, params_from_iter(params.iter()))?)
}

pub fn text(s: &str) -> Sql {
    Sql::Text(s.to_string())
}

pub fn opt(s: Option<&str>) -> Sql {
    s.map_or(Sql::Null, text)
}

/// `json.loads` of a stored text (Python's values: an int stays an int).
pub fn loads(text: &str) -> Result<Value> {
    ferb_py::json::loads(text).map_err(Error::Json)
}

/// A row's text column (`None` and non-text as "").
pub fn col(row: &Map<String, Value>, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => ferb_py::value::py_str(other),
    }
}

/// Run `block` in one write transaction (`BEGIN IMMEDIATE` ... `COMMIT`, rolled back on an error), as Python's
/// explicit `BEGIN IMMEDIATE` blocks and `Database.transaction()` do.
pub fn transaction<T>(
    conn: &Connection,
    block: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    if !conn.is_autocommit() {
        return block(conn);
    }
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match block(conn) {
        Ok(v) => {
            conn.execute_batch("COMMIT")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
