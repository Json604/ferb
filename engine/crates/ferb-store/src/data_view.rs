//! What FERB.app's stored-data viewer reads (Settings → Stored data; owner STORE-422, agent STORE-423): every table of
//! FERB's database with its row count, rows newest first a page at a time, one row in full, a long cell in pieces, and
//! a search for a phrase across the tables. Read only: [`open_read_only`] opens its own connection that can't write
//! (`SQLITE_OPEN_READ_ONLY`, `query_only`), so a long read never holds the engine's own connection.
//!
//! A table name is taken only when it is one `sqlite_master` lists (never the client's text put into SQL), a column only
//! when the table has it; every value goes in as a parameter. Text is read cut at the size asked (`substr` in SQL, so a
//! 100 MB cell never reaches the app whole) with its full length beside it; a blob is only its size. A search stops at
//! its time budget and says so. The database holds what FERB stored, redacted at capture (rule 7): the viewer shows
//! that, never the Keychain or a sealed value.
//!
//! Tables are grouped for a person ([`GROUPS`]); a full-text index and its shadow tables, and SQLite's own, are
//! `internal` (listed, hidden by default). [`row_action`]'s expressions tell the app which rows it can delete as a turn
//! (the message, its plan, answers and tasks) or forget as a memory; everything else is shown, never deleted here.

use std::path::Path;
use std::time::Duration;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Map, Value};

use crate::{Error, Result};

/// A page of rows: its default and largest size.
pub const PAGE: usize = 50;
pub const PAGE_MAX: usize = 200;
/// The characters of each text cell a page shows: default and largest.
pub const CELL: usize = 160;
pub const CELL_MAX: usize = 2_000;
/// One row opened in full: each cell up to this many characters, the row's text up to [`ROW_CHARS`] in all.
pub const ROW_CELL: usize = 200_000;
pub const ROW_CHARS: usize = 600_000;
/// A page's text in all (the reply must stay well under the wire's 1 MiB message).
pub const PAGE_CHARS: usize = 500_000;
/// A piece of a long cell (`cell`): default and largest.
pub const PIECE: usize = 100_000;
pub const PIECE_MAX: usize = 200_000;
/// A search: the matches kept per table, the phrase's length, and the time it may take.
pub const SEARCH_PER_TABLE: usize = 20;
pub const NEEDLE_MAX: usize = 500;
pub const SEARCH_BUDGET: Duration = Duration::from_secs(8);
/// Characters of context shown around a match.
pub const SNIPPET: usize = 80;

/// The journal kinds that hold a turn's conversation (the planner's: ferb-supervisor `CONVERSATION_KINDS`,
/// `SELF_POSTS`, the plan line and the running summary). A turn's delete removes its rows of these kinds; every other
/// journal kind is the audit record and stays.
pub const CONTENT_KINDS: [&str; 10] = [
    "chat.message",
    "chat.reply",
    "chat.clarify",
    "task.answer",
    "chat.notice",
    "schedule.posted",
    "timer.delivered",
    "chat.part",
    "chat.plan",
    "chat.summary",
];

/// (id, title, tables) for a person, in the order the app lists them; a table in none is "Everything else".
pub const GROUPS: [(&str, &str, &[&str]); 6] = [
    (
        "conversation",
        "Conversation",
        &[
            "chat_history",
            "chat_entries",
            "conversations",
            "current_conversation",
            "chat_streams",
            "chat_submissions",
            "chat_clients",
            "chat_client_marks",
            "chat_entries_from",
            "attention_notices",
            "delegated_conversations",
            "conversation_turns",
            "conversation_events",
            "conversation_workers",
            "conversation_owner_changes",
        ],
    ),
    (
        "tasks",
        "Tasks and results",
        &[
            "tasks",
            "task_results",
            "task_result_cards",
            "task_result_names",
            "task_context",
            "task_plans",
            "plan_steps",
            "plan_events",
            "owner_verdicts",
            "evidence",
            "downloads",
            "shell_downloads",
            "loop_state",
        ],
    ),
    ("model_input", "What FERB sent for thinking", &["spans"]),
    (
        "memory",
        "Memory",
        &[
            "memory_items",
            "memory_sources",
            "memory_profile",
            "memory_context",
            "memory_set_aside",
            "memory_questions",
            "memory_forgotten",
            "memory_import_runs",
            "memory_import_done",
            "memory_import_items",
            "forgotten_refs",
            "episodes",
            "episode_how",
            "learning_nodes",
            "learning_sources",
            "learning_titles",
            "learning_card_inputs",
            "learning_changes",
            "learning_digests",
            "learning_search_version",
            "recall_vectors",
            "recall_state",
            "distill_runs",
        ],
    ),
    (
        "activity",
        "Activity log",
        &["journal", "trail", "trail_changes", "resource_samples"],
    ),
    (
        "safety",
        "Safety records",
        &[
            "action_ledger",
            "confirmations",
            "standing_permissions",
            "known_destinations",
            "recovery",
        ],
    ),
];

