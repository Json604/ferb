//! Migration 0066 (APP-979): an older FERB.app filed a task it had already shown a second time, as a turn with no request
//! and no goal dated the launch of a later day (APP-974). The migration removes those copies once, on every install that
//! has them (APP-976), and nothing else. Temporary folders only.

use std::path::Path;

use ferb_store::{chat_history::ChatHistory, chat_history::REPAIR_VERSION, Db, Migration};
use rusqlite::{params, Connection};
use serde_json::json;
use tempfile::TempDir;

/// A store as the build before 0066 left it.
fn store_before_repair(path: &Path) -> Db {
    let db = Db::open(path).unwrap();
    let older: Vec<Migration> = Migration::embedded()
        .into_iter()
        .filter(|m| m.name.as_ref() < "0066")
        .collect();
    db.migrate_files(&older).unwrap();
    assert_eq!(db.schema_version().unwrap(), REPAIR_VERSION - 1);
    db
}

/// A turn's task, its status and its answer.
type Ended<'a> = (&'a str, &'a str, &'a str);

/// A turn as FERB.app writes it (`ConversationTurn`'s saved fields, the ones the repair reads).
fn turn(
    conn: &Connection,
    id: &str,
    at: &str,
    request: &str,
    goal: Option<&str>,
    (task, status, answer): Ended,
) {
    let mut body = json!({"id": id, "request": request, "backendTaskID": task, "status": status, "answer": answer,
                          "sources": [], "artifacts": [], "verified": false, "createdAt": 816_754_193.18});
    if let Some(goal) = goal {
        body["taskGoal"] = json!(goal);
    }
    conn.execute(
        "INSERT INTO chat_history (id, kind, at, day, open, ref, body_json, updated_at) VALUES (?, 'turn', ?, ?, 0, NULL, ?, ?)",
        params![id, at, &at[..10], body.to_string(), format!("{at}~u")],
    )
    .unwrap();
}

