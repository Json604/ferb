//! The owner's delete of a turn from FERB.app (owner STORE-422: "delete in db by user if they want"; agent STORE-424).
//! A turn is one message the owner sent (its request id) with everything that came from it: the message, the plan
//! line, FERB's replies, questions and answers, its tasks with their results and traces. A task is deleted as the turn
//! that started it, so the chat, the tasks and what the planner reads always agree.
//!
//! What goes, in one write transaction (the chat log's, so the entries' seq and the watchers stay right):
//! - the journal's conversation rows of the turn (ferb_store::data_view::CONTENT_KINDS naming its request or one of
//!   its tasks): the planner's conversation reads only these, so the turn stops reaching it. Each running summary of
//!   its chat from the first one that folded one of them in goes too (a summary quotes what it folded; the entries
//!   after the deleted ones come back unfolded and are folded again). The journal's delete trigger takes their
//!   chat_fts copies and recall vectors; `forgotten_refs` gets `journal:<id>` (the id only) so learning reads them as
//!   forgotten, not aged out;
//! - its chat entries become tombstones (ENG-12: body emptied, a new seq, the row kept) so every device drops them;
//! - FERB.app's rows of it in chat_history (the turn and its late replies);
//! - its tasks: tasks, task_results, result cards and names, task_context, plans and steps, and every span of them
//!   and of its planner calls (with their children).
//!
//! Then, in a transaction of its own, the `context` of a later model call that quoted one of its texts word for word is
//! replaced (the attention forget's words). Kept, and said so: the audit rows of the journal that name it (rule 6),
//! the safety records (action ledger, confirmations, standing permissions, recovery copies), screenshots (they age out),
//! learned memories built from it (the Memory page forgets them). Refused, with the reason: a turn FERB is still
//! answering or whose task still runs.

use std::collections::{BTreeMap, BTreeSet};

use std::path::{Component, Path, PathBuf};

use ferb_core::chat_entry::{Entry, EntryKind, EntryState, NoticeBody};
use rusqlite::{params_from_iter, Connection};
use serde_json::{json, Map, Value};

use ferb_store::data_view::CONTENT_KINDS;

/// A text quoted in later calls is looked for only when it is at least this long and has a space (ids and short words
/// like "ok" never blank a trace).
pub const QUOTED_MIN: usize = 24;
/// At most this many of the turn's texts are looked for.
pub const QUOTED_MAX: usize = 20;

/// What a turn's delete removes, read before anything is changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub request_id: String,
    pub tasks: Vec<String>,
    pub trees: Vec<String>,
    /// (journal id, kind) of its conversation rows.
    pub journal: Vec<(String, String)>,
    /// The running summaries that go with them.
    pub summaries: Vec<String>,
    /// Its chat entries not yet deleted.
    pub entries: Vec<String>,
    /// FERB.app's chat_history rows of it.
    pub history: Vec<String>,
    pub spans: usize,
    /// Its texts later calls may quote.
    pub texts: Vec<String>,
    /// (id, words) of the memories learned only from it (STORE-426): forgotten with it.
    pub memories: Vec<(String, String)>,
    /// (id, words) of the memories it is one source of, among others: kept.
    pub memories_kept: Vec<(String, String)>,
    /// (id, title) of the learned cards built only from it: forgotten with it (and what was derived from them).
    pub learned: Vec<(String, String)>,
    /// (id, title) of the learned cards it is one source of, among others: kept.
    pub learned_kept: Vec<(String, String)>,
    /// Its tasks' screenshot and evidence records, and their episode captures.
    pub evidence: usize,
    pub episodes: usize,
    /// What stays, by what it is: counts only.
    pub kept: Map<String, Value>,
}

/// What `apply` did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Done {
    pub counts: Map<String, Value>,
    pub entries: Vec<Entry>,
}

fn marks(n: usize) -> String {
    vec!["?"; n].join(",")
}

fn strings(conn: &Connection, sql: &str, args: &[String]) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| r.get::<_, String>(0))?;
    rows.collect()
}