/// Why a table's rows aren't deleted here (STORE-424), shown with the table.
pub fn note(table: &str) -> &'static str {
    match group(table) {
        "safety" => {
            "Not deletable here: FERB's safety rules read these records (confirmations given, actions taken, standing \
             permissions, recovery copies). Uninstall → delete everything removes them."
        }
        "activity" if table == "journal" => {
            "The audit log is not deletable here. Deleting a turn removes that turn's conversation rows from it (the \
             message, replies, answers and the running summaries that folded them in); its other rows stay."
        }
        "activity" => "Not deletable here; FERB's retention settings age these rows out.",
        _ => match table {
            "chat_history" | "chat_entries" | "tasks" | "task_results" => {
                "A turn's rows can be deleted: its message, plan, answers, tasks, results and traces."
            }
            "memory_items" => "A memory can be forgotten (the Memory page's Forget).",
            "spans" => {
                "Traces: deleting a turn deletes its own; later turns' copies of its words are blanked where found."
            }
            _ => "",
        },
    }
}

/// The group a table is shown under.
pub fn group(table: &str) -> &'static str {
    GROUPS
        .iter()
        .find(|(_, _, tables)| tables.contains(&table))
        .map_or("other", |(id, _, _)| id)
}

/// A connection that can't write: the viewer's own, so a long read never holds the engine's.
pub fn open_read_only(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch("PRAGMA busy_timeout=5000; PRAGMA query_only=ON")?;
    Ok(conn)
}

fn refused<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Refused(message.into()))
}

/// `"name"` with its quotes doubled: an identifier SQLite reads as written.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// One table as `sqlite_master` lists it.
#[derive(Clone, Debug, PartialEq)]
struct Listed {
    name: String,
    virtual_table: bool,
    rowid: bool,
}

fn listed(conn: &Connection) -> Result<Vec<Listed>> {
    let mut stmt = conn.prepare(
        "SELECT name, COALESCE(sql, '') FROM sqlite_master WHERE type = 'table' ORDER BY name",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (name, sql) = row?;
        let upper = sql.to_ascii_uppercase();
        out.push(Listed {
            virtual_table: upper.starts_with("CREATE VIRTUAL TABLE"),
            rowid: !upper
                .replace(char::is_whitespace, "")
                .contains("WITHOUTROWID"),
            name,
        });
    }
    Ok(out)
}

/// The table `name` names, only when the database has it (the only way a client's text reaches SQL, quoted).
fn known(conn: &Connection, name: &str) -> Result<Listed> {
    match listed(conn)?.into_iter().find(|t| t.name == name) {
        Some(t) => Ok(t),
        None => refused(format!("FERB's database has no table named {name:?}")),
    }
}

/// Whether a table is SQLite's own or a full-text index's (the index and its shadow tables): listed, hidden by
/// default.
fn internal(all: &[Listed], table: &Listed) -> bool {
    table.name.starts_with("sqlite_")
        || table.virtual_table
        || all.iter().any(|v| {
            v.virtual_table
                && table
                    .name
                    .strip_prefix(&format!("{}_", v.name))
                    .is_some_and(|rest| {
                        ["data", "idx", "content", "docsize", "config"].contains(&rest)
                    })
        })
}