fn ids(conn: &Connection) -> Vec<String> {
    let mut st = conn
        .prepare("SELECT id FROM chat_history ORDER BY day, at, id")
        .unwrap();
    st.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn rows(conn: &Connection) -> Vec<(String, String, String, String)> {
    let mut st = conn
        .prepare("SELECT id, at, day, body_json FROM chat_history ORDER BY id")
        .unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

const ANSWER: &str = "Stopped and left the unsent Gmail draft open for your review.\n\nNot done: A recipient's address is twice in the draft.";

// decisions: APP-979, APP-974, APP-976
#[test]
fn only_the_copies_of_a_shown_task_go_and_a_second_run_changes_nothing() {
    let tmp = TempDir::new().unwrap();
    let db = store_before_repair(&tmp.path().join("ferb.db"));
    let conn = db.conn();
    // the task's own turn, from the owner's message
    turn(
        conn,
        "ORIGINAL",
        "2026-11-18T04:14:11.000000+00:00",
        "find them through mailmeteor",
        None,
        ("t-1", "done_with_gaps", ANSWER),
    );
    // its copies: the next day's first launch, and another day's alone
    turn(
        conn,
        "COPY-19",
        "2026-11-19T04:09:53.181000+00:00",
        "",
        Some(""),
        ("t-1", "done_with_gaps", ANSWER),
    );
    turn(
        conn,
        "COPY-21",
        "2026-11-21T03:00:00.000000+00:00",
        "",
        Some(""),
        ("t-1", "done_with_gaps", ANSWER),
    );
    // a later turn of the same task that says something else: a genuine later answer
    turn(
        conn,
        "LATER",
        "2026-11-20T09:00:00.000000+00:00",
        "",
        Some(""),
        ("t-1", "done", "Sent the draft."),
    );
    // a turn filed for a task this chat had no turn for (another device's), and its copy a day later
    turn(
        conn,
        "RECOVERED",
        "2026-11-19T07:00:00.000000+00:00",
        "",
        None,
        ("t-2", "done", "Renamed 12 files."),
    );
    turn(
        conn,
        "RECOVERED-COPY",
        "2026-11-22T07:00:00.000000+00:00",
        "",
        None,
        ("t-2", "done", "Renamed 12 files."),
    );
    // unrelated: the owner's own words, with no task, and a reply
    turn(
        conn,
        "UNRELATED",
        "2026-11-19T08:00:00.000000+00:00",
        "what's on today?",
        None,
        ("t-3", "done", ANSWER),
    );
    conn.execute(
        "INSERT INTO chat_history (id, kind, at, day, open, ref, body_json, updated_at) VALUES ('REPLY', 'reply', \
         '2026-11-19T08:30:00.000000+00:00', '2026-11-19', 0, NULL, '{\"turnID\": \"ORIGINAL\"}', 'u')",
        [],
    )
    .unwrap();
    let history = ChatHistory::new(&db, Box::new(|v| v.clone()), Box::new(|| "now".into()));
    assert_eq!(
        history.days().unwrap(),
        [
            "2026-11-22",
            "2026-11-21",
            "2026-11-20",
            "2026-11-19",
            "2026-11-18"
        ]
    );
    let before = rows(conn);

    assert_eq!(db.migrate().unwrap(), vec![REPAIR_VERSION]);
    assert_eq!(
        ids(conn),
        ["ORIGINAL", "RECOVERED", "UNRELATED", "REPLY", "LATER"],
        "only the copies went"
    );
    let kept: Vec<_> = before
        .iter()
        .filter(|r| !["COPY-19", "COPY-21", "RECOVERED-COPY"].contains(&r.0.as_str()))
        .cloned()
        .collect();
    assert_eq!(
        rows(conn),
        kept,
        "every other row as it was: its time, day and bytes"
    );
    // FERB.app's History lists no day that held only a copy
    assert_eq!(
        history.days().unwrap(),
        ["2026-11-20", "2026-11-19", "2026-11-18"]
    );

    // a second start applies nothing, and the statement run again finds nothing more
    assert!(db.migrate().unwrap().is_empty());
    let file = Migration::embedded()
        .into_iter()
        .find(|m| m.name.as_ref().starts_with("0066_"))
        .unwrap();
    for statement in ferb_store::split_sql(std::str::from_utf8(&file.bytes).unwrap()).unwrap() {
        conn.execute_batch(&statement).unwrap();
    }
    assert_eq!(rows(conn), kept);
    let ok: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
}

// decision: APP-979
#[test]
fn a_store_without_copies_or_without_history_upgrades_unchanged() {
    let tmp = TempDir::new().unwrap();
    let db = store_before_repair(&tmp.path().join("ferb.db"));
    assert_eq!(db.migrate().unwrap(), vec![REPAIR_VERSION]);
    assert!(ids(db.conn()).is_empty());

    let tmp = TempDir::new().unwrap();
    let db = store_before_repair(&tmp.path().join("ferb.db"));
    let conn = db.conn();
    turn(
        conn,
        "ORIGINAL",
        "2026-11-18T04:14:11.000000+00:00",
        "find them",
        None,
        ("t-1", "done_with_gaps", ANSWER),
    );
    // same day, not a later one: left (the copies come only at a later day's launch)
    turn(
        conn,
        "SAME-DAY",
        "2026-11-18T09:00:00.000000+00:00",
        "",
        Some(""),
        ("t-1", "done_with_gaps", ANSWER),
    );
    // a copy with a goal is FERB's own heading for a task: not this bug's
    turn(
        conn,
        "WITH-GOAL",
        "2026-11-19T09:00:00.000000+00:00",
        "",
        Some("Find the emails"),
        ("t-1", "done_with_gaps", ANSWER),
    );
    let before = rows(conn);
    assert_eq!(db.migrate().unwrap(), vec![REPAIR_VERSION]);
    assert_eq!(rows(conn), before);
}
