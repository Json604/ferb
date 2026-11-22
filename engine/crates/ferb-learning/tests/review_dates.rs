//! A remembered fact's check date (MEM-472, the owner's "b" of 2026-11-23): when it comes, the fact joins the review's
//! "Are these still true?" by code alone, with no model call; Yes keeps it and clears the date, No ends it, Not sure
//! asks again in a week. Through the real import job and memory items. Temporary folders only; time is injected.

mod support;

use serde_json::{json, Value};

use ferb_learning::import_job::ImportJob;
use ferb_learning::import_review::{STILL_TRUE, UNSURE_AGAIN_DAYS};
use ferb_learning::{py, sql};
use support::*;

const DAY: i64 = 86_400_000_000;

fn at(env: &Env, when: &str) {
    *env.clock.0.borrow_mut() = py::parse(when).unwrap();
}

fn later(env: &Env, days: i64) {
    let now = *env.clock.0.borrow();
    *env.clock.0.borrow_mut() = py::shift(&now, days * DAY);
}

fn model_calls(env: &Env) -> usize {
    env.events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e[0] == "model.start")
        .count()
}

/// A fact the planner saved with `review_in_days` (as remember.rs does): its check date `days` from now.
fn dated_fact(env: &Env, key: &str, content: &str, days: i64) -> String {
    let item = env
        .memory
        .save(
            "fact",
            "owner",
            key,
            content,
            "inferred",
            &[("journal".into(), format!("j-{key}"))],
            0.6,
            &[],
            "",
            Some(false),
        )
        .unwrap()
        .item;
    let now = *env.clock.0.borrow();
    env.memory
        .set_review(&item.id, Some(&py::shift(&now, days * DAY)))
        .unwrap();
    item.id
}

/// The same, as an import placed it: a line whose `review_on` gives the date.
fn dated_line(env: &Env, key: &str, content: &str, days: i64) -> String {
    let id = dated_fact(env, key, content, days);
    let review_at = env.memory.get(&id).unwrap().unwrap().review_at.unwrap();
    let now = py::iso(&env.clock.0.borrow())[..10].to_owned();
    let row = json!({"you": content.replace("The owner is", "You are"), "offer": "", "said": now, "first_said": now,
                     "days": 1, "sources": ["claude_code"], "ended": false, "until": "",
                     "review_on": py::iso(&review_at)[..10].to_owned(), "use": "repeat", "holds_up": false});
    env.store
        .exec(
            "INSERT INTO memory_import_lines (id, item_id, kind, subject, key, data_json, created_at, updated_at) \
             VALUES (?, ?, 'fact', 'owner', ?, ?, ?, ?)",
            sql![format!("l-{key}"), id, key, row.to_string(), now, now],
        )
        .unwrap();
    id
}

