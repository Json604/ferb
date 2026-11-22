//! The imported profile, the work log and the export files (MEM-432..446; the owner's decisions from the
//! memory-import lab), end to end through the real import job, memory items and learned nodes with the lab's
//! stand-in model (`support::stand_in_answer`) on invented history in the lab's shapes. Temporary folders only.

mod support;

use std::collections::BTreeSet;
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

/// The review's part, as `memory.review` answers it: (item id, text, note) per item, in the order shown.
fn part(job: &ImportJob<'_>, name: &str) -> Vec<(String, String, String)> {
    let review = job.review().unwrap();
    review["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["part"] == name)
        .flat_map(|p| p["groups"].as_array().unwrap().clone())
        .flat_map(|g| g["items"].as_array().unwrap().clone())
        .map(|i| {
            (
                i["id"].as_str().unwrap().to_owned(),
                i["text"].as_str().unwrap().to_owned(),
                i["note"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn in_profile(env: &Env, item_id: &str) -> Option<String> {
    env.store
        .one(
            "SELECT added_by FROM memory_profile WHERE item_id = ?",
            sql![item_id],
        )
        .unwrap()
        .map(|r| r.s_at(0))
}

// decision: MEM-460 (the owner, 2026-11-23: "None by themselves; it gives me an easy to read list of thing s it thinks
// shoudn be added in always on and i apprive/decline"), superseding MEM-445 for imports; IMP-47's recent repeat now
// chooses what is suggested
#[test]
fn an_import_never_puts_anything_in_always_on_it_lists_what_it_suggests_for_his_word() {
    let env = Env::new("joins", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    typed(
        &env,
        &[
            ("2026-11-01T10:00:00", S1, "a", "I prefer short answers"),
            ("2026-11-01T10:00:00", S1, "a", "I use the work laptop"),
            ("2026-09-01T10:00:00", S1, "a", "I sail every summer"),
            ("2026-11-05T10:00:00", S2, "b", "I play chess"),
        ],
    );
    stand_in(&env);
    let job = env.importer();
    import(&env, &job, "claude_code", "");
    // a second day for two of them, in a later import: a recent repeat, which used to join at once (IMP-47)
    at(&env, "2026-11-12T12:00:00+00:00");
    typed(
        &env,
        &[
            (
                "2026-11-12T08:00:00",
                S2,
                "b",
                "I prefer short answers please",
            ),
            (
                "2026-11-12T08:01:00",
                S2,
                "b",
                "I use the work laptop daily",
            ),
        ],
    );
    import(&env, &job, "claude_code", "");
    let lines = job.lines().unwrap();
    assert!(lines.len() >= 3, "{lines:?}");
    let holds = |l: &Line| {
        let item = env.memory.get(&l.item_id).unwrap().unwrap();
        env.profile.holds_up(&item, None).unwrap()
    };
    // nothing joined by itself: not in the profile, not holding up, however often or lately it was said
    for l in &lines {
        assert!(!holds(l), "{}", l.content);
        assert_eq!(in_profile(&env, &l.item_id), None, "{}", l.content);
        // still remembered: an active memory item, found by the one search's store
        let item = env.memory.get(&l.item_id).unwrap().unwrap();
        assert_eq!(item.status, "active");
    }
    assert!(
        !env.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e[0] == "profile.joined"),
        "no join was told"
    );
    // the recent repeats are what FERB suggests, grouped, in plain words
    let short = line_of(&job, "short answers");
    let laptop = line_of(&job, "laptop");
    assert!(job.joins(&short) && job.joins(&laptop));
    let suggested = part(&job, "always_on");
    let ids: Vec<&str> = suggested.iter().map(|(i, _, _)| i.as_str()).collect();
    assert_eq!(ids, vec![short.item_id.as_str(), laptop.item_id.as_str()]);
    assert!(
        suggested[0].1.starts_with("You prefer short answers"),
        "{suggested:?}"
    );
    let review = job.review().unwrap();
    let groups: Vec<&str> = review["parts"][0]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["title"].as_str().unwrap())
        .collect();
    assert_eq!(groups, vec!["How you like me to work", "About you"]);
    assert_eq!(
        review["parts"][0]["title"],
        "Keep these in mind all the time?"
    );
    let text = review.to_string();
    assert!(
        !text.contains(" AI") && !text.contains("\"AI"),
        "never 'AI'"
    );
    // Approve: in the profile as his pin, holding up; it leaves the list
    let n = job
        .answer_review(
            &env.profile,
            "always_on",
            std::slice::from_ref(&short.item_id),
            "approve",
        )
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(in_profile(&env, &short.item_id).as_deref(), Some("owner"));
    assert!(holds(&line_of(&job, "short answers")));
    // Decline: remembered only, never always on; it leaves the list
    job.answer_review(
        &env.profile,
        "always_on",
        std::slice::from_ref(&laptop.item_id),
        "decline",
    )
    .unwrap();
    assert_eq!(in_profile(&env, &laptop.item_id), None);
    assert!(!holds(&line_of(&job, "laptop")));
    assert_eq!(
        env.memory.get(&laptop.item_id).unwrap().unwrap().status,
        "active"
    );
    assert!(part(&job, "always_on").is_empty());
    assert_eq!(job.review().unwrap()["count"], 0);
    // an answered item answered again changes nothing; a later repeat never suggests it again
    assert_eq!(
        job.answer_review(
            &env.profile,
            "always_on",
            std::slice::from_ref(&laptop.item_id),
            "approve"
        )
        .unwrap(),
        0
    );
    at(&env, "2026-11-14T12:00:00+00:00");
    typed(
        &env,
        &[(
            "2026-11-14T08:01:00",
            S1,
            "a",
            "I use the work laptop again",
        )],
    );
    import(&env, &job, "claude_code", "");
    assert!(part(&job, "always_on").is_empty());
    // the weekly pass can't place a declined or unanswered imported line either
    let eligible: Vec<String> = env
        .profile
        .eligible()
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(!eligible.contains(&line_of(&job, "laptop").item_id));
    assert!(!eligible.contains(&line_of(&job, "chess").item_id));
    assert!(eligible.contains(&line_of(&job, "short answers").item_id));
}

// decision: MEM-461 (the owner, 2026-11-23: "Show me one list to review")
#[test]
fn lines_an_earlier_build_put_in_always_on_stay_on_until_he_declines_each() {
    let env = Env::new("earlier", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    typed(
        &env,
        &[
            ("2026-11-01T10:00:00", S1, "a", "I prefer short answers"),
            ("2026-11-02T10:00:00", S1, "a", "I use the work laptop"),
            ("2026-11-03T10:00:00", S1, "a", "I play chess"),
        ],
    );
    stand_in(&env);
    let job = env.importer();
    import(&env, &job, "claude_code", "");
    let (short, laptop, chess) = (
        line_of(&job, "short answers"),
        line_of(&job, "laptop"),
        line_of(&job, "chess"),
    );
    // what an earlier build did by itself (MEM-445's join), and one he pinned himself on the Memory page
    for (id, by) in [
        (&short.item_id, "code"),
        (&laptop.item_id, "code"),
        (&chess.item_id, "owner"),
    ] {
        env.store
            .exec(
                "INSERT INTO memory_profile (item_id, section, position, added_by, added_at) VALUES (?, 'x', 0, ?, 'y')",
                sql![id, by],
            )
            .unwrap();
        // the earlier build marked no answer
        let mut l = line_of(&job, &env.memory.get(id).unwrap().unwrap().content);
        l.data.remove("always_on");
    }
    for l in job.lines().unwrap() {
        let mut l = l;
        l.data.remove("always_on");
        env.store
            .exec(
                "UPDATE memory_import_lines SET data_json = ? WHERE id = ?",
                sql![serde_json::Value::Object(l.data.clone()).to_string(), l.id],
            )
            .unwrap();
    }
    // the start's step: what is on stays on and waits for his word; his own pin is his answer already
    assert_eq!(job.suggest_always_on().unwrap(), 3);
    assert_eq!(job.suggest_always_on().unwrap(), 0, "marked once");
    let listed = part(&job, "always_on");
    let ids: BTreeSet<&str> = listed.iter().map(|(i, _, _)| i.as_str()).collect();
    assert_eq!(
        ids,
        BTreeSet::from([short.item_id.as_str(), laptop.item_id.as_str()])
    );
    assert!(listed
        .iter()
        .all(|(_, _, note)| note == "Already always on"));
    // decision: MEM-464 — one already always on is kept or removed, and a part all on has Keep all / Remove all
    let review = job.review().unwrap();
    let labels = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|b| b["label"].as_str().unwrap().to_owned())
            .collect()
    };
    for g in review["parts"][0]["groups"].as_array().unwrap() {
        for i in g["items"].as_array().unwrap() {
            assert_eq!(labels(&i["answers"]), vec!["Keep", "Remove"]);
        }
    }
    assert_eq!(
        labels(&review["parts"][0]["all"]),
        vec!["Keep all", "Remove all"]
    );
    let holds = |id: &str| {
        env.profile
            .holds_up(&env.memory.get(id).unwrap().unwrap(), None)
            .unwrap()
    };
    assert!(holds(&short.item_id) && holds(&laptop.item_id) && holds(&chess.item_id));
    // he declines one: it leaves always on, remembered; he approves the other: it stays, his pin now
    job.answer_review(
        &env.profile,
        "always_on",
        std::slice::from_ref(&laptop.item_id),
        "decline",
    )
    .unwrap();
    assert_eq!(in_profile(&env, &laptop.item_id), None);
    assert!(!holds(&laptop.item_id));
    job.answer_review(
        &env.profile,
        "always_on",
        std::slice::from_ref(&short.item_id),
        "approve",
    )
    .unwrap();
    assert_eq!(in_profile(&env, &short.item_id).as_deref(), Some("owner"));
    assert_eq!(job.review().unwrap()["count"], 0);
}

// decision: MEM-444 (the owner's IMP-46, IMP-50, IMP-51), MEM-462
#[test]
fn older_offers_and_lines_are_asked_once_each_under_its_own_question() {
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
    // two older lines: one that steers FERB's work, one that is only about him
    rows.push((
        "2026-09-20T10:00:00",
        S1,
        "home",
        "I prefer replies in bullet points",
    ));
    rows.push(("2026-09-21T10:00:00", S1, "home", "I find maths easy"));
    typed(&env, &rows);
    stand_in(&env);
    let job = env.importer();
    let run = import(&env, &job, "claude_code", "");
    let lines = job.lines().unwrap();
    let (still, aside) = job.offers(&lines);
    assert_eq!(still.len(), 1, "only the recent offer is made");
    assert!(still[0].content.contains("dog"));
    assert_eq!(aside.len(), 7);
    job.tell_done(run["run"].as_str().unwrap()).unwrap();
    // the review: offers under their own question, facts under theirs, never mixed
    assert!(job.review_round().unwrap());
    let offers = part(&job, "offers");
    assert_eq!(offers.len(), 5, "{offers:?}");
    assert!(
        offers[0].1.contains("recurring_water_plants"),
        "newest first: {offers:?}"
    );
    assert!(
        offers.iter().all(|(_, t, _)| !t.contains("bake")),
        "the sixth waits"
    );
    let old = part(&job, "still_true");
    assert_eq!(old.len(), 1, "{old:?}");
    assert!(old[0].1.contains("bullet points"), "{old:?}");
    assert!(old[0].2.starts_with("Last said 20 Sep 2026"), "{old:?}");
    assert!(
        !part(&job, "still_true")
            .iter()
            .any(|(_, t, _)| t.contains("maths")),
        "a fact that steers nothing is not asked"
    );
    let review = job.review().unwrap();
    let titles: Vec<&str> = review["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        vec!["Are these still true?", "Still want help with these?"]
    );
    let question = job.review_question().unwrap().unwrap();
    assert_eq!(
        question,
        "Can you check a few things I brought in?\n1 older thing that may not be true any more and 5 things I could \
         help with. Answer each one here, or just tell me."
    );
    // his buttons: one offer done, the older line still true
    let water = line_of(&job, "water");
    job.answer_review(
        &env.profile,
        "offers",
        std::slice::from_ref(&water.item_id),
        "done",
    )
    .unwrap();
    assert_eq!(line_of(&job, "water").data["offer_answer"]["what"], "done");
    let bullets = line_of(&job, "bullet points");
    job.answer_review(
        &env.profile,
        "still_true",
        std::slice::from_ref(&bullets.item_id),
        "yes",
    )
    .unwrap();
    assert!(!job.old(&line_of(&job, "bullet points")), "current again");
    // his words for the rest, read against the list as it stands now
    let applied = rt()
        .block_on(job.apply_answer(&env.profile, &json!({}), "first one yes, the rest are done"))
        .unwrap();
    assert_eq!(
        (applied["still_on"].as_i64(), applied["set_aside"].as_i64()),
        (Some(1), Some(3))
    );
    assert_eq!(
        ImportJob::answer_line(&applied),
        "Got it: I'll keep 1 and I've put 3 aside."
    );
    let rent = line_of(&job, "rent");
    assert_eq!(rent.data["offer_answer"]["what"], "open");
    assert_eq!(job.review().unwrap()["count"], 0);
    // asked once: the next import's round asks none of them again
    assert!(!job.review_round().unwrap());
    // his answer on the Memory page: done, never offered again
    job.answer_offer(&rent.item_id, "done").unwrap();
    let (_, aside) = job.offers(&job.lines().unwrap());
    assert!(!aside.iter().any(|(l, _)| l.id == rent.id));
}

// decision: MEM-460
#[test]
fn his_words_approve_or_decline_what_is_suggested() {
    let env = Env::new("words", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    typed(
        &env,
        &[
            ("2026-11-01T10:00:00", S1, "a", "I prefer short answers"),
            ("2026-11-01T10:00:00", S1, "a", "I use the work laptop"),
        ],
    );
    stand_in(&env);
    let job = env.importer();
    import(&env, &job, "claude_code", "");
    at(&env, "2026-11-12T12:00:00+00:00");
    typed(
        &env,
        &[
            (
                "2026-11-12T08:00:00",
                S2,
                "b",
                "I prefer short answers please",
            ),
            (
                "2026-11-12T08:01:00",
                S2,
                "b",
                "I use the work laptop daily",
            ),
        ],
    );
    import(&env, &job, "claude_code", "");
    assert_eq!(part(&job, "always_on").len(), 2);
    let applied = rt()
        .block_on(job.apply_answer(&env.profile, &json!({}), "first one yes, the rest are done"))
        .unwrap();
    assert_eq!(
        ImportJob::answer_line(&applied),
        "Got it: 1 always on now and 1 remembered but not always on."
    );
    assert_eq!(
        in_profile(&env, &line_of(&job, "short answers").item_id).as_deref(),
        Some("owner")
    );
    assert_eq!(in_profile(&env, &line_of(&job, "laptop").item_id), None);
    assert_eq!(job.review().unwrap()["count"], 0);
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

// decision: MEM-466 (the owner, 2026-11-23: "not keep looking at my session eveyr hour if ive not said it - adn that
// tooo the model") — FERB knows which sessions he has by code alone; a summary waits for his ask
#[test]
fn the_session_index_is_kept_by_code_and_only_his_ask_summarises() {
    let env = Env::new("index", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    typed(
        &env,
        &[
            (
                "2026-11-04T10:00:00",
                R1,
                "net",
                "choose a relay for the phone link",
            ),
            (
                "2026-11-05T10:01:00",
                R2,
                "garden",
                "water schedule for tomatoes",
            ),
        ],
    );
    stand_in(&env);
    let job = env.importer();
    // the index: no model call, every session-day there with its tool, folder, day and count, no prompt text
    let got = job.worklog_index(&env.learning).unwrap();
    assert_eq!(got["added"].as_u64(), Some(2));
    assert_eq!(calls(&env), 0);
    let kept = ferb_learning::worklog::kept(&env.learning).unwrap();
    let net = kept.values().find(|b| b["folder"] == json!("net")).unwrap();
    assert_eq!(net["thread_id"], json!(R1));
    assert_eq!(net["title"], json!("Claude Code in net"), "{net}");
    assert_eq!(net["prompts"], json!(1));
    assert_eq!(net["hash"], json!(""), "no summary yet");
    assert!(
        !net.to_string().contains("choose a relay"),
        "no prompt text"
    );
    // "what was I working on in garden?" is found by the one search, with no model
    let found = env
        .learning
        .search(
            "garden",
            &ferb_learning::learning::Query {
                kind: Some("work_log"),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(found.len(), 1);
    // nothing changed: nothing is read again
    assert_eq!(
        job.worklog_index(&env.learning).unwrap()["unchanged"],
        json!(true)
    );
    // a new prompt the next day, an hour later: the index grows by code, still no model call
    at(&env, "2026-11-10T13:00:00+00:00");
    typed(
        &env,
        &[("2026-11-10T12:30:00", R1, "net", "pick the cheaper relay")],
    );
    let grown = job.worklog_index(&env.learning).unwrap();
    assert_eq!(grown["added"].as_u64(), Some(1));
    assert_eq!(calls(&env), 0);
    // his ask: the summaries are written now, for every entry without one, and only once
    let rt = rt();
    let asked = rt.block_on(job.worklog_update(&env.learning)).unwrap();
    assert_eq!(asked["new"].as_u64(), Some(3));
    assert!(calls(&env) > 0);
    let before = calls(&env);
    let again = rt.block_on(job.worklog_update(&env.learning)).unwrap();
    assert_eq!(again["calls"].as_u64(), Some(0));
    assert_eq!(calls(&env), before);
    // the index after a summary keeps it
    typed(&env, &[("2026-11-10T12:40:00", R1, "net", "and set it up")]);
    job.worklog_index(&env.learning).unwrap();
    let kept = ferb_learning::worklog::kept(&env.learning).unwrap();
    let today = kept
        .iter()
        .find(|(k, _)| k.ends_with("2026-11-10"))
        .map(|(_, b)| b.clone())
        .unwrap();
    assert_ne!(today["hash"], json!(""), "its summary's hash is kept");
    assert_ne!(
        today["seen"], today["hash"],
        "it grew since: his next ask summarises it again"
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
