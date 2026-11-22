//! The owner's memory-import flow on group A (MEM-447, MEM-448, MEM-451): the coding assistants' question once setup
//! is done, the websites' question with the browser profiles, the two quiet watches a profile makes, how a watch's
//! runs are posted, and memory.import's boundary. Temporary folders only; no model, browser or mail.

mod a_support;

use std::rc::Rc;

use a_support::{Daemon, Seq};
use ferb_clock::clock::FakeClock;
use ferb_daemon::areas_a::{memory, memory_import, schedules, State};
use serde_json::{json, Map, Value};

async fn daemon(tmp: &std::path::Path) -> Daemon {
    Daemon::start(tmp, "01", "id", FakeClock::new(None), Seq::new("id")).await
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

fn answer(st: &State, id: &str, state: &str, choice: &str) -> String {
    memory::question_answered(
        st,
        &memory::Answered {
            approval_id: id,
            state,
            choice: Some(choice),
            answer: None,
        },
    )
    .unwrap()
}

// decision: MEM-447
#[tokio::test]
async fn the_coding_assistants_are_asked_once_after_setup_and_no_reads_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    let history = d.user.join(".claude/history.jsonl");
    std::fs::create_dir_all(history.parent().unwrap()).unwrap();
    std::fs::write(
        &history,
        "{\"display\": \"I keep bees\", \"timestamp\": 1, \"sessionId\": \"s\"}\n",
    )
    .unwrap();
    // nothing before setup is done
    d.on(|st| Box::pin(async move { memory_import::offer(&st, Vec::new()) }))
        .await;
    assert!(asks(&d).is_empty());
    setup_done(&d).await;
    d.on(|st| Box::pin(async move { memory_import::offer(&st, Vec::new()) }))
        .await;
    let asked = asks(&d);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["kind"], "memory_local");
    assert_eq!(
        asked[0]["summary"],
        "Bring in what Claude Code knows about you?"
    );
    assert_eq!(
        asked[0]["labels"],
        json!(["Yes, bring it in", "No, thanks"])
    );
    let detail = asked[0]["detail"].as_str().unwrap();
    assert!(
        detail.contains("over the last month") && !detail.contains(" AI"),
        "{detail}"
    );
    let id = asked[0]["id"].as_str().unwrap().to_owned();
    let said = d
        .on(move |st| Box::pin(async move { answer(&st, &id, "denied", "No") }))
        .await;
    assert_eq!(said, "Okay, I won't read them.");
    let (setting, runs) = d
        .on(|st| {
            Box::pin(async move {
                (
                    st.config_get("memory", "local_import"),
                    memory_import::job(&st).status().unwrap(),
                )
            })
        })
        .await;
    assert_eq!(setting, Some(json!(false)));
    assert_eq!(runs["runs"], json!([]), "nothing is read on No");
    // never asked again
    d.on(|st| Box::pin(async move { memory_import::offer(&st, Vec::new()) }))
        .await;
    assert_eq!(asks(&d).len(), 1);
}

