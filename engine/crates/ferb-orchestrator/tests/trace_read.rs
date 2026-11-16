//! ORCH-663: task.trace's reading of a long running task stays within its bound and keeps what a progress answer
//! needs: every send and submit counted, the latest ones and the latest calls listed.

#![cfg(feature = "store")]

mod common;

use common::real_tempdir;
use ferb_orchestrator::trace_read::{trace, TraceArgs};
use ferb_store::Db;
use serde_json::json;

// decision: ORCH-663
#[test]
fn a_long_task_s_trace_stays_bounded_and_keeps_the_counts_and_latest_calls() {
    let (_dir, real) = real_tempdir();
    let db = Db::open(real.join("ferb.db")).unwrap();
    db.migrate().unwrap();
    db.conn()
        .execute(
            "INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, outcome_json, \
             created_at, updated_at) VALUES ('t', 't', 'r', 'g', 'owner', 'worker', '[]', 'running', '{}', \
             '2026-01-01T09:00:00+00:00', '2026-01-01T09:00:00+00:00')",
            [],
        )
        .unwrap();
    let long = "x".repeat(180);
    for n in 0..400 {
        let send = n % 10 == 0;
        let input = if send {
            json!({"tool": "browser.act", "kind": "click", "declared": "send",
                   "target": {"site": "a.example", "page": format!("https://a.example/p/{n}"), "recipients": ["Office"]},
                   "why": format!("Submit solution {n}. {long}")})
        } else {
            json!({"tool": "browser.snapshot", "kind": "read", "target": {"site": "a.example"}, "why": long})
        };
        db.conn()
            .execute(
                "INSERT INTO spans (id, task_id, kind, started_at, ended_at, input_json, output_json) \
                 VALUES (?, 't.worker', 'tool.call', ?, ?, ?, '{\"ok\": true}')",
                rusqlite::params![
                    format!("c{n:03}"),
                    format!("2026-01-01T09:{:02}:{:02}+00:00", n / 60, n % 60),
                    format!("2026-01-01T09:{:02}:{:02}+00:00", n / 60, n % 60),
                    ferb_py::json::dumps(&input)
                ],
            )
            .unwrap();
    }
    for max_chars in [4000, 12000] {
        let v = trace(
            &db,
            "t",
            &TraceArgs {
                max_chars,
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        let text = ferb_py::json::dumps_utf8(&v);
        assert!(
            text.chars().count() <= max_chars,
            "{max_chars}: {}",
            text.chars().count()
        );
        assert_eq!(v["sent_or_submitted"]["count"], 40);
        assert_eq!(v["did"]["calls"], 400);
        let shown = v["calls"]["shown"].as_array().unwrap();
        assert_eq!(
            shown.last().unwrap()["span_id"],
            "c399",
            "the latest call stays"
        );
        let each = v["sent_or_submitted"]["each"].as_array().unwrap();
        if let Some(last) = each.last() {
            assert_eq!(last["span_id"], "c390", "the latest send stays listed");
        }
        assert!(v["calls"]["earlier"].is_string());
    }
}
