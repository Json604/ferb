//! A line FERB brought in from the owner's coding-agent chats (the import) is never always on (the owner, 2026-11-26:
//! "never alwasy on", MEM-489; it ends MEM-483's "yeah keep them" for the 65 lines he approved with one Approve all
//! on 23 Nov). Whatever answer, pin, Keep or Approve it has, it reaches no prompt, is never offered in the review, and
//! the rows an earlier build left leave once at the start; nothing is deleted, and the memory search still finds it.
//! His own words are a different item and stay always on. Through the real import job (the lab's stand-in model),
//! memory items, core profile and search. Temporary folders only; time is injected; invented data.

mod support;

use std::collections::BTreeSet;

use serde_json::{json, Value};

use ferb_learning::import_job::ImportJob;
use ferb_learning::import_profile::Line;
use ferb_learning::import_review::{ALWAYS_ON, APPROVED, IMPORTED_LEFT_KIND};
use ferb_learning::profile::IMPORTED_REFUSAL;
use ferb_learning::{py, sql};
use support::*;

const S1: &str = "11111111-1111-4111-8111-111111111111";
const S2: &str = "22222222-2222-4222-8222-222222222222";
const DAY: i64 = 86_400_000_000;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
}

fn at(env: &Env, when: &str) {
    *env.clock.0.borrow_mut() = py::parse(when).unwrap();
}

fn later(env: &Env, days: i64) {
    let now = *env.clock.0.borrow();
    *env.clock.0.borrow_mut() = py::shift(&now, days * DAY);
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

/// One import of the typed history and its placement, as the daemon runs them, with the lab's stand-in model.
fn import(env: &Env, job: &ImportJob<'_>) {
    *env.answers.borrow_mut() = vec![json!({"stand_in": true}); 200].into();
    let run = job.create("claude_code", "").unwrap();
    let rt = rt();
    rt.block_on(job.run(&run)).unwrap();
    rt.block_on(job.place_run(&run)).unwrap();
}

/// The world his store has: two lines imported, said on two days (a recent repeat, once suggested for always on).
fn imported_world(name: &str) -> (std::rc::Rc<Env>, Line, Line) {
    let env = Env::new(name, None);
    at(&env, "2026-11-10T12:00:00+00:00");
    typed(
        &env,
        &[
            ("2026-11-01T10:00:00", S1, "a", "I prefer short answers"),
            ("2026-11-01T10:00:00", S1, "a", "I use the work laptop"),
        ],
    );
    let job = env.importer();
    import(&env, &job);
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
    import(&env, &job);
    let (short, laptop) = (line_of(&job, "short answers"), line_of(&job, "laptop"));
    (env, short, laptop)
}

fn line_of(job: &ImportJob<'_>, needle: &str) -> Line {
    job.lines()
        .unwrap()
        .into_iter()
        .find(|l| l.content.contains(needle))
        .unwrap_or_else(|| panic!("no line holds {needle}"))
}

/// What an earlier build's Approve (one, or Approve all) did: the line marked approved, a profile row as his pin.
fn approved_by_an_earlier_build(env: &Env, line: &Line) {
    let mut data = line.data.clone();
    data.insert(ALWAYS_ON.into(), json!(APPROVED));
    data.insert("always_on_at".into(), json!("2026-11-12T03:40:49+00:00"));
    env.store
        .exec(
            "UPDATE memory_import_lines SET data_json = ? WHERE id = ?",
            sql![Value::Object(data).to_string(), line.id],
        )
        .unwrap();
    pinned(env, &line.item_id);
}

/// A profile row as his own pin (the Memory page's Always on of an earlier build).
fn pinned(env: &Env, item_id: &str) {
    env.store
        .exec(
            "INSERT INTO memory_profile (item_id, section, position, added_by, added_at) \
             VALUES (?, 'How you like things done', 0, 'owner', '2026-11-12T03:40:49+00:00')",
            sql![item_id],
        )
        .unwrap();
}

/// Everything always on in the prompts: the settled block and what joined since (MEM-429).
fn always_on(env: &Env) -> String {
    env.profile.prompt_text().unwrap() + &env.profile.pending_text().unwrap()
}

fn on_memory_page(env: &Env) -> BTreeSet<String> {
    env.profile
        .view()
        .unwrap()
        .to_string()
        .split('"')
        .map(str::to_owned)
        .collect()
}

struct Now<'a>(&'a Env);
impl ferb_memory::search::Clock for Now<'_> {
    fn now(&self) -> ferb_learning::DateTime {
        *self.0.clock.0.borrow()
    }
}

/// The memory search `memory.read` runs (keywords over the current facts): the ids it finds.
fn found(env: &Env, query: &str) -> Vec<String> {
    let conn = env.store.conn();
    let clock = Now(env);
    ferb_memory::search::MemorySearch::new(conn, &clock, None)
        .facts(
            &ferb_memory::search::StoreFacts(conn),
            query,
            None,
            "",
            None,
            &[],
        )
        .unwrap()
        .into_iter()
        .map(|h| h.data.id)
        .collect()
}

