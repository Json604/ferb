//! ORCH-756 (ORCH-719 item 3's weekly guard): `ferb stats` flags a worker run of the last 7 days that said not done
//! while a tool it had listed by line only, never called, fits its task's targets (a site: the browser tools; an
//! app: the Mac tools; a path: the file tools). Code only; invented records on a temp store (TEST-508).

use ferb_py::time::DateTime;
use ferbd::stats::{stats_in, Now};
use serde_json::{json, Value};

fn now() -> Now {
    Now {
        utc: DateTime::from_isoformat("2026-11-28T12:00:00+00:00").unwrap(),
        local: DateTime::from_isoformat("2026-11-28T17:30:00").unwrap(),
    }
}

/// A task (its own tree) with these targets and one worker run: what it listed by line, its answer's status and
/// the tools it called.
fn run(
    c: &rusqlite::Connection,
    task: &str,
    at: &str,
    targets: Value,
    listed: &[&str],
    status: &str,
    called: &[&str],
) {
    c.execute(
        "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, created_at, \
         updated_at) VALUES (?1, ?1, NULL, ?1, 'g', 'owner', 'worker', '[]', 'failed', ?2, ?2)",
        rusqlite::params![task, at],
    )
    .unwrap();
    c.execute(
        "INSERT INTO task_context (task_id, conversation_id, spec_json, owner_words) VALUES (?1, 'owner', ?2, 'w')",
        rusqlite::params![task, json!({"targets": targets}).to_string()],
    )
    .unwrap();
    let w = format!("{task}-w");
    c.execute(
        "INSERT INTO spans (id, task_id, kind, started_at, input_json, output_json) VALUES (?1, ?2, 'worker', ?3, ?4, ?5)",
        rusqlite::params![
            w,
            task,
            at,
            json!({"role": "worker", "tool_core": {"kind": "worker", "full": ["tools.find"], "listed": listed,
                "similar_tasks": 0}})
            .to_string(),
            json!({"status": status, "result": "{}"}).to_string()
        ],
    )
    .unwrap();
    for (i, t) in called.iter().enumerate() {
        c.execute(
            "INSERT INTO spans (id, parent_id, task_id, kind, started_at, input_json) VALUES (?1, ?2, ?3, 'tool.call', ?4, ?5)",
            rusqlite::params![format!("{w}-{i}"), w, task, at, json!({"tool": t}).to_string()],
        )
        .unwrap();
    }
}

fn stats(home: &std::path::Path) -> String {
    let mut out = Vec::new();
    assert_eq!(stats_in(home, &mut out, 7, None, now()), 0);
    String::from_utf8(out).unwrap()
}

// decision: ORCH-719
// decision: ORCH-756
#[test]
fn a_run_that_said_not_done_with_a_fitting_listed_tool_never_called_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let db = ferb_store::Db::open(dir.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let c = db.conn();
    c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    // nothing recorded: the section isn't printed
    assert!(!stats(dir.path()).contains("weekly guard"));
    let day = "2026-11-27T10:00:00+05:30";
    let listed = [
        "browser.pdf",
        "browser.downloads",
        "mac.applescript",
        "fs.copy",
        "audio.listen",
    ];
    // a site task that said not done and never called browser.pdf or browser.downloads: flagged with those two
    run(
        c,
        "t-site",
        day,
        json!({"sites": ["shop.example.com"]}),
        &listed,
        "not_done",
        &["browser.downloads"],
    );
    // done: not flagged
    run(
        c,
        "t-done",
        day,
        json!({"sites": ["shop.example.com"]}),
        &listed,
        "done",
        &[],
    );
    // not done, but its target (a person) fits none of the families: not flagged
    run(
        c,
        "t-person",
        day,
        json!({"recipients": ["Sam"]}),
        &listed,
        "not_done",
        &[],
    );
    // an app task that called the listed Mac tool: nothing left to flag
    run(
        c,
        "t-app",
        day,
        json!({"apps": ["Notes"]}),
        &listed,
        "not_done",
        &["mac.applescript"],
    );
    // older than the week: not read
    run(
        c,
        "t-old",
        "2026-11-10T10:00:00+05:30",
        json!({"paths": ["~/a"]}),
        &listed,
        "not_done",
        &[],
    );
    let text = stats(dir.path());
    let section: Vec<&str> = text
        .lines()
        .skip_while(|l| !l.contains("weekly guard"))
        .take_while(|l| l.contains("weekly guard") || l.starts_with("  ferb trace"))
        .collect();
    assert_eq!(
        section,
        [
            "Tools listed by line (ORCH-719 weekly guard, last 7 days): 4 worker run(s), 3 said not done, 1 of them \
             with a fitting listed tool never called",
            "  ferb trace t-site — site: browser.pdf"
        ],
        "{text}"
    );
}
