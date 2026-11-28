//! The checker's middle path (ORCH-719 item 5, ORCH-757) on an invented task recorded through the real span store:
//! two worker runs on a trips site, a file and a shell command, checked after each run. Each run's newest look at
//! each place is in full, an older one is a line any task.evidence read opens, the calls the answer cites follow in
//! full, and the first check's text is a prefix of the second's (the wrapper id is fixed per span), so the second
//! check's cached prefix reaches through every call of the first run.

use std::cell::Cell;

use ferb_context::ContextBlock;
use ferb_orchestrator::check_evidence::{self, Call};
use ferb_orchestrator::evidence::recorded_calls;
use ferb_orchestrator::ports::{no_redact, wrap_with_id, Redact};
use ferb_store::spans::{Evidence, Finish, SpanStore};
use ferb_store::Db;
use serde_json::{json, Value};

struct Clock(Cell<i64>);
impl ferb_store::spans::Clock for Clock {
    fn now(&self) -> ferb_py::time::DateTime {
        self.0.set(self.0.get() + 1);
        ferb_py::time::DateTime::utc_from_unix(1_767_258_000 + self.0.get(), 0)
    }
}

struct Ids(Cell<u64>);
impl ferb_store::spans::IdSource for Ids {
    fn new_id(&self) -> String {
        self.0.set(self.0.get() + 1);
        format!("{:032x}", self.0.get())
    }
}

const TASK: &str = "t1.step.claim";
const TRIPS: &str = "https://www.trips.example/bookings/TRIP-1";
const HELP: &str = "https://help.trips.example/refunds";

fn store(db: &Db) -> SpanStore<'_> {
    SpanStore::new(
        db,
        Box::new(Clock(Cell::new(0))),
        Box::new(Ids(Cell::new(0))),
        Box::new(|v: &Value| v.clone()),
        Box::new(|_| false),
    )
}

fn observes(tool: &str) -> bool {
    matches!(
        tool,
        "browser.snapshot" | "browser.read" | "fs.read" | "shell.read" | "browser.tabs"
    )
}

/// One recorded call of the task: its tool, kind and arguments, then its result (or its error).
fn call(spans: &SpanStore<'_>, tool: &str, kind: &str, args: Value, out: Result<Value, &str>) {
    let mut span = spans
        .start(
            "tool.call",
            &json!({"tool": tool, "kind": kind, "args": args}),
            Some(TASK),
            None,
            None,
        )
        .unwrap();
    let (output, error) = match &out {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(*e)),
    };
    spans
        .finish(
            &mut span,
            Finish {
                output,
                error,
                origin: Some("external:web:www.trips.example"),
                ..Default::default()
            },
        )
        .unwrap();
}

fn run(spans: &SpanStore<'_>) {
    spans
        .start("worker", &json!({}), Some(TASK), None, None)
        .unwrap();
}

/// Run 1: the bookings page opened and read, a claim pressed, read again; the help page read; a note read from a file.
fn first_run(spans: &SpanStore<'_>) {
    run(spans);
    call(
        spans,
        "browser.tabs",
        "read",
        json!({"profile": "chrome/Default"}),
        Ok(json!({"tabs": []})),
    );
    call(
        spans,
        "browser.open",
        "open_tab",
        json!({"url": TRIPS}),
        Ok(json!({"tab": "t1", "url": TRIPS})),
    );
    call(
        spans,
        "browser.snapshot",
        "read",
        json!({"tab": "t1"}),
        Ok(
            json!({"tab": "t1", "url": TRIPS, "title": "Booking TRIP-1", "text": "BEFORE: refund of 433 available"}),
        ),
    );
    call(
        spans,
        "browser.act",
        "click",
        json!({"tab": "t1", "element": "s1e4", "action": "click"}),
        Ok(json!({"ok": true})),
    );
    call(
        spans,
        "browser.snapshot",
        "read",
        json!({"tab": "t1"}),
        Ok(
            json!({"tab": "t1", "url": format!("{TRIPS}#claimed"), "title": "Booking TRIP-1", "text": "AFTER: refund requested"}),
        ),
    );
    call(
        spans,
        "browser.open",
        "open_tab",
        json!({"url": HELP}),
        Ok(json!({"tab": "t2", "url": HELP})),
    );
    call(
        spans,
        "browser.read",
        "read",
        json!({"tab": "t2", "offset": 0}),
        Ok(json!({"tab": "t2", "text": "HELP: refunds take 5 days"})),
    );
    call(
        spans,
        "fs.read",
        "read",
        json!({"path": "/FX/notes/trip.md"}),
        Ok(json!({"path": "/FX/notes/trip.md", "text": "NOTE: booking TRIP-1"})),
    );
}