fn count(conn: &Connection, sql: &str, args: &[String]) -> rusqlite::Result<i64> {
    conn.query_row(sql, params_from_iter(args.iter()), |r| r.get(0))
}

/// The request id of the turn that started `task_id`, when FERB has that task.
pub fn request_of(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<String>> {
    match conn.query_row(
        "SELECT request_id FROM tasks WHERE id = ?",
        [task_id],
        |r| r.get(0),
    ) {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Every text of a stored value at least [`QUOTED_MIN`] characters long with a space in it.
fn texts_of(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) if s.chars().count() >= QUOTED_MIN && s.contains(' ') => {
            out.push(s.clone())
        }
        Value::Array(a) => a.iter().for_each(|x| texts_of(x, out)),
        Value::Object(m) => m.values().for_each(|x| texts_of(x, out)),
        _ => {}
    }
}

/// What deleting the turn of `request_id` would remove and keep. `Ok(None)`: nothing is stored for it.
pub fn plan(conn: &Connection, request_id: &str) -> rusqlite::Result<Option<Plan>> {
    let r = request_id.to_string();
    let mut p = Plan {
        request_id: r.clone(),
        ..Plan::default()
    };
    let found: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, tree_id FROM tasks WHERE request_id = ?1 \
             OR tree_id IN (SELECT tree_id FROM tasks WHERE request_id = ?1) ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([&r], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (task, tree) in found {
        if !p.trees.contains(&tree) {
            p.trees.push(tree);
        }
        p.tasks.push(task);
    }
    let mut args = vec![r.clone()];
    args.extend(p.tasks.iter().cloned());
    let task_marks = marks(p.tasks.len());
    let names_it = if p.tasks.is_empty() {
        "json_extract(data_json, '$.request_id') = ?".to_string()
    } else {
        format!(
            "(json_extract(data_json, '$.request_id') = ? OR json_extract(data_json, '$.task_id') IN ({task_marks}))"
        )
    };
    let conversation_kinds: Vec<&str> = CONTENT_KINDS
        .iter()
        .copied()
        .filter(|k| *k != "chat.summary")
        .collect();
    let kind_marks = marks(conversation_kinds.len());
    let mut sessions: BTreeSet<String> = BTreeSet::new();
    {
        let mut all: Vec<String> = conversation_kinds.iter().map(|k| k.to_string()).collect();
        all.extend(args.iter().cloned());
        let mut stmt = conn.prepare(&format!(
            "SELECT id, kind, data_json FROM journal WHERE kind IN ({kind_marks}) AND json_valid(data_json) \
             AND {names_it} ORDER BY seq"
        ))?;
        let rows = stmt.query_map(params_from_iter(all.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, kind, data) = row?;
            let data: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
            sessions.insert(
                data.get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or("owner")
                    .to_string(),
            );
            if p.texts.len() < QUOTED_MAX {
                let mut t = Vec::new();
                texts_of(&data, &mut t);
                for text in t {
                    if !p.texts.contains(&text) && p.texts.len() < QUOTED_MAX {
                        p.texts.push(text);
                    }
                }
            }
            p.journal.push((id, kind));
        }
    }
    // the running summaries of those chats, from the first that folded in one of the turn's rows
    let handles: BTreeSet<String> = p
        .journal
        .iter()
        .map(|(id, _)| format!("{}{id}", crate::words::ENTRY))
        .collect();
    for session in &sessions {
        let mut stmt = conn.prepare(
            "SELECT id, data_json FROM journal WHERE kind = 'chat.summary' AND json_valid(data_json) \
             AND json_extract(data_json, '$.session_id') = ? ORDER BY seq",
        )?;
        let rows = stmt.query_map([session], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut from_here = false;
        for row in rows {
            let (id, data) = row?;
            let data: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
            let covers = data
                .get("covered")
                .and_then(Value::as_array)
                .is_some_and(|c| {
                    c.iter()
                        .any(|h| h.as_str().is_some_and(|h| handles.contains(h)))
                });
            from_here |= covers;
            if from_here {
                p.summaries.push(id);
            }
        }
    }
    p.entries = if p.tasks.is_empty() {
        strings(
            conn,
            "SELECT id FROM chat_entries WHERE deleted = 0 AND request_id = ? ORDER BY pos",
            &args,
        )?
    } else {
        strings(
            conn,
            &format!(
                "SELECT id FROM chat_entries WHERE deleted = 0 AND (request_id = ? OR task_id IN ({task_marks})) \
                 ORDER BY pos"
            ),
            &args,
        )?
    };
    let turns = strings(
        conn,
        "SELECT id FROM chat_history WHERE kind = 'turn' AND json_valid(body_json) \
         AND json_extract(body_json, '$.requestID') = ?",
        std::slice::from_ref(&r),
    )?;
    p.history = turns.clone();
    if !turns.is_empty() {
        p.history.extend(strings(
            conn,
            &format!(
                "SELECT id FROM chat_history WHERE kind = 'reply' AND json_valid(body_json) \
                 AND json_extract(body_json, '$.turnID') IN ({})",
                marks(turns.len())
            ),
            &turns,
        )?);
    }
    p.spans = count(
        conn,
        &format!(
            "{} SELECT COUNT(*) FROM doomed",
            doomed_spans(p.tasks.len())
        ),
        &args,
    )? as usize;
    if p.tasks.is_empty()
        && p.journal.is_empty()
        && p.entries.is_empty()
        && p.history.is_empty()
        && p.spans == 0
    {
        return Ok(None);
    }
    learned_from(conn, &mut p)?;
    if !p.tasks.is_empty() {
        let m = marks(p.tasks.len());
        p.evidence = count(
            conn,
            &format!("SELECT COUNT(*) FROM evidence WHERE task_id IN ({m})"),
            &p.tasks,
        )? as usize;
        p.episodes = count(
            conn,
            &format!("SELECT COUNT(*) FROM episodes WHERE task_id IN ({m})"),
            &p.tasks,
        )? as usize;
    }
    p.kept = kept(conn, &p, &args)?;
    Ok(Some(p))
}

/// The recursive set `doomed` of the turn's spans: its tasks', its planner calls' (no task, its request id), and every
/// child of those. Its arguments: the request id, then the task ids.
fn doomed_spans(tasks: usize) -> String {
    let of_tasks = if tasks == 0 {
        String::new()
    } else {
        format!("OR task_id IN ({})", marks(tasks))
    };
    format!(
        "WITH RECURSIVE doomed(id) AS (SELECT id FROM spans WHERE (task_id IS NULL AND instr(input_json, ?1) > 0 \
         AND json_valid(input_json) AND json_extract(input_json, '$.request_id') = ?1) {of_tasks} \
         UNION SELECT s.id FROM spans s JOIN doomed d ON s.parent_id = d.id)"
    )
}

/// What stays (counts only): the audit rows of the journal that name it, the safety records, its screenshots, and
/// what FERB learned from it.
fn kept(conn: &Connection, p: &Plan, args: &[String]) -> rusqlite::Result<Map<String, Value>> {
    let mut out = Map::new();
    let task_marks = marks(p.tasks.len());
    let kinds = CONTENT_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    let names_it = if p.tasks.is_empty() {
        "json_extract(data_json, '$.request_id') = ?".to_string()
    } else {
        format!(
            "(json_extract(data_json, '$.request_id') = ? OR json_extract(data_json, '$.task_id') IN ({task_marks}))"
        )
    };
    let audit = count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM journal WHERE kind NOT IN ({kinds}) AND json_valid(data_json) AND {names_it}"
        ),
        args,
    )?;
    out.insert("audit_log".into(), json!(audit));
    if !p.tasks.is_empty() {
        let tasks: Vec<String> = p.tasks.clone();
        let trees: Vec<String> = p.trees.clone();
        for (name, sql, a) in [
            (
                "action_ledger",
                format!("SELECT COUNT(*) FROM action_ledger WHERE task_id IN ({task_marks})"),
                &tasks,
            ),
            (
                "recovery",
                format!("SELECT COUNT(*) FROM recovery WHERE task_id IN ({task_marks})"),
                &tasks,
            ),
            (
                "standing_permissions",
                format!(
                    "SELECT COUNT(*) FROM standing_permissions WHERE task_id IN ({task_marks})"
                ),
                &tasks,
            ),
            (
                "confirmations",
                format!(
                    "SELECT COUNT(*) FROM confirmations WHERE tree_id IN ({})",
                    marks(trees.len())
                ),
                &trees,
            ),
        ] {
            out.insert(name.into(), json!(count(conn, &sql, a)?));
        }
    }
    out.insert("memories_shared".into(), json!(p.memories_kept.len()));
    out.insert("learned_shared".into(), json!(p.learned_kept.len()));
    Ok(out)
}

/// The plan as the app shows it before the owner's Yes: what goes and what stays, counts only.
pub fn describe(p: &Plan) -> Value {
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, k) in &p.journal {
        *kinds.entry(k.as_str()).or_default() += 1;
    }
    json!({
        "request_id": p.request_id,
        "will_delete": {
            "tasks": p.tasks.len(), "conversation_rows": p.journal.len(), "conversation_kinds": kinds,
            "summaries": p.summaries.len(), "chat_entries": p.entries.len(), "chat_history": p.history.len(),
            "traces": p.spans, "screenshots": p.evidence, "episodes": p.episodes,
            "memories": pairs(&p.memories), "learned": pairs(&p.learned),
        },
        "keeps": p.kept,
        "keeps_memories": pairs(&p.memories_kept),
        "keeps_learned": pairs(&p.learned_kept),
    })
}