/// (name, declared type) of each column.
fn columns(conn: &Connection, table: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", quoted(table)))?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// `data.tables`: every table with its group, row count, columns, whether it is internal, what a row can have done to
/// it here (`turn`, `forget` or nothing) and why not.
pub fn tables(conn: &Connection) -> Result<Value> {
    let all = listed(conn)?;
    let mut out = Vec::new();
    for t in &all {
        let rows: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {}", quoted(&t.name)),
            [],
            |r| r.get(0),
        )?;
        let cols: Vec<String> = columns(conn, &t.name)?
            .into_iter()
            .map(|(c, _)| c)
            .collect();
        let action = row_action(&t.name).map(|(a, _)| a);
        out.push(json!({
            "name": t.name, "group": group(&t.name), "rows": rows, "columns": cols,
            "internal": internal(&all, t), "action": action, "note": note(&t.name),
        }));
    }
    let groups: Vec<Value> = GROUPS
        .iter()
        .map(|(id, title, _)| json!({"id": id, "title": title}))
        .chain(std::iter::once(
            json!({"id": "other", "title": "Everything else"}),
        ))
        .collect();
    Ok(json!({"groups": groups, "tables": out}))
}

/// What a row of `table` can have done to it here, and the SQL expression (over the row's own columns) that gives the
/// id it acts on (NULL: nothing for this row): `turn` deletes the turn its request id names (STORE-424), `forget`
/// forgets the memory (memory.forget).
pub fn row_action(table: &str) -> Option<(&'static str, String)> {
    let kinds = CONTENT_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    Some(match table {
        "chat_history" => (
            "turn",
            "CASE WHEN kind = 'turn' AND json_valid(body_json) THEN json_extract(body_json, '$.requestID') END".into(),
        ),
        "chat_entries" => ("turn", "CASE WHEN deleted = 0 THEN request_id END".into()),
        "tasks" => ("turn", "request_id".into()),
        "task_results" => (
            "turn",
            "(SELECT t.request_id FROM tasks t WHERE t.id = task_results.task_id)".into(),
        ),
        "journal" => (
            "turn",
            format!(
                "CASE WHEN kind IN ({kinds}) AND kind <> 'chat.summary' AND json_valid(data_json) \
                 THEN json_extract(data_json, '$.request_id') END"
            ),
        ),
        "memory_items" => ("forget", "id".into()),
        _ => return None,
    })
}

/// How rows are picked: all of them, or those whose `column` equals or starts with a text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub column: String,
    pub equals: Option<String>,
    pub starts_with: Option<String>,
}

/// What `rows` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct RowsQuery {
    pub table: String,
    /// The `next` of the page before (a rowid; an offset for a table without one).
    pub cursor: Option<i64>,
    pub limit: usize,
    pub cell: usize,
    pub filter: Option<Filter>,
}

/// One cell read cut: (value, its full length in characters when cut).
fn cell_value(kind: &str, length: i64, cut: ValueRef<'_>, max: usize) -> (Value, Option<i64>) {
    match kind {
        "null" => (Value::Null, None),
        "integer" => (
            match cut {
                ValueRef::Integer(n) => json!(n),
                _ => Value::Null,
            },
            None,
        ),
        "real" => (
            match cut {
                ValueRef::Real(f) => json!(f),
                _ => Value::Null,
            },
            None,
        ),
        "blob" => (json!({"bytes": length}), None),
        _ => {
            let text = match cut {
                ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                ValueRef::Integer(n) => n.to_string(),
                ValueRef::Real(f) => f.to_string(),
                _ => String::new(),
            };
            let over = length as usize > max;
            (Value::String(text), over.then_some(length))
        }
    }
}

