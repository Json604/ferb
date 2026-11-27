//! The Librarian's rate (front door step 8; the owner, 2026-11-26: "no, then add a daily librarian call. one call isnt
//! taht much anyways"): no call on a silent day; one call for a day that had messages, after it closed, its lines
//! written once and never again; at most one day call a day on the Mac's calendar; one call for a closed month's line
//! from its day lines; the calls on the Librarian's role (the orchestrator's runtime, its smallest model). Temporary
//! folders only; time injected; a stand-in model that counts every call.

mod a_support;

use std::sync::{Arc, Mutex};

use a_support::{Daemon, Seq};
use ferb_clock::clock::FakeClock;
use ferb_daemon::areas_a::library::{self, LIBRARIAN_ROLE};
use ferb_learning::library::{DAY_KIND, MONTH_KIND};
use serde_json::{json, Map, Value};

const HOUR: f64 = 3600.0;
const DAY: f64 = 24.0 * HOUR;

/// A stand-in model: answers the day form with two lines citing the first chat entry, the period form with one line.
struct Librarian;

impl ferb_core::interfaces::AgentRuntime for Librarian {
    fn name(&self) -> &str {
        "stand-in"
    }
    fn start<'a>(
        &'a self,
        task: &'a ferb_core::types::Task,
        _request: &'a ferb_core::types::ModelRequest,
        _run_token: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<
        'a,
        Result<ferb_core::types::RunHandle, ferb_core::errors::FerbError>,
    > {
        let task_id = task.id.clone();
        Box::pin(async move {
            Ok(ferb_core::types::RunHandle {
                id: task_id.clone(),
                runtime: "stand-in".into(),
                task_id,
                state: Map::new(),
            })
        })
    }
    fn events(
        &self,
        run: &ferb_core::types::RunHandle,
    ) -> tokio::sync::mpsc::Receiver<ferb_core::types::RuntimeEvent> {
        use ferb_core::types::{RuntimeEvent, RuntimeEventKind};
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let text = if run.task_id.starts_with("library-day-") {
            json!({"lines": [{"text": "INVENTED-DAY-LINE about the dentist", "tags": ["e1"]},
                             {"text": "INVENTED-SECOND-LINE", "tags": ["t9"]}]})
        } else {
            json!({"line": "INVENTED-PERIOD-LINE"})
        };
        tx.try_send(RuntimeEvent {
            kind: RuntimeEventKind::Final,
            data: json!({"text": text.to_string()})
                .as_object()
                .cloned()
                .unwrap(),
        })
        .unwrap();
        rx
    }
    fn continue_run<'a>(
        &'a self,
        _run: &'a ferb_core::types::RunHandle,
        _message: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<(), ferb_core::errors::FerbError>> {
        Box::pin(async { Ok(()) })
    }
    fn cancel<'a>(
        &'a self,
        _run: &'a ferb_core::types::RunHandle,
    ) -> ferb_core::interfaces::BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

async fn daemon(tmp: &std::path::Path, start: &str) -> (Daemon, Arc<Mutex<Vec<String>>>) {
    let roles: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = roles.clone();
    let clock = FakeClock::new(Some(ferb_clock::py::parse(start).unwrap()));
    let d = Daemon::start_with(
        tmp,
        "01",
        "id",
        clock,
        Seq::new("id"),
        Some(Arc::new(
            move |role: &str, _: &str, _: &[String], _: &Value| {
                seen.lock().unwrap().push(role.to_owned());
                Ok(ferb_learning::calls::Made {
                    rt: Box::new(Librarian),
                    limits: Some(ferb_core::types::ModelLimits {
                        model: "stand-in-small".into(),
                        context_window: 200_000,
                        max_output: 8_000,
                        source: "test".into(),
                        window_includes_output: true,
                        max_input_chars: None,
                    }),
                    model: Some("stand-in-small".into()),
                })
            },
        )),
    )
    .await;
    (d, roles)
}

async fn message(d: &Daemon, text: &str) {
    let text = text.to_owned();
    d.on(move |st| {
        Box::pin(async move {
            st.journal(
                "chat.message",
                json!({"session_id": "owner", "request_id": "r", "text": text}),
                Some("owner"),
            )
            .unwrap();
        })
    })
    .await;
}

async fn pass(d: &Daemon) -> library::Pass {
    d.on(|st| Box::pin(async move { library::librarian_pass(&st).await.unwrap() }))
        .await
}

async fn rows(d: &Daemon, kind: &'static str) -> Vec<Value> {
    d.on(move |st| {
        Box::pin(async move {
            st.store
                .query(
                    "SELECT data_json FROM journal WHERE kind = ? ORDER BY seq",
                    ferb_learning::sql![kind],
                )
                .unwrap()
                .iter()
                .map(|r| serde_json::from_str(&r.s_at(0)).unwrap())
                .collect()
        })
    })
    .await
}