/// Delete what `p` names, inside the caller's write transaction; `at` is now (ISO 8601). The tombstoned entries come
/// back for the chat log's watchers.
pub fn apply(
    conn: &Connection,
    p: &Plan,
    at: &str,
    evidence_root: Option<&Path>,
) -> ferb_store::Result<Done> {
    let mut counts = Map::new();
    // the conversation rows and the summaries that folded them in (the delete trigger takes chat_fts and the vectors)
    let mut doomed: Vec<String> = p.journal.iter().map(|(id, _)| id.clone()).collect();
    doomed.extend(p.summaries.iter().cloned());
    let mut journal = 0;
    for id in &doomed {
        journal += conn.execute("DELETE FROM journal WHERE id = ?", [id])?;
        conn.execute(
            "INSERT OR IGNORE INTO forgotten_refs VALUES (?, ?)",
            [format!("journal:{id}"), at.to_string()],
        )?;
    }
    counts.insert("conversation_rows".into(), json!(journal));
    counts.insert("summaries".into(), json!(p.summaries.len()));
    let mut entries = Vec::new();
    for id in &p.entries {
        if let Some(e) = ferb_store::chat_entries::tombstone(conn, id, at)? {
            entries.push(e);
        }
    }
    counts.insert("chat_entries".into(), json!(entries.len()));
    // a later message that quoted one of them shows no copy of its words: its entry's `reply_to` goes (the message
    // stays). Its chat.message row is append-only and keeps it; the conversation line shows no quote whose line is
    // gone (history::timeline)
    let quoting = unquote(conn, &p.entries, at)?;
    counts.insert("quotes_removed".into(), json!(quoting.len()));
    entries.extend(quoting);
    let mut history = 0;
    for id in &p.history {
        history += conn.execute("DELETE FROM chat_history WHERE id = ?", [id])?;
    }
    counts.insert("chat_history".into(), json!(history));
    let mut args = vec![p.request_id.clone()];
    args.extend(p.tasks.iter().cloned());
    let spans = conn.execute(
        &format!(
            "{} DELETE FROM spans WHERE id IN (SELECT id FROM doomed)",
            doomed_spans(p.tasks.len())
        ),
        params_from_iter(args.iter()),
    )?;
    counts.insert("traces".into(), json!(spans));
    if !p.tasks.is_empty() {
        let m = marks(p.tasks.len());
        let tasks = params_from_iter(p.tasks.iter());
        for sql in [
            format!("DELETE FROM task_result_cards WHERE task_id IN ({m})"),
            format!("DELETE FROM task_result_names WHERE task_id IN ({m})"),
            format!("DELETE FROM task_results WHERE task_id IN ({m})"),
            format!("DELETE FROM plan_steps WHERE task_id IN ({m})"),
            format!("DELETE FROM task_plans WHERE task_id IN ({m})"),
            format!("UPDATE task_context SET prior_task = NULL WHERE prior_task IN ({m})"),
            format!("DELETE FROM task_context WHERE task_id IN ({m})"),
        ] {
            conn.execute(&sql, params_from_iter(p.tasks.iter()))?;
        }
        let gone = conn.execute(&format!("DELETE FROM tasks WHERE id IN ({m})"), tasks)?;
        counts.insert("tasks".into(), json!(gone));
        // the episode captures (their how-notes go by the foreign key)
        let episodes = conn.execute(
            &format!("DELETE FROM episodes WHERE task_id IN ({m})"),
            params_from_iter(p.tasks.iter()),
        )?;
        counts.insert("episodes".into(), json!(episodes));
        let (shots, kept) = delete_evidence(conn, &p.tasks, evidence_root)?;
        counts.insert("screenshots".into(), json!(shots));
        counts.insert("screenshots_kept".into(), json!(kept));
    } else {
        counts.insert("tasks".into(), json!(0));
    }
    Ok(Done { counts, entries })
}