/// Per column: `typeof`, `length` and the text cut at `max` characters (NULL for a blob: never read).
fn cut_columns(cols: &[(String, String)], max: usize) -> String {
    cols.iter()
        .map(|(c, _)| {
            let q = quoted(c);
            format!(
                "typeof({q}), length({q}), CASE WHEN typeof({q}) IN ('text', 'blob') THEN \
                 CASE WHEN typeof({q}) = 'text' THEN substr({q}, 1, {max}) END ELSE {q} END"
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The columns' values of one result row read by `cut_columns` from index `from`: the cells, the cut ones' lengths,
/// and the characters read.
fn read_cells(
    row: &rusqlite::Row<'_>,
    cols: &[(String, String)],
    from: usize,
    max: usize,
) -> rusqlite::Result<(Vec<Value>, Map<String, Value>, usize)> {
    let mut cells = Vec::with_capacity(cols.len());
    let mut cut = Map::new();
    let mut chars = 0;
    for (i, (name, _)) in cols.iter().enumerate() {
        let at = from + i * 3;
        let kind: String = row.get(at)?;
        let length: i64 = row.get::<_, Option<i64>>(at + 1)?.unwrap_or(0);
        let (value, full) = cell_value(&kind, length, row.get_ref(at + 2)?, max);
        if let Value::String(s) = &value {
            chars += s.chars().count();
        }
        if let Some(full) = full {
            cut.insert(name.clone(), json!(full));
        }
        cells.push(value);
    }
    Ok((cells, cut, chars))
}

fn check_filter(cols: &[(String, String)], filter: &Filter) -> Result<(String, String)> {
    if !cols.iter().any(|(c, _)| *c == filter.column) {
        return refused(format!(
            "that table has no column named {:?}",
            filter.column
        ));
    }
    let q = quoted(&filter.column);
    match (&filter.equals, &filter.starts_with) {
        (Some(v), None) => Ok((format!("{q} = ?"), v.clone())),
        (None, Some(v)) => Ok((
            format!("substr({q}, 1, {}) = ?", v.chars().count()),
            v.clone(),
        )),
        _ => refused("a filter takes one of equals or starts_with"),
    }
}

/// `data.rows`: a page of `table`, newest first (by rowid), each text cell cut at `cell` characters with its full
/// length in `cut`; `next` reads the page after it (absent at the end). A page also stops when its text passes
/// [`PAGE_CHARS`].
pub fn rows(conn: &Connection, q: &RowsQuery) -> Result<Value> {
    let t = known(conn, &q.table)?;
    if q.limit == 0 || q.limit > PAGE_MAX {
        return refused(format!("limit must be from 1 to {PAGE_MAX}"));
    }
    if q.cell == 0 || q.cell > CELL_MAX {
        return refused(format!("cell must be from 1 to {CELL_MAX}"));
    }
    let cols = columns(conn, &t.name)?;
    let mut wheres: Vec<String> = Vec::new();
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(f) = &q.filter {
        let (sql, arg) = check_filter(&cols, f)?;
        wheres.push(sql);
        args.push(arg.into());
    }
    let total: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM {}{}",
            quoted(&t.name),
            if wheres.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", wheres.join(" AND "))
            }
        ),
        rusqlite::params_from_iter(args.iter()),
        |r| r.get(0),
    )?;
    let action = row_action(&t.name);
    let action_sql = action
        .as_ref()
        .map_or("NULL".to_string(), |(_, sql)| sql.clone());
    let (order, key) = if t.rowid {
        if let Some(c) = q.cursor {
            wheres.push("rowid < ?".into());
            args.push(c.into());
        }
        ("ORDER BY rowid DESC LIMIT ?".to_string(), "rowid")
    } else {
        ("LIMIT ? OFFSET ?".to_string(), "NULL")
    };
    let sql = format!(
        "SELECT {key}, ({action_sql}), {} FROM {}{} {order}",
        cut_columns(&cols, q.cell),
        quoted(&t.name),
        if wheres.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", wheres.join(" AND "))
        },
    );
    args.push((q.limit as i64 + 1).into());
    let offset = q.cursor.unwrap_or(0).max(0);
    if !t.rowid {
        args.push(offset.into());
    }
    let mut stmt = conn.prepare(&sql)?;
    let mut found = stmt.query(rusqlite::params_from_iter(args.iter()))?;
    let mut out = Vec::new();
    let mut chars = 0usize;
    let mut more = false;
    let mut last_rowid: Option<i64> = None;
    while let Some(row) = found.next()? {
        if out.len() == q.limit || chars > PAGE_CHARS {
            more = true;
            break;
        }
        let rowid: Option<i64> = row.get(0)?;
        let act: Option<String> = match row.get_ref(1)? {
            ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Integer(n) => Some(n.to_string()),
            _ => None,
        };
        let (cells, cut, n) = read_cells(row, &cols, 2, q.cell)?;
        chars += n;
        let mut r = Map::new();
        r.insert("rowid".into(), json!(rowid));
        r.insert("cells".into(), Value::Array(cells));
        if !cut.is_empty() {
            r.insert("cut".into(), Value::Object(cut));
        }
        if let (Some((name, _)), Some(id)) = (&action, act.filter(|a| !a.is_empty())) {
            r.insert("action".into(), json!({"do": name, "id": id}));
        }
        last_rowid = rowid;
        out.push(Value::Object(r));
    }
    let mut page = Map::new();
    page.insert("table".into(), json!(t.name));
    page.insert(
        "columns".into(),
        json!(cols.iter().map(|(c, _)| c).collect::<Vec<_>>()),
    );
    page.insert("total".into(), json!(total));
    page.insert("note".into(), json!(note(&t.name)));
    if more {
        let next = if t.rowid {
            last_rowid
        } else {
            Some(offset + out.len() as i64)
        };
        page.insert("next".into(), json!(next));
    }
    page.insert("rows".into(), Value::Array(out));
    Ok(Value::Object(page))
}