// decision: MEM-448, MEM-449, MEM-451
#[tokio::test]
async fn the_websites_question_makes_two_quiet_watches_in_the_chosen_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    setup_done(&d).await;
    let label = "Chrome “Work” (someone@example.invalid)".to_owned();
    let profiles = vec![(label.clone(), "Chrome/Profile 1".to_owned())];
    d.on(move |st| Box::pin(async move { memory_import::offer(&st, profiles) }))
        .await;
    let asked = asks(&d);
    assert_eq!(
        asked.len(),
        1,
        "no coding assistant here: the websites come first"
    );
    assert_eq!(asked[0]["kind"], "memory_sites");
    assert_eq!(
        asked[0]["summary"],
        "Want me to bring in what ChatGPT and Claude know about you?"
    );
    let id = asked[0]["id"].as_str().unwrap().to_owned();
    let chosen = label.clone();
    let said = d
        .on(move |st| Box::pin(async move { answer(&st, &id, "granted", &chosen) }))
        .await;
    assert!(
        said.starts_with("Great, I'll ask ChatGPT and Claude for your data in Chrome “Work”"),
        "{said}"
    );
    let rows = d
        .on(|st| {
            Box::pin(async move {
                let s = st.schedules();
                let all = s.all(&["active"]).unwrap();
                let booked = st.timers().pending(&["schedule"]).unwrap().len();
                (all, booked, st.config_get("memory", "sites_asked"))
            })
        })
        .await;
    let (all, booked, asked_flag) = rows;
    assert_eq!(all.len(), 2);
    assert_eq!(booked, 2);
    assert_eq!(asked_flag, Some(json!("Chrome/Profile 1")));
    for (s, site) in all.iter().zip(["chatgpt", "claude"]) {
        assert_eq!(s.targets["job"], json!(format!("memory_import:{site}")));
        assert_eq!(s.watch, "until_met");
        assert_eq!(s.rule.every_minutes, Some(15));
        assert_eq!(s.run_scope().unwrap()["profile"], json!(label));
        assert!(
            s.request
                .contains(&format!("memory.import with source {site}")),
            "{}",
            s.request
        );
        assert!(s.request.contains("don't try another way"), "{}", s.request);
    }
    // the watch's runs: quiet while nothing needs the owner; one that does is posted once
    let sid = all[0].id.clone();
    let posted = |d: &Daemon| {
        d.stand
            .calls
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.get("task_result").is_some())
            .count()
    };
    for (status, want) in [
        ("done", 0),
        ("done_with_gaps", 1),
        ("done_with_gaps", 1),
        ("done", 1),
        ("failed", 2),
    ] {
        let sid = sid.clone();
        d.on(move |st: Rc<State>| {
            Box::pin(async move {
                let s = st.schedules().get(&sid).unwrap().unwrap();
                let mut f = Map::new();
                f.insert("status".into(), json!(status));
                f.insert("answer".into(), json!("checked the mail: not there yet"));
                f.insert("task_id".into(), json!("t1"));
                f.insert("watch_met".into(), json!(false));
                let now = st.now();
                schedules::schedule_finished(&st, &s, f, 0.0, true, &now)
                    .await
                    .unwrap();
            })
        })
        .await;
        assert_eq!(posted(&d), want, "after a {status} run");
    }
}

// decision: MEM-448
#[tokio::test]
async fn no_to_the_websites_changes_nothing_and_isnt_asked_again() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    setup_done(&d).await;
    let profiles = vec![("Brave “Personal”".to_owned(), "Brave/Default".to_owned())];
    let again = profiles.clone();
    d.on(move |st| Box::pin(async move { memory_import::offer(&st, profiles) }))
        .await;
    let id = asks(&d)[0]["id"].as_str().unwrap().to_owned();
    let said = d
        .on(move |st| Box::pin(async move { answer(&st, &id, "granted", "No, thanks") }))
        .await;
    assert_eq!(
        said,
        "Okay, I'll leave them be. If you change your mind, just ask me."
    );
    let n = d
        .on(|st| Box::pin(async move { st.schedules().all(&["active"]).unwrap().len() }))
        .await;
    assert_eq!(n, 0);
    d.on(move |st| Box::pin(async move { memory_import::offer(&st, again) }))
        .await;
    assert_eq!(asks(&d).len(), 1);
}

