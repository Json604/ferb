//! The planner's question that waits on the owner (a `clarify`), as a phone's `chat.status` lists it beside the
//! gateway's questions (kb PHN-760..769). FERB.app's notch comes down for a clarify the way it does for a question
//! (DaemonConnection.swift: a `clarify` reply asks with needs-you); a phone shows the same thing as a notification, so
//! it must know that one waits even while it is locked and reads no chat.
//!
//! One rule, read from the conversation the Mac keeps (ENG-8's entries, the one every client draws): in the current
//! conversation, the newest answer that asks the owner something (`asks`: a clarify, a turn's own answer or a later
//! one) waits until the owner says anything after it, from any client (a `message` entry placed after it). A newer
//! question takes its place; a deleted one, or one in a conversation that is no longer the current one, waits no more.
//! Its id is `clarify:<the request it answered>`: a new question has a new id, the same one keeps its id.

use ferb_store::Db;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

/// The id prefix of a planner's question in a phone's status (never a gateway approval id).
pub const PREFIX: &str = "clarify:";

/// The planner's question that waits now, as one more entry of the status's `approvals`: `kind` "clarify", no options,
/// its words as `reason` (the phone gate holds them for a locked session as it does any question's, PHN-126).
pub fn waiting(db: &Db) -> Option<Value> {
    let conn = db.conn();
    // the conversation a phone's chat shows (ferb-supervisor chat_log.rs `current`): the one FERB.app attached, else
    // the one the owner last wrote in, else `owner`
    let one = |sql: &str| -> Option<String> {
        conn.query_row(sql, [], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    };
    let conversation = one("SELECT conversation_id FROM current_conversation WHERE singleton = 1")
        .or_else(|| {
            one("SELECT conversation_id FROM chat_entries WHERE kind = 'message' ORDER BY created_at DESC, pos DESC LIMIT 1")
        })
        .unwrap_or_else(|| "owner".to_owned());
    // (the open partial index: an answer that asks stays open, ferb-supervisor flow/entries.rs `category`)
    let (pos, request, body): (i64, Option<String>, String) = conn
        .query_row(
            "SELECT pos, request_id, body_json FROM chat_entries
             WHERE conversation_id = ?1 AND open = 1 AND kind = 'answer' AND deleted = 0
               AND json_extract(body_json, '$.asks') = 1
             ORDER BY pos DESC LIMIT 1",
            [&conversation],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .ok()
        .flatten()?;
    let said: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM chat_entries WHERE conversation_id = ?1 AND kind = 'message' AND deleted = 0 AND pos > ?2
             LIMIT 1",
            rusqlite::params![conversation, pos],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    if said.is_some() {
        return None;
    }
    let body: Value = serde_json::from_str(&body).ok()?;
    let words = ["display_text", "text"]
        .iter()
        .filter_map(|k| body.get(*k).and_then(Value::as_str))
        .find(|s| !s.trim().is_empty())?
        .to_owned();
    let request = request.filter(|r| !r.is_empty())?;
    Some(json!({
        "approval_id": format!("{PREFIX}{request}"),
        "kind": "clarify",
        "reason": words,
        "options": [],
        "task_id": Value::Null,
        "request_id": request,
    }))
}
