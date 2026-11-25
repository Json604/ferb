//! STORE-419's retention (owner, 2026-11-02): perception batches and their receipts go 30 days after the batch was
//! made, notices' records and learned-node change rows 90 days after they were written, and a loop wake-up once it
//! has fired. The boundaries are exact: a row as old as the window stays, one a second older goes; times compare as
//! instants whatever offset they were written with. The chat's copies of a notice and the learned nodes stay. Invented
//! rows in a temporary store; the cutoffs are what the daemon passes for an injected "now" of 2026-06-01 12:00 UTC.

use ferb_store::{retention, Db};
use rusqlite::{params, types::Value as Sql, Connection};
use serde_json::json;
use tempfile::TempDir;

/// 2026-06-01T12:00:00+00:00 less 30 days.
const DAYS_30: &str = "2026-05-02T12:00:00+00:00";
/// 2026-06-01T12:00:00+00:00 less 90 days.
const DAYS_90: &str = "2026-03-03T12:00:00+00:00";

fn store() -> (TempDir, Db) {
    let tmp = TempDir::new().unwrap();
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    (tmp, db)
}

fn ids(conn: &Connection, sql: &str) -> Vec<String> {
    let mut statement = conn.prepare(sql).unwrap();
    let rows = statement.query_map([], |r| r.get(0)).unwrap();
    rows.map(Result::unwrap).collect()
}

fn aged(db: &Db) -> Vec<(&'static str, Vec<i64>)> {
    retention::aged(db.conn(), DAYS_30, DAYS_90, DAYS_90, Some(i64::MAX)).unwrap()
}

