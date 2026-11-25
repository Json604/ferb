//! T-1277: the owner's follow-up message in every state a task can be in, through the real program (tests/life: the
//! composed `ferbd daemon`, its gateway and chat, stand-in models, the Mac's own parts off). One table: every pair of
//! state (planning, running, waiting on a safety question, paused, done, failed, stopped) and follow-up (the planner
//! reads it as stop, a correction, a re-plan, pause, or new work) is a cell. A cell a decision defines runs on its own
//! daemon and asserts that outcome; a cell no decision defines is listed as undecided with what is missing, never
//! encoded from today's behaviour.
//!
//! Every run cell also keeps the invariants the test audit of 2026-11-21 named: the chat's entries keep stage 2's
//! order rules; no question ends as a No nobody gave; a stop's words are said once, only where the owner stopped;
//! the restart's words only where a re-plan replaced running work; nothing is left open once every task ended.

#[path = "../../ferb-supervisor/tests/support_entries/mod.rs"]
mod entry_rules;
mod life;
mod support;

use std::collections::BTreeSet;

use ferb_orchestrator::answer_marks::STOPPED;
use ferb_supervisor::words::RESTARTED_SAID;
use life::*;
use serde_json::{json, Value};
use support::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    Planning,
    Running,
    Question,
    Paused,
    Done,
    Failed,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Follow {
    Stop,
    Correct,
    Replan,
    Pause,
    NewTask,
}

const STATES: [State; 7] = [
    State::Planning,
    State::Running,
    State::Question,
    State::Paused,
    State::Done,
    State::Failed,
    State::Stopped,
];
const FOLLOWS: [Follow; 5] = [
    Follow::Stop,
    Follow::Correct,
    Follow::Replan,
    Follow::Pause,
    Follow::NewTask,
];

/// A cell's run on its own temporary root.
type CellRun = fn(&std::path::Path) -> Ended;

