//! Python's handling of SQLite rows, where the stores lean on it (`dict(row)`, `json.loads` of a column), and
//! `ferb_py::value`'s handling of decoded JSON values under the names the stores use.

use rusqlite::types::Value as Sql;
use serde_json::{Map, Value};

pub use ferb_py::value::{
    char_len, get, is_py_space, merged, prefix, py_eq, py_str, squeeze, truthy,
};

use crate::{pyjson, Error, Result};

/// A column as `sqlite3.Row` gives it, as JSON (a blob has no JSON form and is refused).
pub fn sql_to_json(value: Sql) -> Result<Value> {
    Ok(match value {
        Sql::Null => Value::Null,
        Sql::Integer(n) => Value::from(n),
        Sql::Real(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .ok_or_else(|| Error::Json("a stored number is not finite".to_owned()))?,
        Sql::Text(t) => Value::String(t),
        Sql::Blob(_) => return Err(Error::Json("a blob column has no JSON form".to_owned())),
    })
}

/// `dict(row)`: every column by name, in the statement's order.
pub fn row_map(row: &rusqlite::Row<'_>) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    for (i, name) in row.as_ref().column_names().into_iter().enumerate() {
        out.insert(name.to_owned(), sql_to_json(row.get::<_, Sql>(i)?)?);
    }
    Ok(out)
}

/// Every row of a query as `dict(row)`.
pub fn query_maps(
    conn: &rusqlite::Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<Map<String, Value>>> {
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row_map(row)?);
    }
    Ok(out)
}

/// `json.loads(text)` of a stored column.
pub fn loads(text: &str) -> Result<Value> {
    pyjson::loads(text).map_err(Error::Json)
}

/// `?,?,?` for `count` parameters.
pub fn marks(count: usize) -> String {
    vec!["?"; count].join(",")
}
