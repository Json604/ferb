//! FERB's understanding waits for the owner's Yes before it is always on (MEM-480, the owner's "idk ig wait" of
//! 2026-11-25; MEM-481): the core profile carries only what he said or confirmed and plainly observed facts; what
//! FERB worked out joins only on his Approve in the review, a Decline keeps it out, and nothing is deleted. Through the
//! real memory items, core profile and import job. Temporary folders only; time is injected.

mod support;

use serde_json::json;

use ferb_learning::import_review::ALWAYS_ON;
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

/// A fact FERB saved from something the owner said now (a journal row of this moment, so each day counts).
fn save(env: &Env, key: &str, content: &str, origin: &str) -> String {
    let said = env
        .journal
        .append("chat.message", Some(json!({"key": key})), None)
        .unwrap();
    env.memory
        .save(
            "preference",
            "owner",
            key,
            content,
            origin,
            &[("journal".into(), said.id)],
            0.6,
            &[],
            "",
            Some(origin == "owner_stated"),
        )
        .unwrap()
        .item
        .id
}

fn suggested(env: &Env) -> Vec<String> {
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

/// Everything always on in the prompts: the settled block and what joined since (MEM-429).
fn always_on(env: &Env) -> String {
    env.profile.prompt_text().unwrap() + &env.profile.pending_text().unwrap()
}

fn eligible(env: &Env) -> Vec<String> {
    env.profile
        .eligible()
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect()
}

// decision: MEM-480, MEM-481
#[test]
fn what_ferb_worked_out_waits_for_his_yes_and_a_no_keeps_it_out() {
    let env = Env::new("waits", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    let his = save(&env, "answers", "I like short answers", "owner_stated");
    let tea = save(&env, "drink", "The owner prefers tea", "inferred");
    let arc = save(&env, "browser", "The owner browses in Arc", "inferred");
    // his own words are always on at once (MEM-35); FERB's understanding is not
    let text = always_on(&env);
    assert!(text.contains(&his), "{text}");
    assert!(!text.contains(&tea) && !text.contains(&arc), "{text}");
    // a week on and seen again on another day, MEM-424's rule would have joined them by themselves: they wait in the
    // review instead
    later(&env, 8);
    save(&env, "drink", "The owner prefers tea", "inferred");
    save(&env, "browser", "The owner browses in Arc", "inferred");
    assert!(!eligible(&env).contains(&tea) && !eligible(&env).contains(&arc));
    let pass = env.profile.pass_context().unwrap();
    assert!(
        !pass.contains(&tea),
        "the weekly pass can't place it: {pass}"
    );
    let mut waiting = suggested(&env);
    waiting.sort();
    let mut both = vec![tea.clone(), arc.clone()];
    both.sort();
    assert_eq!(waiting, both);
    let review = env.importer().review().unwrap();
    assert_eq!(
        review["parts"][0]["groups"][0]["title"],
        "How you like me to work"
    );
    assert_eq!(
        review["parts"][0]["title"],
        "Keep these in mind all the time?"
    );
    // a pass answer naming one anyway is refused by code
    let out = env
        .profile
        .apply(&json!({"sections": [{"title": "You", "items": [tea.clone(), his.clone()]}], "ended": []}))
        .unwrap();
    assert_eq!(out["refused"], json!([tea.clone()]));
    assert!(!always_on(&env).contains(&tea));
    // his Approve: always on, as his pin, and it leaves the list
    let job = env.importer();
    assert_eq!(
        job.answer_review(
            &env.profile,
            ALWAYS_ON,
            std::slice::from_ref(&tea),
            "approve"
        )
        .unwrap(),
        1
    );
    assert!(always_on(&env).contains(&tea));
    assert!(eligible(&env).contains(&tea));
    // his Decline: out, remembered, never suggested again
    job.answer_review(
        &env.profile,
        ALWAYS_ON,
        std::slice::from_ref(&arc),
        "decline",
    )
    .unwrap();
    assert!(suggested(&env).is_empty());
    assert_eq!(job.review().unwrap()["count"], 0);
    later(&env, 8);
    save(&env, "browser", "The owner browses in Arc", "inferred");
    assert!(suggested(&env).is_empty(), "a No stays a No");
    assert!(!always_on(&env).contains(&arc));
    let kept = env.memory.get(&arc).unwrap().unwrap();
    assert_eq!(kept.status, "active", "nothing is deleted");
    // the weekly pass keeps his Yes
    env.profile
        .apply(&json!({"sections": [{"title": "You", "items": [his.clone()]}], "ended": []}))
        .unwrap();
    assert!(always_on(&env).contains(&tea));
}

// decision: MEM-480 — a row that holds FERB's understanding without his Yes (an older build's join, any write) never
// reaches the prompts; the Memory page's Always on is his Yes
#[test]
fn only_his_words_his_yes_and_observed_facts_reach_the_prompts() {
    let env = Env::new("rows", None);
    at(&env, "2026-11-10T12:00:00+00:00");
    let guess = save(&env, "editor", "The owner codes in Zed", "inferred");
    env.store
        .exec(
            "INSERT INTO memory_profile (item_id, section, position, added_by, added_at) VALUES (?, 'Old', 0, 'code', 'x')",
            sql![guess],
        )
        .unwrap();
    let text = always_on(&env);
    assert!(!text.contains(&guess), "{text}");
    assert!(env.profile.entries().unwrap().is_empty());
    later(&env, 8);
    assert!(!eligible(&env).contains(&guess));
    // the Memory page's Always on is his Yes
    env.profile.add(&guess, "", "owner").unwrap();
    assert!(always_on(&env).contains(&guess));
}
