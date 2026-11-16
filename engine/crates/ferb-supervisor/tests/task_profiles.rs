//! HAND-580 and HAND-581: the browser profiles the owner's own words name for a task. Live 2026-11-16 04:17 (task
//! 0abecb83, a fresh install with seven paired profiles and nothing picked): "open my iit chrom and compelte my
//! already open iit assingemnts. submti them too". The planner's goal named the IIT profile, but FERB first asked its
//! general "Which browser profile should FERB use for web work?" (the owner's words name no profile as whole words),
//! the owner picked another profile as the general default, and the browser tools then refused the IIT profile
//! (`picked_only`); the worker fell back to driving Chrome's window with the Mac tools.
//!
//! Scripted on recorded control scenarios with the planner's answers replaced (a plan that only waits, so no worker
//! runs). The owner's profiles here: chrome/Profile 29 ("ds.study.iitm.ac.in", the IIT one), chrome/Profile 15
//! ("New_Personal", the general pick when there is one), chrome/Profile 2 ("scaler").

mod common;
mod support_control;

use serde_json::{json, Map, Value};
use support_control::{run_with, world_with, World};

fn wait_step() -> Value {
    json!({"id": "wait_noon", "kind": "wait", "label": "", "goal": "Wait for the morning",
        "depends_on": [], "inputs": [], "worker_type": "default", "conditions": [],
        "until": "2030-01-01T09:00:00+00:00", "event": null, "event_match": [], "question": "", "options": [],
        "conversation": null, "repeat": null, "earlier_results": []})
}

fn task(identity: &str, sites: &[&str], profiles: &[&str]) -> Value {
    json!({
        "goal": "In your IIT Chrome profile complete the invented assignment", "name": "Assignment",
        "identity": identity, "worker_type": "default",
        "targets": {"apps": [], "sites": sites, "paths": [], "recipients": []},
        "success_conditions": [{"text": "The assignment is submitted", "owner_text": "", "layer": "outcome",
                                "check": null}],
        "plan": {"steps": [wait_step()], "input_paths": [], "big_job": false, "time_estimate": "",
                 "cost_estimate": "", "standing_sends": []},
        "folders": [], "keep_tabs_open": false, "parts": [], "earlier_results": [], "profiles": profiles})
}

fn plan_answer(identity: &str, sites: &[&str], profiles: &[&str]) -> String {
    json!({"kind": "task", "answers_question": false, "task": task(identity, sites, profiles),
        "control": null, "side_entries": null, "text": "On it."})
    .to_string()
}

fn final_text(text: &str) -> Value {
    json!([
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001}},
        {"kind": "final", "data": {"text": text}}
    ])
}

fn profile(b: &str, f: &str) -> (String, String) {
    (b.to_string(), f.to_string())
}

fn known(w: &World) {
    *w.daemon.owner_profile_names.lock().unwrap() = vec![
        ("chrome/Profile 29".into(), profile("chrome", "Profile 29")),
        (
            "ds.study.iitm.ac.in".into(),
            profile("chrome", "Profile 29"),
        ),
        ("chrome/Profile 15".into(), profile("chrome", "Profile 15")),
        ("New_Personal".into(), profile("chrome", "Profile 15")),
        ("chrome/Profile 2".into(), profile("chrome", "Profile 2")),
        ("scaler".into(), profile("chrome", "Profile 2")),
    ];
}

/// The plan as given; the browser area answers `browsing` as given; `looks` are the test's `task_profiles` steps,
/// made while the plan waits. The run ends with the owner's stop while the plan waits.
fn run(plan: String, browsing: Vec<Value>, looks: Vec<Value>) -> World {
    run_with_sources(plan, browsing, looks, None)
}