/// The message entries whose `reply_to` names one of `deleted` (entry ids), revised without it.
fn unquote(conn: &Connection, deleted: &[String], at: &str) -> ferb_store::Result<Vec<Entry>> {
    if deleted.is_empty() {
        return Ok(Vec::new());
    }
    let m = marks(deleted.len());
    let ids = strings(
        conn,
        &format!(
            "SELECT id FROM chat_entries WHERE kind = 'message' AND deleted = 0 \
             AND json_extract(body_json, '$.reply_to.entry_id') IN ({m})"
        ),
        deleted,
    )?;
    let mut revised = Vec::new();
    for id in ids {
        let Some(mut e) = ferb_store::chat_entries::get(conn, &id)? else {
            continue;
        };
        if let Value::Object(body) = &mut e.body {
            body.remove(crate::words::REPLY_TO);
        }
        revised.push(ferb_store::chat_entries::revise(conn, e, at)?);
    }
    Ok(revised)
}

/// The `context` of each later model call that quotes one of `texts` word for word (as its JSON stores it, once or
/// twice escaped) becomes `replacement`. Returns how many calls were changed.
pub fn blank_quotes(
    conn: &Connection,
    texts: &[String],
    replacement: &str,
) -> ferb_store::Result<usize> {
    let mut changed: BTreeSet<String> = BTreeSet::new();
    for text in texts {
        let once = inner_json(text);
        let twice = inner_json(&once);
        let ids = strings(
            conn,
            "SELECT id FROM spans WHERE kind LIKE 'model.%' AND (instr(input_json, ?) > 0 OR instr(input_json, ?) > 0)",
            &[once, twice],
        )?;
        for id in ids {
            if changed.contains(&id) {
                continue;
            }
            let input: String =
                conn.query_row("SELECT input_json FROM spans WHERE id = ?", [&id], |r| {
                    r.get(0)
                })?;
            let Ok(Value::Object(mut m)) = ferb_py::json::loads(&input) else {
                continue;
            };
            if !m.get("context").is_some_and(Value::is_string) {
                continue;
            }
            m.insert("context".into(), Value::String(replacement.to_string()));
            conn.execute(
                "UPDATE spans SET input_json = ? WHERE id = ?",
                [ferb_py::json::dumps(&Value::Object(m)), id.clone()],
            )?;
            changed.insert(id);
        }
    }
    Ok(changed.len())
}