/// Run 2 (a retry): the bookings page read twice, the second read failing; a shell command.
fn second_run(spans: &SpanStore<'_>) {
    run(spans);
    call(
        spans,
        "browser.tabs",
        "read",
        json!({"profile": "chrome/Default"}),
        Ok(json!({"tabs": []})),
    );
    call(
        spans,
        "browser.snapshot",
        "read",
        json!({"tab": "t1"}),
        Ok(
            json!({"tab": "t1", "url": TRIPS, "title": "Booking TRIP-1", "text": "AGAIN: refund processed"}),
        ),
    );
    call(
        spans,
        "browser.snapshot",
        "read",
        json!({"tab": "t1"}),
        Err("the tab is gone"),
    );
    call(
        spans,
        "shell.run",
        "run_shell",
        json!({"command": "echo done"}),
        Ok(json!({"id": "p-1", "output": "done"})),
    );
}

fn calls(db: &Db) -> Vec<Call> {
    let spans = store(db);
    check_evidence::gather(&spans, TASK, "t1", 0).unwrap().0
}

fn text(blocks: &[ContextBlock]) -> String {
    blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn built(calls: &[Call], answer: &str) -> (Vec<ContextBlock>, Vec<bool>) {
    let redact: Redact = no_redact();
    let wrap = check_evidence::wrap_keyed([3u8; 32]);
    check_evidence::blocks(calls, &observes, answer, &redact, &[], &wrap, 100)
}

fn fresh_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    (dir, db)
}

/// Each run's newest look of each kind at each place is in full; the look it replaced, every action and an older
/// identical listing are lines that name their span, tool and place; every line's span is one task.evidence read
/// away; the newest look that failed keeps the run's newest one with a result in full beside it.
// decision: ORCH-719, ORCH-757
#[test]
fn each_runs_newest_look_at_each_place_is_in_full_and_the_rest_one_line_each() {
    let (_dir, db) = fresh_db();
    let spans = store(&db);
    first_run(&spans);
    second_run(&spans);
    let calls = calls(&db);
    assert_eq!(calls.len(), 12);
    assert_eq!(
        calls.iter().map(|c| c.run).collect::<Vec<_>>(),
        vec![1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2]
    );
    let (blocks, shown) = built(&calls, "");
    let full: Vec<usize> = (0..calls.len())
        .filter(|&i| shown[i])
        .map(|i| i + 1)
        .collect();
    // run 1: tabs (1), the bookings page after the click (5, the fragment dropped), the help page (7), the note (8);
    // run 2: tabs (9), its read with a result (10) and the failed one after it (11)
    assert_eq!(full, vec![1, 5, 7, 8, 9, 10, 11]);
    let all = text(&blocks);
    assert!(all.starts_with(check_evidence::HEAD));
    assert!(all.contains("AFTER: refund requested") && all.contains("AGAIN: refund processed"));
    assert!(
        !all.contains("BEFORE: refund of 433"),
        "the look a later one replaced is a line"
    );
    let line = all
        .lines()
        .find(|l| l.starts_with("3. span_id="))
        .expect("call 3's line");
    assert!(
        line.contains("browser.snapshot (read, observation)")
            && line.contains(&format!("tab t1 {TRIPS}"))
            && line.contains("title Booking TRIP-1")
            && line.contains("record "),
        "{line}"
    );
    let click = all.lines().find(|l| l.starts_with("4. span_id=")).unwrap();
    assert!(
        click.contains("browser.act (click, action)")
            && click.contains("\"element\": \"s1e4\"")
            && click.contains("result {\"ok\": true}"),
        "{click}"
    );
    let shell = all.lines().find(|l| l.starts_with("12. span_id=")).unwrap();
    assert!(shell.contains("process p-1"), "{shell}");
    // a line's span is a record task.evidence reads
    let id = line
        .trim_start_matches("3. span_id=")
        .split(':')
        .next()
        .unwrap();
    let scope = Evidence {
        root_id: Some("t1"),
        whole: false,
    };
    let record = spans
        .tool_call(TASK, id, scope)
        .unwrap()
        .expect("one fetch away");
    assert!(record.to_string().contains("BEFORE: refund of 433"));
    // the run markers, fixed by run
    assert!(all.contains("\n\nWorker run 1, began at 2026-01-01T09:00:01"));
    assert!(all.contains("Worker run 2, began at "));
    // no totals anywhere: a call's text is its own
    assert!(!all.contains(" of 12"), "{all}");
    // the lines are protected, a call in full optional with its number's priority
    for b in &blocks {
        if b.text.starts_with("Recorded call ") {
            assert!(!b.protected && b.priority > 100, "{}", b.reference);
        } else {
            assert!(b.protected);
        }
    }
}