/// As [`run`], with the facts the planned profiles are checked against (HAND-587) set by the test.
fn run_with_sources(
    plan: String,
    browsing: Vec<Value>,
    looks: Vec<Value>,
    sources: Option<ferb_supervisor::flow::ProfileSources>,
) -> World {
    let mut w = world_with("stop_while_step_asks", |calls| {
        let planner = calls.iter_mut().find(|c| c["key"] == "supervisor").unwrap();
        planner["events"] = final_text(&plan);
    });
    known(&w);
    *w.daemon.profile_sources.lock().unwrap() = sources;
    *w.daemon.browsing.lock().unwrap() = browsing
        .into_iter()
        .map(|v| v.as_object().cloned().unwrap_or_else(Map::new))
        .collect();
    let report = run_with(&mut w, "stop_while_step_asks", |steps| {
        for st in steps.iter_mut() {
            if st["do"] == "wait" {
                st["until"] = json!("a plan step waits");
            }
        }
        let first = steps.iter().position(|st| st["do"] == "wait").unwrap();
        for (i, look) in looks.into_iter().enumerate() {
            steps.insert(first + 1 + i, look);
        }
        let at = steps.iter().position(|st| st["do"] == "close").unwrap();
        steps.insert(at + 1, json!({"do": "wait", "until": "the task ended"}));
    });
    eprintln!("scripted: {}", report.verdict());
    w
}

fn look() -> Value {
    json!({"do": "task_profiles"})
}

fn answered(question: &str, options: &[&str], answer: &str) -> Value {
    json!({"do": "task_profiles", "answer": {"question": question, "options": options, "answer": answer}})
}

fn journal(w: &World, kind: &str) -> Vec<Value> {
    let db = w.db.lock().unwrap();
    let mut q = db
        .conn()
        .prepare("SELECT data_json FROM journal WHERE kind = ? ORDER BY seq")
        .unwrap();
    let rows = q
        .query_map([kind], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    rows
}

fn asked_while_here(w: &World) -> Vec<bool> {
    w.daemon
        .browsing_calls
        .lock()
        .unwrap()
        .iter()
        .map(|(attended, _)| *attended)
        .collect()
}

fn seen(w: &World) -> Vec<Value> {
    w.daemon.seen_profiles.lock().unwrap().clone()
}

/// The live case without a pick: the profile the planner resolved from the owner's words is the task's; FERB's
/// general question isn't asked (the browser area is never consulted) and no pick is applied.
// decision: HAND-580
#[test]
fn a_task_whose_words_name_a_profile_browses_there_and_nothing_is_asked() {
    let w = run(
        plan_answer("owner", &["ds.study.iitm.ac.in"], &["ds.study.iitm.ac.in"]),
        vec![json!({"ask": "ap-browsing"})],
        vec![look()],
    );
    assert!(asked_while_here(&w).is_empty(), "no general question");
    assert!(
        journal(&w, "task.browses_as_owner").is_empty(),
        "no pick applied"
    );
    let rows = journal(&w, "task.browses_in");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["profiles"], json!(["chrome/Profile 29"]));
    assert_eq!(rows[0]["how"], "planned");
    assert_eq!(seen(&w), [json!([false, [["chrome", "Profile 29"]]])]);
}

/// With a saved pick elsewhere (New_Personal) the task still browses in the profile its words named: the pick isn't
/// read, let alone imposed.
// decision: HAND-580
#[test]
fn a_saved_pick_never_narrows_a_task_whose_words_named_a_profile() {
    let w = run(
        plan_answer("owner", &["ds.study.iitm.ac.in"], &["chrome/Profile 29"]),
        vec![json!({"as_owner": true, "label": "Google Chrome “New_Personal”"})],
        vec![look()],
    );
    assert!(asked_while_here(&w).is_empty());
    assert!(journal(&w, "task.browses_as_owner").is_empty());
    assert_eq!(seen(&w), [json!([false, [["chrome", "Profile 29"]]])]);
}

