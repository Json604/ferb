//! T-1277: the real program across processes, restarts and time (tests/life: the composed `ferbd daemon` with
//! stand-in models and the Mac's own parts off, TEST-512), read back as the owner sees it after each step: the chat's
//! stored entries, `chat.status`, the open questions, the journal, the files of FERB's home.
//!
//! What a `kill -9` (a crash, a forced quit, a power cut) leaves is what the next start finds: no clean shutdown ran, no
//! plan was suspended, the turn's stream died with the process. The regressions for 2026-11-21's live bugs that a test
//! through the program can catch are here too (journal 158, 159).

#[path = "../../ferb-supervisor/tests/support_entries/mod.rs"]
mod entry_rules;
mod life;
mod support;

use std::sync::atomic::{AtomicBool, Ordering};

use life::*;
use serde_json::{json, Value};
use support::*;

/// The task's answers in the chat (FERB's words for a task's end), each `(state, words)`.
fn task_answers(life: &Life, task: &str) -> Vec<(String, String)> {
    life.entries("answer")
        .iter()
        .filter(|e| e["task_id"] == task && e["body"]["from"] == "task")
        .map(|e| (e["state"].as_str().unwrap_or("").to_string(), said(e)))
        .collect()
}

fn open_entries(life: &Life) -> Vec<String> {
    life.chat()
        .iter()
        .filter(|e| e["open"] == true)
        .map(|e| format!("{} {}", e["kind"], said(e)))
        .collect()
}

/// A task running when FERB is killed (`kill -9`) runs again in place at the next start (restart.rs
/// `resume_saved_plans`: "each tree a shutdown or crash cut off runs again in place", main.py's `resume_plans`), and the
/// owner sees one turn: the message and its plan stay under way across the crash, then one answer, the resumed run's.
/// The database is the same one, never set aside; the start-up pass names what the crash cut off.
// decision: ENG-13, STORE-1, TEST-515
#[test]
fn a_task_cut_by_kill_9_runs_again_in_place_and_is_told_once() {
    let tmp = root();
    let mut life = Life::start(
        tmp.path(),
        "01",
        Model::default()
            .planner(plan_task("Say hello in the notes", "On it."))
            .worker(held(worker_done("Said hello."), "never")),
    );
    let mut turn = life.send("say hello in my notes");
    life.wait("the worker started", 30, |l| {
        l.calls_of("worker:default") == 1
    });
    let task = life.tasks()[0].0.clone();
    life.wait("the plan is in the chat", 10, |l| {
        l.entries("plan").len() == 1
    });
    assert_eq!(
        open_entries(&life),
        vec!["\"message\" say hello in my notes", "\"plan\" On it."],
        "mid-task: the turn is under way"
    );
    let running = life.call("chat.status", json!({}))["result"]["running"].clone();
    assert_eq!(running.as_array().map(Vec::len), Some(1), "{running}");

    life.kill();
    assert_eq!(
        turn.end(),
        &json!({"closed": true}),
        "the stream dies with the process"
    );
    life.start_again(
        Model::default()
            .worker(worker_done("Said hello again."))
            .checker(always(check_accept())),
    );
    life.wait("the resumed task ended", 30, |l| {
        l.root_status().as_deref() == Some("done")
    });
    life.settle();

    assert_eq!(life.set_aside(), Vec::<String>::new(), "nothing set aside");
    assert_eq!(life.journal("home.set_aside"), Vec::<Value>::new());
    let cut = life.journal("tasks.interrupted_after_restart");
    assert_eq!(cut.len(), 1, "{cut:?}");
    assert!(
        cut[0]["task_ids"]
            .as_array()
            .unwrap()
            .contains(&json!(task)),
        "{cut:?}"
    );
    assert_eq!(
        life.made(),
        vec!["worker:default", "checker"],
        "one run again"
    );
    assert_eq!(
        task_answers(&life, &task),
        vec![("done".to_string(), "Said hello again.".to_string())],
        "told once, the resumed run's answer"
    );
    let answer = &life.entries("answer")[0];
    let message = &life.entries("message")[0];
    assert_eq!(answer["answers"], message["id"], "it answers the message");
    assert_eq!(open_entries(&life), Vec::<String>::new());
    assert_eq!(message["state"], "done");
    // nobody is asked to pick up a task that resumed by itself
    let status = life.call("chat.status", json!({}))["result"].clone();
    assert_eq!(status["running"], json!([]), "{status}");
    let results = status["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{status}");
    assert_eq!(results[0]["answer"], "Said hello again.", "{status}");
    assert_eq!(life.order_rules("kill_mid_task"), Vec::<String>::new());
}