fn still_true(job: &ImportJob<'_>) -> Vec<Value> {
    job.review().unwrap()["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["part"] == STILL_TRUE)
        .flat_map(|p| p["groups"].as_array().unwrap().clone())
        .flat_map(|g| g["items"].as_array().unwrap().clone())
        .collect()
}

fn answer(env: &Env, job: &ImportJob<'_>, id: &str, said: &str) -> usize {
    job.answer_review(&env.profile, STILL_TRUE, &[id.to_owned()], said)
        .unwrap()
}

// decision: MEM-472
#[test]
fn a_fact_joins_the_review_on_its_check_date_by_code_and_each_answer_does_what_it_says() {
    let env = Env::new("dates", None);
    at(&env, "2026-11-05T10:00:00+00:00");
    let job = env.importer();
    let course = dated_fact(&env, "course", "The owner is taking a pottery course", 30);
    let trip = dated_line(&env, "trip", "The owner is in Goa", 30);
    let exam = dated_fact(&env, "exam", "The owner has exams this month", 30);
    // nothing is asked before the date
    later(&env, 29);
    assert!(job.due_checks().unwrap().is_empty());
    assert_eq!(job.review().unwrap()["count"], 0);
    assert_eq!(job.review_question().unwrap(), None);
    // on the date all three join "Are these still true?", each with its date, by code alone
    later(&env, 1);
    let items = still_true(&job);
    assert_eq!(items.len(), 3, "{items:?}");
    let texts: Vec<&str> = items.iter().map(|i| i["text"].as_str().unwrap()).collect();
    assert!(
        texts.contains(&"You are in Goa"),
        "an import line in its own words: {texts:?}"
    );
    assert!(texts.contains(&"The owner is taking a pottery course"));
    assert!(items
        .iter()
        .all(|i| i["note"] == "I said I'd check this on 5 Dec 2026"));
    assert_eq!(
        job.review_question().unwrap().unwrap(),
        "Can you check a few things I brought in?\n3 things I said I'd check with you. Answer each one here, or just \
         tell me."
    );
    assert_eq!(model_calls(&env), 0);
    // Yes: kept, the date cleared, gone from the list
    assert_eq!(answer(&env, &job, &course, "yes"), 1);
    let kept = env.memory.get(&course).unwrap().unwrap();
    assert!(kept.review_at.is_none() && kept.valid_to.is_none() && kept.status == "active");
    // No: ended as No to an older line ends it; the import line's date is answered, never set again by its facts
    assert_eq!(answer(&env, &job, &trip, "no"), 1);
    let ended = env.memory.get(&trip).unwrap().unwrap();
    assert!(ended.valid_to.is_some() && ended.review_at.is_none());
    let line = job
        .lines()
        .unwrap()
        .into_iter()
        .find(|l| l.item_id == trip)
        .unwrap();
    assert_eq!(
        (line.s("review_on"), line.s("review_checked")),
        (String::new(), "2026-12-05".into())
    );
    // Not sure: off the list, asked again in a week
    assert_eq!(UNSURE_AGAIN_DAYS, 7);
    assert_eq!(answer(&env, &job, &exam, "unsure"), 1);
    assert_eq!(job.review().unwrap()["count"], 0);
    later(&env, UNSURE_AGAIN_DAYS - 1);
    assert!(job.due_checks().unwrap().is_empty());
    later(&env, 1);
    let again: Vec<String> = job
        .due_checks()
        .unwrap()
        .into_iter()
        .map(|d| d.item_id)
        .collect();
    assert_eq!(again, vec![exam.clone()]);
    assert_eq!(
        still_true(&job)[0]["note"],
        "I said I'd check this on 12 Dec 2026"
    );
    // the kept one is never asked again
    later(&env, 60);
    assert!(!job
        .due_checks()
        .unwrap()
        .iter()
        .any(|d| d.item_id == course));
    assert_eq!(model_calls(&env), 0);
}

// decision: MEM-472, MEM-462
#[test]
fn every_fact_whose_date_came_is_asked_past_the_five_older_lines_limit_and_in_words_too() {
    let env = Env::new("dates-many", None);
    at(&env, "2026-11-05T10:00:00+00:00");
    let job = env.importer();
    let ids: Vec<String> = (0..7)
        .map(|n| {
            dated_fact(
                &env,
                &format!("plan_{n}"),
                &format!("The owner plans trip number {n}"),
                10,
            )
        })
        .collect();
    later(&env, 10);
    assert_eq!(still_true(&job).len(), 7);
    // the typed answer reads them as older lines (still true, or not any more)
    let numbered = job.review_numbered().unwrap();
    assert_eq!(numbered.len(), 7);
    assert!(numbered.iter().all(|n| n["what"] == "line"));
    // all at once
    assert_eq!(
        job.answer_review(&env.profile, STILL_TRUE, &ids, "yes")
            .unwrap(),
        7
    );
    assert_eq!(job.review().unwrap()["count"], 0);
    assert_eq!(model_calls(&env), 0);
}
