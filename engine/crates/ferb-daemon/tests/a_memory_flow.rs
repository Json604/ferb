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
    // MEM-478: the import is paused in the shipped program; its own tests drive it as if resumed
    memory_import::unpause_for_tests();
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

// decision: MEM-466
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

// decision: MEM-448, MEM-467, MEM-451
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
        assert_eq!(s.rule.every_minutes, Some(60), "every hour (MEM-477)");
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

/// A line as an import placed it: its memory item and its row, `days_ago` old, with `data` over the usual fields.
fn imported_line(
    st: &State,
    id: &str,
    key: &str,
    content: &str,
    days_ago: i64,
    data: Value,
) -> String {
    let now = st.now();
    let said = ferb_clock::py::plus_us(&now, -days_ago * 86_400_000_000).unwrap();
    let day = ferb_clock::py::iso(&said)[..10].to_owned();
    let entry = st
        .journal(
            ferb_learning::import_job::SOURCE_KIND,
            json!({"run": "r1"}),
            None,
        )
        .unwrap();
    let item = st
        .memory
        .save(
            "fact",
            "owner",
            key,
            content,
            "inferred",
            &[("journal".into(), entry.id)],
            0.6,
            &[],
            "imported from Claude Code",
            Some(false),
        )
        .unwrap()
        .item;
    let mut row = json!({"you": content.replace("The owner", "You"), "offer": "", "said": day, "first_said": day,
                         "days": 1, "sources": ["claude_code"], "ended": false, "until": "", "review_on": "",
                         "use": "repeat", "holds_up": false});
    for (k, v) in data.as_object().unwrap() {
        row[k] = v.clone();
    }
    st.store
        .exec(
            "INSERT INTO memory_import_lines (id, item_id, kind, subject, key, data_json, created_at, updated_at) \
             VALUES (?, ?, 'fact', 'owner', ?, ?, ?, ?)",
            ferb_learning::sql![id, item.id, key, row.to_string(), day, day],
        )
        .unwrap();
    item.id
}

fn told(d: &Daemon) -> Vec<Value> {
    d.stand
        .calls
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| c.get("tell_owner").cloned())
        .collect()
}

// decision: MEM-444, MEM-460, MEM-485
#[tokio::test]
async fn the_review_after_an_import_waits_for_the_days_first_open_and_is_answered_on_its_card_or_in_words(
) {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    setup_done(&d).await;
    // a line 45 days old carrying an offer, as an earlier import placed it
    d.on(|st| {
        Box::pin(async move {
            imported_line(
                &st,
                "l1",
                "recurring_rent",
                "The owner pays rent every month",
                45,
                json!({"offer": "Offer to help with recurring_rent"}),
            );
            memory_import::ask_offers(&st);
        })
    })
    .await;
    assert!(asks(&d).is_empty(), "nothing is asked when it is learned");
    d.on(|st| Box::pin(async move { memory_import::daily(&st, "mac") }))
        .await;
    let asked = asks(&d);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["kind"], "memory_review");
    assert_eq!(
        asked[0]["summary"],
        "Can you check a few things I learned about you?"
    );
    assert_eq!(
        asked[0]["detail"],
        "1 thing I could help with. Untick anything that isn't right, or just tell me."
    );
    // asked once: the next import's round and the day's later opens ask it no more
    d.on(|st| {
        Box::pin(async move {
            memory_import::ask_offers(&st);
            memory_import::daily(&st, "mac");
        })
    })
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
    let mut last = String::new();
    for _ in 0..50 {
        last = told(&d)
            .last()
            .map(|t| t["text"].as_str().unwrap_or("").to_owned())
            .unwrap_or_default();
        if !last.is_empty() {
            break;
        }
        d.on(|_st| Box::pin(async move { tokio::task::yield_now().await }))
            .await;
    }
    assert_eq!(
        last,
        "I couldn't apply that just now; nothing was changed. Tell me again in a moment."
    );
}