/// A turn whose planner was still thinking when FERB was killed: the next start closes it (ENG-13's start-up pass: cut
/// work interrupted) with FERB.app's words for a response that ended unconfirmed, so no client shows it working forever;
/// no task was made; the next message is answered as usual.
// decision: ENG-13, TEST-515
#[test]
fn a_turn_cut_by_kill_9_before_any_reply_is_closed_and_the_next_message_answered() {
    let tmp = root();
    let mut life = Life::start(tmp.path(), "02", Model::default().planner(stall()));
    let mut turn = life.send("what's on my calendar");
    life.wait("the planner started", 30, |l| {
        l.calls_of("orchestrator") == 1
    });
    life.wait("the message is in the chat", 10, |l| {
        l.entries("message").len() == 1
    });
    life.kill();
    turn.end();
    life.start_again(Model::default().planner(plan_reply("Nothing today.")));
    let message = &life.entries("message")[0];
    assert_eq!(message["open"], false, "{message}");
    assert_eq!(message["state"], "interrupted", "{message}");
    assert_eq!(
        message["body"]["notice"],
        "The response ended before FERB confirmed it. Check tasks before resubmitting.",
        "{message}"
    );
    assert_eq!(life.tasks(), Vec::<(String, String)>::new());
    assert_eq!(life.made(), Vec::<String>::new(), "nothing runs again");

    let mut next = life.send("anything tomorrow?");
    assert!(next.end().get("result").is_some(), "{:?}", next.ended);
    let answers = life.entries("answer");
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(said(&answers[0]), "Nothing today.");
    assert_eq!(open_entries(&life), Vec::<String>::new());
    assert_eq!(
        life.order_rules("kill_while_planning"),
        Vec::<String>::new()
    );
}

/// A safety question waiting when FERB is killed: the next start closes the old one (ENG-13: open questions expired),
/// never as the owner's No (ORCH-684's rule for a question its run left), and the step that asked runs again and asks
/// again; the owner's Yes to the new one carries the task to its end, the file written once.
// decision: ENG-13, ORCH-684, TEST-515
#[test]
fn a_question_open_at_kill_9_is_closed_never_as_a_no_and_asked_again() {
    let write = (
        "fs.write",
        json!({"path": "~/.zshrc", "content": "alias ll='ls -l'\n", "overwrite": true, "effect": "edit",
               "why": "add the alias"}),
    );
    let tmp = root();
    let mut life = Life::start(
        tmp.path(),
        "03",
        Model::default()
            .planner(plan_task(
                "Add the alias ll to ~/.zshrc",
                "Adding the alias.",
            ))
            .worker(with_tools(
                worker_done("Added."),
                std::slice::from_ref(&write),
            )),
    );
    let mut turn = life.send("add an alias ll for ls -l to my zshrc");
    life.wait("the question waits", 30, |l| !l.open_questions().is_empty());
    let first = life.open_questions()[0]["approval_id"].clone();
    life.wait("the question is in the chat", 10, |l| {
        l.entries("question").len() == 1
    });
    life.kill();
    turn.end();
    life.start_again(
        Model::default()
            .worker(with_tools(worker_done("Added."), &[write]))
            // the check reads the file the step changed before it says met (the code refuses a pass that didn't)
            .checker(with_tools(
                check_accept(),
                &[("fs.read", json!({"path": "~/.zshrc"}))],
            )),
    );
    life.wait("asked again", 30, |l| {
        l.open_questions().iter().any(|q| q["approval_id"] != first)
    });
    let again = life.open_questions()[0]["approval_id"].clone();
    let old = life
        .entries("question")
        .into_iter()
        .find(|e| e["approval_id"] == first)
        .expect("the first question's entry stays");
    assert_eq!(old["open"], false, "{old}");
    assert_eq!(old["state"], "expired", "never declined: {old}");
    let granted = life.call(
        "chat.approve",
        // FERB.app's Yes: the question's first option
        json!({"approval_id": again, "granted": true, "choice": 1}),
    );
    assert_eq!(granted["result"]["state"], "granted", "{granted}");
    life.wait("the task ended", 30, |l| {
        l.root_status().as_deref() == Some("done")
    });
    life.settle();
    let resolved = life.journal("approval.resolved");
    assert!(
        resolved.iter().all(|r| r["state"] != "denied"),
        "no No anyone gave: {resolved:?}"
    );
    assert_eq!(
        std::fs::read_to_string(life.run.user.join(".zshrc")).unwrap_or_default(),
        "alias ll='ls -l'\n",
        "written once, after the Yes"
    );
    let task = life.tasks()[0].0.clone();
    assert_eq!(
        task_answers(&life, &task),
        vec![("done".to_string(), "Added.".to_string())]
    );
    assert_eq!(open_entries(&life), Vec::<String>::new());
    assert_eq!(life.order_rules("kill_with_question"), Vec::<String>::new());
}