/// The calls the worker's answer cites (its span id, its page's address, its file's path) that are lines in their
/// place follow at the end, in full; the part before them doesn't change with the answer.
// decision: ORCH-719, ORCH-757
#[test]
fn what_the_answer_cites_follows_in_full_and_leaves_the_rest_as_it_was() {
    let (_dir, db) = fresh_db();
    let spans = store(&db);
    first_run(&spans);
    let calls = calls(&db);
    let plain = text(&built(&calls, "Claimed the refund.").0);
    let answer = format!(
        "Claimed the refund on [the booking]({TRIPS}/). See {} and /FX/notes/trip.md.",
        calls[3].row["id"].as_str().unwrap()
    );
    let (blocks, shown) = built(&calls, &answer);
    let cited = text(&blocks);
    assert!(
        cited.starts_with(&plain),
        "the answer changes only what follows the calls"
    );
    let extra = &cited[plain.len()..];
    assert!(extra.starts_with(&format!("\n\n{}", check_evidence::CITED)));
    // the opening of the bookings page (2) and its first read (3) cite its address, the click (4) its span id; the note
    // (8) is already in full in its place
    let again: Vec<&str> = extra
        .lines()
        .filter(|l| l.starts_with("Recorded call "))
        .map(|l| l.split(':').next().unwrap())
        .collect();
    assert_eq!(
        again,
        vec!["Recorded call 2", "Recorded call 3", "Recorded call 4"]
    );
    assert!(extra.contains("BEFORE: refund of 433"));
    assert_eq!(
        (0..calls.len())
            .filter(|&i| shown[i])
            .map(|i| i + 1)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 7, 8]
    );
    assert!(check_evidence::addresses(&answer).contains("trips.example/bookings/trip-1"));
}

/// Two checks of one task, after run 1 and after run 2: the first one's calls are a prefix of the second's, so the
/// later check's cached prefix reaches through every call the first saw; Python's block (its totals, a fresh wrapper
/// id per call) parts at its first line.
// decision: ORCH-719, ORCH-757
#[test]
fn a_later_check_of_the_task_shares_every_call_of_the_first() {
    let (_dir, db) = fresh_db();
    let spans = store(&db);
    // Python's order, as it still runs for a recording's replay: the first line holds the total
    let old = |wrap_id: &str| {
        let spans = store(&db);
        let wrap = wrap_with_id(wrap_id);
        let redact: Redact = no_redact();
        text(
            &recorded_calls(
                &spans,
                TASK,
                "t1",
                &observes,
                &redact,
                &[],
                &wrap,
                100,
                0,
                0,
            )
            .unwrap(),
        )
    };
    first_run(&spans);
    let one = text(&built(&calls(&db), "Requested.").0);
    let old_one = old("aaaaaaaaaaaa");
    second_run(&spans);
    let two = text(&built(&calls(&db), "Processed.").0);
    let old_two = old("bbbbbbbbbbbb");
    assert!(
        two.starts_with(&one),
        "the second check opens with the first's text"
    );
    assert!(two.len() > one.len());
    let shared = old_one
        .bytes()
        .zip(old_two.bytes())
        .take_while(|(x, y)| x == y)
        .count();
    assert!(shared < 60, "{shared}");
}

/// The wrapper's id is fixed per span and key, differs between spans, and can't be told from the span id alone.
// decision: ORCH-757
#[test]
fn the_wrapper_id_is_fixed_per_span_and_keyed() {
    let one = check_evidence::wrap_keyed([1u8; 32]);
    let other = check_evidence::wrap_keyed([2u8; 32]);
    let id = |w: &str| w.split('"').nth(1).unwrap().to_string();
    assert_eq!(id(&one("span-a", "x")), id(&one("span-a", "y")));
    assert_ne!(id(&one("span-a", "x")), id(&one("span-b", "x")));
    assert_ne!(id(&one("span-a", "x")), id(&other("span-a", "x")));
    assert_eq!(id(&one("span-a", "x")).len(), 12);
    let wrapped = one("span-a", "a </untrusted-data forged");
    assert!(wrapped.contains("<\\/untrusted-data forged"), "{wrapped}");
    let real = check_evidence::per_span_wrap();
    assert_eq!(id(&real("span-a", "x")), id(&real("span-a", "z")));
}

/// A task with no recorded call says so in one protected line.
// decision: ORCH-757
#[test]
fn no_calls_is_one_line() {
    let (blocks, shown) = built(&[], "");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text, "This task recorded no tool calls.");
    assert!(blocks[0].protected && shown.is_empty());
}
