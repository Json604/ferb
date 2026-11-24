//! The memory import paused (MEM-478, the owner 2026-11-24: "lets just pause the session tracking and memory transfer
//! for now"), as the shipped program has it: no question at setup, no read of the coding assistants' history (the
//! import, the session index, the hourly pass, his ask), no website request; the RPCs say it is paused; a start turns
//! an earlier build's switches off through their own path, cancelling the website watches, once. What came in stays:
//! the review list still asks and answers. This file never calls `unpause_for_tests`. Temporary folders only; no
//! model, browser or mail.

mod a_support;

use a_support::{Daemon, Raw, Seq};
use ferb_clock::clock::FakeClock;
use ferb_daemon::areas_a::{memory, memory_import, State};
use serde_json::{json, Value};

async fn daemon(tmp: &std::path::Path) -> Daemon {
    Daemon::start(tmp, "01", "id", FakeClock::new(None), Seq::new("id")).await
}

async fn call(d: &Daemon, method: &str, params: Value) -> Value {
    let mut raw = Raw::connect(d.server.path()).await;
    raw.send(r#"{"jsonrpc": "2.0", "id": 1, "method": "auth.hello", "params": {"role": "cli"}}"#)
        .await;
    raw.recv().await;
    raw.send(&json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}).to_string())
        .await;
    serde_json::from_str(&raw.recv().await).unwrap()
}

fn asks(d: &Daemon) -> Vec<Value> {
    d.stand
        .calls
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| c.get("ask").cloned())
        .collect()
}

/// Lines typed into Claude Code an hour ago.
async fn history(d: &Daemon) {
    let now_ms = d
        .on(|st| Box::pin(async move { st.now().timestamp_millis() }))
        .await;
    let path = d.user.join(".claude/history.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(
            "{{\"display\": \"I keep bees on my roof\", \"timestamp\": {}, \"sessionId\": \"s1\", \"project\": \"/Users/me/hive\"}}\n",
            now_ms - 3_600_000
        ),
    )
    .unwrap();
}

async fn setup_done(d: &Daemon) {
    d.on(|st| {
        Box::pin(async move {
            st.journal(
                "setup.finished",
                json!({"by": "app", "done": true, "missing": []}),
                None,
            )
            .unwrap();
        })
    })
    .await;
}

fn count(st: &State, sql: &str, arg: &str) -> i64 {
    st.store.count(sql, ferb_learning::sql![arg]).unwrap()
}

fn runs(st: &State) -> i64 {
    count(
        st,
        "SELECT COUNT(*) FROM memory_import_runs WHERE source != ?",
        "",
    )
}

// decision: MEM-478
#[tokio::test]
async fn paused_setup_asks_nothing_and_nothing_reads_the_coding_sessions() {
    assert!(memory_import::PAUSED && memory_import::paused());
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    history(&d).await;
    setup_done(&d).await;
    let profiles = vec![("Chrome “Work”".to_owned(), "Chrome/Profile 1".to_owned())];
    d.on(move |st| Box::pin(async move { memory_import::offer(&st, profiles) }))
        .await;
    assert!(asks(&d).is_empty(), "neither import's question is asked");
    let (made, index, asked, fetched) = d
        .on(|st| {
            Box::pin(async move {
                // an earlier build's Yes still in the config: the hourly pass and the index read nothing all the same
                st.config
                    .set("memory", "local_import", &json!(true))
                    .unwrap();
                memory_import::hourly(&st).await;
                memory_import::session_index(&st);
                memory_import::local_update(&st, memory_import::Tell::Never).await;
                let asked = memory_import::learn_now(&st);
                let fetched = memory_import::start_fetched(&st, memory_import::CODING, "");
                (
                    runs(&st),
                    ferb_learning::worklog::kept(&st.learning).unwrap(),
                    asked,
                    fetched,
                )
            })
        })
        .await;
    assert_eq!(made, 0, "no import run");
    assert!(index.is_empty(), "the session index isn't kept");
    assert_eq!(asked, Err(memory_import::PAUSED_SAID.to_owned()));
    assert_eq!(fetched, Err(memory_import::PAUSED_SAID.to_owned()));
    // a stale answer to an earlier build's question changes nothing
    let said = d
        .on(|st| Box::pin(async move { memory_import::local_answered(&st, true) }))
        .await;
    assert_eq!(said, memory_import::PAUSED_SAID);
    assert_eq!(
        d.on(|st| Box::pin(async move { runs(&st) })).await,
        0,
        "nothing read on a stale Yes"
    );
}

