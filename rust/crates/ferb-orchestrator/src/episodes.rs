//! ferb/orchestrator/episodes.py: the permanent per-task record (L1, D21), composed by code from the task's trace when
//! it ends (`ferb/trace/compose.py`, the trace part's: a seam here), kept forever, rebuildable from spans (MP1).

use ferb_sanitize::origin;
use ferb_store::Db;
use serde_json::{json, Value};

use crate::sql::{self, text, Result};

pub const MADE_BY: &str = "code:episode/1";
pub const JSON_FIELDS: &[&str] = &[
    "conditions",
    "steps",
    "problems",
    "tools",
    "usage",
    "desktop",
];

/// episodes.py:16 `build`: the composed trace (`compose.compose({"task_id", "spans", "outcome"})`) with its maker.
pub fn build(composed: Value, now: &str) -> Value {
    let mut ep = composed.as_object().cloned().unwrap_or_default();
    ep.insert("made_by".into(), json!(MADE_BY));
    ep.insert("built_at".into(), json!(now));
    Value::Object(ep)
}

fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().map(crate::py::str_of).collect(),
        Value::String(s) => s.chars().map(String::from).collect(),
        _ => Vec::new(),
    }
}

/// episodes.py:34 `write`.
pub fn write(db: &Db, ep: &Value) -> Result<()> {
    let get = |k: &str| ep.get(k).cloned().unwrap_or(Value::Null);
    let dump = |k: &str| ferb_py::json::dumps_utf8(&get(k));
    let origins = strings(&get("origins"));
    let tool_origins = match ep.get("tool_origins") {
        Some(v) if ferb_py::value::truthy(v) => strings(v),
        _ => Vec::new(),
    };
    let verified = match get("verified") {
        Value::Null => rusqlite::types::Value::Null,
        v => rusqlite::types::Value::Integer(i64::from(ferb_py::value::truthy(&v))),
    };
    sql::exec(
        db.conn(),
        &format!(
            "INSERT OR REPLACE INTO episodes (task_id, request_id, request_text, goal, status, verified, started_at, \
             ended_at, seconds, answer, conditions_json, steps_json, problems_json, tools_json, origins, tool_origins, \
             external_read, usage_json, desktop_json, made_by, built_at) VALUES ({})",
            vec!["?"; 21].join(",")
        ),
        &[
            sql::to_sql(&get("task_id")),
            sql::to_sql(&get("request_id")),
            sql::to_sql(&get("request_text")),
            sql::to_sql(&get("goal")),
            sql::to_sql(&get("status")),
            verified,
            sql::to_sql(&get("started_at")),
            sql::to_sql(&get("ended_at")),
            sql::to_sql(&get("seconds")),
            sql::to_sql(&get("answer")),
            text(&dump("conditions")),
            text(&dump("steps")),
            text(&dump("problems")),
            text(&dump("tools")),
            text(&origin::join(origins.iter().map(String::as_str))),
            text(&origin::join(tool_origins.iter().map(String::as_str))),
            rusqlite::types::Value::Integer(i64::from(origins.iter().any(|o| origin::is_external(o)))),
            text(&dump("usage")),
            text(&dump("desktop")),
            sql::to_sql(&get("made_by")),
            sql::to_sql(&get("built_at")),
        ],
    )?;
    match ep.get("how").filter(|h| ferb_py::value::truthy(h)) {
        Some(how) => {
            sql::exec(
                db.conn(),
                "INSERT OR REPLACE INTO episode_how (task_id, how) VALUES (?, ?)",
                &[sql::to_sql(&get("task_id")), sql::to_sql(how)],
            )?;
        }
        None => {
            sql::exec(
                db.conn(),
                "DELETE FROM episode_how WHERE task_id = ?",
                &[sql::to_sql(&get("task_id"))],
            )?;
        }
    }
    Ok(())
}

/// episodes.py:21 `record`: build and write; never fails the caller. A database error is journaled
/// (`episode.failed`); `append` is the journal's.
pub fn record(
    db: &Db,
    compose: impl FnOnce() -> std::result::Result<Value, (String, String)>,
    now: &str,
    task_id: &str,
    append: impl FnOnce(&str, Value) -> std::result::Result<(), String>,
) {
    let result = compose().and_then(|c| {
        write(db, &build(c, now)).map_err(|e| ("OperationalError".to_string(), e.to_string()))
    });
    if let Err((kind, text_)) = result {
        let _ = append(
            "episode.failed",
            json!({"task_id": task_id, "error": format!("{kind}: {text_}")}),
        );
    }
}

/// episodes.py:52 `get`.
pub fn get(db: &Db, task_id: &str) -> Result<Option<Value>> {
    let Some(mut row) = sql::row(
        db.conn(),
        "SELECT * FROM episodes WHERE task_id = ?",
        &[text(task_id)],
    )?
    else {
        return Ok(None);
    };
    for key in JSON_FIELDS {
        let raw = row
            .shift_remove(&format!("{key}_json"))
            .unwrap_or(Value::Null);
        row.insert((*key).into(), sql::loads(raw.as_str().unwrap_or("null"))?);
    }
    let split =
        |v: &Value| -> Value { json!(origin::split(v.as_str()).into_iter().collect::<Vec<_>>()) };
    let origins = split(&row["origins"]);
    row.insert("origins".into(), origins);
    let tool_origins = split(&row["tool_origins"]);
    row.insert("tool_origins".into(), tool_origins);
    let external = row.get("external_read").is_some_and(ferb_py::value::truthy);
    row.insert("external_read".into(), json!(external));
    let verified = match row.get("verified") {
        Some(Value::Null) | None => Value::Null,
        Some(v) => json!(ferb_py::value::truthy(v)),
    };
    row.insert("verified".into(), verified);
    let how = sql::row(
        db.conn(),
        "SELECT how FROM episode_how WHERE task_id = ?",
        &[text(task_id)],
    )?
    .map(|r| r["how"].clone())
    .unwrap_or(json!(""));
    row.insert("how".into(), how);
    Ok(Some(Value::Object(row)))
}