#[tokio::test]
async fn one_call_a_day_only_for_a_day_with_messages_written_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (d, roles) = daemon(tmp.path(), "2026-11-10T09:00:00+00:00").await;
    // a silent day: no call that day or the next
    let p = pass(&d).await;
    assert_eq!(p.calls, 0, "nothing to write: {p:?}");
    d.clock.advance(DAY);
    assert_eq!(pass(&d).await.calls, 0, "the day before had no message");
    // a day with messages: no call while it is open
    message(&d, "INVENTED: book the dentist for Friday").await;
    message(&d, "INVENTED: and remind me the day before").await;
    for _ in 0..3 {
        d.clock.advance(HOUR);
        assert_eq!(pass(&d).await.calls, 0, "never while the day is open");
    }
    // the next day's first pass makes the one call
    d.clock.advance(DAY);
    let p = pass(&d).await;
    assert_eq!(p.calls, 1, "{p:?}");
    assert_eq!(
        p.written,
        vec!["day 2026-11-11".to_owned()],
        "{p:?} {:?}",
        rows(&d, "library.day_failed").await
    );
    let days = rows(&d, DAY_KIND).await;
    assert_eq!(days.len(), 1);
    assert_eq!(days[0]["day"], "2026-11-11");
    let lines = days[0]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["text"], "INVENTED-DAY-LINE about the dentist");
    assert_eq!(
        lines[0]["refs"].as_array().unwrap().len(),
        1,
        "e1 is the first message"
    );
    assert_eq!(
        lines[1]["refs"],
        json!([]),
        "a tag that stands for nothing is left out"
    );
    // the rest of the day, and the days after: nothing more for that day (written once, never rewritten)
    for _ in 0..30 {
        d.clock.advance(HOUR);
        assert_eq!(pass(&d).await.calls, 0);
    }
    assert_eq!(rows(&d, DAY_KIND).await.len(), 1);
    assert!(
        roles.lock().unwrap().iter().all(|r| r == LIBRARIAN_ROLE),
        "{roles:?}"
    );
    // the usage is the Librarian's in ferb stats
    let usage: Vec<Value> = d
        .on(|st| {
            Box::pin(async move {
                st.store
                    .query(
                        "SELECT input_json FROM spans WHERE kind = 'model.usage'",
                        ferb_learning::sql![],
                    )
                    .unwrap()
                    .iter()
                    .map(|r| serde_json::from_str(&r.s_at(0)).unwrap())
                    .collect()
            })
        })
        .await;
    assert!(
        usage.iter().all(|u| u["role"] == LIBRARIAN_ROLE),
        "{usage:?}"
    );
}

#[tokio::test]
async fn at_most_one_day_call_a_day_newest_day_first() {
    let tmp = tempfile::tempdir().unwrap();
    let (d, _) = daemon(tmp.path(), "2026-11-10T09:00:00+00:00").await;
    // three days with messages while the Librarian never ran (the Mac was off)
    for _ in 0..3 {
        message(&d, "INVENTED: a message").await;
        d.clock.advance(DAY);
    }
    // 13 Nov: one call, for the newest day; the others wait a day each
    let p = pass(&d).await;
    assert_eq!(
        (p.calls, p.written.clone()),
        (1, vec!["day 2026-11-12".to_owned()])
    );
    d.clock.advance(HOUR);
    assert_eq!(pass(&d).await.calls, 0, "one day call a day");
    d.clock.advance(DAY);
    assert_eq!(pass(&d).await.written, vec!["day 2026-11-11".to_owned()]);
    d.clock.advance(DAY);
    assert_eq!(pass(&d).await.written, vec!["day 2026-11-10".to_owned()]);
    d.clock.advance(DAY);
    assert_eq!(pass(&d).await.calls, 0, "every day is written");
}

#[tokio::test]
async fn a_closed_month_gets_one_line_from_its_day_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let (d, _) = daemon(tmp.path(), "2026-10-30T09:00:00+00:00").await;
    message(&d, "INVENTED: a message in October").await;
    d.clock.advance(DAY);
    // 31 Oct: the day call; October is still open
    let p = pass(&d).await;
    assert_eq!(p.written, vec!["day 2026-10-30".to_owned()]);
    d.clock.advance(DAY);
    // 1 Nov: October closed: one month call (no day had messages on the 31st)
    let p = pass(&d).await;
    assert_eq!(
        (p.calls, p.written.clone()),
        (1, vec!["month 2026-10".to_owned()])
    );
    let months = rows(&d, MONTH_KIND).await;
    assert_eq!(months.len(), 1);
    assert_eq!(months[0]["text"], "INVENTED-PERIOD-LINE");
    assert_eq!(
        months[0]["refs"].as_array().unwrap().len(),
        1,
        "the day line's ref"
    );
    for _ in 0..40 {
        d.clock.advance(DAY);
        assert_eq!(pass(&d).await.calls, 0, "written once");
    }
}

#[test]
fn the_librarian_runs_on_the_providers_smallest_model() {
    use ferb_daemon::areas_b::runtimes::smallest_model;
    assert_eq!(
        smallest_model(
            "claude",
            "claude-opus-5-5",
            &[
                "claude-opus-5-5".into(),
                "claude-haiku-5-5".into(),
                "claude-sonnet-5-5".into()
            ]
        ),
        "claude-haiku-5-5"
    );
    assert_eq!(
        smallest_model("anthropic", "claude-sonnet-5-5", &[]),
        "claude-haiku-5-5"
    );
    assert_eq!(smallest_model("openai", "gpt-6.1-sol", &[]), "gpt-6-luna");
    // no priced model it offers: the owner's own
    assert_eq!(
        smallest_model("codex", "gpt-5.6-sol", &["unpriced-model".into()]),
        "gpt-5.6-sol"
    );
}
