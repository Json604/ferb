//! Retention's walk down a span tree ends on a parent cycle. Fixed against Python (ENG-6):
//! `ferb/store/retention.py` `_with_descendants` deepens a span each time it finds it deeper, so
//! spans whose parents form a cycle (the foreign key alone does not forbid one) keep it walking
//! for ever. Here each span comes back once, and every child that is not in the cycle still goes
//! before its parent. Invented spans in a temporary store.
//!
//! decision: ENG-6, STORE-406

use std::{sync::mpsc, thread, time::Duration};

use ferb_store::{retention, Db};
use rusqlite::{params, types::Value as Sql, Connection};
use tempfile::TempDir;

const TASK: &str = "t-0001";

fn store() -> (TempDir, Db) {
    let tmp = TempDir::new().unwrap();
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    db.conn()
        .execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, \
             created_at, updated_at) VALUES (?1, ?1, NULL, 'r-1', 'goal', 'owner', 'worker', '[]', 'done', \
             '2020-01-01T00:00:00+00:00', '2020-01-01T00:00:00+00:00')",
            [TASK],
        )
        .unwrap();
    (tmp, db)
}

fn span(conn: &Connection, id: &str, parent: Option<&str>) {
    conn.execute(
        "INSERT INTO spans (id, parent_id, task_id, kind, started_at, input_json) \
         VALUES (?, ?, ?, 'step', '2020-01-01T00:00:00+00:00', '{}')",
        params![id, parent, TASK],
    )
    .unwrap();
}

fn reparent(conn: &Connection, id: &str, parent: &str) {
    conn.execute(
        "UPDATE spans SET parent_id = ? WHERE id = ?",
        params![parent, id],
    )
    .unwrap();
}

/// The task's tree as `plan` gives it, or a failure if it does not answer within a few seconds
/// (Python's walk never answers).
fn tree(db: Db) -> (Db, Vec<String>) {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let plan = retention::plan(
            db.conn(),
            "2021-01-01T00:00:00+00:00",
            "2000-01-01T00:00:00+00:00",
            "2000-01-01T00:00:00+00:00",
            10,
        )
        .unwrap();
        send.send((db, plan)).unwrap();
    });
    let (db, plan) = receive
        .recv_timeout(Duration::from_secs(30))
        .expect("the walk ends on a parent cycle");
    assert_eq!(plan.roots, vec![TASK.to_string()]);
    let (root, spans) = plan.trees.into_iter().next().unwrap();
    assert_eq!(root, TASK);
    (db, spans)
}

fn position(spans: &[String], id: &str) -> usize {
    spans.iter().position(|s| s == id).unwrap()
}

#[test]
fn a_parent_cycle_ends_the_walk_and_each_span_comes_back_once() {
    // a <-> b is the cycle; c hangs under b and d under c; e is a second cycle of one (its own parent)
    let (_tmp, db) = store();
    let conn = db.conn();
    span(conn, "a", None);
    span(conn, "b", Some("a"));
    reparent(conn, "a", "b");
    span(conn, "c", Some("b"));
    span(conn, "d", Some("c"));
    span(conn, "e", None);
    reparent(conn, "e", "e");
    let (db, spans) = tree(db);
    let mut sorted = spans.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        ["a", "b", "c", "d", "e"],
        "each span once: {spans:?}"
    );
    // every child outside the cycle goes before its parent
    assert!(position(&spans, "d") < position(&spans, "c"), "{spans:?}");
    assert!(position(&spans, "c") < position(&spans, "b"), "{spans:?}");
    // and the batches delete the whole tree, the cycle in one statement
    let ids: Vec<Sql> = spans.iter().cloned().map(Sql::Text).collect();
    let deleted: usize = retention::delete_rows(&db, "spans", "id", &ids, retention::BATCH)
        .map(Result::unwrap)
        .sum();
    assert_eq!(deleted, 5);
    let left: i64 = db
        .conn()
        .query_row("SELECT count(*) FROM spans", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0);
}

#[test]
fn a_tree_without_a_cycle_comes_back_deepest_first_as_before() {
    // the walk Python makes, unchanged: a span found again deeper (here a seed under another) moves down
    let (_tmp, db) = store();
    let conn = db.conn();
    span(conn, "r", None);
    span(conn, "s", Some("r"));
    span(conn, "u", Some("s"));
    span(conn, "v", Some("r"));
    let (_db, spans) = tree(db);
    assert_eq!(spans, ["u", "s", "v", "r"]);
}
