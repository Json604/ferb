//! `setup --cleanup` composed (ferb/store/cleanup.py with its episode-card side over ferb-learning): the preview a
//! temp store gives, a refused OK that removes nothing, and the run the right OK starts, journaled. ferb-store's plan
//! and apply are fixture-checked on Python's recordings with a fake card side (rust/fixtures/store), the card side's
//! plan and pruning in ferb-learning (rust/fixtures/learning); this checks they are joined.

use serde_json::Value;

#[test]
fn the_cleanup_previews_refuses_a_stale_ok_and_runs_with_the_right_one() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let path = home.join("ferb.db");
    {
        let db = ferb_store::Db::open(&path).unwrap();
        db.migrate().unwrap();
        // one old rollup node the clean-up removes, and one episode card it keeps
        for (id, kind, key, made_by) in [
            ("roll-1", "rollup", "day-1", "code:rollup/1"),
            ("ep-1", "episode", "task-1", "code:episode/1"),
        ] {
            db.conn()
                .execute(
                    "INSERT INTO learning_nodes (id, kind, key, body_json, origin, made_by, recorded_at, \
                     valid_from) VALUES (?, ?, ?, '{}', 'ferb', ?, '2026-01-01T09:00:00+00:00', \
                     '2026-01-01T09:00:00+00:00')",
                    [id, kind, key, made_by],
                )
                .unwrap();
        }
    }
    let plan = ferbd::cleanup_cards::preview(&path);
    assert!(plan.get("error").is_none(), "{plan}");
    let ok = plan["ok"].as_str().unwrap().to_string();
    assert!(plan["rows"].as_i64().unwrap() >= 1, "{plan}");
    assert!(
        plan.get("cards").is_none(),
        "the app's preview leaves out the card plan"
    );

    let said = std::cell::RefCell::new(Vec::<String>::new());
    let stale = ferbd::cleanup_cards::apply(&home, "0000000000000000", &|| false, &mut |l| {
        said.borrow_mut().push(l.to_string())
    })
    .unwrap();
    assert_eq!(stale["refused"], true, "{stale}");
    let left: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM learning_nodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 2, "a refused OK removes nothing");

    let done = ferbd::cleanup_cards::apply(&home, &ok, &|| false, &mut |l| {
        said.borrow_mut().push(l.to_string())
    })
    .unwrap();
    assert!(done.get("refused").is_none(), "{done}");
    let conn = rusqlite::Connection::open(&path).unwrap();
    let kinds: Vec<String> = conn
        .prepare("SELECT kind FROM learning_nodes ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(kinds, ["episode"], "the rollup went, the card stayed");
    let journaled: Vec<String> = conn
        .prepare("SELECT kind FROM journal WHERE kind = 'store.cleaned'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!journaled.is_empty(), "the run is journaled: {done}");
    assert!(!said.borrow().is_empty());
    let _: Value = done;
}