/// Two profiles named ("both"): both are the task's. A task FERB was to run as itself that names the owner's
/// profiles works as the owner there.
// decision: HAND-580
#[test]
fn two_named_profiles_are_both_the_tasks() {
    let w = run(
        plan_answer(
            "ferb",
            &["ds.study.iitm.ac.in"],
            &["ds.study.iitm.ac.in", "scaler"],
        ),
        vec![json!({"ask": "ap-browsing"})],
        vec![look()],
    );
    assert!(asked_while_here(&w).is_empty());
    assert_eq!(
        seen(&w),
        [json!([
            false,
            [["chrome", "Profile 29"], ["chrome", "Profile 2"]]
        ])]
    );
}

/// A name that resolves to none of the owner's profiles (or to several) is as if none was named: FERB's one browsing
/// question, once, then the answer's profile (the existing which-question).
// decision: HAND-580
#[test]
fn a_name_that_resolves_to_no_profile_meets_the_one_question_once() {
    let w = run(
        plan_answer("owner", &["ds.study.iitm.ac.in"], &["my iit chrome"]),
        vec![
            json!({"ask": "ap-browsing"}),
            json!({"as_owner": true, "label": "Google Chrome “New_Personal”"}),
        ],
        vec![look()],
    );
    assert_eq!(
        journal(&w, "task.profiles_unresolved")[0]["named"],
        json!(["my iit chrome"])
    );
    assert_eq!(
        asked_while_here(&w),
        [true, false],
        "asked once, then read again"
    );
    assert_eq!(seen(&w), [json!([true, []])], "the pick");
}

/// A task that names no profile uses the pick (unchanged, ORCH-648).
// decision: HAND-580
#[test]
fn a_task_naming_no_profile_uses_the_pick() {
    let w = run(
        plan_answer("owner", &["ds.study.iitm.ac.in"], &[]),
        vec![json!({"as_owner": true, "label": "Google Chrome “New_Personal”"})],
        vec![look()],
    );
    assert_eq!(asked_while_here(&w), [true]);
    assert_eq!(seen(&w), [json!([true, []])]);
}

/// While the task runs, the owner's answers to its own questions: Yes to a Yes/No question naming a profile adds it;
/// No adds nothing; words naming a profile add it; a question the owner didn't answer Yes to adds nothing. Only this
/// task's profiles change: the general pick is never written (no `browsing.chosen`).
// decision: HAND-581
#[test]
fn the_owners_yes_or_words_add_a_profile_to_the_running_task_only() {
    let w = run(
        plan_answer("owner", &["ds.study.iitm.ac.in"], &["ds.study.iitm.ac.in"]),
        vec![],
        vec![
            answered(
                "Should this task also use chrome/Profile 15 (New_Personal)?",
                &["Yes", "No"],
                "No",
            ),
            answered(
                "Should this task also use chrome/Profile 2 (scaler)?",
                &["Yes", "No"],
                "Yes",
            ),
            answered(
                "Which account has the course?",
                &[],
                "the one in New_Personal",
            ),
            answered(
                "Can I go on with chrome/Profile 15?",
                &["Go on", "Stop"],
                "Go on",
            ),
        ],
    );
    let iit = json!(["chrome", "Profile 29"]);
    let scaler = json!(["chrome", "Profile 2"]);
    let personal = json!(["chrome", "Profile 15"]);
    assert_eq!(
        seen(&w),
        [
            json!([false, [iit]]),
            json!([false, [iit, scaler]]),
            json!([false, [iit, scaler, personal]]),
            json!([false, [iit, scaler, personal]]),
        ]
    );
    let added = journal(&w, "task.profiles_added");
    assert_eq!(added.len(), 2);
    assert_eq!(added[0]["profiles"], json!(["chrome/Profile 2"]));
    assert_eq!(added[0]["how"], "answered_yes");
    assert_eq!(added[1]["profiles"], json!(["chrome/Profile 15"]));
    assert_eq!(added[1]["how"], "answered");
    assert!(
        journal(&w, "browsing.chosen").is_empty(),
        "the general pick is untouched"
    );
}

