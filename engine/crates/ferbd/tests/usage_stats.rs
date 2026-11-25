//! ORCH-720: `ferb stats`' model-use section on a temp store of invented spans: per role and runtime, calls (Codex
//! per record, Claude by the relay's request count or estimated with `~`), steps, tokens per call and per step, the
//! cached share, cache writes by lifetime, the fixed part, records counted at a run's end, per day and over a chosen
//! range; nothing is printed when no usage was recorded (the Python-recorded cases keep their bytes).

use ferb_py::time::DateTime;
use ferbd::stats::{stats_in, Now};
use serde_json::json;

fn now() -> Now {
    Now {
        utc: DateTime::from_isoformat("2026-11-25T12:00:00+00:00").unwrap(),
        local: DateTime::from_isoformat("2026-11-25T17:30:00").unwrap(),
    }
}

#[allow(clippy::too_many_arguments)]
fn span(
    c: &rusqlite::Connection,
    id: &str,
    parent: Option<&str>,
    task: Option<&str>,
    kind: &str,
    at: &str,
    input: serde_json::Value,
    output: serde_json::Value,
) {
    c.execute(
        "INSERT INTO spans (id, parent_id, task_id, kind, started_at, ended_at, input_json, output_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7)",
        rusqlite::params![id, parent, task, kind, at, input.to_string(), output.to_string()],
    )
    .unwrap();
}

fn usage(input: i64, cached: i64, w5: i64, w1: i64, output: i64) -> serde_json::Value {
    json!({"model": "m", "input_tokens": input, "cached_input_tokens": cached, "cache_write_tokens": w5 + w1,
        "output_tokens": output, "cache_write_5m_tokens": w5, "cache_write_1h_tokens": w1})
}

fn home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let db = ferb_store::Db::open(dir.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let c = db.conn();
    c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    let day1 = "2026-11-24T10:00:00+05:30";
    let day2 = "2026-11-25T10:00:00+05:30";
    let system = "s".repeat(1_000);
    // a planner turn on Claude: an older record with no request count, one lookup
    span(
        c,
        "turn1",
        None,
        None,
        "model.turn",
        day1,
        json!({"role": "supervisor", "request_id": "r1", "system_prompt": system}),
        json!({}),
    );
    span(
        c,
        "u1",
        Some("turn1"),
        None,
        "model.usage",
        day1,
        json!({"runtime": "claude"}),
        usage(40_000, 30_000, 0, 10_000, 100),
    );
    span(
        c,
        "t1",
        Some("turn1"),
        None,
        "tool.call",
        day1,
        json!({"tool": "about.read"}),
        json!({}),
    );
    // a worker run on Claude with the relay's count, three steps, and what it reported at its end
    for (id, task, request) in [
        ("task-w", "task-w", "r1"),
        ("task-s", "task-s", "r-sched"),
        ("task-t", "task-t", "temporary-x"),
    ] {
        c.execute(
            "INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, updated_at) \
             VALUES (?1, ?1, ?2, 'g', 'owner', 'worker', '[]', 'done', ?3, ?3)",
            rusqlite::params![id, request, day2],
        )
        .unwrap();
        let _ = task;
    }
    span(
        c,
        "w1",
        None,
        Some("task-w"),
        "worker",
        day2,
        json!({"role": "worker", "system_prompt": "w".repeat(500)}),
        json!({}),
    );
    span(
        c,
        "u2",
        Some("w1"),
        Some("task-w"),
        "model.usage",
        day2,
        json!({"runtime": "claude"}),
        {
            let mut u = usage(300_000, 270_000, 0, 20_000, 500);
            u["requests"] = json!(4);
            u
        },
    );
    span(
        c,
        "u3",
        Some("w1"),
        Some("task-w"),
        "model.usage",
        day2,
        json!({"runtime": "claude"}),
        {
            let mut u = usage(100_000, 90_000, 0, 0, 10);
            u["requests"] = json!(1);
            u["recorded_at_end"] = json!(true);
            u
        },
    );
    for i in 0..3 {
        span(
            c,
            &format!("t-w{i}"),
            Some("w1"),
            Some("task-w"),
            "tool.call",
            day2,
            json!({}),
            json!({}),
        );
    }
    span(
        c,
        "tools-w1",
        Some("w1"),
        Some("task-w"),
        "runtime.tools",
        day2,
        json!({"runtime": "claude"}),
        json!({"tools_seen": ["a", "b", "c"]}),
    );
    // a checker on Codex: two calls
    span(
        c,
        "v1",
        None,
        Some("task-w"),
        "worker",
        day2,
        json!({"role": "verifier"}),
        json!({}),
    );
    for i in 0..2 {
        span(
            c,
            &format!("u-v{i}"),
            Some("v1"),
            Some("task-w"),
            "model.usage",
            day2,
            json!({"runtime": "codex"}),
            usage(20_000, 10_000, 0, 0, 5),
        );
    }
    // a scheduled run's worker and a temporary chat's worker, by their requests
    c.execute(
        "INSERT INTO journal (id, at, kind, data_json) VALUES ('j1', ?1, 'schedule.run', ?2)",
        rusqlite::params![day2, json!({"request_id": "r-sched"}).to_string()],
    )
    .unwrap();
    span(
        c,
        "ws",
        None,
        Some("task-s"),
        "worker",
        day2,
        json!({"role": "worker"}),
        json!({}),
    );
    span(
        c,
        "u-s",
        Some("ws"),
        Some("task-s"),
        "model.usage",
        day2,
        json!({"runtime": "codex"}),
        usage(1_000, 0, 0, 0, 1),
    );
    span(
        c,
        "wt",
        None,
        Some("task-t"),
        "worker",
        day2,
        json!({"role": "worker"}),
        json!({}),
    );
    span(
        c,
        "u-t",
        Some("wt"),
        Some("task-t"),
        "model.usage",
        day2,
        json!({"runtime": "codex"}),
        usage(2_000, 0, 0, 0, 1),
    );
    // a learning pass: no run of its own
    span(
        c,
        "u-l",
        None,
        None,
        "model.usage",
        day2,
        json!({"runtime": "claude", "role": "profile"}),
        usage(10_000, 0, 9_000, 1_000, 50),
    );
    db.close().unwrap();
    dir
}