/// 2026-11-21, journal 158: the owner's healthy database was set aside twice while FERB.app's quiet update ran `ferb
/// setup --service --update --app-service` beside the running daemon, whose writes made setup's database check read
/// stale search-index pages as damage. Here the real daemon writes its trail and merges the index, through its own
/// connection, while FERB.app's setup runs five times beside it (programs only recorded, never launchctl): the
/// database is never set aside, setup says nothing was, and the daemon goes on serving the same chat.
// decision: STORE-429, STORE-430, STORE-1, TEST-515
#[test]
fn setup_beside_a_writing_daemon_never_sets_the_database_aside() {
    let tmp = root();
    let life = Life::start(
        tmp.path(),
        "04",
        Model::default().planner(plan_reply("Hello.")),
    );
    life.send("hi").end();
    // a trail with an index worth merging
    life.run.control(json!({"do": "sql", "statements": [[
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000) \
         INSERT INTO trail(id, at, day, device_id, actor, source, data_json) \
         SELECT 'seed' || i, '2026-01-01T09:00:00+00:00', '2026-01-01', 'd', 'owner', 'app', \
                json_object('t', 'seed word' || i || ' other' || (i % 7)) FROM n", []]]}));
    let stop = AtomicBool::new(false);
    let runs = tmp.path().join("setup-runs.jsonl");
    std::fs::write(tmp.path().join("answers.json"), "[]").unwrap();
    let setups: Vec<(i32, String, String)> = std::thread::scope(|scope| {
        // the daemon's own writes, as its sensing writes the trail: rows in and out, the index merged
        scope.spawn(|| {
            let mut n = 0;
            while !stop.load(Ordering::Relaxed) {
                n += 1;
                let mut statements = vec![json!([
                    "INSERT INTO trail(id, at, day, device_id, actor, source, data_json) \
                     VALUES (?, '2026-01-01T09:00:00+00:00', '2026-01-01', 'd', 'owner', 'app', ?)",
                    [
                        format!("w{n}"),
                        format!(r#"{{"t": "busy word{n} other{}"}}"#, n % 7)
                    ]
                ])];
                if n % 5 == 0 {
                    statements.push(json!([
                        "DELETE FROM trail WHERE seq = (SELECT min(seq) FROM trail)",
                        []
                    ]));
                }
                if n % 20 == 0 {
                    statements.push(json!([
                        "INSERT INTO trail_fts(trail_fts, rank) VALUES('merge', 16)",
                        []
                    ]));
                }
                life.run
                    .control(json!({"do": "sql", "statements": statements}));
            }
        });
        let out = (0..5)
            .map(|_| {
                let out = std::process::Command::new(BIN)
                    .args(["setup", "--service", "--update", "--app-service"])
                    .env_clear()
                    .env("FERB_HOME", &life.run.home)
                    .env("HOME", &life.run.user)
                    .env("PATH", "/usr/bin:/bin")
                    .env("FERBD_FAKE_RUN", &runs)
                    .env("FERBD_FAKE_ANSWERS", tmp.path().join("answers.json"))
                    .env("FERBD_APP_BUNDLE", tmp.path().join("Applications/FERB.app"))
                    .stdin(std::process::Stdio::null())
                    .output()
                    .unwrap();
                (
                    out.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&out.stdout).into_owned(),
                    String::from_utf8_lossy(&out.stderr).into_owned(),
                )
            })
            .collect();
        stop.store(true, Ordering::Relaxed);
        out
    });
    for (code, out, err) in &setups {
        assert!(
            !out.contains("set aside") && !err.contains("set aside"),
            "setup {code}: {out}{err}"
        );
    }
    assert_eq!(life.set_aside(), Vec::<String>::new());
    assert_eq!(life.journal("home.set_aside"), Vec::<Value>::new());
    // the same daemon, the same chat
    let answers = life.entries("answer");
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(said(&answers[0]), "Hello.");
}

/// 2026-11-21, journal 159: "Your Mac is getting hot…" and the other alerts stayed in the phone's chat as bubbles. An
/// alert (the notifier's `alert` category) is named `alert:<id>` and never becomes a chat entry, before or after a
/// restart, while `chat.status` still lists it for every client to show for a while; a plain notice still becomes one.
// decision: APP-1105, APP-1106, TEST-515
#[test]
fn an_alert_never_becomes_a_chat_entry_before_or_after_a_restart() {
    let tmp = root();
    let mut life = Life::start(tmp.path(), "05", Model::default());
    life.run.control(
        json!({"do": "notice", "ident": "insomnia-hot-1", "category": "alert",
                            "text": "Your Mac is getting hot, so FERB let it sleep."}),
    );
    life.run.control(
        json!({"do": "notice", "ident": "learned-1", "text": "FERB learned how you file receipts."}),
    );
    life.wait("the plain notice is in the chat", 10, |l| {
        l.entries("notice").len() == 1
    });
    let notices = |life: &Life| -> Vec<String> {
        life.entries("notice")
            .iter()
            .map(|e| format!("{} {}", e["ref"], said(e)))
            .collect()
    };
    assert_eq!(
        notices(&life),
        vec!["\"learned-1\" FERB learned how you file receipts."]
    );
    let status = life.call("chat.status", json!({}));
    let alert = status["result"]["notices"]
        .as_array()
        .and_then(|a| a.iter().find(|n| n["id"] == "alert:insomnia-hot-1"))
        .cloned()
        .unwrap_or_else(|| panic!("the alert is held for the clients: {status}"));
    assert_eq!(alert["alert"], true, "{alert}");

    life.stop();
    life.start_again(Model::default());
    life.settle();
    assert_eq!(
        notices(&life),
        vec!["\"learned-1\" FERB learned how you file receipts."],
        "after the restart too"
    );
}

/// PHN-488: "a crash or shutdown of the daemon closes the pipe, so the helper never outlives it". The phone link's
/// helper (a stand-in script, never the real program: tests/link_helper.rs) runs while the phone feature is on; a
/// `kill -9` of the daemon ends it too, and the next start runs it again by itself, the feature still on, with no
/// action from FERB.app: a phone's session has a helper to come back to.
// decision: PHN-488, TEST-515
#[test]
fn the_phone_link_helper_ends_with_a_killed_daemon_and_runs_again_at_the_start() {
    let tmp = root();
    let seen = tmp.path().join("seen");
    std::fs::create_dir_all(&seen).unwrap();
    let script = tmp.path().join("helper");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nn=$(ls {o} | wc -l | tr -d ' ')\necho $$ > {o}/pid-$n\nIFS= read -r keys\n\
             cat > /dev/null\necho ended > {o}/ended-$n\n",
            o = seen.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let pids = || -> Vec<String> {
        let mut files: Vec<_> = std::fs::read_dir(&seen)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("pid-"))
            .collect();
        files.sort();
        files
            .iter()
            .map(|f| {
                std::fs::read_to_string(seen.join(f))
                    .unwrap()
                    .trim()
                    .to_string()
            })
            .collect()
    };
    let alive = |pid: &str| {
        std::process::Command::new("/bin/kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let mut life = Life::start_with(
        tmp.path(),
        "07",
        Model::default(),
        json!({"link_helper": script.to_string_lossy()}),
    );
    let on = life.call("phone.enable", json!({"on": true}));
    assert_eq!(on["result"]["enabled"], true, "{on}");
    life.wait("the helper runs", 10, |_| pids().len() == 1);
    let first = pids()[0].clone();
    life.kill();
    life.wait("the helper ended with the killed daemon", 10, |_| {
        !alive(&first)
    });
    life.start_again(Model::default());
    life.wait("the helper runs again by itself", 10, |_| pids().len() == 2);
    let second = pids()[1].clone();
    assert!(alive(&second));
    let st = life.call("phone.link_status", json!({}));
    assert_eq!(st["result"]["enabled"], true, "{st}");
}

/// Time across a crash: a timer the owner set from chat is kept in the store, so a `kill -9` before it is due loses
/// nothing; at the next start, when its time comes (the fake clock moved on), it fires once and is posted in the chat
/// once; a later tick and another restart post it again never (ORCH-511: schedule bookkeeping survives crashes; ENG-13:
/// a post is made once, by its `reminder:<timer>` id).
// decision: ORCH-511, ENG-13, TEST-515
#[test]
fn a_timer_set_before_kill_9_fires_once_after_the_restart() {
    let words = "set a timer for 2 minutes to take the bread out";
    let timer = planner_answer(
        json!({"kind": "reply", "answers_question": false, "task": null, "control": null,
        "side_entries": {"answer_question": null, "remember": [], "forget": [],
            "timers": [{"when": {"in_minutes": 2, "in_days": null, "day": null, "date": null, "time": null},
                        "message": "Take the bread out", "quote": "a timer for 2 minutes", "when_active": false,
                        "name": "Bread", "kind": "timer"}],
            "schedules": [], "schedule_changes": [], "show_schedules": false, "reminder_cancels": [],
            "conversation_changes": []},
        "text": "Timer set for 2 minutes."}),
    );
    let tmp = root();
    let mut life = Life::start(tmp.path(), "08", Model::default().planner(timer));
    life.send(words).end();
    let listed = life.call("reminders.list", json!({}));
    assert!(
        listed.to_string().contains("Take the bread out"),
        "{listed}"
    );
    life.kill();
    life.start_again(Model::default());
    let early = life.run.control(json!({"do": "timers_tick"}));
    assert_eq!(early, json!(0), "not due yet");
    life.run.control(json!({"do": "clock", "advance_s": 120}));
    let fired = life.run.control(json!({"do": "timers_tick"}));
    assert_eq!(fired, json!(1), "fired once when due");
    life.wait("the timer is posted", 10, |l| l.entries("post").len() == 1);
    assert_eq!(life.run.control(json!({"do": "timers_tick"})), json!(0));
    life.stop();
    life.start_again(Model::default());
    life.run.control(json!({"do": "clock", "advance_s": 600}));
    assert_eq!(life.run.control(json!({"do": "timers_tick"})), json!(0));
    life.settle();
    let posts = life.entries("post");
    assert_eq!(posts.len(), 1, "posted once: {posts:?}");
    assert_eq!(life.order_rules("timer_across_kill"), Vec::<String>::new());
}