/// A task kept to the pick takes an added profile beside the pick; a task kept to none (no sites, nothing named, no
/// pick applied) isn't narrowed by one.
// decision: HAND-581
#[test]
fn an_added_profile_joins_the_pick_and_never_narrows_a_task_kept_to_none() {
    let picked = run(
        plan_answer("owner", &["ds.study.iitm.ac.in"], &[]),
        vec![json!({"as_owner": true, "label": "Google Chrome “New_Personal”"})],
        vec![answered("Use scaler too?", &["Yes", "No"], "Yes")],
    );
    assert_eq!(seen(&picked), [json!([true, [["chrome", "Profile 2"]]])]);
    let free = run(
        plan_answer("owner", &[], &[]),
        vec![],
        vec![answered("Use scaler too?", &["Yes", "No"], "Yes")],
    );
    assert_eq!(seen(&free), [Value::Null]);
    assert!(journal(&free, "task.profiles_added").is_empty());
}

/// The owner's chat message while the task runs ("use my scaler chrome too"): the planner's correction lists the
/// profile in the task's `profiles`, and it joins the running task's (steered_extras).
// decision: HAND-581
#[test]
fn a_correction_naming_another_profile_adds_it_to_the_running_task() {
    let name = "steer_while_checking";
    let first = plan_answer("owner", &["ds.study.iitm.ac.in"], &["ds.study.iitm.ac.in"]);
    let mut w = world_with(name, |calls| {
        calls[0]["events"] = final_text(&first);
        let second = json!({"kind": "reply", "answers_question": false,
            "task": task("owner", &["ds.study.iitm.ac.in"], &["ds.study.iitm.ac.in", "scaler"]),
            "control": {"task_id": "id-000021", "action": "continue", "new_goal": false,
                        "standing_sends": "unchanged", "supersedes": []},
            "side_entries": null, "text": "I'll use your scaler profile too."});
        calls[1]["events"] = final_text(&second.to_string());
    });
    known(&w);
    {
        let mut q = w.daemon.supervise.lock().unwrap();
        let last = q.last().cloned().unwrap();
        q.push(last);
    }
    let report = run_with(&mut w, name, |steps| {
        let ask = steps
            .iter()
            .position(|st| st["do"] == "stream" && st["conn"] == "app")
            .unwrap();
        let mut asked = steps[ask].clone();
        let mut sent: Value =
            serde_json::from_str(asked["lines"][0]["send"].as_str().unwrap()).unwrap();
        sent["params"]["text"] = json!("use my scaler chrome too");
        asked["lines"][0]["send"] = json!(sent.to_string());
        let app: Vec<Value> = steps
            .iter()
            .filter(|st| st["conn"] == "app" && (st["do"] == "connect" || st["do"] == "call"))
            .cloned()
            .collect();
        let mut out: Vec<Value> = steps[..3].to_vec();
        out.push(json!({"do": "wait", "until": "a plan step waits"}));
        out.push(look());
        out.extend(app);
        out.push(asked);
        // the reply's end: the correction is applied by then
        let end = steps[ask + 1..]
            .iter()
            .find(|st| st["do"] == "stream_end" && st["conn"] == "app")
            .cloned()
            .unwrap();
        out.push(end);
        out.push(look());
        *steps = out;
    });
    eprintln!("{name} (scripted): {}", report.verdict());
    let iit = json!(["chrome", "Profile 29"]);
    let scaler = json!(["chrome", "Profile 2"]);
    assert_eq!(
        seen(&w),
        [json!([false, [iit]]), json!([false, [iit, scaler]])]
    );
    assert_eq!(journal(&w, "task.profiles_added")[0]["how"], "corrected");
}

fn sources(
    said: &[(&str, &str)],
    owner: &[(&str, &str)],
    other: &[(&str, &str)],
    paired: &[(&str, &str)],
) -> Option<ferb_supervisor::flow::ProfileSources> {
    let v = |l: &[(&str, &str)]| l.iter().map(|(b, f)| profile(b, f)).collect::<Vec<_>>();
    Some(ferb_supervisor::flow::ProfileSources {
        said: v(said),
        owner_memory: v(owner),
        other_memory: v(other),
        paired: v(paired),
    })
}

