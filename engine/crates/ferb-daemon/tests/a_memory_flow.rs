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

// decision: MEM-444, MEM-460
#[tokio::test]
async fn the_review_after_an_import_is_one_question_answered_on_its_card_or_in_words() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
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
    let asked = asks(&d);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["kind"], "memory_review");
    assert_eq!(
        asked[0]["summary"],
        "Can you check a few things I brought in?"
    );
    assert_eq!(
        asked[0]["detail"],
        "1 thing I could help with. Answer each one here, or just tell me."
    );
    // asked once: the next import's round asks it no more
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

// decision: MEM-460, MEM-461, MEM-465
#[tokio::test]
async fn a_start_folds_the_old_notices_into_one_review_and_answers_close_it() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    // what an earlier build left: two imported lines it put in always on by itself, and its offers question waiting
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
                st.store
                    .exec(
                        "INSERT INTO memory_questions (approval_id, kind, target, reason, request_id, at) \
                         VALUES ('ap-old', 'memory_offers', '{\"items\": []}', 'Are these still on?', 'k', 'x')",
                        ferb_learning::sql![],
                    )
                    .unwrap();
                let converted = memory::restore_memory_questions(&st).unwrap();
                memory_import::review_start(&st, converted);
                (rent, tea)
            })
        })
        .await;
    // one review asked; the "Now always on" notices about imported lines are folded into it
    let asked = asks(&d);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0]["kind"], "memory_review");
    assert!(asked[0]["detail"]
        .as_str()
        .unwrap()
        .starts_with("2 things to keep in mind all the time"));
    let folds: Vec<Value> = d
        .stand
        .calls
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| c.get("fold_notices").cloned())
        .collect();
    let refs = folds.last().unwrap()["refs"].as_array().unwrap().clone();
    assert!(refs.contains(&json!(format!("memory-always-on-{rent}"))));
    assert!(refs.contains(&json!(format!("memory-always-on-{tea}"))));
    assert_eq!(folds.last().unwrap()["into"], "memory-review");
    // they stay on until he answers; one declined leaves always on, one approved stays as his pin
    let (r2, t2) = (rent.clone(), tea.clone());
    let left = d
        .on(move |st| {
            Box::pin(async move {
                let j = memory_import::job(&st);
                let ids = st.profile.ids().unwrap();
                assert!(ids.contains(&r2) && ids.contains(&t2));
                j.answer_review(
                    &st.profile,
                    "always_on",
                    std::slice::from_ref(&r2),
                    "decline",
                )
                .unwrap();
                memory_import::review_settled(&st);
                let left = j.review().unwrap()["count"].clone();
                j.answer_review(
                    &st.profile,
                    "always_on",
                    std::slice::from_ref(&t2),
                    "approve",
                )
                .unwrap();
                memory_import::review_settled(&st);
                let ids = st.profile.ids().unwrap();
                assert!(!ids.contains(&r2) && ids.contains(&t2));
                assert!(
                    st.memory
                        .get(&r2)
                        .unwrap()
                        .is_some_and(|i| i.status == "active"),
                    "still remembered"
                );
                left
            })
        })
        .await;
    assert_eq!(left, json!(1), "answered in parts, it stays until done");
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

// decision: MEM-472
#[tokio::test]
async fn the_hourly_pass_asks_about_a_fact_on_its_check_date_with_no_model() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
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
    assert!(asks(&d).is_empty(), "nothing before the date");
    d.clock.advance(30.0 * 86_400.0);
    // (one that waits is not asked twice: `pending_own`, which this stand-in's questions never report)
    hourly(&d).await;
    let asked = asks(&d);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0]["kind"], "memory_review");
    assert_eq!(
        asked[0]["detail"],
        "1 thing I said I'd check with you. Answer each one here, or just tell me."
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
    assert_eq!(asks(&d).len(), 1);
    d.clock.advance(7.0 * 86_400.0);
    hourly(&d).await;
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