/// The text as a JSON string stores it (Python's `json.dumps`), without its quotes.
fn inner_json(text: &str) -> String {
    let d = ferb_py::json::dumps(&Value::String(text.to_string()));
    d[1..d.len() - 1].to_string()
}

fn pairs(list: &[(String, String)]) -> Value {
    Value::Array(
        list.iter()
            .map(|(id, text)| json!({"id": id, "text": text}))
            .collect(),
    )
}

/// What FERB learned from the turn (STORE-426), by the links FERB records: a memory's sources
/// (`memory_sources`: the turn's journal rows, or activity-trail rows of its tasks) and a learned card's sources
/// (`learning_sources`: `journal:<id>` of its rows, `episode:<task>` of its tasks). One with no other source is
/// forgotten with the turn; one that has others too is kept and named.
fn learned_from(conn: &Connection, p: &mut Plan) -> rusqlite::Result<()> {
    let journal: Vec<String> = p.journal.iter().map(|(id, _)| id.clone()).collect();
    let trail: Vec<String> = if p.tasks.is_empty() {
        Vec::new()
    } else {
        strings(
            conn,
            &format!(
                "SELECT id FROM trail WHERE task_id IN ({})",
                marks(p.tasks.len())
            ),
            &p.tasks,
        )?
    };
    let mine = |kind: &str, id: &str| match kind {
        "journal" => journal.iter().any(|j| j == id),
        "trail" => trail.iter().any(|t| t == id),
        _ => false,
    };
    let mut sources: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT item_id, kind, source_id FROM memory_sources WHERE item_id IN \
             (SELECT item_id FROM memory_sources WHERE kind = ? AND source_id = ?)",
        )?;
        for (kind, list) in [("journal", &journal), ("trail", &trail)] {
            for id in list {
                let rows = stmt.query_map([kind, id.as_str()], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?;
                for row in rows {
                    let (item, k, sid) = row?;
                    let all = sources.entry(item).or_default();
                    if !all.contains(&(k.clone(), sid.clone())) {
                        all.push((k, sid));
                    }
                }
            }
        }
    }
    for (item, all) in sources {
        let words: String = conn
            .query_row(
                "SELECT content FROM memory_items WHERE id = ?",
                [&item],
                |r| r.get(0),
            )
            .unwrap_or_default();
        if all.iter().all(|(k, sid)| mine(k, sid)) {
            p.memories.push((item, words));
        } else {
            p.memories_kept.push((item, words));
        }
    }
    let mut refs: Vec<String> = journal.iter().map(|id| format!("journal:{id}")).collect();
    refs.extend(p.tasks.iter().map(|t| format!("episode:{t}")));
    if refs.is_empty() {
        return Ok(());
    }
    let nodes = strings(
        conn,
        &format!(
            "SELECT DISTINCT node_id FROM learning_sources WHERE source_ref IN ({}) ORDER BY node_id",
            marks(refs.len())
        ),
        &refs,
    )?;
    for node in nodes {
        let theirs = strings(
            conn,
            "SELECT source_ref FROM learning_sources WHERE node_id = ?",
            std::slice::from_ref(&node),
        )?;
        let title: String = conn
            .query_row(
                "SELECT COALESCE(CASE WHEN json_valid(body_json) THEN COALESCE(json_extract(body_json, '$.title'), \
                 json_extract(body_json, '$.goal'), json_extract(body_json, '$.text')) END, kind) FROM learning_nodes WHERE id = ?",
                [&node],
                |r| r.get(0),
            )
            .unwrap_or_default();
        if theirs.iter().all(|r| refs.contains(r)) {
            p.learned.push((node, title));
        } else {
            p.learned_kept.push((node, title));
        }
    }
    Ok(())
}