fn listed_in_review(env: &Env) -> Vec<String> {
    let review = env.importer().review().unwrap();
    review["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["part"] == ALWAYS_ON)
        .flat_map(|p| p["groups"].as_array().unwrap().clone())
        .flat_map(|g| g["items"].as_array().unwrap().clone())
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect()
}

// decision: MEM-489 — an imported line his Approve all made always on is not, and is still found
#[test]
fn an_imported_line_he_approved_is_not_always_on_and_is_still_found() {
    let (env, short, laptop) = imported_world("approved");
    approved_by_an_earlier_build(&env, &short);
    approved_by_an_earlier_build(&env, &laptop);
    let text = always_on(&env);
    assert!(
        !text.contains(&short.item_id) && !text.contains(&laptop.item_id),
        "{text}"
    );
    assert!(env.profile.entries().unwrap().is_empty());
    let item = env.memory.get(&short.item_id).unwrap().unwrap();
    assert!(!env.profile.holds_up(&item, None).unwrap());
    assert!(env.profile.eligible().unwrap().is_empty());
    assert!(!env.profile.pass_context().unwrap().contains(&short.item_id));
    // the weekly pass naming it is refused by code
    let out = env
        .profile
        .apply(
            &json!({"sections": [{"title": "You", "items": [short.item_id.clone()]}], "ended": []}),
        )
        .unwrap();
    assert_eq!(out["refused"], json!([short.item_id.clone()]));
    assert!(!always_on(&env).contains(&short.item_id));
    // remembered: active, its words and origin kept, found by the memory search
    let item = env.memory.get(&short.item_id).unwrap().unwrap();
    assert_eq!(
        (item.status.as_str(), item.origin.as_str()),
        ("active", "inferred")
    );
    assert!(
        found(&env, "short answers").contains(&short.item_id),
        "memory.read's search finds it"
    );
    assert!(!on_memory_page(&env).contains(&short.item_id));
}

// decision: MEM-489 — his pin of an imported line does not make it always on; a new pin is refused, plainly
#[test]
fn an_imported_line_he_pinned_is_not_always_on() {
    let (env, short, laptop) = imported_world("pinned");
    pinned(&env, &short.item_id);
    assert!(!always_on(&env).contains(&short.item_id));
    let item = env.memory.get(&short.item_id).unwrap().unwrap();
    assert!(!env.profile.holds_up(&item, None).unwrap());
    // the Memory page's Always on now: refused, said plainly, no row written
    let err = env.profile.add(&laptop.item_id, "", "owner").unwrap_err();
    assert_eq!(err.to_string(), IMPORTED_REFUSAL);
    assert_eq!(
        env.store
            .count(
                "SELECT COUNT(*) FROM memory_profile WHERE item_id = ?",
                sql![laptop.item_id]
            )
            .unwrap(),
        0
    );
    assert!(env.profile.keep(&laptop.item_id, "owner").is_err());
    assert!(!always_on(&env).contains(&laptop.item_id));
}

// decision: MEM-489 — his own words are another item: always on at once, an imported line beside them is not, and a
// line he restated in his own words is his
#[test]
fn his_own_words_stay_always_on() {
    let (env, short, laptop) = imported_world("his");
    approved_by_an_earlier_build(&env, &short);
    let said = env
        .journal
        .append("chat.message", Some(json!({"key": "editor"})), None)
        .unwrap();
    let mine = env
        .memory
        .save(
            "preference",
            "owner",
            "editor",
            "I write code in Zed",
            "owner_stated",
            &[("journal".into(), said.id)],
            1.0,
            &[],
            "",
            Some(true),
        )
        .unwrap()
        .item;
    // he restates the imported laptop line in his own words, under its name: a new item, his
    let said = env
        .journal
        .append("chat.message", Some(json!({"key": "laptop"})), None)
        .unwrap();
    let restated = env
        .memory
        .save(
            &laptop.kind,
            &laptop.subject,
            &laptop.key,
            "I use the work laptop for everything",
            "owner_stated",
            &[("journal".into(), said.id)],
            1.0,
            std::slice::from_ref(&laptop.item_id),
            "",
            Some(true),
        )
        .unwrap()
        .item;
    assert_ne!(restated.id, laptop.item_id);
    // the import's line follows his words (they are its current item now): still his, still always on
    let job = env.importer();
    let follows = job
        .lines()
        .unwrap()
        .into_iter()
        .any(|l| l.item_id == restated.id);
    assert!(follows, "the line follows his words");
    // as stored once any later write of the line keeps what it follows: his item is an import line's item too
    env.store
        .exec(
            "UPDATE memory_import_lines SET item_id = ? WHERE id = ?",
            sql![restated.id, laptop.id],
        )
        .unwrap();
    assert!(ferb_learning::import_review::imported(&env.store, &restated.id).unwrap());
    let text = always_on(&env);
    assert!(text.contains(&mine.id), "{text}");
    assert!(text.contains(&restated.id), "{text}");
    assert!(!text.contains(&short.item_id), "{text}");
    // the start's step leaves his words where they are
    job.wait_for_yes(&env.profile).unwrap();
    let text = always_on(&env);
    assert!(
        text.contains(&mine.id) && text.contains(&restated.id),
        "{text}"
    );
}

// decision: MEM-489 — the rows an earlier build left leave once, journaled once with the count; nothing is deleted
#[test]
fn the_rows_already_always_on_leave_once_and_nothing_is_deleted() {
    let (env, short, laptop) = imported_world("rows");
    approved_by_an_earlier_build(&env, &short);
    approved_by_an_earlier_build(&env, &laptop);
    let said = env
        .journal
        .append("chat.message", Some(json!({"key": "name"})), None)
        .unwrap();
    let name = env
        .memory
        .save(
            "fact",
            "owner",
            "name",
            "My name is Sam Example",
            "owner_stated",
            &[("journal".into(), said.id)],
            1.0,
            &[],
            "",
            Some(true),
        )
        .unwrap()
        .item;
    let rows = |env: &Env| -> BTreeSet<String> { env.profile.ids().unwrap() };
    assert_eq!(rows(&env).len(), 3);
    let lines_before = env.importer().lines().unwrap().len();
    let left = |env: &Env| -> Vec<String> {
        env.store
            .query(
                "SELECT data_json FROM journal WHERE kind = ?",
                sql![IMPORTED_LEFT_KIND],
            )
            .unwrap()
            .iter()
            .map(|r| r.s_at(0))
            .collect()
    };
    let job = env.importer();
    job.wait_for_yes(&env.profile).unwrap();
    assert_eq!(rows(&env), BTreeSet::from([name.id.clone()]));
    assert_eq!(left(&env), vec![r#"{"left": 2}"#.to_owned()]);
    for l in [&short, &laptop] {
        let it = env.memory.get(&l.item_id).unwrap().unwrap();
        assert_eq!(it.status, "active", "nothing is deleted");
        assert_eq!(it.content, l.content);
        assert_eq!(it.origin, "inferred");
    }
    assert_eq!(job.lines().unwrap().len(), lines_before, "the lines stay");
    assert!(found(&env, "laptop").contains(&laptop.item_id));
    // the second start: nothing to move, no second row
    let journal_rows = env
        .store
        .count("SELECT COUNT(*) FROM journal", sql![])
        .unwrap();
    job.wait_for_yes(&env.profile).unwrap();
    assert_eq!(rows(&env), BTreeSet::from([name.id.clone()]));
    assert_eq!(left(&env).len(), 1);
    assert_eq!(
        env.store
            .count("SELECT COUNT(*) FROM journal", sql![])
            .unwrap(),
        journal_rows,
        "a second start writes nothing"
    );
    assert!(always_on(&env).contains(&name.id));
}

// decision: MEM-489 — the daily review never asks about an imported line being always on; FERB's own understanding
// still waits there for his Yes (MEM-481)
#[test]
fn the_review_asks_nothing_about_imported_lines() {
    let (env, short, laptop) = imported_world("review");
    // an earlier build's marks: one suggested, one put in always on by itself
    for (l, mark) in [(&short, "suggested"), (&laptop, "on")] {
        let mut data = l.data.clone();
        data.insert(ALWAYS_ON.into(), json!(mark));
        env.store
            .exec(
                "UPDATE memory_import_lines SET data_json = ? WHERE id = ?",
                sql![Value::Object(data).to_string(), l.id],
            )
            .unwrap();
    }
    env.store
        .exec(
            "INSERT INTO memory_profile (item_id, section, position, added_by, added_at) VALUES (?, 'x', 0, 'code', 'y')",
            sql![laptop.item_id],
        )
        .unwrap();
    let job = env.importer();
    assert_eq!(job.suggest_always_on().unwrap(), 0);
    assert!(listed_in_review(&env).is_empty());
    assert_eq!(job.review().unwrap()["count"], 0);
    assert_eq!(job.review_question().unwrap(), None);
    assert!(job.review_numbered().unwrap().is_empty());
    assert!(!job.review_round().unwrap());
    assert_eq!(
        job.wait_for_yes(&env.profile).unwrap(),
        0,
        "none asked again"
    );
    assert!(listed_in_review(&env).is_empty());
    // FERB's own understanding, said on two days, is still asked
    let own = |content: &str| {
        let said = env
            .journal
            .append("chat.message", Some(json!({"key": "drink"})), None)
            .unwrap();
        env.memory
            .save(
                "preference",
                "owner",
                "drink",
                content,
                "inferred",
                &[("journal".into(), said.id)],
                0.6,
                &[],
                "",
                Some(false),
            )
            .unwrap()
            .item
            .id
    };
    let tea = own("The owner prefers tea");
    later(&env, 1);
    own("The owner prefers tea");
    assert_eq!(listed_in_review(&env), vec![tea]);
}