// decision: MEM-451, MEM-440
#[tokio::test]
async fn memory_import_takes_only_an_export_in_ferbs_downloads_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    let elsewhere = d.user.join("Downloads/chatgpt.zip");
    std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    std::fs::write(&elsewhere, "x").unwrap();
    let downloads = d.home.join("workspace/downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let export = downloads.join("a1-conversations.json");
    std::fs::write(
        &export,
        json!([{"conversation_id": "g1", "title": "chat", "current_node": "n0", "mapping": {"n0": {"id": "n0",
            "parent": null, "message": {"author": {"role": "user"}, "create_time": 1_799_000_000.0,
            "content": {"content_type": "text", "parts": ["I live in Lisbon"]}}}}}])
        .to_string(),
    )
    .unwrap();
    let (e1, path) = (
        elsewhere.to_string_lossy().into_owned(),
        export.to_string_lossy().into_owned(),
    );
    let refused = d
        .on(move |st| Box::pin(async move { memory_import::start_fetched(&st, "chatgpt", &e1) }))
        .await;
    assert!(refused
        .unwrap_err()
        .contains("only an export FERB downloaded"));
    let local = d
        .on(|st| Box::pin(async move { memory_import::start_fetched(&st, "claude_code", "") }))
        .await;
    assert!(local.unwrap_err().contains("a website's export"));
    let started = d
        .on(move |st| Box::pin(async move { memory_import::start_fetched(&st, "chatgpt", &path) }))
        .await
        .unwrap();
    let run = started["run"].as_str().unwrap().to_owned();
    let recorded = d
        .on(move |st| {
            Box::pin(async move {
                st.store
                    .exists(
                        "SELECT 1 FROM memory_import_files WHERE run_id = ?",
                        ferb_learning::sql![run],
                    )
                    .unwrap()
            })
        })
        .await;
    assert!(
        recorded,
        "a fetched export is recorded to be deleted once its import is done"
    );
    // with no model here every chunk fails: the export is kept for a day, never deleted with parts unread
    assert!(export.exists());
}

// decision: MEM-444
#[tokio::test]
async fn the_ask_after_an_import_is_a_question_answered_in_the_owners_words() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    // a line 45 days old carrying an offer, as an earlier import placed it
    d.on(|st| {
        Box::pin(async move {
            let now = st.now();
            let said = ferb_clock::py::plus_us(&now, -45 * 86_400_000_000).unwrap();
            let day = ferb_clock::py::iso(&said)[..10].to_owned();
            let item = st
                .memory
                .save("fact", "owner", "recurring_rent", "The owner pays rent every month", "inferred",
                      &[("journal".into(), "j1".into())], 0.6, &[], "imported from Claude Code", Some(false))
                .unwrap()
                .item;
            let data = json!({"you": "You pay rent every month", "offer": "Offer to help with recurring_rent",
                              "said": day, "first_said": day, "days": 1, "sources": ["claude_code"], "ended": false,
                              "until": "", "review_on": "", "use": "repeat", "holds_up": false});
            st.store
                .exec(
                    "INSERT INTO memory_import_lines (id, item_id, kind, subject, key, data_json, created_at, updated_at) \
                     VALUES ('l1', ?, 'fact', 'owner', 'recurring_rent', ?, ?, ?)",
                    ferb_learning::sql![item.id, data.to_string(), day, day],
                )
                .unwrap();
            memory_import::ask_offers(&st);
        })
    })
    .await;
    let asked = asks(&d);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["kind"], "memory_offers");
    assert_eq!(asked[0]["summary"], "Are these still on?");
    assert!(asked[0]["detail"]
        .as_str()
        .unwrap()
        .contains("1. Offer to help with recurring_rent"));
    // asked once
    d.on(|st| Box::pin(async move { memory_import::ask_offers(&st) }))
        .await;
    assert_eq!(asks(&d).len(), 1);
    // his reply in his own words goes to FERB's reading of it (no model in this daemon: nothing is changed, and FERB
    // says so)
    let id = asked[0]["id"].as_str().unwrap().to_owned();
    let said = d
        .on(move |st| {
            Box::pin(async move {
                memory::question_answered(
                    &st,
                    &memory::Answered {
                        approval_id: &id,
                        state: "granted",
                        choice: None,
                        answer: Some("first one yes"),
                    },
                )
                .unwrap()
            })
        })
        .await;
    assert_eq!(said, "Thanks, I'll update them.");
    let mut told = String::new();
    for _ in 0..50 {
        told = d
            .stand
            .calls
            .0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter_map(|c| {
                c.get("tell_owner")
                    .map(|t| t["text"].as_str().unwrap_or("").to_owned())
            })
            .next()
            .unwrap_or_default();
        if !told.is_empty() {
            break;
        }
        d.on(|_st| Box::pin(async move { tokio::task::yield_now().await }))
            .await;
    }
    assert_eq!(
        told,
        "I couldn't apply that just now; nothing was changed. Tell me again in a moment."
    );
}