/// A capture's file under the evidence folder, never through a link: `rel` is plain names only, its folder resolves
/// inside the folder, and the path is not followed (a link there is itself what goes, never what it points to).
/// `Ok(None)`: nothing is there.
fn evidence_file(root: &Path, rel: &str) -> Result<Option<PathBuf>, ()> {
    let rel = Path::new(rel);
    if rel.as_os_str().is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(());
    }
    let full = root.join(rel);
    let inside = |dir: &Path| match (std::fs::canonicalize(dir), std::fs::canonicalize(root)) {
        (Ok(d), Ok(r)) => d.starts_with(&r),
        _ => false,
    };
    match std::fs::symlink_metadata(&full) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
        Ok(m) if m.is_dir() => Err(()),
        Ok(_) => match full.parent() {
            Some(parent) if inside(parent) => Ok(Some(full)),
            _ => Err(()),
        },
    }
}

/// The turn's tasks' evidence: each file (only under `root`, never through a link), then its row. A file that can't
/// be removed keeps its row (the age-out removes both later). Returns (gone, kept).
fn delete_evidence(
    conn: &Connection,
    tasks: &[String],
    root: Option<&Path>,
) -> ferb_store::Result<(usize, usize)> {
    let rows: Vec<(String, Option<String>)> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT id, path FROM evidence WHERE task_id IN ({})",
            marks(tasks.len())
        ))?;
        let it = stmt.query_map(params_from_iter(tasks.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        it.collect::<rusqlite::Result<_>>()?
    };
    let (mut gone, mut kept) = (0, 0);
    for (id, rel) in rows {
        let removable = match (rel.as_deref().filter(|r| !r.is_empty()), root) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(rel), Some(root)) => match evidence_file(root, rel) {
                Ok(None) => true,
                Ok(Some(file)) => match std::fs::remove_file(&file) {
                    Ok(()) => true,
                    Err(e) => e.kind() == std::io::ErrorKind::NotFound,
                },
                Err(()) => false,
            },
        };
        if removable {
            gone += conn.execute("DELETE FROM evidence WHERE id = ?", [&id])?;
        } else {
            kept += 1;
        }
    }
    Ok((gone, kept))
}