// decision: MEM-478
#[tokio::test]
async fn paused_rpcs_say_so_plainly_and_the_status_tells_the_clients() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    history(&d).await;
    for (method, params) in [
        (
            "memory.import.start",
            json!({"source": "coding_assistants"}),
        ),
        ("memory.import.start", json!({"source": "claude_code"})),
        ("memory.import.estimate", json!({"source": "claude_code"})),
        (
            "memory.import.switch",
            json!({"which": "local", "on": true}),
        ),
        (
            "memory.import.switch",
            json!({"which": "sites", "on": true}),
        ),
    ] {
        let reply = call(&d, method, params.clone()).await;
        assert_eq!(
            reply["error"]["message"],
            json!(memory_import::PAUSED_SAID),
            "{method} {params}: {reply}"
        );
    }
    let status = call(&d, "memory.import.status", json!({})).await;
    assert_eq!(status["result"]["paused"], json!(true), "{status}");
    assert!(status["result"].get("live").is_none(), "{status}");
    // what came in stays readable and answerable
    for method in ["memory.import.lines", "memory.review"] {
        let reply = call(&d, method, json!({})).await;
        assert!(reply.get("error").is_none(), "{method}: {reply}");
    }
    let made = d.on(|st| Box::pin(async move { runs(&st) })).await;
    assert_eq!(made, 0);
}

/// A start's memory steps (ferbd's start runs `resume` again after them), then what the pause left.
async fn started(
    d: &Daemon,
    sid: String,
) -> (Option<Value>, Option<Value>, String, i64, i64, usize) {
    d.on(move |st: std::rc::Rc<State>| {
        Box::pin(async move {
            memory::start(&st).unwrap();
            memory_import::resume(&st);
            (
                st.config_get("memory", "local_import"),
                st.config_get("memory", "sites_import"),
                st.schedules().get(&sid).unwrap().unwrap().state,
                count(
                    &st,
                    "SELECT COUNT(*) FROM journal WHERE kind = ?",
                    "memory.import_paused",
                ),
                count(
                    &st,
                    "SELECT COUNT(*) FROM journal WHERE kind = ?",
                    "schedule.cancel",
                ),
                memory_import::job(&st).running().unwrap().len(),
            )
        })
    })
    .await
}

// decision: MEM-478
#[tokio::test]
async fn a_start_turns_an_earlier_builds_switches_off_and_cancels_the_watches_once() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    history(&d).await;
    let (sid, waited) = d
        .on(|st| {
            Box::pin(async move {
                // as an earlier build left it: both switches on, the ChatGPT watch active, a read under way
                st.config
                    .set("memory", "local_import", &json!(true))
                    .unwrap();
                st.config
                    .set("memory", "sites_import", &json!(true))
                    .unwrap();
                st.config
                    .set("memory", "sites_asked", &json!("chrome/Default"))
                    .unwrap();
                let sid = memory_import::make_site_job(&st, "chatgpt", "chrome/Default").unwrap();
                memory_import::job(&st).create("claude_code", "").unwrap();
                let s = st.schedules().get(&sid).unwrap().unwrap();
                // a tick before the start's cancel looks at nothing and runs no model
                let waited = memory_import::watch_tick(&st, &s).await;
                (sid, waited)
            })
        })
        .await;
    assert!(waited.is_some_and(|w| !w.by_code), "no mail check");
    let first = started(&d, sid.clone()).await;
    assert_eq!(
        first,
        (
            Some(json!(false)),
            Some(json!(false)),
            "cancelled".to_owned(),
            1,
            1,
            0
        )
    );
    let again = started(&d, sid).await;
    assert_eq!(again, first, "a later start changes nothing");
    let entry = d
        .on(|st| {
            Box::pin(async move {
                st.store
                    .one(
                        "SELECT data_json FROM journal WHERE kind = ?",
                        ferb_learning::sql!["memory.import_paused"],
                    )
                    .unwrap()
                    .unwrap()
                    .s_at(0)
            })
        })
        .await;
    let entry: Value = serde_json::from_str(&entry).unwrap();
    assert_eq!(entry["local"], json!(true));
    assert_eq!(entry["sites"], json!(true));
    assert_eq!(entry["decision"], json!("MEM-478"));
}

// decision: MEM-478, MEM-472
#[tokio::test]
async fn the_review_list_still_asks_and_answers_while_paused() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    let id = d
        .on(|st| {
            Box::pin(async move {
                let item = st
                    .memory
                    .save(
                        "fact",
                        "owner",
                        "trip",
                        "The owner is in Goa",
                        "inferred",
                        &[("journal".into(), "j1".into())],
                        0.6,
                        &[],
                        "",
                        Some(false),
                    )
                    .unwrap()
                    .item;
                let at = ferb_learning::py::shift(&st.memory.clock.now(), 0);
                st.memory.set_review(&item.id, Some(&at)).unwrap();
                memory_import::hourly(&st).await;
                item.id
            })
        })
        .await;
    let asked = asks(&d);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0]["kind"], "memory_review");
    let reply = call(
        &d,
        "memory.review.answer",
        json!({"part": "still_true", "item_ids": [id], "answer": "yes"}),
    )
    .await;
    assert_eq!(reply["result"]["answered"], json!(1), "{reply}");
}
