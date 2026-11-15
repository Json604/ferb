//! PRM-355 (the owner, 2026-11-14) and GATE-593: an answer to one of FERB's checks that asks for more than the check
//! has ("yes forever for everything") is never taken as applied: FERB says nothing was applied and offers the choices
//! it has; the check stays open. The owner's own typed words that are exactly a choice still answer it. Scripted on the
//! recorded plain_reply scenario (fake model, no provider): the file question is open in the gateway's real question
//! store; the owner writes in chat; the planner routes the words to the question (answer_question).

mod common;
mod support_control;

use ferb_gateway::answers::{options, Question};
use ferb_gateway::file_grants::{self, EVERY_SITE};
use ferb_gateway::questions::{Asker, State};
use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const CARD: &str = "Send your file to rex.example?";

/// The journal's rows of `kind`, their data.
fn rows(w: &World, kind: &str) -> Vec<Value> {
    let db = w.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT data_json FROM journal WHERE kind = ? ORDER BY seq")
        .unwrap();
    let rows = st
        .query_map([kind], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    rows
}

/// What FERB said in the chat: its replies and its clarifying questions.
fn said(w: &World) -> Vec<String> {
    ["chat.reply", "chat.clarify"]
        .iter()
        .flat_map(|k| rows(w, k))
        .map(|v| v["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// plain_reply with the file question open, the owner's message `said`, and the planner reading it as `reading`.
fn answered(said: &'static str, reading: &'static str, text: &'static str) -> (World, String) {
    let name = "plain_reply";
    let mut w = world_with(name, |calls| {
        let planner = calls.iter_mut().find(|c| c["key"] == "supervisor").unwrap();
        let out = json!({"kind": "reply", "answers_question": false, "task": null, "control": null,
                         "side_entries": {"answer_question": {"approval_id": "ap-file", "answer": reading},
                                          "remember": [], "forget": [], "timers": [], "schedules": [],
                                          "schedule_changes": [], "show_schedules": false,
                                          "reminder_cancels": [], "conversation_changes": []},
                         "text": text});
        planner["events"] = json!([
            {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001}},
            {"kind": "final", "data": {"text": out.to_string()}}
        ]);
    });
    // the gateway's file question, as `ask_confirmation` sets it
    let need = Question::FileSend.example();
    let opts = options(std::slice::from_ref(&need));
    {
        let q = w.daemon.questions.lock().unwrap().clone();
        let mut store = q.store.lock().unwrap();
        let ap = store.question(
            &Asker {
                capability: "ferb.confirm",
                task_id: "",
                request_id: "req-earlier",
            },
            CARD,
            &opts,
            Some("ap-file"),
        );
        ap.summary = CARD.into();
        ap.corrects = true;
        ap.labels = vec!["Send".into(), "Don't send".into()];
        let mut note = String::new();
        file_grants::card(&mut ap.labels, &mut note, std::slice::from_ref(&need));
        ap.note = note;
    }
    let report = run_with(&mut w, name, |steps| {
        for st in steps.iter_mut().filter(|st| st["do"] == "stream") {
            let mut sent: Value =
                serde_json::from_str(st["lines"][0]["send"].as_str().unwrap()).unwrap();
            sent["params"]["text"] = json!(said);
            st["lines"][0]["send"] = json!(sent.to_string());
        }
    });
    let verdict = report.verdict();
    (w, verdict)
}

fn file_question(w: &World) -> ferb_gateway::questions::Approval {
    let q = w.daemon.questions.lock().unwrap().clone();
    let store = q.store.lock().unwrap();
    store.get("ap-file").cloned().expect("the file question")
}

// decision: PRM-355
// decision: PRM-356
// decision: GATE-593
#[test]
fn words_asking_for_more_than_the_choices_apply_nothing_and_ferb_says_so() {
    let (w, verdict) = answered(
        "yes forever for everything",
        EVERY_SITE,
        "Done: I'll send it everywhere from now on.",
    );
    eprintln!("plain_reply (scripted): {verdict}");
    let ap = file_question(&w);
    assert_eq!(ap.state, State::Pending, "the question stays open");
    assert_eq!(ap.choice, None, "no choice was taken");
    let replies = said(&w);
    let said = format!(
        "{}\nIs that Send, Send to every site in this task or Don't send for “{CARD}”?",
        ferb_supervisor::flow::answer::NOT_APPLIED
    );
    assert!(
        replies.contains(&said),
        "FERB says nothing was applied and which choices it has: {replies:?}"
    );
    assert!(
        !replies.iter().any(|r| r.contains("everywhere from now on")),
        "the planner's claim that it was applied never reaches the owner: {replies:?}"
    );
    assert!(rows(&w, "approval.resolved").is_empty(), "nothing resolved");
}

// decision: GATE-593
#[test]
fn the_owners_exact_choice_words_in_chat_answer_the_file_question() {
    let words = "Send to every site in this task";
    let (w, _) = answered(words, EVERY_SITE, "");
    let ap = file_question(&w);
    let at = ap
        .option_for(words)
        .expect("the button's words are a choice");
    assert_eq!(ap.options[at - 1], EVERY_SITE);
    // this harness's daemon keeps no answers (`resolve_question`): the routed answer is read from the journal
    let resolved = rows(&w, "approval.resolved");
    assert_eq!(resolved.len(), 1, "{resolved:?}");
    assert_eq!(resolved[0]["approval_id"], "ap-file");
    assert_eq!(resolved[0]["state"], "granted");
    assert!(
        !said(&w)
            .iter()
            .any(|r| r.contains(ferb_supervisor::flow::answer::NOT_APPLIED)),
        "an exact choice is applied"
    );
}