/// `data.row`: one row in full, each text cell up to [`ROW_CELL`] characters and the row up to [`ROW_CHARS`] (a cell
/// over either comes back cut with its length in `cut`; `cell` reads the rest).
pub fn row(conn: &Connection, table: &str, rowid: i64) -> Result<Value> {
    let t = known(conn, table)?;
    if !t.rowid {
        return refused("this table has no row ids, so its rows can only be read in pages");
    }
    let cols = columns(conn, &t.name)?;
    // the lengths first, so the whole row's text stays within ROW_CHARS
    let lengths: Vec<i64> = {
        let sql = format!(
            "SELECT {} FROM {} WHERE rowid = ?",
            cols.iter()
                .map(|(c, _)| format!(
                    "CASE WHEN typeof({0}) = 'text' THEN length({0}) ELSE 0 END",
                    quoted(c)
                ))
                .collect::<Vec<_>>()
                .join(", "),
            quoted(&t.name)
        );
        let found = conn.query_row(&sql, [rowid], |r| {
            (0..cols.len())
                .map(|i| r.get::<_, i64>(i))
                .collect::<rusqlite::Result<Vec<i64>>>()
        });
        match found {
            Ok(l) => l,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return refused("that row is not there any more")
            }
            Err(e) => return Err(e.into()),
        }
    };
    let total: usize = lengths.iter().map(|l| *l as usize).sum();
    let each = if total <= ROW_CHARS {
        ROW_CELL
    } else {
        (ROW_CHARS / cols.len().max(1)).max(1)
    };
    let action = row_action(&t.name);
    let action_sql = action
        .as_ref()
        .map_or("NULL".to_string(), |(_, sql)| sql.clone());
    let sql = format!(
        "SELECT ({action_sql}), {} FROM {} WHERE rowid = ?",
        cut_columns(&cols, each.min(ROW_CELL)),
        quoted(&t.name)
    );
    conn.query_row(&sql, [rowid], |r| {
        let act: Option<String> = match r.get_ref(0)? {
            ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Integer(n) => Some(n.to_string()),
            _ => None,
        };
        let (cells, cut, _) = read_cells(r, &cols, 1, each.min(ROW_CELL))?;
        let mut out = Map::new();
        out.insert("table".into(), json!(t.name));
        out.insert("rowid".into(), json!(rowid));
        out.insert(
            "columns".into(),
            json!(cols.iter().map(|(c, _)| c).collect::<Vec<_>>()),
        );
        out.insert("cells".into(), Value::Array(cells));
        if !cut.is_empty() {
            out.insert("cut".into(), Value::Object(cut));
        }
        if let (Some((name, _)), Some(id)) = (&action, act.filter(|a| !a.is_empty())) {
            out.insert("action".into(), json!({"do": name, "id": id}));
        }
        out.insert("note".into(), json!(note(&t.name)));
        Ok(Value::Object(out))
    })
    .map_err(Error::from)
}