enum Cell {
    /// Run: the decisions that define the outcome, the run.
    Decided(&'static str, CellRun),
    /// Defined by another cell's run (the same rule makes no difference by what the message says).
    Same(State, Follow, &'static str),
    /// No decision defines it: what is missing.
    Undecided(&'static str),
}

/// The grid. Every (state, follow-up) pair is here once (checked below).
fn grid() -> Vec<(State, Follow, Cell)> {
    use Cell::*;
    use Follow::*;
    use State::*;
    vec![
        // before FERB's first reply any message restarts the turn once and both are read together (ORCH-529): what it
        // says is the planner's to read, so one run stands for the row
        (Planning, Correct, Decided("ORCH-529", planning_second_message)),
        (Planning, Stop, Same(Planning, Correct, "ORCH-529")),
        (Planning, Replan, Same(Planning, Correct, "ORCH-529")),
        (Planning, Pause, Same(Planning, Correct, "ORCH-529")),
        (Planning, NewTask, Same(Planning, Correct, "ORCH-529")),
        (Running, Stop, Decided("ORCH-579, ORCH-688", running_stop)),
        (Running, Correct, Decided("ORCH-308, ORCH-683", running_correct)),
        (Running, Replan, Decided("ORCH-682, ORCH-683", running_replan)),
        (Running, Pause, Decided("ORCH-308", running_pause)),
        (Running, NewTask, Decided("ORCH-529", running_new_task)),
        (Question, Stop, Decided("ORCH-684, ORCH-579", question_stop)),
        (Question, Correct, Decided("ORCH-538", question_correct)),
        (Question, Replan, Decided("ORCH-684, ORCH-682", question_replan)),
        (
            Question,
            Pause,
            Undecided(
                "whether a safety question open when its task is paused is set aside (asked again at the resume) or \
                 stays open: ORCH-308 and ORCH-312 say what a pause stops, ORCH-684 covers only a stop or a restart",
            ),
        ),
        (Question, NewTask, Decided("ORCH-529", question_new_task)),
        (
            Paused,
            Stop,
            Undecided(
                "a stop of a paused task: ORCH-579 and ORCH-688 define a running task's stop and its words; for a \
                 paused one only the recorded Python scenario stop_chat_while_paused says what happens",
            ),
        ),
        (Paused, Correct, Decided("ORCH-308, ORCH-324", paused_correct)),
        (
            Paused,
            Replan,
            Undecided(
                "a re-plan of a paused task: ORCH-682 names running and ended tasks; whether a paused one restarts \
                 like a running one (its old run ended with the restart's words) or resumes into the new plan isn't said",
            ),
        ),
        (
            Paused,
            Pause,
            Undecided("a pause of a task already paused: no decision says what FERB answers"),
        ),
        (Paused, NewTask, Decided("ORCH-308", paused_new_task)),
        (
            Done,
            Stop,
            Undecided(
                "a stop of a task that already ended: nothing runs; what FERB answers is defined only by the recorded \
                 Python scenario stop_after_end",
            ),
        ),
        (Done, Correct, Decided("ORCH-324, ORCH-682", done_correct)),
        (Done, Replan, Decided("ORCH-682, ORCH-683", done_replan)),
        (
            Done,
            Pause,
            Undecided("a pause of a task that already ended: no decision says what FERB answers"),
        ),
        (Done, NewTask, Decided("APP-974", done_new_task)),
        (
            Failed,
            Stop,
            Undecided(
                "a stop of a failed task: nothing runs; no decision says what FERB answers",
            ),
        ),
        (Failed, Correct, Decided("ORCH-324, ORCH-682", failed_correct)),
        (Failed, Replan, Decided("ORCH-682, ORCH-683", failed_replan)),
        (
            Failed,
            Pause,
            Undecided("a pause of a failed task: no decision says what FERB answers"),
        ),
        (Failed, NewTask, Same(Done, NewTask, "APP-974")),
        (
            Stopped,
            Stop,
            Undecided(
                "a second stop of a stopped task: no decision says whether FERB says the stop again",
            ),
        ),
        (Stopped, Correct, Decided("ORCH-324, ORCH-682", stopped_correct)),
        (Stopped, Replan, Decided("ORCH-682, ORCH-683", stopped_replan)),
        (
            Stopped,
            Pause,
            Undecided("a pause of a stopped task: no decision says what FERB answers"),
        ),
        (Stopped, NewTask, Same(Done, NewTask, "APP-974")),
    ]
}

/// What a cell's run hands to the invariants: the program, whether the owner stopped something, whether a re-plan
/// replaced running work, and whether every task has ended.
struct Ended {
    life: Life,
    stopped: bool,
    restarted: bool,
    all_ended: bool,
}

// ---------------------------------------------------------------------------------------------------------------
// the states
// ---------------------------------------------------------------------------------------------------------------

const TASK: &str = "Draft a note about friday";

/// The task's first message, its worker under way (`worker` is its call: held, or one that never answers).
fn running(root: &std::path::Path, model: Model) -> (Life, Send) {
    let life = Life::start(root, "01", model);
    let first = life.send("draft a note about friday");
    life.wait("the worker started", 30, |l| {
        l.calls_of("worker:default") == 1
    });
    (life, first)
}

fn zshrc() -> (&'static str, Value) {
    (
        "fs.write",
        json!({"path": "~/.zshrc", "content": "alias ll='ls -l'\n", "overwrite": true, "effect": "edit",
               "why": "add the alias"}),
    )
}

/// The check of a step that changed ~/.zshrc: it reads the file first (the code refuses a pass that didn't).
fn check_reading_zshrc() -> Value {
    with_tools(check_accept(), &[("fs.read", json!({"path": "~/.zshrc"}))])
}

/// A task whose worker's write waits on FERB's safety question (the gateway's confirmation for ~/.zshrc).
fn question(root: &std::path::Path, model: Model) -> (Life, Send) {
    let life = Life::start(root, "01", model);
    let first = life.send("add an alias ll for ls -l to my zshrc");
    life.wait("the question waits", 30, |l| !l.open_questions().is_empty());
    (life, first)
}

fn ended(life: &Life, status: &str) {
    life.wait(&format!("the task is {status}"), 30, |l| {
        l.root_status().as_deref() == Some(status)
    });
}

/// The task paused by the owner's words (its worker never answers), its pause settled: the step holds for the owner
/// (its stored status `paused`), so a follow-up meets a paused task, not one whose attempt is still ending (that race
/// is `a_correction_while_a_pause_settles_resumes_the_task`).
fn paused(root: &std::path::Path, model: Model) -> (Life, Send) {
    let (life, first) = running(root, model);
    life.send("wait, hold on").end();
    ended(&life, "paused");
    let task = root_task(&life);
    life.wait("the step holds for the owner", 30, |l| {
        l.step_status(&task, "worker").as_deref() == Some("paused")
    });
    (life, first)
}

fn finished(root: &std::path::Path, model: Model, status: &str) -> Life {
    let life = Life::start(root, "01", model);
    life.send("draft a note about friday").end();
    ended(&life, status);
    life.settle();
    life
}

/// The task stopped by the owner's words while its worker ran.
fn stopped(root: &std::path::Path, model: Model) -> Life {
    let (life, mut first) = running(root, model);
    life.send("stop that").end();
    first.end();
    ended(&life, "cancelled");
    life.settle();
    life
}

fn tasks_done(life: &Life) -> BTreeSet<String> {
    life.tasks()
        .into_iter()
        .filter(|(id, s)| !id.contains('.') && s == "done")
        .map(|(id, _)| id)
        .collect()
}

/// The words of the answers FERB gave for this task's ends.
fn task_words(life: &Life, task: &str) -> Vec<String> {
    life.entries("answer")
        .iter()
        .filter(|e| e["task_id"] == task && e["body"]["from"] == "task")
        .map(said)
        .collect()
}

fn root_task(life: &Life) -> String {
    life.tasks()
        .into_iter()
        .find(|(id, _)| !id.contains('.'))
        .map(|(id, _)| id)
        .expect("a task")
}

/// The model calls made after the first `n`.
fn made_after(life: &Life, n: usize) -> Vec<String> {
    life.made().into_iter().skip(n).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// the cells
// ---------------------------------------------------------------------------------------------------------------

/// ORCH-529: a second message before FERB's first reply restarts the turn once; one planner call reads both; the
/// second message is merged into the first, whose turn gets the one plan and answer.
fn planning_second_message(root: &std::path::Path) -> Ended {
    let life = Life::start(
        root,
        "01",
        Model::default()
            .planner(stall())
            .planner(plan_task(
                "Check the weather for Avalon and Brindle",
                "Checking both.",
            ))
            .worker(worker_done("Avalon: sunny. Brindle: rain."))
            .checker(always(check_accept())),
    );
    let mut first = life.send("what's the weather in Avalon");
    life.wait("the planner started", 30, |l| {
        l.calls_of("orchestrator") == 1
    });
    let mut second = life.send("and Brindle too");
    second.end();
    first.end();
    ended(&life, "done");
    life.settle();
    assert_eq!(
        second.frames("merged").len(),
        1,
        "the second's stream says it was merged: {:?}",
        second.events
    );
    assert_eq!(life.calls_of("orchestrator"), 2, "restarted once");
    let messages = life.entries("message");
    assert_eq!(messages[1]["state"], "merged", "{:?}", messages[1]);
    assert_eq!(life.entries("plan").len(), 1);
    let task = root_task(&life);
    assert_eq!(
        task_words(&life, &task),
        vec!["Avalon: sunny. Brindle: rain."]
    );
    Ended {
        life,
        stopped: false,
        restarted: false,
        all_ended: true,
    }
}

/// ORCH-579, ORCH-688: the stop message's reply says the stop once; the task's own turn shows only that it ended; the
/// task is cancelled and no model call follows the stop.
fn running_stop(root: &std::path::Path) -> Ended {
    let (life, mut first) = running(
        root,
        Model::default()
            .planner(plan_task(TASK, "On it."))
            .planner(plan_control("stop", ""))
            .worker(stall()),
    );
    let mut stop = life.send("stop that");
    stop.end();
    first.end();
    let calls = life.made().len();
    life.settle();
    let replies = stop.frames("reply");
    assert_eq!(replies.len(), 1, "{:?}", stop.events);
    assert!(
        replies[0]["text"]
            .as_str()
            .unwrap_or("")
            .starts_with(STOPPED),
        "{:?}",
        replies[0]
    );
    let task = root_task(&life);
    assert_eq!(life.root_status().as_deref(), Some("cancelled"));
    assert_eq!(
        task_words(&life, &task),
        vec![""],
        "the task's own turn only says it ended"
    );
    assert_eq!(
        made_after(&life, calls),
        Vec::<String>::new(),
        "nothing after the stop"
    );
    Ended {
        life,
        stopped: true,
        restarted: false,
        all_ended: true,
    }
}

/// ORCH-308 (a plain correction steers live), ORCH-683 (a continue with no new step graph steers in place): the words
/// reach the running work, which goes on to the task's one end; no new worker run; the follow-up's reply is the
/// planner's own.
fn running_correct(root: &std::path::Path) -> Ended {
    let (life, mut first) = running(
        root,
        Model::default()
            .planner(plan_task(TASK, "On it."))
            .planner(plan_control("continue", "Adding the 3pm start."))
            .worker(held(
                worker_done("Drafted the note with the 3pm start."),
                "w1",
            ))
            .checker(always(check_accept())),
    );
    let mut correct = life.send("mention it starts at 3pm");
    correct.end();
    life.release("w1");
    first.end();
    ended(&life, "done");
    life.settle();
    let replies: Vec<&Value> = correct.frames("reply");
    assert_eq!(replies.len(), 1, "{:?}", correct.events);
    assert_eq!(replies[0]["text"], "Adding the 3pm start.");
    assert_eq!(
        life.calls_of("worker:default"),
        1,
        "steered, not started again"
    );
    let task = root_task(&life);
    assert_eq!(
        task_words(&life, &task),
        vec!["Drafted the note with the 3pm start."]
    );
    Ended {
        life,
        stopped: false,
        restarted: false,
        all_ended: true,
    }
}

const STEPS: [(&str, &str, &[&str]); 3] = [
    ("a", "Search the mail", &[]),
    ("b", "Search the drive", &[]),
    ("answer", "Put it together", &["a", "b"]),
];

fn replan_model(model: Model) -> Model {
    model
        .planner(plan_retry_with_steps(
            TASK,
            "Doing them at the same time.",
            &STEPS,
        ))
        .worker(worker_done("Mail: two."))
        .worker(worker_done("Drive: one."))
        .worker(worker_done("Three in all."))
}

/// The new plan's steps all ran in the same task, which ended with the last step's answer.
fn new_plan_ran(life: &Life, task: &str) {
    ended(life, "done");
    life.settle();
    for step in ["a", "b", "answer"] {
        let id = format!("{task}.step.{step}");
        assert!(
            life.tasks().contains(&(id.clone(), "done".to_string())),
            "{id} ran: {:?}",
            life.tasks()
        );
    }
    let words = task_words(life, task);
    assert_eq!(
        words.last().map(String::as_str),
        Some("Three in all."),
        "{words:?}"
    );
}

/// ORCH-682, ORCH-683: a re-plan of running work stops it with the restart's own words (never the owner's stop) and
/// runs the new plan at once, in the same task.
fn running_replan(root: &std::path::Path) -> Ended {
    let (life, mut first) = running(
        root,
        replan_model(
            Model::default()
                .planner(plan_task(TASK, "On it."))
                .worker(stall()),
        )
        .checker(always(check_accept())),
    );
    let task = root_task(&life);
    life.send("do it faster, all at the same time").end();
    first.end();
    new_plan_ran(&life, &task);
    assert_eq!(
        task_words(&life, &task),
        vec![RESTARTED_SAID.to_string(), "Three in all.".to_string()]
    );
    Ended {
        life,
        stopped: false,
        restarted: true,
        all_ended: true,
    }
}

/// ORCH-308: a pause ends the current attempt and marks the task paused, not cancelled: no final answer.
fn running_pause(root: &std::path::Path) -> Ended {
    let (life, _first) = paused(
        root,
        Model::default()
            .planner(plan_task(TASK, "On it."))
            .planner(plan_control("pause", "Paused."))
            .worker(stall()),
    );
    life.settle();
    let task = root_task(&life);
    assert_eq!(life.root_status().as_deref(), Some("paused"));
    assert_eq!(
        task_words(&life, &task),
        Vec::<String>::new(),
        "no final answer"
    );
    Ended {
        life,
        stopped: false,
        restarted: false,
        all_ended: false,
    }
}

/// ORCH-529: once FERB has replied, a running task is unaffected by the next message; new work is its own task.
fn running_new_task(root: &std::path::Path) -> Ended {
    let (life, mut first) = running(
        root,
        Model::default()
            .planner(plan_task(TASK, "On it."))
            .planner(plan_task("Check the weather", "Checking."))
            .worker(held(worker_done("Drafted."), "w1"))
            .worker(worker_done("Sunny."))
            .checker(always(check_accept())),
    );
    let task = root_task(&life);
    life.send("what's the weather").end();
    life.wait("the second task ended", 30, |l| tasks_done(l).len() == 1);
    life.release("w1");
    first.end();
    life.wait("both ended", 30, |l| tasks_done(l).len() == 2);
    life.settle();
    assert_eq!(task_words(&life, &task), vec!["Drafted."], "unaffected");
    assert_eq!(life.calls_of("worker:default"), 2);
    Ended {
        life,
        stopped: false,
        restarted: false,
        all_ended: true,
    }
}

/// The question entry of this approval: closed and never declined.
fn question_set_aside(life: &Life, approval: &Value) {
    let q = life
        .entries("question")
        .into_iter()
        .find(|e| &e["approval_id"] == approval)
        .unwrap_or_else(|| panic!("the question {approval} stays in the chat"));
    assert_eq!(q["open"], false, "{q}");
    assert_eq!(q["state"], "expired", "set aside, never declined: {q}");
}

fn zshrc_text(life: &Life) -> String {
    std::fs::read_to_string(life.run.user.join(".zshrc")).unwrap_or_default()
}

/// ORCH-684 (a question open at a stop is set aside, never a No), ORCH-579: the stop said once; the change never made.
fn question_stop(root: &std::path::Path) -> Ended {
    let (life, mut first) = question(
        root,
        Model::default()
            .planner(plan_task(
                "Add the alias ll to ~/.zshrc",
                "Adding the alias.",
            ))
            .planner(plan_control("stop", ""))
            .worker(with_tools(worker_done("Added."), &[zshrc()])),
    );
    let asked = life.open_questions()[0]["approval_id"].clone();
    let mut stop = life.send("stop");
    stop.end();
    first.end();
    let calls = life.made().len();
    ended(&life, "cancelled");
    life.settle();
    question_set_aside(&life, &asked);
    assert_eq!(zshrc_text(&life), "", "the change was never made");
    assert_eq!(
        made_after(&life, calls),
        Vec::<String>::new(),
        "nothing after the stop"
    );
    let replies = stop.frames("reply");
    assert!(
        replies.len() == 1
            && replies[0]["text"]
                .as_str()
                .unwrap_or("")
                .starts_with(STOPPED),
        "{:?}",
        stop.events
    );
    Ended {
        life,
        stopped: true,
        restarted: false,
        all_ended: true,
    }
}

/// ORCH-538: a correction sets aside the task's open safety check (never a No) once the worker has the words, and the
/// action asks again; the owner's Yes to the new question lets it run, once.
fn question_correct(root: &std::path::Path) -> Ended {
    let (life, mut first) = question(
        root,
        Model::default()
            .planner(plan_task(
                "Add the alias ll to ~/.zshrc",
                "Adding the alias.",
            ))
            .planner(plan_control("continue", "Adding it as asked."))
            // the worker tries its write again after the first question is set aside
            .worker(with_tools(worker_done("Added."), &[zshrc(), zshrc()]))
            .checker(always(check_reading_zshrc())),
    );
    let asked = life.open_questions()[0]["approval_id"].clone();
    life.send("keep the comment at the top too").end();
    life.wait("asked again", 30, |l| {
        l.open_questions().iter().any(|q| q["approval_id"] != asked)
    });
    question_set_aside(&life, &asked);
    let again = life.open_questions()[0]["approval_id"].clone();
    let yes = life.call(
        "chat.approve",
        json!({"approval_id": again, "granted": true, "choice": 1}),
    );
    assert_eq!(yes["result"]["state"], "granted", "{yes}");
    first.end();
    ended(&life, "done");
    life.settle();
    assert_eq!(zshrc_text(&life), "alias ll='ls -l'\n");
    Ended {
        life,
        stopped: false,
        restarted: false,
        all_ended: true,
    }
}

/// ORCH-684 (a restart while a question waits sets it aside, never a No), ORCH-682: the new plan runs at once.
fn question_replan(root: &std::path::Path) -> Ended {
    let (life, mut first) = question(
        root,
        replan_model(
            Model::default()
                .planner(plan_task(TASK, "On it."))
                .worker(with_tools(worker_done("Added."), &[zshrc()])),
        )
        .checker(always(check_accept())),
    );
    let task = root_task(&life);
    let asked = life.open_questions()[0]["approval_id"].clone();
    life.send("do it differently, all at the same time").end();
    first.end();
    new_plan_ran(&life, &task);
    question_set_aside(&life, &asked);
    assert_eq!(
        zshrc_text(&life),
        "",
        "the old plan's change was never made"
    );
    Ended {
        life,
        stopped: false,
        restarted: true,
        all_ended: true,
    }
}

/// ORCH-529: the running task is unaffected by new work: its question stays open, the same one, until the owner
/// answers it; then it ends as usual.
fn question_new_task(root: &std::path::Path) -> Ended {
    let (life, mut first) = question(
        root,
        Model::default()
            .planner(plan_task(
                "Add the alias ll to ~/.zshrc",
                "Adding the alias.",
            ))
            .planner(plan_task("Check the weather", "Checking."))
            .worker(with_tools(worker_done("Added."), &[zshrc()]))
            .worker(worker_done("Sunny."))
            .checker(always(check_reading_zshrc())),
    );
    let asked = life.open_questions()[0]["approval_id"].clone();
    life.send("what's the weather").end();
    life.wait("the new task ended", 30, |l| tasks_done(l).len() == 1);
    let open: Vec<Value> = life
        .open_questions()
        .iter()
        .map(|q| q["approval_id"].clone())
        .collect();
    assert_eq!(open, vec![asked.clone()], "the same question still waits");
    life.call(
        "chat.approve",
        json!({"approval_id": asked, "granted": true, "choice": 1}),
    );
    first.end();
    life.wait("both ended", 30, |l| tasks_done(l).len() == 2);
    life.settle();
    assert_eq!(zshrc_text(&life), "alias ll='ls -l'\n");
    Ended {
        life,
        stopped: false,
        restarted: false,
        all_ended: true,
    }
}

/// ORCH-308 (resume continues in place with the owner's words), ORCH-324 (one continue verb): the same task runs
/// again and ends once.
fn paused_correct(root: &std::path::Path) -> Ended {
    let (life, mut first) = paused(
        root,
        Model::default()
            .planner(plan_task(TASK, "On it."))
            .planner(plan_control("pause", "Paused."))
            .planner(plan_control("continue", "Carrying on with the 3pm start."))
            .worker(stall())
            .worker(worker_done("Drafted the note with the 3pm start."))
            .checker(always(check_accept())),
    );
    let task = root_task(&life);
    life.send("ok, carry on and mention 3pm").end();
    first.end();
    ended(&life, "done");
    life.settle();
    assert_eq!(root_task(&life), task);
    assert_eq!(
        task_words(&life, &task),
        vec!["Drafted the note with the 3pm start."]
    );
    Ended {
        life,
        stopped: false,
        restarted: false,
        all_ended: true,
    }
}

/// ORCH-308: a paused task stays paused until resumed; new work runs as its own task.
fn paused_new_task(root: &std::path::Path) -> Ended {
    let (life, _first) = paused(
        root,
        Model::default()
            .planner(plan_task(TASK, "On it."))
            .planner(plan_control("pause", "Paused."))
            .planner(plan_task("Check the weather", "Checking."))
            .worker(stall())
            .worker(worker_done("Sunny."))
            .checker(always(check_accept())),
    );
    let task = root_task(&life);
    life.send("what's the weather").end();
    life.wait("the new task ended", 30, |l| tasks_done(l).len() == 1);
    life.settle();
    let status = life
        .tasks()
        .into_iter()
        .find(|(id, _)| *id == task)
        .map(|(_, s)| s);
    assert_eq!(status.as_deref(), Some("paused"), "still paused");
    assert_eq!(task_words(&life, &task), Vec::<String>::new());
    Ended {
        life,
        stopped: false,
        restarted: false,
        all_ended: false,
    }
}

/// Model calls that end a one-step task done ("Drafted.").
fn done_model() -> Model {
    Model::default()
        .planner(plan_task(TASK, "On it."))
        .worker(worker_done("Drafted."))
}

/// A one-step task that failed (its check said not done).
fn failed_model() -> Model {
    Model::default()
        .planner(plan_task(TASK, "On it."))
        .worker(worker_not_done("The notes app was closed."))
        .checker(check_fail("Not drafted."))
}

/// An ended task corrected (ORCH-324's one continue verb, ORCH-682): the same task runs again; the follow-up's turn
/// gets its one new answer; the earlier answer stays as it was, never shown again (APP-974).
fn ended_then_corrected(life: Life, before: Vec<String>) -> Ended {
    let task = root_task(&life);
    life.send("mention it starts at 3pm").end();
    ended(&life, "done");
    life.settle();
    assert_eq!(root_task(&life), task, "the same task");
    let mut want = before;
    want.push("Drafted the note with the 3pm start.".into());
    assert_eq!(task_words(&life, &task), want);
    Ended {
        life,
        stopped: false,
        restarted: false,
        all_ended: true,
    }
}

fn corrected(model: Model) -> Model {
    model
        .planner(plan_control("continue", "Adding the 3pm start."))
        .worker(worker_done("Drafted the note with the 3pm start."))
}

fn done_correct(root: &std::path::Path) -> Ended {
    let life = finished(
        root,
        corrected(done_model()).checker(always(check_accept())),
        "done",
    );
    ended_then_corrected(life, vec!["Drafted.".into()])
}

fn failed_correct(root: &std::path::Path) -> Ended {
    let life = finished(
        root,
        corrected(failed_model()).checker(always(check_accept())),
        "failed",
    );
    let before = task_words(&life, &root_task(&life));
    assert_eq!(before.len(), 1, "{before:?}");
    ended_then_corrected(life, before)
}

fn stopped_correct(root: &std::path::Path) -> Ended {
    let life = stopped(
        root,
        Model::default()
            .planner(plan_task(TASK, "On it."))
            .planner(plan_control("stop", ""))
            .planner(plan_control("continue", "Adding the 3pm start."))
            .worker(stall())
            .worker(worker_done("Drafted the note with the 3pm start."))
            .checker(always(check_accept())),
    );
    let mut out = ended_then_corrected(life, vec![String::new()]);
    out.stopped = true;
    out
}

/// An ended task re-planned (ORCH-682): the new plan runs inside the same task.
fn ended_then_replanned(life: Life, stopped: bool) -> Ended {
    let task = root_task(&life);
    life.send("do it again, all at the same time").end();
    new_plan_ran(&life, &task);
    Ended {
        life,
        stopped,
        restarted: false,
        all_ended: true,
    }
}

fn done_replan(root: &std::path::Path) -> Ended {
    let life = finished(
        root,
        replan_model(done_model()).checker(always(check_accept())),
        "done",
    );
    ended_then_replanned(life, false)
}

fn failed_replan(root: &std::path::Path) -> Ended {
    let life = finished(
        root,
        replan_model(failed_model()).checker(always(check_accept())),
        "failed",
    );
    ended_then_replanned(life, false)
}

fn stopped_replan(root: &std::path::Path) -> Ended {
    let life = stopped(
        root,
        replan_model(
            Model::default()
                .planner(plan_task(TASK, "On it."))
                .planner(plan_control("stop", ""))
                .worker(stall()),
        )
        .checker(always(check_accept())),
    );
    ended_then_replanned(life, true)
}

/// APP-974: new work after an ended task is its own task; the ended one's answer stays as it was, shown once.
fn done_new_task(root: &std::path::Path) -> Ended {
    let life = finished(
        root,
        done_model()
            .planner(plan_task("Check the weather", "Checking."))
            .worker(worker_done("Sunny."))
            .checker(always(check_accept())),
        "done",
    );
    let task = root_task(&life);
    life.send("what's the weather").end();
    life.wait("the new task ended", 30, |l| tasks_done(l).len() == 2);
    life.settle();
    assert_eq!(task_words(&life, &task), vec!["Drafted."]);
    Ended {
        life,
        stopped: false,
        restarted: false,
        all_ended: true,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the invariants and the run
// ---------------------------------------------------------------------------------------------------------------

/// The 2026-11-21 audit's invariants, on what the program wrote: one line each that breaks.
fn invariants(name: &str, e: &Ended) -> Vec<String> {
    let life = &e.life;
    let mut out = life.order_rules(name);
    let chat = life.chat();
    for q in chat.iter().filter(|x| x["kind"] == "question") {
        if q["state"] == "declined" {
            out.push(format!("{name}: a question ended as a No nobody gave: {q}"));
        }
    }
    for r in life.journal("approval.resolved") {
        if r["state"] == "denied" {
            out.push(format!("{name}: a question was resolved No: {r}"));
        }
    }
    let saying = |words: &str| chat.iter().filter(|x| said(x).starts_with(words)).count();
    let stops = saying(STOPPED);
    if stops != usize::from(e.stopped) {
        out.push(format!(
            "{name}: the stop's words are said {stops} times (the owner stopped: {})",
            e.stopped
        ));
    }
    let restarts = saying(RESTARTED_SAID);
    if restarts != usize::from(e.restarted) {
        out.push(format!(
            "{name}: the restart's words are said {restarts} times (a re-plan replaced running work: {})",
            e.restarted
        ));
    }
    if e.all_ended {
        let open: Vec<String> = chat
            .iter()
            .filter(|x| x["open"] == true)
            .map(|x| format!("{} {}", x["kind"], said(x)))
            .collect();
        if !open.is_empty() {
            out.push(format!(
                "{name}: left open after every task ended: {open:?}"
            ));
        }
        let running = life.call("chat.status", json!({}))["result"]["running"].clone();
        if running != json!([]) {
            out.push(format!("{name}: chat.status still runs {running}"));
        }
    }
    out
}

// decision: TEST-516
#[test]
fn the_grid_names_every_state_and_follow_up_once() {
    let cells = grid();
    let pairs: BTreeSet<(State, Follow)> = cells.iter().map(|(s, f, _)| (*s, *f)).collect();
    assert_eq!(pairs.len(), cells.len(), "a pair twice");
    for s in STATES {
        for f in FOLLOWS {
            assert!(pairs.contains(&(s, f)), "{s:?} x {f:?} missing");
        }
    }
    let mut undecided = Vec::new();
    for (s, f, c) in &cells {
        match c {
            Cell::Same(ss, ff, why) => {
                assert!(
                    cells.iter().any(|(s2, f2, c2)| (s2, f2) == (ss, ff)
                        && matches!(c2, Cell::Decided(..))),
                    "{s:?} x {f:?} names a run that isn't one"
                );
                assert!(!why.is_empty(), "{s:?} x {f:?}: by which rule");
            }
            Cell::Decided(by, _) => assert!(!by.is_empty(), "{s:?} x {f:?}: by which decision"),
            Cell::Undecided(missing) => {
                assert!(!missing.is_empty(), "{s:?} x {f:?}: what is missing");
                undecided.push(format!("{s:?} x {f:?}: {missing}"));
            }
        }
    }
    // the list for whoever decides them (`--nocapture`)
    eprintln!("undecided cells:\n{}", undecided.join("\n"));
}

/// Every decided cell through its own daemon, four at a time; every failure is reported, then the test fails.
// decision: ORCH-529, ORCH-579, ORCH-688, ORCH-308, ORCH-683, ORCH-682, ORCH-684, ORCH-538, ORCH-324, APP-974, TEST-515, TEST-516
#[test]
fn a_follow_up_in_every_state_does_what_its_decision_says() {
    let only = std::env::var("GRID_CELL").ok();
    let runs: Vec<(String, CellRun)> = grid()
        .into_iter()
        .filter_map(|(s, f, c)| match c {
            Cell::Decided(by, run) => Some((format!("{s:?} x {f:?} ({by})"), run)),
            _ => None,
        })
        .filter(|(name, _)| only.as_deref().is_none_or(|o| name.contains(o)))
        .collect();
    let mut failures: Vec<String> = Vec::new();
    for chunk in runs.chunks(4) {
        let results: Vec<(String, Result<Vec<String>, String>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|(name, run)| {
                    let name = name.clone();
                    let run = *run;
                    scope.spawn(move || {
                        let tmp = root();
                        let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let ended = run(tmp.path());
                            invariants(&name, &ended)
                        }));
                        let got = got.map_err(|p| {
                            p.downcast_ref::<String>()
                                .cloned()
                                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                                .unwrap_or_default()
                        });
                        (name, got)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (name, got) in results {
            match got {
                Ok(broken) => failures.extend(broken),
                Err(panic) => failures.push(format!("{name}: {panic}")),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} problems in {} cells run:\n{}",
        failures.len(),
        runs.len(),
        failures.join("\n\n")
    );
}

/// ORCH-308 ("resume continues in place with the owner's discussion as the update"), with a slow cancel: the owner
/// pauses, and while the paused attempt's model run is still ending (a real run can take seconds) he says to carry on.
/// FERB answers that it carries on; the task must then resume and end, not stay paused. Found by the grid under load
/// on 2026-11-25 (journal 233): the correction is delivered to the ending run (`task.steering_delivered`) and the task
/// stays paused for good, in about one run of three here; four runs, so a by-hand run nearly always shows it. T-1329.
// decision: ORCH-308, TEST-515
#[test]
#[ignore = "T-1329: a correction while a pause settles can leave the task paused; run by hand until it is fixed"]
fn a_correction_while_a_pause_settles_resumes_the_task() {
    let stuck: Vec<String> = (0..4)
        .filter_map(|n| {
            let tmp = root();
            let (life, mut first) = running(
                tmp.path(),
                Model::default()
                    .planner(plan_task(TASK, "On it."))
                    .planner(plan_control("pause", "Paused."))
                    .planner(plan_control("continue", "Carrying on with the 3pm start."))
                    .worker(slow_cancel(stall(), "the paused run ends"))
                    .worker(worker_done("Drafted the note with the 3pm start."))
                    .checker(always(check_accept())),
            );
            life.send("wait, hold on").end();
            ended(&life, "paused");
            life.send("ok, carry on and mention 3pm").end();
            life.release("the paused run ends");
            first.end();
            let resumed =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ended(&life, "done")));
            resumed.is_err().then(|| {
                format!(
                    "run {n}: still {:?} after FERB said it carries on; journal: {}",
                    life.root_status(),
                    life.journal_kinds().join(" ")
                )
            })
        })
        .collect();
    assert!(stuck.is_empty(), "{}", stuck.join("\n"));
}
