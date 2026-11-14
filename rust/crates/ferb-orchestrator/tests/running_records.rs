//! ORCH-640: what `task.evidence` gives the planner about a task that is still running, as the owner's real case
//! stood when he asked "whats the list so far???" (a research task: its first worker run had reported a list the
//! check sent back, its second had just reported, its checker was running). Read with the task's id, as the planner
//! reads one of its chat's tasks: the worker runs' reports (the one still being checked too) and the tool calls'
//! results are there, each with the origins the sink rule counts; a run that hasn't ended is listed without a result.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_orchestrator::evidence::{EvidenceArgs, TaskEvidence};
use ferb_store::spans::{Finish, SpanStore};
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
        format!("span-{:03}", self.0.get())
    }
}

const TREE: &str = "t1";
const FIRST: &str = "| Instinct | personal assistant | instinct.example |";
const SECOND: &str = "| Instinct | personal assistant | instinct.example |\n| Pally | text assistant | pally.example |";

fn store(db: &Db) -> SpanStore<'_> {
    SpanStore::new(
        db,
        Box::new(Clock(Cell::new(0))),
        Box::new(Ids(Cell::new(0))),
        Box::new(|v: &Value| v.clone()),
        Box::new(|_| false),
    )
}

/// The tree at the moment of the question: (the span ids of the two worker runs, the checker's run).
fn the_running_task(spans: &SpanStore<'_>) -> (String, String, String) {
    // the tree's own record: it ends with the task, not yet
    spans
        .start(
            "task",
            &json!({"goal": "List personal AI agents"}),
            Some(TREE),
            None,
            None,
        )
        .unwrap();
    let worker = |n: usize, answer: Option<&str>| {
        let mut run = spans
            .start(
                "worker",
                &json!({"run": n}),
                Some(&format!("{TREE}.worker")),
                None,
                None,
            )
            .unwrap();
        for page in ["agents.example", "instinct.example"] {
            let mut call = spans
                .start(
                    "tool.call",
                    &json!({"tool": "browser.read", "args": {"url": format!("https://{page}/")}}),
                    Some(&format!("{TREE}.worker")),
                    Some(&run.id),
                    None,
                )
                .unwrap();
            spans
                .finish(
                    &mut call,
                    Finish {
                        output: Some(
                            &json!({"text": format!("Agents listed on {page}: Instinct, Pally")}),
                        ),
                        origin: Some(&format!("external:web:{page}")),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        if let Some(answer) = answer {
            let report = json!({"result": ferb_py::json::dumps(&json!({"status": "done", "answer": answer}))});
            spans
                .finish(
                    &mut run,
                    Finish {
                        output: Some(&report),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        run.id
    };
    let first = worker(1, Some(FIRST));
    let second = worker(2, Some(SECOND));
    // the checker of the second report is still running: its run has no result yet
    let checker = spans
        .start(
            "worker",
            &json!({"run": "check"}),
            Some(&format!("{TREE}.worker.verify")),
            None,
            None,
        )
        .unwrap();
    (first, second, checker.id)
}

fn reader<'a>(spans: &'a SpanStore<'a>) -> TaskEvidence<'a> {
    TaskEvidence {
        spans,
        parent: Arc::new(|_| None),
        // the planner's lookup record has no task row: no root of its own
        root: Arc::new(|_| None),
        captures: None,
        secrets: Arc::new(Vec::new),
        // the running task is one of the chat's tasks (its task_context row is written when it starts)
        readable: Arc::new(|_| [TREE.to_string()].into()),
        record_sources: Arc::new(|_| BTreeSet::new()),
    }
}

// decision: ORCH-640
#[test]
fn a_running_tasks_reports_and_results_are_read_with_their_origins() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let spans = store(&db);
    let (first, second, checker) = the_running_task(&spans);
    let ev = reader(&spans);
    let planner = "lookup-1";

    // kind worker: every worker run of the tree, the checker's too; the running one without a result
    let runs = ev
        .run(
            &EvidenceArgs {
                kind: "worker".into(),
                task_id: Some(TREE.into()),
                ..Default::default()
            },
            planner,
        )
        .unwrap();
    assert!(!runs.is_error, "{}", runs.content);
    let listed: Vec<(String, bool)> = runs.content["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["id"].as_str().unwrap().to_string(),
                r["has_output"] == json!(true),
            )
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            (first.clone(), true),
            (second.clone(), true),
            (checker, false)
        ]
    );

    // the latest report, still being checked, read whole: the list so far
    let latest = ev
        .run(
            &EvidenceArgs {
                span_id: Some(second),
                task_id: Some(TREE.into()),
                ..Default::default()
            },
            planner,
        )
        .unwrap();
    let text = latest.content["text"].as_str().unwrap();
    assert!(text.contains("Pally | text assistant"), "{text}");
    // a worker's report carries the task's origins: what it read from the web counts for the sink rule
    let origins: BTreeSet<String> = latest.read_sources.iter().map(|(_, o)| o.clone()).collect();
    assert!(origins.contains("inferred"), "{origins:?}");
    assert!(
        origins.iter().any(|o| o.starts_with("external:web")),
        "the report inherits what the task read: {origins:?}"
    );

    // the tool results: what the pages said, labelled with each page's origin
    let calls = ev
        .run(
            &EvidenceArgs {
                kind: "tool.call".into(),
                task_id: Some(TREE.into()),
                ..Default::default()
            },
            planner,
        )
        .unwrap();
    let records = calls.content["records"].as_array().unwrap();
    assert_eq!(records.len(), 4);
    assert!(records.iter().all(|r| r["has_output"] == json!(true)));
    let read: BTreeSet<String> = calls.read_sources.iter().map(|(_, o)| o.clone()).collect();
    assert!(read.contains("external:web:instinct.example"), "{read:?}");
}