/// `data.cell`: `chars` characters of one text cell from `offset` (0-based), with its full length and whether this
/// piece reaches its end.
pub fn cell(
    conn: &Connection,
    table: &str,
    rowid: i64,
    column: &str,
    offset: usize,
    chars: usize,
) -> Result<Value> {
    let t = known(conn, table)?;
    if !t.rowid {
        return refused("this table has no row ids, so its rows can only be read in pages");
    }
    if chars == 0 || chars > PIECE_MAX {
        return refused(format!("chars must be from 1 to {PIECE_MAX}"));
    }
    let cols = columns(conn, &t.name)?;
    if !cols.iter().any(|(c, _)| c == column) {
        return refused(format!("that table has no column named {column:?}"));
    }
    let q = quoted(column);
    let sql = format!(
        "SELECT typeof({q}), length({q}), CASE WHEN typeof({q}) = 'text' THEN substr({q}, ?, ?) END FROM {} WHERE rowid = ?",
        quoted(&t.name)
    );
    let found = conn.query_row(
        &sql,
        rusqlite::params![offset as i64 + 1, chars as i64, rowid],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                r.get::<_, Option<String>>(2)?,
            ))
        },
    );
    let (kind, length, text) = match found {
        Ok(v) => v,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return refused("that row is not there any more")
        }
        Err(e) => return Err(e.into()),
    };
    if kind != "text" {
        return refused("only a text cell is read in pieces");
    }
    let text = text.unwrap_or_default();
    let end = offset + text.chars().count() >= length as usize;
    Ok(
        json!({"table": t.name, "rowid": rowid, "column": column, "offset": offset, "chars": length,
              "text": text, "end": end}),
    )
}