// decision: MEM-465, MEM-485, MEM-489
#[tokio::test]
async fn a_start_takes_an_earlier_builds_imported_lines_out_and_the_day_asks_nothing_about_them() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    setup_done(&d).await;
    // what an earlier build left: two imported lines it put in always on by itself
    let (rent, tea) = d
        .on(|st| {
            Box::pin(async move {
                let rent = imported_line(&st, "l1", "recurring_rent", "The owner pays rent every month", 3,
                                         json!({"use": "repeat"}));
                let tea = imported_line(&st, "l2", "drink", "The owner prefers tea", 3, json!({"use": "follow"}));
                for id in [&rent, &tea] {
                    st.store
                        .exec(
                            "INSERT INTO memory_profile (item_id, section, position, added_by, added_at) \
                             VALUES (?, 'Recently said', 0, 'code', 'x')",
                            ferb_learning::sql![id],
                        )
                        .unwrap();
                }
                memory::start(&st).unwrap();
                memory_import::review_start(&st);
                (rent, tea)
            })
        })
        .await;
    // they left always on at the start, both still remembered; nothing is asked, then or at the day's first open
    let (r, t) = (rent.clone(), tea.clone());
    d.on(move |st| {
        Box::pin(async move {
            let ids = st.profile.ids().unwrap();
            assert!(!ids.contains(&r) && !ids.contains(&t), "{ids:?}");
            for id in [&r, &t] {
                assert_eq!(st.memory.get(id).unwrap().unwrap().status, "active");
            }
        })
    })
    .await;
    assert!(asks(&d).is_empty());
    d.on(|st| Box::pin(async move { memory_import::daily(&st, "mac") }))
        .await;
    let asked = asks(&d);
    assert!(
        asked.iter().all(|a| a["kind"] != "memory_review"),
        "{asked:?}"
    );
}

// decision: MEM-460, MEM-481 — the last items answered by a button close the review's question for every client: the
// close is published, so its card goes and the chat's question entry closes (it stayed open, and a phone kept a card
// with no list; owner 2026-11-23: "its blank after i approved all the memory messages")
#[tokio::test]
async fn answering_the_last_items_closes_the_review_on_every_client() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    setup_done(&d).await;
    *d.stand.keeps.lock().unwrap() = Some(Default::default());
    // FERB's own understanding, each said on two days three days apart: it waits for his Yes
    own_understanding(&d).await;
    d.clock.advance(3.0 * 86_400.0);
    let ids = own_understanding(&d).await;
    d.on(|st| Box::pin(async move { memory_import::hourly(&st).await }))
        .await;
    d.on(|st| Box::pin(async move { memory_import::daily(&st, "mac") }))
        .await;
    let asked = asks(&d);
    assert_eq!(asked.len(), 1, "{asked:?}");
    let id = asked[0]["id"].as_str().unwrap().to_owned();
    d.published.lock().unwrap().clear();
    d.on(move |st| {
        Box::pin(async move {
            memory_import::job(&st)
                .answer_review(&st.profile, "always_on", &ids, "approve")
                .unwrap();
            memory_import::review_settled(&st);
            assert_eq!(memory_import::job(&st).review().unwrap()["count"], json!(0));
            assert!(memory::pending_own(&st, memory_import::REVIEW_KIND).is_none());
        })
    })
    .await;
    let resolved: Vec<Value> = d
        .published
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e["topic"] == "approval.resolved")
        .map(|e| e["data"].clone())
        .collect();
    assert_eq!(
        resolved,
        vec![json!({"approval_id": id, "state": "superseded"})]
    );
}

