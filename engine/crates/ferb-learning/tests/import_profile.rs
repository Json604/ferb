//! The imported profile, the work log and the export files (MEM-432..446; the owner's decisions from the
//! memory-import lab), end to end through the real import job, memory items and learned nodes with the lab's
//! stand-in model (`support::stand_in_answer`) on invented history in the lab's shapes. Temporary folders only.

mod support;

use std::rc::Rc;

use serde_json::{json, Value};

use ferb_learning::import_job::ImportJob;
use ferb_learning::import_profile::{self as profile, Line};
use ferb_learning::{py, sql};
use support::*;

const S1: &str = "11111111-1111-4111-8111-111111111111";
const S2: &str = "22222222-2222-4222-8222-222222222222";
/// Session ids shaped like the tools' own (hex), which the redactor leaves alone.
const R1: &str = "1a2b3c4d-5e6f-4a1b-8c9d-0e1f2a3b4c5d";
const R2: &str = "7f3e9c1a-2b4d-4e6f-8a0b-1c2d3e4f5a6b";

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
}

fn at(env: &Env, when: &str) {
    *env.clock.0.borrow_mut() = py::parse(when).unwrap();
}

/// Lines typed into Claude Code: (time, session, folder, text).
fn typed(env: &Env, rows: &[(&str, &str, &str, &str)]) {
    let path = env.root.join("user/.claude/history.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    for (when, session, folder, said) in rows {
        let t = ferb_memory::times::instant(&py::parse(&format!("{when}+00:00")).unwrap()) / 1000;
        text.push_str(
            &json!({"display": said, "timestamp": t, "sessionId": session,
                    "project": format!("/Users/someone/code/{folder}")})
            .to_string(),
        );
        text.push('\n');
    }
    std::fs::write(path, text).unwrap();
}

fn stand_in(env: &Env) {
    *env.answers.borrow_mut() = vec![json!({"stand_in": true}); 200].into();
}

fn calls(env: &Env) -> usize {
    env.events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e[0] == "model.start")
        .count()
}

/// One import of `source` and its placement, as the daemon runs them.
fn import(env: &Env, job: &ImportJob<'_>, source: &str, path: &str) -> Value {
    let run = job.create(source, path).unwrap();
    let rt = rt();
    let row = rt.block_on(job.run(&run)).unwrap();
    rt.block_on(job.place_run(&run)).unwrap();
    let _ = env;
    json!({"run": run, "row": row})
}

fn line_of(job: &ImportJob<'_>, needle: &str) -> Line {
    job.lines()
        .unwrap()
        .into_iter()
        .find(|l| l.content.contains(needle))
        .unwrap_or_else(|| panic!("no line holds {needle}"))
}

// decision: MEM-432, MEM-433, MEM-439, MEM-402
#[test]
fn what_was_typed_becomes_profile_lines_and_a_later_run_reads_only_what_is_new() {
    let env = Env::new("local", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    typed(
        &env,
        &[
            (
                "2026-11-01T10:00:00",
                S1,
                "garden",
                "I grow tomatoes on the balcony",
            ),
            (
                "2026-11-01T10:01:00",
                S1,
                "garden",
                "make the shed door blue",
            ),
            ("2026-11-02T10:00:00", S2, "shed", "I prefer short answers"),
        ],
    );
    stand_in(&env);
    let job = env.importer();
    let first = import(&env, &job, "claude_code", "");
    assert_eq!(first["row"]["status"], "done");
    let lines = job.lines().unwrap();
    assert_eq!(lines.len(), 2, "{lines:?}");
    let tomatoes = line_of(&job, "tomatoes");
    let item = env.memory.get(&tomatoes.item_id).unwrap().unwrap();
    assert_eq!(item.origin, "inferred");
    assert!(
        item.about.starts_with("imported from Claude Code"),
        "{}",
        item.about
    );
    let place: String = env
        .store
        .one(
            "SELECT place FROM memory_import_facts WHERE line_id = ?",
            sql![tomatoes.id],
        )
        .unwrap()
        .unwrap()
        .s_at(0);
    assert_eq!(place, "garden");
    // nothing new: nothing is read again
    let before = calls(&env);
    import(&env, &job, "claude_code", "");
    assert_eq!(calls(&env), before);
    // a new day: only its conversation is read; the line it repeats keeps its id and words
    at(&env, "2026-11-12T12:00:00+00:00");
    typed(
        &env,
        &[
            (
                "2026-11-12T09:00:00",
                S1,
                "garden",
                "I grow tomatoes on the balcony",
            ),
            ("2026-11-12T09:05:00", S1, "garden", "I keep bees too"),
        ],
    );
    let before = calls(&env);
    import(&env, &job, "claude_code", "");
    let recall_texts: Vec<String> = env
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e[0] == "model.start")
        .skip(before)
        .map(|e| e[1]["parts"].to_string())
        .filter(|p| !p.contains("Candidate ") && !p.contains("N | ") && !p.contains("L | "))
        .collect();
    assert_eq!(recall_texts.len(), 1);
    assert!(recall_texts[0].contains("bees") && !recall_texts[0].contains("short answers"));
    assert_eq!(line_of(&job, "tomatoes").id, tomatoes.id);
    assert_eq!(job.lines().unwrap().len(), 3);
}