const SCALER: (&str, &str) = ("chrome", "Profile 2");
const IIT: (&str, &str) = ("chrome", "Profile 29");
const PERSONAL: (&str, &str) = ("chrome", "Profile 15");

/// HAND-587 (the owner, 2026-11-16: "yes, only from my own words"): the owner once said "use my scaler Chrome for
/// scaler things" (a memory item of origin owner_stated); a scaler task whose message names no profile browses there,
/// nothing asked, the pick not applied.
// decision: HAND-587
#[test]
fn the_owners_remembered_words_give_a_task_its_profile() {
    let w = run_with_sources(
        plan_answer("owner", &["scaler.com"], &["scaler"]),
        vec![json!({"ask": "ap-browsing"})],
        vec![look()],
        sources(&[], &[SCALER], &[], &[SCALER, IIT, PERSONAL]),
    );
    assert!(asked_while_here(&w).is_empty(), "no general question");
    assert_eq!(seen(&w), [json!([false, [["chrome", "Profile 2"]]])]);
    assert_eq!(
        journal(&w, "task.profiles_remembered")[0]["profiles"],
        json!(["chrome/Profile 2"])
    );
}

/// A memory FERB guessed (observed or inferred) naming the profile is not the owner's words: dropped, and the task
/// meets the pick-or-question path. A forgotten memory is no memory at all: the same.
// decision: HAND-587
#[test]
fn a_profile_only_ferbs_own_memory_names_or_a_forgotten_one_is_dropped() {
    for (other, why) in [
        (
            vec![SCALER],
            "only FERB's own memory names it, not the owner's words",
        ),
        (vec![], "nothing in the owner's words names it"),
    ] {
        let w = run_with_sources(
            plan_answer("owner", &["scaler.com"], &["scaler"]),
            vec![
                json!({"ask": "ap-browsing"}),
                json!({"as_owner": true, "label": "Google Chrome “New_Personal”"}),
            ],
            vec![look()],
            sources(&[], &[], &other, &[SCALER, IIT, PERSONAL]),
        );
        assert_eq!(
            journal(&w, "task.profiles_dropped")[0]["dropped"],
            json!([{"profile": "chrome/Profile 2", "why": why}])
        );
        assert_eq!(
            asked_while_here(&w),
            [true, false],
            "the one question, once"
        );
        assert_eq!(seen(&w), [json!([true, []])], "the pick");
    }
}

/// This message names another profile: that one, and the remembered one is dropped (the current message wins).
// decision: HAND-587
#[test]
fn a_profile_named_now_wins_over_a_remembered_one() {
    let w = run_with_sources(
        plan_answer("owner", &["scaler.com"], &["New_Personal", "scaler"]),
        vec![json!({"ask": "ap-browsing"})],
        vec![look()],
        sources(&[PERSONAL], &[SCALER], &[], &[SCALER, IIT, PERSONAL]),
    );
    assert!(asked_while_here(&w).is_empty());
    assert_eq!(seen(&w), [json!([false, [["chrome", "Profile 15"]]])]);
    assert_eq!(
        journal(&w, "task.profiles_dropped")[0]["dropped"],
        json!([{"profile": "chrome/Profile 2", "why": "this message names another profile"}])
    );
}

/// A remembered profile that isn't paired any more is dropped: the task falls to the pick or the question.
// decision: HAND-587
#[test]
fn a_remembered_profile_that_isnt_paired_is_dropped() {
    let w = run_with_sources(
        plan_answer("owner", &["scaler.com"], &["scaler"]),
        vec![json!({"ask": "ap-browsing"}), json!({"declined": true})],
        vec![look()],
        sources(&[], &[SCALER], &[], &[IIT, PERSONAL]),
    );
    assert_eq!(
        journal(&w, "task.profiles_dropped")[0]["dropped"],
        json!([{"profile": "chrome/Profile 2", "why": "the owner's remembered profile isn't paired"}])
    );
    assert_eq!(asked_while_here(&w), [true, false]);
}