/// `pattern` for LIKE with `\` escaping `%`, `_` and itself.
fn like(text: &str) -> String {
    let mut out = String::from("%");
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// The phrase as FERB's JSON columns store it (Python's `json.dumps`: non-ASCII as `\uXXXX`, quotes and backslashes
/// escaped), without its quotes; None when that is the phrase itself.
fn as_stored_json(text: &str) -> Option<String> {
    let dumped = ferb_py::json::dumps(&Value::String(text.to_string()));
    let inner = dumped.get(1..dumped.len().saturating_sub(1))?.to_string();
    (inner != text).then_some(inner)
}

/// The tables a search reads first: the ones a person asks about, then the rest (a search stopped by its budget has
/// read the important ones).
fn search_order(all: &[Listed]) -> Vec<&Listed> {
    let mut order: Vec<&Listed> = Vec::new();
    for (_, _, tables) in GROUPS {
        for name in tables {
            if let Some(t) = all.iter().find(|t| t.name == *name) {
                order.push(t);
            }
        }
    }
    for t in all {
        if !order.iter().any(|o| o.name == t.name) && !internal(all, t) {
            order.push(t);
        }
    }
    order
}

/// `data.search`: rows whose text holds `needle` (ASCII letters in any case, as SQLite's LIKE; the JSON-escaped form
/// too, so words with accents or quotes are found inside FERB's JSON), newest first, at most [`SEARCH_PER_TABLE`] per
/// table; `tables` limits it (internal tables only when named). Each match names its table, row, column and a snippet.
/// It stops when `budget` is spent (`stopped`, with the tables it read).
pub fn search(
    conn: &Connection,
    needle: &str,
    tables: Option<&[String]>,
    budget: Duration,
) -> Result<Value> {
    let needle = needle.trim();
    if needle.is_empty() {
        return refused("say what to look for");
    }
    if needle.chars().count() > NEEDLE_MAX {
        return refused(format!("look for at most {NEEDLE_MAX} characters"));
    }
    let all = listed(conn)?;
    let chosen: Vec<&Listed> = match tables {
        Some(names) => {
            let mut out = Vec::new();
            for n in names {
                match all.iter().find(|t| t.name == *n) {
                    Some(t) => out.push(t),
                    None => return refused(format!("FERB's database has no table named {n:?}")),
                }
            }
            out
        }
        None => search_order(&all),
    };
    let patterns: Vec<String> = std::iter::once(needle.to_string())
        .chain(as_stored_json(needle))
        .map(|p| like(&p))
        .collect();
    let probe = needle.to_ascii_lowercase();
    let stored = as_stored_json(needle).map(|s| s.to_ascii_lowercase());
    // the time budget: a watchdog interrupts the read in progress
    let deadline = std::time::Instant::now() + budget;
    let handle = conn.get_interrupt_handle();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        if done_rx.recv_timeout(budget).is_err() {
            handle.interrupt();
            true
        } else {
            false
        }
    });
    let mut hits = Vec::new();
    let mut read = 0usize;
    let mut stopped = false;
    'tables: for t in chosen {
        if std::time::Instant::now() >= deadline {
            stopped = true;
            break;
        }
        let cols = match columns(conn, &t.name) {
            Ok(cols) => cols,
            Err(Error::Sqlite(rusqlite::Error::SqliteFailure(e, _)))
                if e.code == rusqlite::ErrorCode::OperationInterrupted =>
            {
                stopped = true;
                break;
            }
            Err(e) => {
                let _ = done_tx.send(());
                let _ = watchdog.join();
                return Err(e);
            }
        };
        let key = if t.rowid { "rowid" } else { "NULL" };
        let mut found_here = 0usize;
        for (c, declared) in &cols {
            if declared.eq_ignore_ascii_case("BLOB") || found_here >= SEARCH_PER_TABLE {
                continue;
            }
            if std::time::Instant::now() >= deadline {
                stopped = true;
                break 'tables;
            }
            let q = quoted(c);
            let matches = patterns
                .iter()
                .map(|_| format!("{q} LIKE ? ESCAPE '\\'"))
                .collect::<Vec<_>>()
                .join(" OR ");
            let order = if t.rowid { "ORDER BY rowid DESC " } else { "" };
            let sql = format!(
                "SELECT {key}, length({q}), substr({q}, max(1, max(instr(lower({q}), ?), instr(lower({q}), ?)) - {SNIPPET}), \
                 {}) FROM {} WHERE typeof({q}) = 'text' AND ({matches}) {order}LIMIT ?",
                needle.chars().count() + 2 * SNIPPET,
                quoted(&t.name)
            );
            let mut args: Vec<rusqlite::types::Value> = vec![
                probe.clone().into(),
                stored.clone().unwrap_or_else(|| probe.clone()).into(),
            ];
            args.extend(
                patterns
                    .iter()
                    .map(|p| rusqlite::types::Value::from(p.clone())),
            );
            args.push(((SEARCH_PER_TABLE - found_here) as i64).into());
            let result = (|| -> rusqlite::Result<Vec<(Option<i64>, i64, String)>> {
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
                    Ok((
                        r.get(0)?,
                        r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                        r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    ))
                })?;
                rows.collect()
            })();
            match result {
                Ok(rows) => {
                    for (rowid, length, snippet) in rows {
                        found_here += 1;
                        hits.push(json!({"table": t.name, "group": group(&t.name), "rowid": rowid, "column": c,
                                         "chars": length, "snippet": snippet}));
                    }
                }
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::OperationInterrupted =>
                {
                    stopped = true;
                    break 'tables;
                }
                Err(e) => {
                    let _ = done_tx.send(());
                    let _ = watchdog.join();
                    return Err(e.into());
                }
            }
        }
        read += 1;
    }
    let _ = done_tx.send(());
    let _ = watchdog.join();
    Ok(json!({"text": needle, "hits": hits, "tables_read": read, "stopped": stopped}))
}