// decision: MEM-442, MEM-409, MEM-2
#[test]
fn the_owners_corrections_outrank_every_later_import() {
    let env = Env::new("corrections", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    typed(
        &env,
        &[
            ("2026-11-01T10:00:00", S1, "garden", "I live in Lisbon"),
            ("2026-11-01T10:01:00", S1, "garden", "I own a red bike"),
            ("2026-11-01T10:02:00", S1, "garden", "I study law"),
        ],
    );
    stand_in(&env);
    let job = env.importer();
    import(&env, &job, "claude_code", "");
    // his own words replace one line; he removes another; he says a third is over
    let lisbon = line_of(&job, "Lisbon");
    env.memory
        .edit(
            &lisbon.item_id,
            "The owner lives in Porto",
            &("chat".into(), "m1".into()),
        )
        .unwrap();
    let bike = line_of(&job, "bike");
    env.memory
        .retract(&bike.item_id, "not right", true)
        .unwrap();
    let law = line_of(&job, "law");
    env.memory
        .retire(&law.item_id, "the owner said it is over")
        .unwrap();
    // later he says all three again (law after it ended), and something new
    at(&env, "2026-11-20T12:00:00+00:00");
    typed(
        &env,
        &[
            (
                "2026-11-20T10:00:00",
                S2,
                "garden",
                "I live in Lisbon these days",
            ),
            (
                "2026-11-20T10:01:00",
                S2,
                "garden",
                "I own a red bike still",
            ),
            ("2026-11-20T10:02:00", S2, "garden", "I study law again"),
        ],
    );
    let run = import(&env, &job, "claude_code", "");
    let left: Vec<(String, String)> = env
        .store
        .query(
            "SELECT content, left_why FROM memory_import_facts WHERE left_why IS NOT NULL",
            sql![],
        )
        .unwrap()
        .iter()
        .map(|r| (r.s("content"), r.s("left_why")))
        .collect();
    assert!(
        left.contains(&(
            "The owner live in Lisbon these days".into(),
            profile::OWNER.into()
        )),
        "{left:?}"
    );
    assert!(
        left.contains(&(
            "The owner own a red bike still".into(),
            profile::WRONG.into()
        )),
        "{left:?}"
    );
    // his words stand on the item, untouched
    let now_lisbon = env
        .memory
        .items(Some("active"), Some("owner"), true)
        .unwrap()
        .into_iter()
        .find(|i| i.key == lisbon.key)
        .unwrap();
    assert_eq!(
        (now_lisbon.origin.as_str(), now_lisbon.content.as_str()),
        ("owner_stated", "The owner lives in Porto")
    );
    // the line he ended is current again: said after he ended it
    let law_now = line_of(&job, "law");
    assert!(!law_now.data.contains_key("done_by_owner"));
    assert!(env
        .memory
        .get(&law_now.item_id)
        .unwrap()
        .unwrap()
        .valid_to
        .is_none());
    assert!(run["run"].is_string());
}

// decision: MEM-443 (the owner's IMP-42)
#[test]
fn a_line_is_general_unless_its_words_tie_it_and_the_owners_scope_is_final() {
    let env = Env::new("scope", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    typed(
        &env,
        &[
            (
                "2026-11-01T10:00:00",
                S1,
                "kite",
                "I prefer tests before each commit for the Kite repo",
            ),
            (
                "2026-11-01T10:01:00",
                S1,
                "kite",
                "I prefer plans before code",
            ),
        ],
    );
    stand_in(&env);
    let job = env.importer();
    import(&env, &job, "claude_code", "");
    let tied = line_of(&job, "commit");
    // its words name the project; a project named as its folder is (IMP-43)
    assert_eq!(tied.s("scope"), "kite");
    assert_eq!(tied.s("scope_by"), "words");
    let item = env.memory.get(&tied.item_id).unwrap().unwrap();
    assert!(
        item.about.contains("only in the kite project"),
        "{}",
        item.about
    );
    let general = line_of(&job, "plans");
    assert_eq!(
        general.s("scope"),
        "",
        "said only in one project, its words tie it to none"
    );
    assert_eq!(general.data["said_in"]["projects"], json!(["kite"]));
    // his word makes the tied one general, and no later reading moves it
    job.scope_by_owner(&tied.item_id, "general").unwrap();
    rt().block_on(job.judge_scope("x")).unwrap();
    let again = line_of(&job, "commit");
    assert_eq!(
        (again.s("scope").as_str(), again.s("scope_by").as_str()),
        ("", "owner")
    );
    assert!(!env
        .memory
        .get(&again.item_id)
        .unwrap()
        .unwrap()
        .about
        .contains("only in"));
}

// decision: MEM-445 (the owner's IMP-47)
#[test]
fn a_repeat_joins_the_always_on_profile_at_once_only_when_said_lately() {
    let env = Env::new("joins", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    typed(
        &env,
        &[
            ("2026-11-01T10:00:00", S1, "a", "I run every morning"),
            ("2026-11-03T10:00:00", S2, "b", "I run every morning"),
            ("2026-09-01T10:00:00", S1, "a", "I sail every summer"),
            ("2026-09-03T10:00:00", S2, "b", "I sail every summer"),
            ("2026-11-05T10:00:00", S2, "b", "I play chess"),
        ],
    );
    stand_in(&env);
    let job = env.importer();
    import(&env, &job, "claude_code", "");
    // repeats in the history are dropped by the reader; a repeat on two days reaches the profile as one fact each
    // time it is said in a new import, so here the two days come from two imports
    let run_line = line_of(&job, "run");
    let holds = |l: &Line| {
        let item = env.memory.get(&l.item_id).unwrap().unwrap();
        env.profile.holds_up(&item, None).unwrap()
    };
    assert!(!holds(&run_line), "said on one day so far");
    at(&env, "2026-11-12T12:00:00+00:00");
    typed(
        &env,
        &[("2026-11-12T08:00:00", S2, "b", "I still run every morning")],
    );
    import(&env, &job, "claude_code", "");
    let _ = run_line;
    let lines = job.lines().unwrap();
    let joined: Vec<&Line> = lines
        .iter()
        .filter(|l| l.data["holds_up"] == json!(true))
        .collect();
    for l in &joined {
        assert!(holds(l) == job.joins(l));
    }
    let chess = line_of(&job, "chess");
    assert!(
        !holds(&chess),
        "one day, one assistant: waits the normal way"
    );
}

// decision: MEM-444 (the owner's IMP-46, IMP-50, IMP-51)
#[test]
fn older_offers_are_asked_about_once_at_most_five_newest_first() {
    let env = Env::new("offers", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    let mut rows = Vec::new();
    let days = [
        "2026-10-04",
        "2026-10-03",
        "2026-10-02",
        "2026-10-01",
        "2026-09-30",
        "2026-09-29",
        "2026-08-15",
    ];
    let words = [
        "I water plants every day",
        "I pay rent every month",
        "I call mum every sunday",
        "I back up photos every week",
        "I clean the bike every saturday",
        "I bake bread every friday",
        "I file taxes every year",
    ];
    let stamps: Vec<String> = days.iter().map(|d| format!("{d}T10:00:00")).collect();
    for (i, w) in words.iter().enumerate() {
        rows.push((stamps[i].as_str(), S1, "home", *w));
    }
    rows.push((
        "2026-11-08T10:00:00",
        S2,
        "home",
        "I walk the dog every evening",
    ));
    typed(&env, &rows);
    stand_in(&env);
    let job = env.importer();
    let run = import(&env, &job, "claude_code", "");
    let lines = job.lines().unwrap();
    let (still, aside) = job.offers(&lines);
    assert_eq!(still.len(), 1, "only the recent offer is made");
    assert!(still[0].content.contains("dog"));
    assert_eq!(aside.len(), 7);
    // the notice tells; the ask is FERB's own question, asked once (MEM-444)
    job.tell_done(run["run"].as_str().unwrap()).unwrap();
    let told: Vec<Value> = env
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e[0] == "tell")
        .map(|e| e[1].clone())
        .collect();
    let notice = told.last().unwrap()["text"].as_str().unwrap().to_owned();
    assert!(!notice.contains(" AI"), "never 'AI' in owner-facing words");
    let (text, ident) = job.ask_now().unwrap().expect("an ask");
    assert!(text.starts_with("Are these still on?\n"), "{text}");
    // five offers 30-60 days old, newest first (the August one is only listed)
    assert!(
        text.contains("1. Offer to help with recurring_water_plants"),
        "{text}"
    );
    assert!(
        text.contains("5. Offer to help with recurring_clean_the"),
        "{text}"
    );
    assert!(
        !text.contains("recurring_bake"),
        "the sixth waits; it is never asked"
    );
    assert!(
        text.ends_with("Just say which, like \"first one yes, the rest are done\"."),
        "{text}"
    );
    assert_eq!(ident["items"].as_array().unwrap().len(), 5);
    // asked once: nothing is asked again
    assert!(job.ask_now().unwrap().is_none());
    // his reply, read by the model, applied by code
    let applied = rt()
        .block_on(job.apply_answer(&ident, "first one yes, the rest are done"))
        .unwrap();
    assert_eq!(
        (applied["still_on"].as_i64(), applied["set_aside"].as_i64()),
        (Some(1), Some(4))
    );
    assert_eq!(
        ImportJob::answer_line(&applied),
        "Got it: I'll keep 1 and I've put 4 aside."
    );
    let water = line_of(&job, "water");
    assert_eq!(water.data["offer_answer"]["what"], "open");
    let (still, _) = job.offers(&job.lines().unwrap());
    assert!(
        still.iter().any(|l| l.id == water.id),
        "his 'still on' makes it an offer again"
    );
    assert_eq!(line_of(&job, "rent").data["offer_answer"]["what"], "done");
    // his answer on the Memory page: done, never offered again
    job.answer_offer(&water.item_id, "done").unwrap();
    let (_, aside) = job.offers(&job.lines().unwrap());
    assert!(!aside.iter().any(|(l, _)| l.id == water.id));
}

// decision: MEM-441 (the owner's IMP-1, IMP-6, IMP-17)
#[test]
fn the_work_log_is_found_by_the_one_search_and_kept_up_to_date_cheaply() {
    let env = Env::new("worklog", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    typed(
        &env,
        &[
            (
                "2026-10-14T10:00:00",
                R1,
                "net",
                "choose a relay for the phone link",
            ),
            ("2026-10-14T10:30:00", R1, "net", "compare the two relays"),
            ("2026-11-02T10:00:00", R2, "garden", "/clear"),
            (
                "2026-11-02T10:01:00",
                R2,
                "garden",
                "water schedule for tomatoes",
            ),
        ],
    );
    stand_in(&env);
    let job = env.importer();
    let rt = rt();
    let got = rt.block_on(job.worklog_update(&env.learning)).unwrap();
    assert_eq!(
        (got["new"].as_u64(), got["calls"].as_u64()),
        (Some(2), Some(1))
    );
    let found = env
        .learning
        .search(
            "relay",
            &ferb_learning::learning::Query {
                kind: Some("work_log"),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(found.len(), 1);
    let body = &found[0]["body"];
    assert_eq!(body["thread_id"], json!(R1));
    assert_eq!(body["folder"], json!("net"));
    assert_eq!(body["reopen"], json!(format!("claude --resume {R1}")));
    assert!(
        !found[0]["body"]
            .to_string()
            .contains("compare the two relays"),
        "no prompt text is kept"
    );
    // "what did I do about relays in October": the session's own day dates it
    let october = env
        .learning
        .search(
            "relay",
            &ferb_learning::learning::Query {
                since: Some("2026-10-01"),
                until: Some("2026-10-31T23:59"),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(october.len(), 1);
    let text = ferb_learning::notes::shown_text("work_log", "", body, "");
    assert!(
        text.contains("Claude Code session") && text.contains("reopen: claude --resume"),
        "{text}"
    );
    // nothing new: no call; a grown day: one entry again
    let again = rt.block_on(job.worklog_update(&env.learning)).unwrap();
    assert_eq!(again["calls"].as_u64(), Some(0));
    typed(
        &env,
        &[("2026-11-02T11:00:00", R2, "garden", "and the peppers")],
    );
    let grown = rt.block_on(job.worklog_update(&env.learning)).unwrap();
    assert_eq!(
        (grown["grown"].as_u64(), grown["calls"].as_u64()),
        (Some(1), Some(1))
    );
}

// decision: MEM-440 (the owner's IMP-32), MEM-438
#[test]
fn an_export_ferb_fetched_is_deleted_when_done_and_the_owners_own_file_never() {
    let env = Env::new("files", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    let chat = |id: &str, text: &str, when: &str| {
        json!({"uuid": id, "name": format!("About {id}"), "chat_messages": [
            {"sender": "human", "created_at": when, "content": [{"type": "text", "text": text}]}]})
    };
    let fetched = env.root.join("workspace/downloads/claude-export");
    std::fs::create_dir_all(&fetched).unwrap();
    std::fs::write(
        fetched.join("conversations.json"),
        json!([chat("c1", "I play the cello", "2026-11-01T10:00:00Z")]).to_string(),
    )
    .unwrap();
    let given = env.root.join("Downloads/mine");
    std::fs::create_dir_all(&given).unwrap();
    std::fs::write(
        given.join("conversations.json"),
        json!([chat("c2", "I play the oboe", "2026-10-01T10:00:00Z")]).to_string(),
    )
    .unwrap();
    stand_in(&env);
    let job = env.importer();
    let run = job.create("claude", &fetched.to_string_lossy()).unwrap();
    job.fetched(&fetched.to_string_lossy(), "claude", &run)
        .unwrap();
    let rt = rt();
    rt.block_on(job.run(&run)).unwrap();
    rt.block_on(job.place_run(&run)).unwrap();
    let gone = job.delete_fetched(Some(&run)).unwrap();
    assert_eq!(gone.len(), 1);
    assert!(!fetched.exists());
    assert!(line_of(&job, "cello").id.starts_with('l'));
    // the owner's own export, older than the one imported: nothing is sent, and it is left as it was
    let before = calls(&env);
    let mine = job.create("claude", &given.to_string_lossy()).unwrap();
    let row = rt.block_on(job.run(&mine)).unwrap();
    assert!(row["counts"]["older_export"].is_string());
    assert_eq!(calls(&env), before);
    assert!(job.delete_fetched(Some(&mine)).unwrap().is_empty());
    assert!(given.join("conversations.json").exists());
    job.tell_done(&mine).unwrap();
    let told: Vec<Value> = env
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e[0] == "tell")
        .map(|e| e[1].clone())
        .collect();
    assert!(told.last().unwrap()["text"]
        .as_str()
        .unwrap()
        .contains("is older than the one I brought in on"));
    // a fetched export whose import failed is deleted a day later
    let broken = env.root.join("workspace/downloads/chatgpt.zip");
    std::fs::write(&broken, "<html>expired</html>").unwrap();
    let bad = job.create("chatgpt", &broken.to_string_lossy()).unwrap();
    job.fetched(&broken.to_string_lossy(), "chatgpt", &bad)
        .unwrap();
    let row = rt.block_on(job.run(&bad)).unwrap();
    assert_eq!(row["status"], "failed");
    assert!(row["error"]
        .as_str()
        .unwrap()
        .contains("isn't a complete zip file"));
    assert!(job.delete_fetched(None).unwrap().is_empty());
    at(&env, "2026-11-11T13:00:00+00:00");
    assert_eq!(job.delete_fetched(None).unwrap().len(), 1);
    assert!(!broken.exists());
    let _ = Rc::clone(&env);
}