// decision: MEM-463
#[tokio::test]
async fn a_burst_of_joins_is_one_notice_with_a_list() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    d.on(|st| {
        Box::pin(async move {
            let mut items = Vec::new();
            for (k, c) in [
                ("a", "The owner likes tea"),
                ("b", "The owner runs daily"),
                ("c", "The owner uses Arc"),
            ] {
                items.push(
                    st.memory
                        .save(
                            "fact",
                            "owner",
                            k,
                            c,
                            "inferred",
                            &[("journal".into(), "j1".into())],
                            0.6,
                            &[],
                            "",
                            Some(false),
                        )
                        .unwrap()
                        .item,
                );
            }
            // three joins a pass makes one after another
            for it in &items {
                (st.profile.on_joined.borrow())(std::slice::from_ref(it));
            }
        })
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(
        memory::JOINS_GATHER_MS + 700,
    ))
    .await;
    d.on(|_st| Box::pin(async move { tokio::task::yield_now().await }))
        .await;
    let notices: Vec<Value> = told(&d)
        .into_iter()
        .filter(|t| {
            t["ident"]
                .as_str()
                .unwrap()
                .starts_with("memory-always-on-")
        })
        .collect();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(
        notices[0]["text"],
        "Now always on: 3 things (FERB's understanding)"
    );
    let cards = notices[0]["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 3);
    assert_eq!(cards[0]["type"], "note");
    assert_eq!(cards[0]["always_on"], true);
    // one alone reads as before
    d.on(|st| {
        Box::pin(async move {
            let it = st
                .memory
                .save(
                    "fact",
                    "owner",
                    "d",
                    "The owner sails",
                    "inferred",
                    &[("journal".into(), "j2".into())],
                    0.6,
                    &[],
                    "",
                    Some(false),
                )
                .unwrap()
                .item;
            (st.profile.on_joined.borrow())(std::slice::from_ref(&it));
        })
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(
        memory::JOINS_GATHER_MS + 700,
    ))
    .await;
    d.on(|_st| Box::pin(async move { tokio::task::yield_now().await }))
        .await;
    let last = told(&d).last().cloned().unwrap();
    assert_eq!(
        last["text"],
        "Now always on: “The owner sails” (FERB's understanding)"
    );
}

fn runs(st: &State) -> i64 {
    st.store
        .count(
            "SELECT COUNT(*) FROM memory_import_runs",
            ferb_learning::sql![],
        )
        .unwrap()
}

// decision: MEM-466 (the owner, 2026-11-23: "not keep looking at my session eveyr hour if ive not said it - adn that
// tooo the model") — an hour with new sessions passes with no import run and no model call; the index knows them
#[tokio::test]
async fn an_hour_with_new_sessions_reads_nothing_with_a_model_and_the_index_knows_them() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    let path = d.user.join(".claude/history.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let now_ms = d
        .on(|st| Box::pin(async move { st.now().timestamp_millis() }))
        .await;
    std::fs::write(
        &path,
        format!(
            "{{\"display\": \"I keep bees on my roof\", \"timestamp\": {}, \"sessionId\": \"1a2b3c4d-5e6f-4a1b-8c9d-0e1f2a3b4c5d\", \"project\": \"/Users/me/hive\"}}\n",
            now_ms - 3_600_000
        ),
    )
    .unwrap();
    let (made, index) = d
        .on(|st| {
            Box::pin(async move {
                st.config
                    .set("memory", "local_import", &json!(true))
                    .unwrap();
                memory_import::hourly(&st).await;
                let made = runs(&st);
                memory_import::session_index(&st);
                (made, ferb_learning::worklog::kept(&st.learning).unwrap())
            })
        })
        .await;
    assert_eq!(made, 0, "the hourly pass starts no import");
    assert_eq!(index.len(), 1);
    let entry = index.values().next().unwrap();
    assert_eq!(entry["folder"], json!("hive"));
    assert_eq!(entry["title"], json!("Claude Code in hive"));
    assert_eq!(
        entry["hash"],
        json!(""),
        "no summary written without his ask"
    );
    // his ask from chat (the workers' memory.import, coding_assistants) starts the read now
    let said = d
        .on(|st| {
            Box::pin(async move { memory_import::start_fetched(&st, memory_import::CODING, "") })
        })
        .await
        .unwrap();
    assert_eq!(said["started"], json!(true));
    assert!(said["said"]
        .as_str()
        .unwrap()
        .starts_with("I'm reading what's new in Claude Code now"));
}

// decision: MEM-467 — a website watch waits for its export mail by code: after its first run, no model run comes
// before WATCH_MODEL_EVERY_HOURS
#[tokio::test]
async fn a_website_watch_waits_by_code_between_its_rare_model_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    let waits = d
        .on(|st| {
            Box::pin(async move {
                let sid = memory_import::make_site_job(&st, "chatgpt", "chrome/Default").unwrap();
                let s = st.schedules().get(&sid).unwrap().unwrap();
                let first = memory_import::watch_waits(&st, &s);
                st.journal(
                    "schedule.quiet",
                    json!({"schedule_id": sid, "status": "done_with_gaps"}),
                    None,
                )
                .unwrap();
                // 47 ticks of 15 minutes after it: every one waits, no model run
                let all_wait = memory_import::watch_waits(&st, &s);
                (first, all_wait, sid)
            })
        })
        .await;
    assert!(!waits.0, "the first run requests the export");
    assert!(waits.1, "a tick right after waits by code");
    let sid = waits.2;
    d.on(move |st| {
        Box::pin(async move {
            let s = st.schedules().get(&sid).unwrap().unwrap();
            // the owner's schedules are never held
            let mut mine = s.clone();
            mine.targets = serde_json::Map::new();
            assert!(!memory_import::watch_waits(&st, &mine));
        })
    })
    .await;
}

/// One tick of a website watch (MEM-476): None, the model run goes; Some, it waits.
async fn tick(d: &Daemon, sid: &str) -> Option<memory_import::Wait> {
    let sid = sid.to_owned();
    d.on(move |st| {
        Box::pin(async move {
            let s = st.schedules().get(&sid).unwrap().unwrap();
            memory_import::watch_tick(&st, &s).await
        })
    })
    .await
}

/// A watch's run ended with `status`.
async fn ended(d: &Daemon, sid: &str, status: &str) {
    let (sid, status) = (sid.to_owned(), status.to_owned());
    d.on(move |st| {
        Box::pin(async move {
            st.journal(
                "schedule.quiet",
                json!({"schedule_id": sid, "status": status}),
                None,
            )
            .unwrap();
        })
    })
    .await
}

// decision: MEM-476 — the code looks in his mail on every tick; only the site's mail no run took starts the model run;
// none waits (a run at most every WATCH_LATE_HOURS, to tell him it's late); with no code check, the 12-hour run
#[tokio::test]
async fn a_website_watch_starts_its_model_run_only_for_mail_the_code_found() {
    use ferb_daemon::areas_a::{MailQuery, MailSeen};
    use std::sync::{Arc, Mutex};
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    let seen = Arc::new(Mutex::new(MailSeen::Nothing));
    let asked: Arc<Mutex<Vec<MailQuery>>> = Arc::default();
    let (answer, looked) = (seen.clone(), asked.clone());
    let sid = d
        .on(move |st| {
            Box::pin(async move {
                st.config
                    .set("memory", "sites_asked", &json!("chrome/Default"))
                    .unwrap();
                *st.mail_check.borrow_mut() = Some(Arc::new(move |q| {
                    looked.lock().unwrap().push(q);
                    let out = answer.lock().unwrap().clone();
                    Box::pin(async move { out })
                }));
                memory_import::make_site_job(&st, "chatgpt", "Kartikey").unwrap()
            })
        })
        .await;
    // the first run requests the export: it goes, and nothing looks at the mail before it
    assert_eq!(tick(&d, &sid).await, None);
    assert!(asked.lock().unwrap().is_empty());
    ended(&d, &sid, "done").await;
    // no mail yet: it waits, looked at by code, in the profile he chose, for the site's own domain since the watch began
    let w = tick(&d, &sid).await.expect("it waits");
    assert!(w.by_code, "{w:?}");
    {
        let q = asked.lock().unwrap()[0].clone();
        assert_eq!(q.profile, "chrome/Default");
        assert_eq!(q.domains, ["openai.com"]);
        let first = d
            .on({
                let sid = sid.clone();
                move |st| {
                    Box::pin(async move { st.schedules().get(&sid).unwrap().unwrap().first_run })
                }
            })
            .await;
        assert_eq!(q.after, first.timestamp());
    }
    // the mail came: the model run goes, once for that thread
    *seen.lock().unwrap() = MailSeen::Found(vec!["18aa01".into()]);
    assert_eq!(tick(&d, &sid).await, None);
    ended(&d, &sid, "failed").await;
    let w = tick(&d, &sid).await.expect("a thread a run took waits");
    assert!(w.by_code);
    // another mail from the site: a run again
    *seen.lock().unwrap() = MailSeen::Found(vec!["18aa01".into(), "18aa02".into()]);
    assert_eq!(tick(&d, &sid).await, None);
    ended(&d, &sid, "done").await;
    let found = d
        .on(|st| {
            Box::pin(async move {
                st.store
                    .query(
                        "SELECT data_json FROM journal WHERE kind = 'memory.mail_found' ORDER BY seq",
                        ferb_learning::sql![],
                    )
                    .unwrap()
                    .iter()
                    .map(|r| ferb_py::json::loads(&r.s_at(0)).unwrap()["threads"].clone())
                    .collect::<Vec<Value>>()
            })
        })
        .await;
    assert_eq!(found, [json!(["18aa01"]), json!(["18aa02"])]);
    // nothing for two days after the last run: a run goes, so a late export is told
    *seen.lock().unwrap() = MailSeen::Nothing;
    d.clock
        .advance((memory_import::WATCH_LATE_HOURS * 3600 - 60) as f64);
    assert!(tick(&d, &sid).await.is_some());
    d.clock.advance(120.0);
    assert_eq!(tick(&d, &sid).await, None);
    ended(&d, &sid, "done").await;
    // the check can't run (no browser profile open): the 12-hour run, not looked at by code
    *seen.lock().unwrap() = MailSeen::Unchecked("that browser profile isn't open now".into());
    let w = tick(&d, &sid).await.expect("it waits");
    assert!(!w.by_code);
    d.clock
        .advance((memory_import::WATCH_MODEL_EVERY_HOURS * 3600 + 60) as f64);
    assert_eq!(tick(&d, &sid).await, None);
}

// decision: MEM-477 (the owner, 2026-11-24: "I check your email for it every 1 hour") — a watch ticks, and so looks
// in his mail, once an hour; one an older build made every 15 minutes is brought to it at start; a late export is
// still told after 48 hours
#[tokio::test]
async fn a_website_watch_made_every_15_minutes_looks_at_the_mail_once_an_hour_after_a_start() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    assert_eq!(memory_import::MAIL_EVERY_MINUTES, 60);
    assert_eq!(memory_import::WATCH_LATE_HOURS, 48);
    let (made, older) = d
        .on(|st| {
            Box::pin(async move {
                let made = memory_import::make_site_job(&st, "chatgpt", "chrome/Default").unwrap();
                let older = memory_import::make_site_job(&st, "claude", "chrome/Default").unwrap();
                // the rule an older build gave its watch
                let s = st.schedules().get(&older).unwrap().unwrap();
                let mut rule = s.rule.clone();
                rule.every_minutes = Some(15);
                st.schedules()
                    .edit(&older, None, Some((&rule, &s.first_run)), None, "")
                    .unwrap();
                (made, older)
            })
        })
        .await;
    let gaps = |sid: String| {
        d.on(move |st| {
            Box::pin(async move {
                let s = st.schedules().get(&sid).unwrap().unwrap();
                let mut at = s.first_run;
                let mut gaps = Vec::new();
                for _ in 0..4 {
                    let next = s.next_after(&at).unwrap().unwrap();
                    gaps.push(ferb_clock::py::total_seconds(&next, &at));
                    at = next;
                }
                (s.rule.every_minutes, gaps)
            })
        })
    };
    assert_eq!(gaps(made.clone()).await, (Some(60), vec![3600.0; 4]));
    assert_eq!(gaps(older.clone()).await, (Some(15), vec![900.0; 4]));
    let changed = || {
        d.on(|st| {
            Box::pin(async move {
                memory::start(&st).unwrap();
                st.store
                    .query(
                        "SELECT data_json FROM journal WHERE kind = 'memory.watch_interval' ORDER BY seq",
                        ferb_learning::sql![],
                    )
                    .unwrap()
                    .iter()
                    .map(|r| ferb_py::json::loads(&r.s_at(0)).unwrap())
                    .collect::<Vec<Value>>()
            })
        })
    };
    let first = changed().await;
    assert_eq!(
        first,
        [json!({"schedule_id": older, "from": 15, "to": 60})],
        "only the older watch changes"
    );
    assert_eq!(gaps(older.clone()).await, (Some(60), vec![3600.0; 4]));
    assert_eq!(changed().await, first, "a later start changes nothing");
}

