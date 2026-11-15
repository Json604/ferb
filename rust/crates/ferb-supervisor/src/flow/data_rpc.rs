//! FERB.app's stored-data viewer and the owner's delete (owner STORE-422; agent STORE-423, STORE-424): `data.tables`,
//! `data.rows`, `data.row`, `data.cell`, `data.search` (ferb_store::data_view, on a read-only connection of its own,
//! off the async threads), `data.context` (the conversation the planner would read now) and `data.delete` (a turn,
//! named by its request id or one of its tasks; data_delete.rs). All are FERB.app's own (`app_only` in auth.rs): no
//! model tool, the phone or a command line reaches them.

use std::sync::Arc;

use ferb_rpc::server::{HandlerError, HandlerResult};
use ferb_rpc::wire::Params;
use ferb_store::data_view::{self as dv, Filter, RowsQuery};
use serde_json::{json, Map, Value};

use super::Inner;

/// Every method here, as auth.rs lists them.
pub const METHODS: [&str; 7] = [
    "data.tables",
    "data.rows",
    "data.row",
    "data.cell",
    "data.search",
    "data.context",
    "data.delete",
];

fn refusal(message: impl Into<String>) -> HandlerError {
    HandlerError::refusal("FerbError", message)
}

fn fault(e: ferb_store::Error) -> HandlerError {
    match e {
        ferb_store::Error::Value(m) | ferb_store::Error::Refused(m) => refusal(m),
        other => HandlerError::fault("OperationalError", other.to_string()),
    }
}

fn obj(v: Value) -> Params {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn text(params: &Params, key: &str) -> Result<Option<String>, HandlerError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
        Some(_) => Err(refusal(format!("{key} must be a text"))),
    }
}

fn required(params: &Params, key: &str) -> Result<String, HandlerError> {
    text(params, key)?.ok_or_else(|| refusal(format!("say which {key}")))
}

/// A whole number (a bool is not one), or `default` when absent.
fn number(params: &Params, key: &str, default: i64) -> Result<i64, HandlerError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Number(n)) if n.as_i64().is_some() => Ok(n.as_i64().unwrap_or(default)),
        Some(_) => Err(refusal(format!("{key} must be a whole number"))),
    }
}

fn size(params: &Params, key: &str, default: usize) -> Result<usize, HandlerError> {
    let n = number(params, key, default as i64)?;
    usize::try_from(n).map_err(|_| refusal(format!("{key} must be a whole number of at least 0")))
}

/// A read on the viewer's own read-only connection, on a blocking thread.
async fn read(
    inner: &Arc<Inner>,
    f: impl FnOnce(&rusqlite::Connection) -> ferb_store::Result<Value> + Send + 'static,
) -> HandlerResult {
    let path = inner.db().path().to_path_buf();
    let out = tokio::task::spawn_blocking(move || {
        let conn = dv::open_read_only(&path)?;
        f(&conn)
    })
    .await
    .map_err(|e| HandlerError::fault("RuntimeError", e.to_string()))?
    .map_err(fault)?;
    Ok(obj(out))
}

/// One `data.*` call.
pub async fn call(inner: &Arc<Inner>, method: &str, params: &Params) -> HandlerResult {
    match method {
        "data.tables" => read(inner, dv::tables).await,
        "data.rows" => {
            let filter = match params.get("filter") {
                None | Some(Value::Null) => None,
                Some(Value::Object(f)) => Some(Filter {
                    column: required(f, "column")?,
                    equals: text(f, "equals")?,
                    starts_with: text(f, "starts_with")?,
                }),
                Some(_) => return Err(refusal("filter must be an object")),
            };
            let q = RowsQuery {
                table: required(params, "table")?,
                cursor: match params.get("cursor") {
                    None | Some(Value::Null) => None,
                    _ => Some(number(params, "cursor", 0)?),
                },
                limit: size(params, "limit", dv::PAGE)?,
                cell: size(params, "cell", dv::CELL)?,
                filter,
            };
            read(inner, move |c| dv::rows(c, &q)).await
        }
        "data.row" => {
            let table = required(params, "table")?;
            let rowid = number(params, "rowid", -1)?;
            read(inner, move |c| dv::row(c, &table, rowid)).await
        }
        "data.cell" => {
            let table = required(params, "table")?;
            let column = required(params, "column")?;
            let rowid = number(params, "rowid", -1)?;
            let offset = size(params, "offset", 0)?;
            let chars = size(params, "chars", dv::PIECE)?;
            read(inner, move |c| {
                dv::cell(c, &table, rowid, &column, offset, chars)
            })
            .await
        }
        "data.search" => {
            let needle = required(params, "text")?;
            let tables: Option<Vec<String>> = match params.get("tables") {
                None | Some(Value::Null) => None,
                Some(Value::Array(a)) => Some(
                    a.iter()
                        .map(|t| t.as_str().map(str::to_string))
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| refusal("tables must be a list of table names"))?,
                ),
                Some(_) => return Err(refusal("tables must be a list of table names")),
            };
            read(inner, move |c| {
                dv::search(c, &needle, tables.as_deref(), dv::SEARCH_BUDGET)
            })
            .await
        }
        "data.context" => {
            let conversation = match text(params, "conversation")? {
                Some(c) => c,
                None => inner
                    .continuity(|c| c.current())
                    .map_err(|e| refusal(e.to_string()))?,
            };
            let blocks = inner.conversation_now(&conversation).map_err(refusal)?;
            Ok(obj(json!({"conversation": conversation, "blocks": blocks})))
        }
        "data.delete" => {
            let confirm = matches!(params.get("confirm"), Some(Value::Bool(true)));
            let request = match (text(params, "request_id")?, text(params, "task_id")?) {
                (Some(r), None) => r,
                (None, Some(task)) => {
                    let found = {
                        let db = inner.db();
                        super::data_delete::request_of(db.conn(), &task)
                            .map_err(|e| HandlerError::fault("OperationalError", e.to_string()))?
                    };
                    found.ok_or_else(|| refusal(super::data_delete::NOTHING_STORED))?
                }
                _ => return Err(refusal("say which message: its request_id, or one task_id")),
            };
            let inner = inner.clone();
            let out = tokio::task::spawn_blocking(move || inner.delete_turn(&request, confirm))
                .await
                .map_err(|e| HandlerError::fault("RuntimeError", e.to_string()))?
                .map_err(refusal)?;
            Ok(obj(out))
        }
        _ => Err(refusal("no such data method")),
    }
}