fn of(found: &[(&'static str, Vec<i64>)], table: &str) -> Vec<i64> {
    found
        .iter()
        .find(|(t, _)| *t == table)
        .map(|(_, ids)| ids.clone())
        .unwrap_or_default()
}

/// Deletes what `aged` found, as the daemon's pass does: by rowid, dependents first.
fn delete(db: &Db, found: &[(&'static str, Vec<i64>)]) {
    for (table, rows) in found {
        let rows: Vec<Sql> = rows.iter().map(|r| Sql::Integer(*r)).collect();
        for step in retention::delete_rows(db, table, "rowid", &rows, retention::BATCH) {
            step.unwrap();
        }
    }
}

// decision: STORE-419
#[test]
fn the_windows_are_the_owners_numbers() {
    assert_eq!(retention::PERCEPTION_DAYS, 30);
    assert_eq!(retention::NOTICE_DAYS, 90);
    assert_eq!(retention::CHANGE_DAYS, 90);
}

// decisions: STORE-419, STORE-432
#[test]
fn perception_batches_go_after_30_days_with_their_receipts() {
    let (_tmp, db) = store();
    let conn = db.conn();
    let batches = [
        ("d29", "2026-05-03T12:00:00+00:00"),
        ("d30", DAYS_30),                      // exactly 30 days: stays
        ("d30s", "2026-05-02T11:59:59+00:00"), // a second more: goes
        ("d31", "2026-05-01T12:00:00+00:00"),
        ("east", "2026-05-02T16:30:00+05:30"), // 30 days and an hour, written east of UTC: goes
        ("west", "2026-05-02T08:00:00-05:00"), // 29 days and 23 hours, written west of UTC: stays
        ("odd", "not a time"),                 // a time that isn't one keeps its row
    ];
    for (id, at) in batches {
        conn.execute(
            "INSERT INTO perception_batches (id, created_at, reason, state, event_ids_json) \
             VALUES (?, ?, 'age', 'done', '[]')",
            params![id, at],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO perception_receipts (event_id, batch_id) VALUES (?, ?)",
            params![format!("e-{id}"), id],
        )
        .unwrap();
    }
    let found = aged(&db);
    let rowids: Vec<i64> = ["d30s", "d31", "east"]
        .iter()
        .map(|id| {
            conn.query_row(
                "SELECT rowid FROM perception_batches WHERE id = ?",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(of(&found, "perception_batches"), rowids);
    delete(&db, &found);
    assert_eq!(
        ids(conn, "SELECT id FROM perception_batches ORDER BY rowid"),
        ["d29", "d30", "west", "odd"]
    );
    assert_eq!(
        ids(
            conn,
            "SELECT event_id FROM perception_receipts ORDER BY rowid"
        ),
        ["e-d29", "e-d30", "e-west", "e-odd"]
    );
}

// decisions: STORE-419, STORE-432
#[test]
fn notices_records_go_after_90_days_and_the_chat_keeps_them() {
    let (_tmp, db) = store();
    let conn = db.conn();
    let notices = [
        ("n89", "2026-03-04T12:00:00+00:00"),
        ("n90", DAYS_90),
        ("n90s", "2026-03-03T11:59:59+00:00"),
        ("n91", "2026-03-02T12:00:00+00:00"),
        ("east", "2026-03-03T16:30:00+05:30"),
        ("west", "2026-03-03T08:00:00-05:00"),
        ("unseen", "2026-01-01T09:00:00+00:00"), // never shown by a FERB app: goes all the same
    ];
    for (id, at) in notices {
        let delivered = (id != "unseen").then_some(at);
        conn.execute(
            "INSERT INTO attention_notices (id, source_refs_json, text, origin, created_at, delivered_at) \
             VALUES (?, '[\"task:t1\"]', ?, 'ferb', ?, ?)",
            params![id, format!("notice {id}"), at, delivered],
        )
        .unwrap();
    }
    // the chat's copies of the 91-day-old notice: the planner's conversation, FERB.app's chat
    conn.execute(
        "INSERT INTO journal (id, at, kind, data_json) VALUES ('j1', ?, 'chat.notice', ?)",
        params![
            "2026-03-02T12:00:00+00:00",
            json!({"id": "n91", "text": "notice n91"}).to_string()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chat_history (id, kind, at, day, ref, body_json, updated_at) \
         VALUES ('c1', 'notice', ?, '2026-03-02', 'n91', '{\"text\": \"notice n91\"}', ?)",
        params!["2026-03-02T12:00:00+00:00", "2026-03-02T12:00:00+00:00"],
    )
    .unwrap();
    let found = aged(&db);
    delete(&db, &found);
    assert_eq!(
        ids(conn, "SELECT id FROM attention_notices ORDER BY rowid"),
        ["n89", "n90", "west"]
    );
    assert_eq!(
        ids(conn, "SELECT id FROM journal WHERE kind = 'chat.notice'"),
        ["j1"]
    );
    assert_eq!(
        ids(conn, "SELECT body_json FROM chat_history WHERE ref = 'n91'"),
        ["{\"text\": \"notice n91\"}"]
    );
}

// decisions: STORE-419, STORE-432
#[test]
fn learned_node_change_rows_go_after_90_days_and_the_nodes_stay() {
    let (_tmp, db) = store();
    let conn = db.conn();
    conn.execute(
        "INSERT INTO learning_nodes (id, kind, key, body_json, origin, made_by, recorded_at, valid_from) \
         VALUES ('node-1', 'fact', 'k', '{}', 'ferb', 'code:test', ?, ?)",
        params!["2026-01-01T09:00:00+00:00", "2026-01-01T09:00:00+00:00"],
    )
    .unwrap();
    let changes = [
        ("b89", "2026-03-04T12:00:00+00:00"),
        ("b90", DAYS_90),
        ("b90s", "2026-03-03T11:59:59+00:00"),
        ("b91", "2026-03-02T12:00:00+00:00"),
        ("east", "2026-03-03T16:30:00+05:30"),
        ("west", "2026-03-03T08:00:00-05:00"),
    ];
    for (batch, at) in changes {
        conn.execute(
            "INSERT INTO learning_changes (batch_id, node_id, action, at) VALUES (?, 'node-1', 'add', ?)",
            params![batch, at],
        )
        .unwrap();
    }
    let found = aged(&db);
    assert_eq!(of(&found, "learning_changes"), [3, 4, 5]);
    delete(&db, &found);
    assert_eq!(
        ids(conn, "SELECT batch_id FROM learning_changes ORDER BY seq"),
        ["b89", "b90", "west"]
    );
    assert_eq!(ids(conn, "SELECT id FROM learning_nodes"), ["node-1"]);
}

fn plan(conn: &Connection, task: &str, status: &str, plan: &str) {
    conn.execute(
        "INSERT INTO task_plans (task_id, plan, request, status) VALUES (?, ?, '{}', ?)",
        params![task, plan, status],
    )
    .unwrap();
}

fn wait_step(conn: &Connection, task: &str, step: &str, output: &str) {
    conn.execute(
        "INSERT INTO plan_steps (task_id, step_id, status, output) VALUES (?, ?, 'waiting', ?)",
        params![task, step, output],
    )
    .unwrap();
}

fn loop_row(conn: &Connection, task: &str, step: &str, cursor: i64) {
    conn.execute(
        "INSERT INTO loop_state (task_id, step_id, event_cursor, updated_at) VALUES (?, ?, ?, '2026')",
        params![task, step, cursor],
    )
    .unwrap();
}

fn events(conn: &Connection, names: &[&str]) {
    for name in names {
        conn.execute(
            "INSERT INTO plan_events (event, payload) VALUES (?, '{}')",
            [name],
        )
        .unwrap();
    }
}

// decisions: STORE-419, STORE-432
#[test]
fn a_wake_up_goes_once_every_waiter_of_its_event_has_read_it() {
    let (_tmp, db) = store();
    let conn = db.conn();
    // seq 1..6
    events(conn, &["mail", "mail", "mail", "file", "file", "tab"]);
    // nothing waits: every wake-up has fired
    assert_eq!(
        retention::fired_wake_ups(conn, i64::MAX).unwrap(),
        [1, 2, 3, 4, 5, 6]
    );

    // a running loop on "mail" has read up to 2; a wait step on "file" has read nothing past 3
    plan(
        conn,
        "t1",
        "running",
        r#"{"steps": [{"id": "watch", "kind": "do", "event": null, "repeat": {"end_when": "x", "event": "mail", "event_match": []}},
                      {"id": "probe", "kind": "do", "event": "tab", "repeat": {"end_when": "x", "event": null, "event_match": []}},
                      {"id": "w", "kind": "wait", "event": "file", "repeat": null}]}"#,
    );
    loop_row(conn, "t1", "watch", 2);
    // a loop that only probes waits for no event, though its step names one outside its repeat rule
    loop_row(conn, "t1", "probe", 0);
    wait_step(
        conn,
        "t1",
        "w",
        r#"{"status": "waiting", "event_cursor": 3}"#,
    );
    assert_eq!(
        retention::fired_wake_ups(conn, i64::MAX).unwrap(),
        [1, 2, 6]
    );

    // a suspended plan's loop on "mail" that has read only up to 1 keeps 2 as well
    plan(
        conn,
        "t2",
        "suspended",
        r#"{"steps": [{"id": "watch", "kind": "do", "repeat": {"end_when": "x", "event": "mail", "event_match": []}}]}"#,
    );
    loop_row(conn, "t2", "watch", 1);
    assert_eq!(retention::fired_wake_ups(conn, i64::MAX).unwrap(), [1, 6]);

    // an ended plan's waiters read nothing more
    plan(
        conn,
        "t3",
        "done",
        r#"{"steps": [{"id": "w", "kind": "wait", "event": "tab"}]}"#,
    );
    wait_step(
        conn,
        "t3",
        "w",
        r#"{"status": "waiting", "event_cursor": 0}"#,
    );
    assert_eq!(retention::fired_wake_ups(conn, i64::MAX).unwrap(), [1, 6]);

    // a waiter its plan doesn't name readably counts as waiting for every event after its cursor
    plan(conn, "t4", "steps_done", "not json");
    loop_row(conn, "t4", "gone", 5);
    assert_eq!(retention::fired_wake_ups(conn, i64::MAX).unwrap(), [1]);

    // through the pass: only the fired rows go
    let found = aged(&db);
    assert_eq!(of(&found, "plan_events"), [1]);
    delete(&db, &found);
    assert_eq!(
        ids(
            conn,
            "SELECT CAST(seq AS TEXT) FROM plan_events ORDER BY seq"
        ),
        ["2", "3", "4", "5", "6"]
    );
}

// decisions: STORE-419, STORE-432
#[test]
fn a_fired_wake_up_newer_than_the_graces_mark_stays() {
    // the daemon passes the newest seq it noted at least WAKE_UP_GRACE_S ago: a waiter takes its cursor and saves it
    // in two statements, so a wake-up stored between them is never deleted before the waiter has saved and read it
    let (_tmp, db) = store();
    let conn = db.conn();
    assert_eq!(retention::WAKE_UP_GRACE_S, 600);
    events(conn, &["tab", "tab"]); // seq 1, 2: nothing waits for them
    assert_eq!(retention::wake_up_mark(conn).unwrap(), 2);
    events(conn, &["tab"]); // seq 3: stored after the mark
    assert_eq!(
        retention::fired_wake_ups(conn, i64::MAX).unwrap(),
        [1, 2, 3]
    );
    assert_eq!(retention::fired_wake_ups(conn, 2).unwrap(), [1, 2]);
    assert_eq!(
        retention::fired_wake_ups(conn, 0).unwrap(),
        Vec::<i64>::new()
    );
    // no mark old enough yet (as after a start): no wake-up goes
    let none = retention::aged(conn, DAYS_30, DAYS_90, DAYS_90, None).unwrap();
    assert!(none.is_empty());
    let found = retention::aged(conn, DAYS_30, DAYS_90, DAYS_90, Some(2)).unwrap();
    delete(&db, &found);
    assert_eq!(
        ids(conn, "SELECT CAST(seq AS TEXT) FROM plan_events"),
        ["3"]
    );
    assert_eq!(retention::wake_up_mark(conn).unwrap(), 3);
}

// decisions: STORE-419, STORE-432
#[test]
fn a_new_wake_up_after_the_pass_keeps_counting_from_the_last_seq() {
    // a waiter that starts takes the newest row's seq as its cursor; rows deleted never come back as new ones
    let (_tmp, db) = store();
    let conn = db.conn();
    events(conn, &["mail", "mail"]);
    delete(&db, &aged(&db));
    events(conn, &["mail"]);
    assert_eq!(
        ids(conn, "SELECT CAST(seq AS TEXT) FROM plan_events"),
        ["3"]
    );
}

// decisions: STORE-419, STORE-432
#[test]
fn nothing_aged_leaves_every_table_out() {
    let (_tmp, db) = store();
    assert!(aged(&db).is_empty());
}

struct NoCards;

impl ferb_store::cleanup::CardSources for NoCards {
    fn card_source_plan(
        &self,
        _: &Connection,
        _: &[String],
    ) -> ferb_store::Result<serde_json::Value> {
        Ok(json!({"tasks": 0, "nodes": 0, "untraced": 0, "stopped": false, "drop": {}}))
    }
    fn prune_card_sources(
        &self,
        _: &Connection,
        _: &dyn Fn() -> bool,
        _: &serde_json::Value,
    ) -> ferb_store::Result<serde_json::Value> {
        Ok(json!({"rows": 0, "stopped": false}))
    }
}

struct Kept(Vec<String>);

impl ferb_store::cleanup::JournalSink for Kept {
    fn append(&mut self, kind: &str, _: &serde_json::Value) -> ferb_store::Result<()> {
        self.0.push(kind.to_owned());
        Ok(())
    }
}

// decisions: STORE-419, STORE-432, STORE-412
#[test]
fn the_owners_clean_up_ok_holds_while_the_hourly_pass_ages_batches_out() {
    // the clean-up's preview is read while the daemon runs; if the hourly pass ages a batch out before the owner's OK
    // is applied, the OK still matches: the step is approved by its table
    let (_tmp, db) = store();
    let conn = db.conn();
    for (id, at) in [
        ("old", "2026-05-01T12:00:00+00:00"),
        ("new", "2026-05-31T12:00:00+00:00"),
    ] {
        conn.execute(
            "INSERT INTO perception_batches (id, created_at, reason, state, event_ids_json) \
             VALUES (?, ?, 'age', 'done', '[]')",
            params![id, at],
        )
        .unwrap();
    }
    let preview = ferb_store::cleanup::plan(conn, &NoCards).unwrap();
    let step = preview["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["table"] == "perception_batches")
        .unwrap()
        .clone();
    assert_eq!(step["live"], json!(true));
    delete(&db, &aged(&db));
    let mut journal = Kept(Vec::new());
    let out = ferb_store::cleanup::apply(
        &db,
        &mut journal,
        &NoCards,
        preview["ok"].as_str().unwrap(),
        ferb_store::cleanup::Run {
            stop: &|| false,
            say: &|_| {},
            free: &|_| Ok(0),
        },
    )
    .unwrap();
    assert_eq!(out.get("refused"), None, "{out}");
    assert_eq!(out["removed"]["perception_batches"], json!(1));
    assert!(ids(conn, "SELECT id FROM perception_batches").is_empty());
}