/// Why a turn can't be deleted now, or None: FERB is still answering it (its stream is open), or a task tree of it
/// still runs.
pub fn busy(p: &Plan, answering: bool, running: &BTreeSet<String>) -> Option<&'static str> {
    if answering {
        return Some(STILL_ANSWERING);
    }
    p.trees
        .iter()
        .any(|t| running.contains(t))
        .then_some(STILL_WORKING)
}

/// The line a fresh conversation starts with (STORE-427): in its stream as a notice entry, and in FERB.app's chat.
pub const FRESH_MARKER: &str = "New conversation: FERB doesn't read the messages above.";

/// Why a turn can't be deleted now, in the owner's words.
pub const STILL_ANSWERING: &str =
    "FERB is still answering that message. Wait until it has answered, then delete it.";
pub const STILL_WORKING: &str =
    "FERB is still working on that message's task. Stop it or wait until it ends, then delete it.";
pub const NOTHING_STORED: &str = "FERB has nothing stored for that message.";

impl super::Inner {
    /// `data.delete`: the turn of `request_id` (STORE-424). Without `confirm`, what it would delete and keep; with it,
    /// deleted in the chat log's write transaction, later calls' quotes of it blanked, and a `data.deleted` journal
    /// row with counts only. Refused while FERB is still answering it or one of its tasks runs.
    pub fn delete_turn(&self, request_id: &str, confirm: bool) -> Result<Value, String> {
        let planned = {
            let db = self.db();
            plan(db.conn(), request_id).map_err(|e| e.to_string())?
        };
        let Some(p) = planned else {
            return Err(NOTHING_STORED.into());
        };
        let running: BTreeSet<String> = self.running_trees().into_iter().map(|(t, _)| t).collect();
        if let Some(why) = busy(&p, self.chat_log.is_open(request_id), &running) {
            return Err(why.into());
        }
        if !confirm {
            let mut out = describe(&p);
            out["confirmed"] = json!(false);
            return Ok(out);
        }
        let at = self.now_iso();
        // the evidence store is `evidence` beside the database in FERB's home (ferbd: home/ferb.db, home/evidence)
        let evidence_root = self.db().path().parent().map(|h| h.join("evidence"));
        let done = self
            .chat_log
            .rewrite(self, |conn| {
                let d = apply(conn, &p, &at, evidence_root.as_deref())?;
                Ok((d.counts, d.entries))
            })
            .map_err(|e| e.to_string())?;
        let blanked = {
            let db = self.db();
            db.transaction(|c| {
                blank_quotes(c, &p.texts, ferb_orchestrator::attention::CONTEXT_REMOVED)
            })
            .map_err(|e| e.to_string())?
        };
        let mut counts = done;
        counts.insert("later_quotes_blanked".into(), json!(blanked));
        // what FERB learned only from it, through the memory port's own forget (Memory's Forget)
        let items: Vec<String> = p.memories.iter().map(|(id, _)| id.clone()).collect();
        let nodes: Vec<String> = p.learned.iter().map(|(id, _)| id.clone()).collect();
        let (memories, learned) = if items.is_empty() && nodes.is_empty() {
            (0, 0)
        } else {
            match self.deps.daemon.forget_learned(&items, &nodes) {
                Some(Ok((m, l))) => (m, l),
                Some(Err(e)) => {
                    counts.insert("memories_problem".into(), json!(e));
                    (0, 0)
                }
                None => (0, 0),
            }
        };
        counts.insert("memories".into(), json!(memories));
        counts.insert("learned".into(), json!(learned));
        self.journal(
            "data.deleted",
            json!({"what": "turn", "request_id": request_id, "tasks": p.tasks, "deleted": counts.clone(),
                   "kept": p.kept.clone()}),
            Some(ferb_sanitize::origin::OWNER),
        );
        Ok(json!({"confirmed": true, "request_id": request_id, "deleted": counts, "keeps": p.kept}))
    }

