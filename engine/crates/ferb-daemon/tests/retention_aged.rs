//! The hourly retention pass runs STORE-419 (owner, 2026-11-02) with its other windows: perception batches go 30
//! days after they were made, notices' records and learned-node change rows 90 days after they were written, loop
//! wake-ups once they have fired and a pass noted them at least 10 minutes before (STORE-432); `retention.purged`
//! says how many of each went. Each window is counted from the daemon's clock (injected), a row as old as the window
//! stays and one a second older goes.

mod a_support;

use a_support::{Daemon, Seq};
use chrono::Duration;
use ferb_clock::clock::{Clock, FakeClock};
use ferb_daemon::areas_a::retention::{purge_pass, OverCap, Purges};
use serde_json::{json, Value};

struct NoStores;

impl Purges for NoStores {
    fn compose_episode(
        &self,
        _db: &ferb_store::Db,
        _task_id: &str,
    ) -> Result<Value, (String, String)> {
        Err(("none".into(), "no trees in this test".into()))
    }
    fn trail_accessibility(&self, _days: i64) -> Result<i64, String> {
        Ok(0)
    }
    fn trail(&self, _days: i64) -> Result<(Vec<String>, i64), String> {
        Ok((Vec::new(), 0))
    }
    fn evidence(&self, _before: &str) -> i64 {
        0
    }
    fn recovery(&self, _before: &str, _trash_backstop: &str) -> i64 {
        0
    }
    fn resources(&self, _today: chrono::NaiveDate, _days: i64) -> Result<i64, String> {
        Ok(0)
    }
    fn shell_logs(&self, _before_ts: f64) -> Result<i64, String> {
        Ok(0)
    }
    fn perception_text(&self) {}
    fn downloads_over_cap(&self) -> Result<Option<OverCap>, String> {
        Ok(None)
    }
    fn downloads_delete(&self, _ids: &[String]) -> (i64, i64) {
        (0, 0)
    }
}

// decisions: STORE-419, STORE-432
#[tokio::test]
async fn the_hourly_pass_ages_out_store_419_s_rows_at_their_windows() {
    let tmp = tempfile::tempdir().unwrap();
    let d = Daemon::start(tmp.path(), "01", "id", FakeClock::new(None), Seq::new("id")).await;
    let now = d.clock.now();
    let ago = |days: i64, extra_s: i64| {
        ferb_clock::py::iso(&(now - Duration::days(days) - Duration::seconds(extra_s)))
    };
    let mut sql: Vec<(String, Vec<Value>)> = Vec::new();
    for (id, at) in [
        ("b29", ago(29, 0)),
        ("b30", ago(30, 0)),
        ("b30s", ago(30, 1)),
        ("b31", ago(31, 0)),
    ] {
        sql.push((
            "INSERT INTO perception_batches (id, created_at, reason, state, event_ids_json) \
             VALUES (?, ?, 'age', 'done', '[]')"
                .into(),
            vec![json!(id), json!(at)],
        ));
        sql.push((
            "INSERT INTO perception_receipts (event_id, batch_id) VALUES (?, ?)".into(),
            vec![json!(format!("e-{id}")), json!(id)],
        ));
    }
    for (id, at) in [
        ("n89", ago(89, 0)),
        ("n90", ago(90, 0)),
        ("n90s", ago(90, 1)),
        ("n91", ago(91, 0)),
    ] {
        sql.push((
            "INSERT INTO attention_notices (id, source_refs_json, text, origin, created_at, delivered_at) \
             VALUES (?, '[]', 'a notice', 'ferb', ?, ?)"
                .into(),
            vec![json!(id), json!(at), json!(at)],
        ));
        sql.push((
            "INSERT INTO learning_changes (batch_id, node_id, action, at) VALUES (?, 'node-1', 'add', ?)"
                .into(),
            vec![json!(id), json!(at)],
        ));
    }
    // a wake-up no plan waits for any more
    sql.push((
        "INSERT INTO plan_events (event, payload) VALUES ('mail', '{}')".into(),
        vec![],
    ));
    d.sql(sql).await;

    let pass = || async {
        d.on(|st| Box::pin(async move { purge_pass(&st, &NoStores).await.unwrap() }))
            .await
    };
    let seqs = || async {
        d.table("plan_events")
            .await
            .into_iter()
            .map(|r| r[0].clone())
            .collect::<Vec<Value>>()
    };

    // the first pass since the start notes the wake-ups it sees but deletes none: none is known to be 10 minutes old
    let out = pass().await;
    assert_eq!(out["aged"], json!(6));
    assert_eq!(seqs().await, [json!(1)]);

    let ids =
        |rows: Vec<Vec<Value>>| -> Vec<Value> { rows.into_iter().map(|r| r[0].clone()).collect() };
    assert_eq!(
        ids(d.table("perception_batches").await),
        [json!("b29"), json!("b30")]
    );
    assert_eq!(
        ids(d.table("perception_receipts").await),
        [json!("e-b29"), json!("e-b30")]
    );
    assert_eq!(
        ids(d.table("attention_notices").await),
        [json!("n89"), json!("n90")]
    );
    let changes: Vec<Value> = d
        .table("learning_changes")
        .await
        .into_iter()
        .map(|r| r[1].clone())
        .collect();
    assert_eq!(changes, [json!("n89"), json!("n90")]);

    let purged = || async {
        d.on(|st| {
            Box::pin(async move {
                let conn = st.db().conn();
                let mut stmt = conn
                    .prepare("SELECT data_json FROM journal WHERE kind = 'retention.purged' ORDER BY seq")
                    .unwrap();
                let rows: Vec<Value> = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .unwrap()
                    .map(|t| serde_json::from_str(&t.unwrap()).unwrap())
                    .collect();
                rows
            })
        })
        .await
    };
    let lines = purged().await;
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0]["aged"],
        json!({"perception_batches": 2, "attention_notices": 2, "learning_changes": 2})
    );

    // decision: STORE-432 — a wake-up goes only once a pass noted it at least WAKE_UP_GRACE_S (600 s) earlier
    d.sql(vec![(
        "INSERT INTO plan_events (event, payload) VALUES ('mail', '{}')".into(),
        vec![],
    )])
    .await; // seq 2, stored after the first pass's note
    d.clock.advance(599.0);
    // the rows that were exactly as old as their windows are now older: they go; no wake-up does
    assert_eq!(pass().await["aged"], json!(3));
    assert_eq!(
        purged().await.last().unwrap()["aged"],
        json!({"perception_batches": 1, "attention_notices": 1, "learning_changes": 1})
    );
    assert_eq!(seqs().await, [json!(1), json!(2)]);
    d.clock.advance(1.0); // the first note is 600 s old: seq 1 goes, seq 2 (noted 1 s ago) stays
    assert_eq!(pass().await["aged"], json!(1));
    assert_eq!(seqs().await, [json!(2)]);
    d.clock.advance(598.0);
    assert_eq!(pass().await["aged"], json!(0));
    assert_eq!(seqs().await, [json!(2)]);
    d.clock.advance(1.0);
    assert_eq!(pass().await["aged"], json!(1));
    assert!(seqs().await.is_empty());
    let lines = purged().await;
    assert_eq!(lines.last().unwrap()["aged"], json!({"plan_events": 1}));
}