fn section(text: &str) -> String {
    text.split_once("Model use by role")
        .map(|(_, s)| format!("Model use by role{s}"))
        .unwrap_or_default()
}

// decision: ORCH-720
#[test]
fn the_model_use_section_by_role_runtime_and_day() {
    let dir = home();
    let mut out = Vec::new();
    assert_eq!(stats_in(dir.path(), &mut out, 7, None, now()), 0);
    let got = section(&String::from_utf8(out).unwrap());
    let want = "\
Model use by role and runtime in the last 7 day(s) (the providers' own token counts):
  planner, claude: 1 runs, ~2 calls, 1 steps; input 40,000 (20,000 per call, 40,000 per step); cached 75%; cache written 5m 0, 1h 10,000; output 100; fixed part: instructions 1,000 chars
  worker, claude: 1 runs, 5 calls, 3 steps; input 400,000 (80,000 per call, 133,333 per step); cached 90%; cache written 5m 0, 1h 20,000; output 510; fixed part: instructions 500 chars, 3 tools seen
  checker, codex: 1 runs, 2 calls, 0 steps; input 40,000 (20,000 per call, - per step); cached 50%; cache written 5m 0, 1h 0; output 10
  scheduled, codex: 1 runs, 1 calls, 0 steps; input 1,000 (1,000 per call, - per step); cached 0%; cache written 5m 0, 1h 0; output 1
  temporary, codex: 1 runs, 1 calls, 0 steps; input 2,000 (2,000 per call, - per step); cached 0%; cache written 5m 0, 1h 0; output 1
  memory/learning, claude: 1 runs, 1 calls, 0 steps; input 10,000 (10,000 per call, - per step); cached 0%; cache written 5m 9,000, 1h 1,000; output 50
  all: input 493,000
  ~ estimated: an older Claude record holds one total per turn, counted as its run's tool calls plus one
  1 records counted at a run's end (it stopped before its runtime reported them)
Model use by day:
  2026-11-24 planner, claude: ~2 calls, input 40,000 (20,000 per call), cached 75%, output 100
  2026-11-25 worker, claude: 5 calls, input 400,000 (80,000 per call), cached 90%, output 510
  2026-11-25 checker, codex: 2 calls, input 40,000 (20,000 per call), cached 50%, output 10
  2026-11-25 scheduled, codex: 1 calls, input 1,000 (1,000 per call), cached 0%, output 1
  2026-11-25 temporary, codex: 1 calls, input 2,000 (2,000 per call), cached 0%, output 1
  2026-11-25 memory/learning, claude: 1 calls, input 10,000 (10,000 per call), cached 0%, output 50
";
    assert_eq!(got, want, "\n{got}");
}

// decision: ORCH-720
#[test]
fn a_chosen_range_reads_only_its_days_and_an_empty_one_prints_nothing() {
    let dir = home();
    let mut out = Vec::new();
    let range = Some(("2026-11-24".to_string(), "2026-11-24".to_string()));
    assert_eq!(stats_in(dir.path(), &mut out, 7, range, now()), 0);
    let got = section(&String::from_utf8(out).unwrap());
    assert!(
        got.starts_with("Model use by role and runtime on 2026-11-24"),
        "{got}"
    );
    assert!(
        got.contains("planner, claude") && !got.contains("worker, claude"),
        "{got}"
    );
    let mut out = Vec::new();
    let range = Some(("2026-10-01".to_string(), "2026-10-31".to_string()));
    assert_eq!(stats_in(dir.path(), &mut out, 7, range, now()), 0);
    assert_eq!(
        section(&String::from_utf8(out).unwrap()),
        "",
        "no usage, no section"
    );
}