// decision: MEM-472, MEM-485 — the hourly pass puts it in the list by code; the day's first open asks
#[tokio::test]
async fn a_fact_on_its_check_date_is_asked_at_the_days_first_open_with_no_model() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    setup_done(&d).await;
    // a fact the planner saved with review_in_days 30 (remember.rs)
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
                let at = ferb_learning::py::shift(&st.memory.clock.now(), 30 * 86_400_000_000);
                st.memory.set_review(&item.id, Some(&at)).unwrap();
                memory_import::hourly(&st).await;
                item.id
            })
        })
        .await;
    daily(&d).await;
    assert!(asks(&d).is_empty(), "nothing before the date");
    d.clock.advance(30.0 * 86_400.0);
    hourly(&d).await;
    assert!(asks(&d).is_empty(), "the hourly pass asks nothing");
    daily(&d).await;
    let asked = asks(&d);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0]["kind"], "memory_review");
    assert_eq!(
        asked[0]["detail"],
        "1 thing I said I'd check with you. Untick anything that isn't right, or just tell me."
    );
    // "Not sure" by its button: the question closes, and it is asked again a week later
    let again = id.clone();
    d.on(move |st| {
        Box::pin(async move {
            let j = memory_import::job(&st);
            assert_eq!(
                j.answer_review(&st.profile, "still_true", &[again], "unsure")
                    .unwrap(),
                1
            );
            memory_import::review_settled(&st);
        })
    })
    .await;
    hourly(&d).await;
    d.clock.advance(86_400.0);
    daily(&d).await;
    assert_eq!(asks(&d).len(), 1);
    d.clock.advance(6.0 * 86_400.0);
    hourly(&d).await;
    daily(&d).await;
    assert_eq!(asks(&d).len(), 2);
    // no model is set up in this daemon: a model call would have failed the pass's steps, none was tried
    assert!(
        d.on(|st| Box::pin(async move { st.make_runtime.is_none() }))
            .await
    );
}

