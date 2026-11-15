//! STORE-426: what FERB learned only from a turn the owner deleted goes through the memory port's own forget (the
//! path Memory's Forget takes): the items with their search copies and tombstones, then the learned cards and what was
//! derived from them; what the same call wasn't given stays.

mod a_support;

use a_support::{Daemon, Seq};
use ferb_clock::clock::FakeClock;
use ferb_daemon::areas_a::memory;
use serde_json::{json, Value};

fn count(d: &std::rc::Rc<ferb_daemon::areas_a::State>, sql: &str) -> i64 {
    d.db().conn().query_row(sql, [], |r| r.get(0)).unwrap()
}

// decision: STORE-426
// decision: STORE-428
#[tokio::test]
async fn a_deleted_turn_s_learning_is_forgotten_by_memory_s_own_forget() {
    let tmp = tempfile::tempdir().unwrap();
    let d = Daemon::start(tmp.path(), "01", "id", FakeClock::new(None), Seq::new("id")).await;
    let at = "2026-01-01T09:00:00+00:00";
    let mut statements: Vec<(String, Vec<Value>)> = Vec::new();
    for (id, content) in [
        ("m-only", "The owner greets FERB with hi there"),
        ("m-other", "The owner likes short greetings"),
    ] {
        statements.push((
            "INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, \
             valid_from, created_at) VALUES (?, 'fact', 'owner', ?, ?, 'owner_stated', 'active', 1.0, ?, ?, ?)"
                .into(),
            vec![json!(id), json!(id), json!(content), json!(at), json!(at), json!(at)],
        ));
        statements.push((
            "INSERT INTO memory_sources (item_id, kind, source_id) VALUES (?, 'journal', ?)".into(),
            vec![json!(id), json!(format!("j-{id}"))],
        ));
    }
    for (id, derived_from) in [
        ("n-card", None),
        ("n-derived", Some("n-card")),
        ("n-other", None),
    ] {
        statements.push((
            "INSERT INTO learning_nodes (id, kind, key, body_json, origin, made_by, recorded_at, valid_from) \
             VALUES (?, 'episode', ?, '{\"title\": \"t\"}', 'ferb', 'test', ?, ?)"
                .into(),
            vec![json!(id), json!(id), json!(at), json!(at)],
        ));
        if let Some(from) = derived_from {
            statements.push((
                "INSERT INTO learning_sources (node_id, source_ref) VALUES (?, ?)".into(),
                vec![json!(id), json!(from)],
            ));
        }
    }
    d.sql(statements).await;
    let (counts, left) = d
        .on(|st| {
            Box::pin(async move {
                let out = memory::forget_turn_learning(
                    &st,
                    &["m-only".to_string(), "m-gone-already".to_string()],
                    &["n-card".to_string()],
                )
                .await
                .unwrap();
                let left = (
                    count(&st, "SELECT COUNT(*) FROM memory_items WHERE id = 'm-only'"),
                    count(
                        &st,
                        "SELECT COUNT(*) FROM memory_items WHERE id = 'm-other'",
                    ),
                    count(
                        &st,
                        "SELECT COUNT(*) FROM memory_fts WHERE content LIKE '%hi there%'",
                    ),
                    count(
                        &st,
                        "SELECT COUNT(*) FROM learning_nodes WHERE id IN ('n-card', 'n-derived')",
                    ),
                    count(
                        &st,
                        "SELECT COUNT(*) FROM learning_nodes WHERE id = 'n-other'",
                    ),
                    count(
                        &st,
                        "SELECT COUNT(*) FROM forgotten_refs WHERE ref IN ('n-card', 'n-derived')",
                    ),
                );
                (out, left)
            })
        })
        .await;
    assert_eq!(
        counts,
        (1, 2),
        "one item (the other was already gone), the card and what was derived from it"
    );
    assert_eq!(left, (0, 1, 0, 0, 1, 2));
    d.close().await;
}