    /// A fresh conversation's first entry (STORE-427): a notice with [`FRESH_MARKER`], so every client shows where
    /// FERB stopped reading. Not journaled: the planner's conversation of the new chat starts empty.
    pub fn mark_fresh(&self, conversation: &str) -> Result<(), String> {
        let now = self.now_iso();
        let entry = Entry {
            id: self.new_id(),
            conversation_id: conversation.to_string(),
            pos: 0,
            seq: 0,
            rev: 0,
            kind: EntryKind::Notice,
            state: EntryState::Done,
            open: false,
            request_id: None,
            task_id: None,
            approval_id: None,
            reference: Some(format!("conversation:{conversation}")),
            answers: None,
            placement: None,
            origin: Some(ferb_sanitize::origin::FERB.to_string()),
            client_id: None,
            submission_id: None,
            journal_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
            day: now.chars().take(10).collect(),
            body: serde_json::to_value(NoticeBody {
                text: FRESH_MARKER.into(),
                quotes: Vec::new(),
                cards: None,
                at: Some(now.clone()),
            })
            .unwrap_or(Value::Null),
            deleted: false,
        };
        self.chat_log
            .rewrite(self, |conn| {
                let made = ferb_store::chat_entries::create(conn, entry)?;
                Ok(((), vec![made]))
            })
            .map_err(|e| e.to_string())
    }

    /// `data.context`: the conversation the planner would read for `conversation` now, block by block with each
    /// block's origins (the same `_conversation_blocks` its next turn reads).
    pub fn conversation_now(&self, conversation: &str) -> Result<Vec<Value>, String> {
        let (blocks, _, _, _) = self
            .conversation_view(&BTreeSet::new(), conversation, false)
            .map_err(|e| format!("{e:?}"))?;
        Ok(blocks
            .into_iter()
            .map(|b| json!({"text": b.block.text, "origins": b.origins}))
            .collect())
    }
}