async fn hourly(d: &Daemon) {
    d.on(|st| Box::pin(async move { memory_import::hourly(&st).await }))
        .await;
}

async fn daily(d: &Daemon) {
    d.on(|st| Box::pin(async move { memory_import::daily(&st, "mac") }))
        .await;
}

/// A stand-in model for the import: one usage event, then no candidates (so nothing is verified or placed).
struct NoCandidates;

impl ferb_core::interfaces::AgentRuntime for NoCandidates {
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
                id: "r1".into(),
                runtime: "stand-in".into(),
                task_id,
                state: Map::new(),
            })
        })
    }
    fn events(
        &self,
        _run: &ferb_core::types::RunHandle,
    ) -> tokio::sync::mpsc::Receiver<ferb_core::types::RuntimeEvent> {
        use ferb_core::types::{RuntimeEvent, RuntimeEventKind};
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let event = |kind, data: Value| RuntimeEvent {
            kind,
            data: data.as_object().cloned().unwrap(),
        };
        tx.try_send(event(
            RuntimeEventKind::Usage,
            json!({"input_tokens": 50, "output_tokens": 4}),
        ))
        .unwrap();
        tx.try_send(event(
            RuntimeEventKind::Final,
            json!({"text": "{\"candidates\": []}"}),
        ))
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

// decision: MEM-412 — the import's model calls are `model.learning` spans with its roles (memory_import.py:26 passed
// the daemon's spans and `_record_learning_usage("memory_import")`): each call one span and one usage span, so the
// trace and `ferb stats` see every call `counts.model_calls` counts
#[tokio::test]
async fn each_import_model_call_is_a_learning_span_with_its_usage() {
    let tmp = tempfile::tempdir().unwrap();
    let make: std::sync::Arc<ferb_daemon::areas_a::MakeRuntime> =
        std::sync::Arc::new(|_, _, _, _| {
            Ok(ferb_learning::calls::Made {
                rt: Box::new(NoCandidates),
                limits: Some(ferb_core::types::ModelLimits {
                    model: "stand-in-1".into(),
                    context_window: 200_000,
                    max_output: 8_000,
                    source: "test".into(),
                    window_includes_output: true,
                    max_input_chars: None,
                }),
                model: None,
            })
        });
    memory_import::unpause_for_tests();
    let d = Daemon::start_with(
        tmp.path(),
        "01",
        "id",
        FakeClock::new(None),
        Seq::new("id"),
        Some(make),
    )
    .await;
    // one line typed into Claude Code eleven days before the clock's now (2026-01-01 09:00 UTC)
    let history = d.user.join(".claude/history.jsonl");
    std::fs::create_dir_all(history.parent().unwrap()).unwrap();
    std::fs::write(
        &history,
        "{\"display\": \"I keep bees on my roof in Lisbon\", \"timestamp\": 1766224800000, \"sessionId\": \"s1\", \
         \"project\": \"/Users/me/hive\"}\n",
    )
    .unwrap();
    // the coding assistants' read, to its end: the import run, then the work log (its calls go through the same job)
    d.on(|st| {
        Box::pin(async move {
            st.config
                .set("memory", "local_import", &json!(true))
                .unwrap();
            memory_import::local_update(&st, memory_import::Tell::Never).await
        })
    })
    .await;
    let count = |sql: &'static str| {
        d.on(move |st| Box::pin(async move { st.store.count(sql, ferb_learning::sql![]).unwrap() }))
    };
    assert_eq!(
        count("SELECT COUNT(*) FROM memory_import_runs WHERE status = 'done'").await,
        1
    );
    let calls =
        count("SELECT json_extract(counts_json, '$.model_calls') FROM memory_import_runs").await;
    assert!(calls >= 1, "the import made no model call");
    let learning = count(
        "SELECT COUNT(*) FROM spans WHERE kind = 'model.learning' AND \
         json_extract(input_json, '$.role') IN ('memory_import_recall', 'memory_import_verify') AND \
         json_extract(output_json, '$.outcome') = 'answered'",
    )
    .await;
    assert_eq!(learning, calls, "one model.learning span per model call");
    let worklog = count(
        "SELECT COUNT(*) FROM spans WHERE kind = 'model.learning' AND \
         json_extract(input_json, '$.role') = 'worklog_summary'",
    )
    .await;
    assert!(worklog >= 1, "the work log's call is a span too");
    let usage = count(
        "SELECT COUNT(*) FROM spans WHERE kind = 'model.usage' AND json_extract(input_json, '$.role') = 'memory_import' \
         AND json_extract(input_json, '$.runtime') = 'stand-in'",
    )
    .await;
    assert_eq!(
        usage,
        calls + worklog,
        "one model.usage span per call's usage event"
    );
    // the trace never holds what was read (MEM-412)
    let leaked = count(
        "SELECT COUNT(*) FROM spans WHERE input_json LIKE '%bees%' OR output_json LIKE '%bees%'",
    )
    .await;
    assert_eq!(leaked, 0);
}

/// FERB's own understanding (inferred, not imported) said once now: two preferences, their ids.
async fn own_understanding(d: &Daemon) -> Vec<String> {
    d.on(|st| {
        Box::pin(async move {
            let mut ids = Vec::new();
            for (key, content) in [
                ("drink", "The owner prefers tea"),
                ("browser", "The owner uses Arc"),
            ] {
                let said = st.journal("chat.message", json!({}), None).unwrap();
                let id = st
                    .memory
                    .save(
                        "preference",
                        "owner",
                        key,
                        content,
                        "inferred",
                        &[("journal".into(), said.id.clone())],
                        0.6,
                        &[],
                        "",
                        Some(false),
                    )
                    .unwrap()
                    .item
                    .id;
                ids.push(id);
            }
            ids
        })
    })
    .await
}
