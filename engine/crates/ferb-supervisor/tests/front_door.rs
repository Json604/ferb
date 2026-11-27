//! ORCH-735, ORCH-736, ORCH-737 (kb/plans/front-door.md, step 5): with the front door on, a chat turn is one
//! plain-mode run that streams its reply and acts only through typed calls, each checked by today's checks as it is
//! made (`Inner::front_call`, as the gateway hands it over). Scripted on recorded scenarios through the real
//! supervisor with stand-in models (temp store, injected ids and clock, no provider): the recorded planner answer is
//! converted mechanically into the front door's calls (each part of the answer to its tool) and the effects are
//! compared with the old path's run of the same scenario.

mod common;
mod support_control;

use std::sync::atomic::Ordering;

use ferb_orchestrator::lookups::FRONT_TOOLS;
use serde_json::{json, Map, Value};
use support_control::{run_with, world_with, Report, World};

const CONCIERGE: &str = "You are FERB's front door.";

/// The recorded planner answer as the front door's calls (in an order a model could make them: control and side
/// entries first, then the hand-off or the question, which ends the run) and its plain text ("" for a hand-off or a
/// question, whose text is the call's). A task is handed off complex: the planner writes it (its recorded call).
fn convert(answer: &Value) -> (Vec<(String, Value)>, String) {
    convert_with(answer, None)
}

/// The owner's message a recorded planner call answered (its "owner's complete new message" block): his exact words,
/// which a simple hand-off quotes.
fn message_of(call: &Value) -> String {
    call["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .find_map(|p| p.strip_prefix("Layer 5 — owner's complete new message:\n"))
        .expect("the owner's message")
        .trim()
        .to_string()
}

/// ORCH-749: a recorded task as a simple hand-off: its goal, the owner's words (`quote`), its name, its first
/// condition's text as what the result must show, and the folders, earlier results and profiles it named.
fn simple_args(answer: &Value, quote: &str, aq: bool) -> Value {
    let task = &answer["task"];
    let list = |k: &str| {
        task.get(k)
            .filter(|v| v.is_array())
            .cloned()
            .unwrap_or(json!([]))
    };
    json!({"ack": answer["text"], "goal": task["goal"], "complexity": "simple", "answers_question": aq,
           "quote": quote, "name": task["name"], "done_when": task["success_conditions"][0]["text"],
           "identity": task["identity"], "profiles": list("profiles"), "folders": list("folders"),
           "sites": task["targets"]["sites"].as_array().cloned().unwrap_or_default(),
           "apps": task["targets"]["apps"].as_array().cloned().unwrap_or_default(),
           "paths": task["targets"]["paths"].as_array().cloned().unwrap_or_default(),
           "recipients": task["targets"]["recipients"].as_array().cloned().unwrap_or_default(),
           "earlier_results": list("earlier_results")})
}

/// As [`convert`]; with `quote` (the owner's words) a task one worker does in one go (no plan, at most one part) is
/// handed off simple, its task written by code (ORCH-749), and any other complex.
fn convert_with(answer: &Value, quote: Option<&str>) -> (Vec<(String, Value)>, String) {
    let side = &answer["side_entries"];
    let given = |v: &Value| match v {
        Value::Null | Value::Bool(false) => false,
        Value::Array(a) => !a.is_empty(),
        _ => true,
    };
    let aq = answer["answers_question"].as_bool().unwrap_or(false);
    let mut calls = Vec::new();
    let mut part = |tool: &str, fields: &[(&str, &Value)]| {
        let mut args = Map::new();
        for (k, v) in fields {
            if given(v) {
                args.insert(k.to_string(), (*v).clone());
            }
        }
        if !args.is_empty() {
            args.insert("answers_question".into(), json!(aq));
            calls.push((tool.to_string(), Value::Object(args)));
        }
    };
    part(
        "chat.control_task",
        &[
            ("control", &answer["control"]),
            ("answer_question", &side["answer_question"]),
            ("conversation_changes", &side["conversation_changes"]),
        ],
    );
    part(
        "chat.set_reminder",
        &[
            ("timers", &side["timers"]),
            ("reminder_cancels", &side["reminder_cancels"]),
        ],
    );
    part(
        "chat.schedule",
        &[
            ("schedules", &side["schedules"]),
            ("schedule_changes", &side["schedule_changes"]),
            ("show_schedules", &side["show_schedules"]),
        ],
    );
    part(
        "chat.remember",
        &[("remember", &side["remember"]), ("forget", &side["forget"])],
    );
    let text = answer["text"].as_str().unwrap_or_default().to_string();
    match answer["kind"].as_str() {
        Some("task") => {
            let task = &answer["task"];
            let one_go =
                task["plan"].is_null() && task["parts"].as_array().map_or(0, Vec::len) <= 1;
            let args = match quote {
                Some(q) if one_go => simple_args(answer, q, aq),
                // ORCH-743: the hand-off carries its ack and goal; the planner writes the task (its recorded call)
                _ => json!({"ack": text, "goal": task["goal"], "complexity": "complex",
                            "answers_question": aq}),
            };
            calls.push(("chat.dispatch_task".into(), args));
            (calls, String::new())
        }
        Some("clarify") => {
            calls.push(("chat.ask".into(), json!({"question": text})));
            (calls, String::new())
        }
        _ => (calls, text),
    }
}

/// A front door run's script: each call, then the text streamed and the run's end.
fn script(calls: &[(String, Value)], text: &str) -> Value {
    let mut events: Vec<Value> = calls
        .iter()
        .map(|(name, args)| json!({"kind": "front_call", "name": name, "arguments": args}))
        .collect();
    if !text.is_empty() {
        events.push(json!({"kind": "text_delta", "data": {"text": text}}));
    }
    events.push(json!({"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5}}));
    events.push(json!({"kind": "final", "data": {"text": text}}));
    Value::Array(events)
}

fn recorded_answer(call: &Value) -> Value {
    let text = call["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "final")
        .map(|e| e["data"]["text"].as_str().unwrap().to_string())
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The recorded scenario through the old planner (`front` false) or through the front door, each planner call's
/// recorded answer converted to its calls (or replaced by `scripts`, one per planner call, when given).
fn run(name: &str, front: bool, scripts: Option<Vec<Value>>) -> (World, Report) {
    let handing: std::sync::Arc<std::sync::Mutex<Vec<bool>>> = Default::default();
    let flags = handing.clone();
    let mut w = world_with(name, move |calls| {
        if !front {
            return;
        }
        let mut given = scripts.clone().map(|s| s.into_iter());
        let mut out = Vec::new();
        for c in calls.drain(..) {
            if c["key"] != "supervisor" {
                out.push(c);
                continue;
            }
            let answer = recorded_answer(&c);
            let mut front = c.clone();
            front["events"] = match given.as_mut() {
                Some(g) => g.next().expect("a script for each planner call"),
                None => {
                    let (calls, text) = convert(&answer);
                    script(&calls, &text)
                }
            };
            // ORCH-743: a complex hand-off's task the planner writes (its recorded call follows); a simple one's
            // code writes (ORCH-749: no planner call)
            let hands_off = front["events"].as_array().unwrap().iter().any(|e| {
                e["name"] == "chat.dispatch_task" && e["arguments"]["complexity"] == "complex"
            });
            out.push(front);
            flags.lock().unwrap().push(hands_off);
            // ORCH-743: an accepted hand-off's task is written by the planner: the recorded call answers it
            if hands_off {
                out.push(c);
            }
        }
        *calls = out;
    });
    w.daemon.bounded.store(true, Ordering::SeqCst);
    w.daemon.front.store(front, Ordering::SeqCst);
    if front {
        // ORCH-743: a hand-off's turn makes two planner calls (the front door's, then the planner writing its task):
        // each reads the turn's recorded live facts
        let handing = handing.lock().unwrap().clone();
        let mut q = w.daemon.supervise.lock().unwrap();
        // the recorded queue has one entry per recorded planner call; a hand-off's gets a copy for its second call
        let mut at = 0;
        for hands_off in handing {
            if at >= q.len() {
                break;
            }
            if hands_off {
                let rec = q[at].clone();
                q.insert(at + 1, rec);
                at += 1;
            }
            at += 1;
        }
    }
    let report = run_with(&mut w, name, |_| {});
    (w, report)
}

fn rows(w: &World, sql: &str) -> Vec<Vec<Value>> {
    let db = w.db.lock().unwrap();
    let mut st = db.conn().prepare(sql).unwrap();
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| match r.get::<_, rusqlite::types::Value>(i).unwrap() {
                rusqlite::types::Value::Null => Value::Null,
                rusqlite::types::Value::Integer(v) => json!(v),
                rusqlite::types::Value::Real(v) => json!(v),
                rusqlite::types::Value::Text(t) => json!(t),
                rusqlite::types::Value::Blob(_) => json!("(blob)"),
            })
            .collect())
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn journal(w: &World, kind: &str) -> Vec<Value> {
    rows(
        w,
        &format!("SELECT data_json FROM journal WHERE kind = '{kind}' ORDER BY seq"),
    )
    .into_iter()
    .map(|r| serde_json::from_str(r[0].as_str().unwrap()).unwrap())
    .collect()
}

/// What a turn did that lasts: the task rows (ids aside), the timers, FERB's replies and questions, the questions joined.
fn effects(w: &World) -> Value {
    let said = |kind: &str| -> Vec<Value> {
        journal(w, kind)
            .iter()
            .map(|d| json!([d["request_id"], d["text"]]))
            .collect()
    };
    json!({
        // ids aside (a run that ended on its call reported no usage span, so the ids that follow are one lower)
        "tasks": rows(w, "SELECT replace(id, tree_id, 'TREE'), request_id, goal, identity, role, scopes_json, \
                          status FROM tasks ORDER BY rowid"),
        "timers": rows(w, "SELECT kind, fire_at, message, request_id, state FROM timers ORDER BY seq"),
        "replies": said("chat.reply"),
        "questions": said("chat.clarify"),
        "answered": journal(w, "question.answered"),
        "pending": rows(w, "SELECT id, COALESCE(asked, '') FROM conversations ORDER BY id"),
    })
}

/// Every frame FERB sent on the chat streams, as the client reads them.
fn frames(report: &Report) -> Vec<Value> {
    report
        .streams
        .iter()
        .flat_map(|(_, lines)| lines.iter())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| v["params"].clone())
        .collect()
}

fn of_type<'a>(frames: &'a [Value], kind: &str) -> Vec<&'a Value> {
    frames.iter().filter(|f| f["type"] == kind).collect()
}

fn results(w: &World) -> Vec<(String, Result<String, String>)> {
    w.daemon.front_results.lock().unwrap().clone()
}

// decision: ORCH-735
#[test]
fn a_plain_reply_streams_through_the_front_door_and_is_the_turns_answer() {
    let (old, _) = run("plain_reply", false, None);
    let (w, report) = run("plain_reply", true, None);
    let made = w.daemon.calls.lock().unwrap().made.clone();
    // one plain-mode run: no answer form, the lookups and the front tools offered
    assert_eq!(made.len(), 1, "{made:?}");
    assert_eq!(made[0]["schema"], "(unknown schema)", "no answer form");
    let offered: Vec<String> = serde_json::from_value(made[0]["tools"].clone()).unwrap();
    for t in FRONT_TOOLS {
        assert!(offered.iter().any(|o| o == t), "{t} offered: {offered:?}");
    }
    let started = w.daemon.calls.lock().unwrap().started.clone();
    let system = started[0]["system"].as_str().unwrap();
    assert!(
        system.contains(CONCIERGE),
        "concierge.md in place of supervisor.md"
    );
    assert!(
        !system.contains("Owner-written skills"),
        "no catalog, as the planner"
    );
    // its grant may act through the front tools, beside the lookups' scopes
    let grants = w.daemon.grants.lock().unwrap();
    let grant = grants
        .values()
        .find(|g| g.task_id.ends_with(".lookups"))
        .expect("the front door's grant");
    assert!(!grant.observe_only);
    assert!(grant.scopes.contains("chat:act") && grant.scopes.contains("task:read"));
    drop(grants);
    // the reply streamed, then the turn's reply as the old path's
    let frames = frames(&report);
    assert!(!of_type(&frames, "reply_delta").is_empty(), "{frames:#?}");
    assert_eq!(
        effects(&w)["replies"],
        effects(&old)["replies"],
        "the same reply as the planner's"
    );
    assert_eq!(
        of_type(&frames, "reply").last().unwrap()["text"],
        "Hello! What can I do for you today?"
    );
}

// decision: ORCH-735 (the switch is off by default: the planner's answer form and its tools, unchanged)
#[test]
fn with_the_switch_off_the_planner_answers_through_its_form() {
    let (w, _) = run("plain_reply", false, None);
    let made = w.daemon.calls.lock().unwrap().made.clone();
    assert_ne!(made[0]["schema"], "(unknown schema)");
    assert!(!made[0]["tools"].to_string().contains("chat.dispatch_task"));
    let started = w.daemon.calls.lock().unwrap().started.clone();
    assert!(!started[0]["system"].as_str().unwrap().contains(CONCIERGE));
    let grants = w.daemon.grants.lock().unwrap();
    assert!(grants
        .values()
        .filter(|g| g.task_id.ends_with(".lookups"))
        .all(|g| g.observe_only && !g.scopes.contains("chat:act")));
}

// decision: ORCH-735 (a lookup's start is the turn's live line, as the plan walk's)
#[test]
fn a_lookup_shows_the_turns_live_line_while_it_runs() {
    let events = json!([
        {"kind": "tool_call", "data": {"tool": "memory_read", "name": "memory.read", "phase": "started",
                                         "arguments": {"query": "bread"}}},
        {"kind": "tool_call", "data": {"tool": "memory_read", "name": "memory.read", "phase": "completed",
                                         "arguments": {"query": "bread"}, "error": false}},
        {"kind": "text_delta", "data": {"text": "You bake on Sundays."}},
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5}},
        {"kind": "final", "data": {"text": "You bake on Sundays."}}
    ]);
    let (_, report) = run("plain_reply", true, Some(vec![events]));
    let frames = frames(&report);
    let progress = of_type(&frames, "progress");
    assert_eq!(progress.len(), 1, "{frames:#?}");
    assert_eq!(progress[0]["planning"], true);
    assert!(!progress[0]["text"].as_str().unwrap().is_empty());
    // the line came before the reply's words
    let at = |kind: &str| frames.iter().position(|f| f["type"] == kind).unwrap();
    assert!(at("progress") < at("reply_delta"));
}

// decision: ORCH-735, ORCH-736, ORCH-743
#[test]
fn a_hand_off_starts_the_task_as_the_planners_answer_did() {
    let (old, _) = run("one_step_task", false, None);
    let (w, report) = run("one_step_task", true, None);
    assert_eq!(
        results(&w),
        vec![(
            "chat.dispatch_task".to_string(),
            Ok(ferb_supervisor::flow::turn::front_door::ACCEPTED_TASK.to_string())
        )]
    );
    let (a, b) = (effects(&old), effects(&w));
    assert_eq!(a["tasks"], b["tasks"], "the same task rows");
    assert!(!b["tasks"].as_array().unwrap().is_empty());
    assert_eq!(a["replies"], b["replies"]);
    // the ack is shown on the plan channel once code accepted the call
    let frames = frames(&report);
    let ack: String = of_type(&frames, "plan_delta")
        .iter()
        .map(|f| f["text"].as_str().unwrap())
        .collect();
    assert_eq!(ack, "On it.", "{frames:#?}");
    assert_eq!(of_type(&frames, "plan_end").len(), 1);
}

// decision: ORCH-736 (a refused call goes back to the model, the owner sees nothing; repaired in the same run)
#[test]
fn a_refused_hand_off_is_repaired_in_the_run_and_the_owner_never_sees_the_refusal() {
    let recorded = support_control::scenario("one_step_task").1["model_calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "supervisor")
        .cloned()
        .unwrap();
    let (calls, _) = convert(&recorded_answer(&recorded));
    let good = calls[0].1.clone();
    let mut bad = good.clone();
    bad["goal"] = json!(" ");
    let events = script(
        &[
            ("chat.dispatch_task".into(), bad),
            ("chat.dispatch_task".into(), good),
        ],
        "",
    );
    let (old, _) = run("one_step_task", false, None);
    let (w, report) = run("one_step_task", true, Some(vec![events]));
    let got = results(&w);
    assert_eq!(got.len(), 2, "{got:?}");
    let refusal = got[0].1.clone().unwrap_err();
    assert!(
        refusal.contains("goal must be a non-empty string"),
        "{refusal}"
    );
    assert!(got[1].1.is_ok(), "{got:?}");
    assert_eq!(
        effects(&old)["tasks"],
        effects(&w)["tasks"],
        "the task started"
    );
    // the refusal is the call's own record (its tool.call span), never shown
    let frames = frames(&report);
    let shown = serde_json::to_string(&frames).unwrap();
    assert!(
        !shown.contains("boat") && !shown.contains("quote"),
        "{shown}"
    );
    assert!(of_type(&frames, "plan_abort").is_empty());
}

// decision: ORCH-736 (one task per message)
#[test]
fn a_second_hand_off_in_one_message_is_refused() {
    let recorded = support_control::scenario("one_step_task").1["model_calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "supervisor")
        .cloned()
        .unwrap();
    let (calls, _) = convert(&recorded_answer(&recorded));
    let task = calls[0].1.clone();
    let events = json!([
        {"kind": "front_call", "name": "chat.dispatch_task", "arguments": task,
         "also": [{"name": "chat.dispatch_task", "arguments": task}]},
        {"kind": "final", "data": {"text": ""}}
    ]);
    let (old, _) = run("one_step_task", false, None);
    let (w, _) = run("one_step_task", true, Some(vec![events]));
    let got = results(&w);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[0].1.is_ok());
    assert!(
        got[1]
            .1
            .as_ref()
            .unwrap_err()
            .contains("One task per message"),
        "{got:?}"
    );
    assert_eq!(
        effects(&old)["tasks"],
        effects(&w)["tasks"],
        "one task, as asked"
    );
}

// decision: ORCH-737 (the question is a question entry; the next message joins it)
#[test]
fn a_question_is_kept_and_the_message_that_answers_it_joins_it() {
    let (old, _) = run("clarify_then_answer", false, None);
    let (w, report) = run("clarify_then_answer", true, None);
    let got = results(&w);
    let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["chat.ask", "chat.dispatch_task"], "{got:?}");
    assert!(got.iter().all(|(_, r)| r.is_ok()), "{got:?}");
    let (a, b) = (effects(&old), effects(&w));
    assert_eq!(a["questions"], b["questions"], "asked as a question");
    assert_eq!(
        a["answered"], b["answered"],
        "the answer joined the question"
    );
    assert_eq!(b["answered"].as_array().unwrap().len(), 1);
    assert_eq!(a["tasks"], b["tasks"], "the joined request's task");
    assert_eq!(a["pending"], b["pending"], "the question is cleared");
    let frames = frames(&report);
    assert_eq!(
        of_type(&frames, "clarify")[0]["text"],
        "Which restaurant, and for what time?"
    );
}

// decision: ORCH-736 (several effects in one message: several calls, each taking effect)
#[test]
fn a_reminder_and_a_memory_in_one_message_both_take_effect() {
    let said =
        "set a timer for 2 minutes to take the bread out, and remember that I bake on Sundays";
    let timer = json!({"when": {"in_minutes": 2, "in_days": null, "day": null, "date": null, "time": null},
                       "message": "Take the bread out", "quote": "a timer for 2 minutes", "when_active": false,
                       "name": "Bread", "kind": "timer"});
    let item = json!({"kind": "fact", "subject": "owner", "key": "baking day", "content": "I bake on Sundays",
                      "quote": "I bake on Sundays", "login_origin": null, "until": null, "replaces": [],
                      "review_in_days": 0});
    let events = script(
        &[
            (
                "chat.set_reminder".into(),
                json!({"timers": [timer], "answers_question": false}),
            ),
            (
                "chat.remember".into(),
                json!({"remember": [item], "answers_question": false}),
            ),
        ],
        "",
    );
    let mut w = world_with("plain_reply", |calls| {
        let c = calls.iter_mut().find(|c| c["key"] == "supervisor").unwrap();
        c["events"] = events;
    });
    w.daemon.bounded.store(true, Ordering::SeqCst);
    w.daemon.front.store(true, Ordering::SeqCst);
    *w.daemon.memory_port.lock().unwrap() = Some(Vec::new());
    let report = run_with(&mut w, "plain_reply", |steps| {
        for st in steps.iter_mut() {
            if st["do"] == "stream" {
                let mut sent: Value =
                    serde_json::from_str(st["lines"][0]["send"].as_str().unwrap()).unwrap();
                sent["params"]["text"] = json!(said);
                st["lines"][0]["send"] = json!(sent.to_string());
            }
        }
    });
    let got = results(&w);
    assert!(got.iter().all(|(_, r)| r.is_ok()), "{got:?}");
    let timers = rows(&w, "SELECT kind, message FROM timers");
    assert_eq!(
        timers,
        vec![vec![json!("timer"), json!("Take the bread out")]]
    );
    let saved = w.daemon.memory_port.lock().unwrap().clone().unwrap();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved[0]["content"], "I bake on Sundays");
    // no text from the model: code's own lines say what was done
    let frames = frames(&report);
    let reply = of_type(&frames, "reply");
    let text = reply.last().unwrap()["text"].as_str().unwrap();
    assert!(text.contains("Saved: I bake on Sundays"), "{text}");
}

/// The plain_reply scenario's message, its words replaced by `said`, through the front door with `events` as its run.
fn said_through(said: &str, events: Value) -> (World, Report) {
    let mut w = world_with("plain_reply", |calls| {
        let c = calls.iter_mut().find(|c| c["key"] == "supervisor").unwrap();
        c["events"] = events;
    });
    w.daemon.bounded.store(true, Ordering::SeqCst);
    w.daemon.front.store(true, Ordering::SeqCst);
    let report = run_with(&mut w, "plain_reply", |steps| {
        for st in steps.iter_mut() {
            if st["do"] == "stream" {
                let mut sent: Value =
                    serde_json::from_str(st["lines"][0]["send"].as_str().unwrap()).unwrap();
                sent["params"]["text"] = json!(said);
                st["lines"][0]["send"] = json!(sent.to_string());
            }
        }
    });
    (w, report)
}

/// The Planner line of the episode recorded for each task (`ferb trace`'s and the episode's story).
fn planner_lines(w: &World) -> Vec<String> {
    rows(w, "SELECT steps_json FROM episodes ORDER BY task_id")
        .into_iter()
        .flat_map(|r| {
            let steps: Vec<Value> = serde_json::from_str(r[0].as_str().unwrap()).unwrap();
            steps
                .into_iter()
                .filter(|s| s["role"] == "Planner")
                .map(|s| s["text"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

// decision: ORCH-735 (the trace's and the episode's Planner line reads what the front door decided)
#[test]
fn a_hand_off_reads_in_the_trace_as_the_task_it_started() {
    let (old, _) = run("one_step_task", false, None);
    let (w, _) = run("one_step_task", true, None);
    let (a, b) = (planner_lines(&old), planner_lines(&w));
    assert!(!b.is_empty(), "an episode with its Planner line");
    assert!(b.iter().all(|l| l.starts_with("task — ")), "{b:?}");
    // ORCH-743: the front door's call (its ack), then the planner's call that wrote the task, as the planner's line
    assert_eq!(b.len(), a.len() + 1, "{b:?}");
    assert_eq!(b[0], "task — On it.", "{b:?}");
    assert_eq!(b[1..], a[..], "the planner's line for the task it wrote");
    // the span keeps the run's own answer and its calls beside what it decided
    let out = rows(
        &w,
        "SELECT output_json FROM spans WHERE kind = 'model.turn' AND input_json LIKE '%\"front_door\": true%'",
    );
    assert_eq!(out.len(), 1, "{out:?}");
    let out: Value = serde_json::from_str(out[0][0].as_str().unwrap()).unwrap();
    let keys: Vec<&str> = out
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["text", "calls", "decided", "usage", "timing"]);
    let decided: Value = serde_json::from_str(out["decided"].as_str().unwrap()).unwrap();
    assert_eq!(decided["kind"], "task");
}

// decision: ORCH-736 (an accepted call's reply is FERB's own words: never a lookup the turn read)
#[test]
fn an_accepted_calls_reply_is_never_stored_as_a_lookup() {
    let recorded = support_control::scenario("one_step_task").1["model_calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "supervisor")
        .cloned()
        .unwrap();
    let (calls, _) = convert(&recorded_answer(&recorded));
    let events = json!([
        {"kind": "record_call", "name": "memory.read", "arguments": {"query": "bread"},
         "output": {"text": "The owner buys rye bread."}, "origin": "private"},
        {"kind": "front_call", "name": "chat.dispatch_task", "arguments": calls[0].1, "recorded": true},
        {"kind": "final", "data": {"text": ""}}
    ]);
    let (w, _) = run("one_step_task", true, Some(vec![events]));
    assert!(results(&w)[0].1.is_ok(), "{:?}", results(&w));
    // both calls are recorded under the run's grant, as the gateway records them
    let made = rows(
        &w,
        "SELECT json_extract(input_json, '$.tool') FROM spans WHERE kind = 'tool.call' AND task_id LIKE '%.lookups'",
    );
    assert_eq!(made.len(), 2, "{made:?}");
    let lookups: Vec<String> = rows(
        &w,
        "SELECT output_json FROM spans WHERE kind = 'context.exposure' \
         AND input_json LIKE '%a lookup by this request''s planner%'",
    )
    .into_iter()
    .map(|r| r[0].as_str().unwrap().to_string())
    .collect();
    assert_eq!(lookups.len(), 1, "only the lookup is a read: {lookups:?}");
    assert!(lookups[0].contains("rye bread"), "{lookups:?}");
    assert!(
        lookups.iter().all(|l| !l.contains("Accepted")),
        "{lookups:?}"
    );
}

// decision: ORCH-739 (a side call's entries are checked one at a time: the good one is kept, the bad one refused to
// the model by name, and a resent whole call never doubles the one already kept)
#[test]
fn a_reminder_call_keeps_its_good_entry_and_refuses_only_the_bad_one() {
    let said = "set a timer for 2 minutes to take the bread out, and a timer for 5 minutes to check the oven";
    let timer = |minutes: i64, message: &str, quote: &str| {
        json!({"when": {"in_minutes": minutes, "in_days": null, "day": null, "date": null, "time": null},
               "message": message, "quote": quote, "when_active": false, "name": message, "kind": "timer"})
    };
    let good = timer(2, "Take the bread out", "a timer for 2 minutes");
    let bad = timer(5, "Check the oven", "a timer for an hour");
    let fixed = timer(5, "Check the oven", "a timer for 5 minutes");
    let call = |timers: Vec<&Value>| {
        (
            "chat.set_reminder".to_string(),
            json!({"timers": timers, "answers_question": false}),
        )
    };
    let events = script(
        &[
            call(vec![&bad]),
            call(vec![&good, &bad]),
            call(vec![&good, &bad]),
            call(vec![&good, &fixed]),
        ],
        "",
    );
    let (w, report) = said_through(said, events);
    let got = results(&w);
    assert_eq!(got.len(), 4, "{got:?}");
    // a call whose only entry is refused: an error as before, nothing kept
    let only = got[0].1.clone().unwrap_err();
    assert!(only.contains("'Check the oven'"), "{only}");
    assert!(!only.contains("Accepted"), "{only}");
    // one good and one bad: still an error, so the model sees it, naming what was taken and what was refused
    for (_, said) in &got[1..3] {
        let told = said.clone().unwrap_err();
        assert!(
            told.starts_with("Accepted, and already taken (don't send these again): timers[0]."),
            "{told}"
        );
        assert!(told.contains("Refused"), "{told}");
        assert!(told.contains("timers[1]:"), "{told}");
        assert!(told.contains("'Check the oven'"), "{told}");
        assert!(!told.contains("'Take the bread out'"), "{told}");
    }
    assert!(got[3].1.is_ok(), "{got:?}");
    let repairs = journal(&w, "task.repair");
    assert_eq!(repairs.len(), 3, "{repairs:?}");
    assert_eq!(repairs[1]["accepted"], json!(["timers[0]"]));
    // the good timer once, though it was sent three times, and the corrected one
    let timers = rows(&w, "SELECT message FROM timers ORDER BY seq");
    assert_eq!(
        timers,
        vec![
            vec![json!("Take the bread out")],
            vec![json!("Check the oven")]
        ]
    );
    let frames = frames(&report);
    let shown = serde_json::to_string(&frames).unwrap();
    assert!(
        !shown.contains("an hour"),
        "the refusals never reach the owner"
    );
}

// decision: ORCH-739 (memory: a resent whole call keeps one copy of the entry already taken; the apply path has no
// twin check of its own here, as the timers' "already set in this turn" is)
#[test]
fn a_memory_call_resent_whole_saves_its_good_entry_once() {
    let said = "remember that I bake on Sundays and that the oven runs hot";
    let item = |key: &str, content: &str, quote: &str| {
        json!({"kind": "fact", "subject": "owner", "key": key, "content": content, "quote": quote,
               "login_origin": null, "until": null, "replaces": [], "review_in_days": 0})
    };
    let good = item("baking day", "I bake on Sundays", "I bake on Sundays");
    let bad = item("oven", "The oven runs cold", "the oven runs cold");
    let call = || {
        (
            "chat.remember".to_string(),
            json!({"remember": [&good, &bad], "answers_question": false}),
        )
    };
    let events = script(&[call(), call()], "");
    let mut w = world_with("plain_reply", |calls| {
        let c = calls.iter_mut().find(|c| c["key"] == "supervisor").unwrap();
        c["events"] = events;
    });
    w.daemon.bounded.store(true, Ordering::SeqCst);
    w.daemon.front.store(true, Ordering::SeqCst);
    *w.daemon.memory_port.lock().unwrap() = Some(Vec::new());
    run_with(&mut w, "plain_reply", |steps| {
        for st in steps.iter_mut() {
            if st["do"] == "stream" {
                let mut sent: Value =
                    serde_json::from_str(st["lines"][0]["send"].as_str().unwrap()).unwrap();
                sent["params"]["text"] = json!(said);
                st["lines"][0]["send"] = json!(sent.to_string());
            }
        }
    });
    let got = results(&w);
    assert_eq!(got.len(), 2, "{got:?}");
    for (_, r) in &got {
        let told = r.clone().unwrap_err();
        assert!(
            told.contains("remember[0]") && told.contains("remember[1]:"),
            "{told}"
        );
        assert!(told.contains("'oven'"), "{told}");
    }
    let saved = w.daemon.memory_port.lock().unwrap().clone().unwrap();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved[0]["content"], "I bake on Sundays");
}

// decision: ORCH-740 (a front-door run that fails in its runtime falls back to the planner for this turn)
#[test]
fn a_front_door_run_that_fails_in_its_runtime_falls_back_to_the_planner() {
    let (old, _) = run("one_step_task", false, None);
    let mut w = world_with("one_step_task", |calls| {
        // the front door's run fails as a runtime refusing its tools would; the planner's recorded call follows
        let at = calls.iter().position(|c| c["key"] == "supervisor").unwrap();
        let mut failing = calls[at].clone();
        failing["events"] = json!([{"kind": "error", "data": {
            "message": "INVENTED invalid tool schema: chat.dispatch_task"}}]);
        calls.insert(at, failing);
    });
    w.daemon.bounded.store(true, Ordering::SeqCst);
    w.daemon.front.store(true, Ordering::SeqCst);
    let _ = run_with(&mut w, "one_step_task", |_| {});
    let fell = journal(&w, "front_door.fallback");
    assert_eq!(fell.len(), 1, "one fallback journaled: {fell:?}");
    assert!(fell[0]["error"]
        .as_str()
        .unwrap()
        .contains("invalid tool schema"));
    assert_eq!(
        effects(&w),
        effects(&old),
        "the planner's turn ran as without the front door"
    );
}

// decision: ORCH-742 (live 2026-11-27: "stop all tasks" with two running; the second stop was refused as a second
// control)
#[test]
fn several_stops_in_one_message_are_each_checked_and_other_controls_stay_one() {
    let recorded = support_control::scenario("stop_by_chat").1["model_calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["key"] == "supervisor")
        .find(|c| recorded_answer(c)["control"].is_object())
        .cloned()
        .unwrap();
    let (calls, text) = convert(&recorded_answer(&recorded));
    let stop = calls
        .iter()
        .find(|(n, _)| n == "chat.control_task")
        .unwrap()
        .1
        .clone();
    let mut stop = stop;
    stop["control"]["task_id"] = json!("@TREE@");
    let mut other = stop.clone();
    other["control"]["task_id"] = json!("INVENTEDOTHERTASK");
    let mut pause = stop.clone();
    pause["control"]["action"] = json!("pause");
    let events = script(
        &[
            ("chat.control_task".into(), stop),
            ("chat.control_task".into(), other),
            ("chat.control_task".into(), pause),
        ],
        &text,
    );
    let mut scripts = Vec::new();
    let n = support_control::scenario("stop_by_chat").1["model_calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["key"] == "supervisor")
        .count();
    for c in support_control::scenario("stop_by_chat").1["model_calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["key"] == "supervisor")
    {
        if recorded_answer(c)["control"].is_object() {
            scripts.push(events.clone());
        } else {
            let (calls, text) = convert(&recorded_answer(c));
            scripts.push(script(&calls, &text));
        }
    }
    assert_eq!(scripts.len(), n);
    let (w, _) = run("stop_by_chat", true, Some(scripts));
    let got: Vec<_> = results(&w)
        .into_iter()
        .filter(|(n, _)| n == "chat.control_task")
        .collect();
    assert_eq!(got.len(), 3, "{got:?}");
    assert!(got[0].1.is_ok(), "{got:?}");
    let second = got[1].1.clone().unwrap_err();
    assert!(
        second.contains("Unknown task_id") && !second.contains("One control"),
        "a second stop is checked as a stop: {second}"
    );
    let third = got[2].1.clone().unwrap_err();
    assert!(third.contains("One control per message"), "{third}");
}

// decision: ORCH-747 (the accepted ack ends marked accepted with its shown text; a live planning line runs until the plan)
#[test]
fn a_hand_off_shows_its_accepted_ack_then_a_planning_line_until_the_plan() {
    let (_, report) = run("one_step_task", true, None);
    let frames = frames(&report);
    let kinds: Vec<&str> = frames
        .iter()
        .map(|f| f["type"].as_str().unwrap_or(""))
        .collect();
    let end = frames
        .iter()
        .position(|f| f["type"] == "plan_end")
        .expect("the ack ends");
    assert_eq!(frames[end]["accepted"], json!(true), "{frames:#?}");
    assert_eq!(frames[end]["text"], json!("On it."));
    let plan = frames
        .iter()
        .position(|f| f["type"] == "plan")
        .expect("the plan");
    let line = frames
        .iter()
        .position(|f| f["type"] == "progress" && f["planning"] == json!(true))
        .expect("a planning line");
    assert!(end < line && line < plan, "{kinds:?}");
    assert_eq!(
        frames[line]["text"],
        json!(ferb_supervisor::words::PLANNING_LINES[0])
    );
}

/// Each recorded planner call of the scenario as the front door's run, a task one worker does in one go handed off
/// simple with the owner's message as its quote (ORCH-749).
fn simple_scripts(name: &str) -> Vec<Value> {
    support_control::scenario(name).1["model_calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["key"] == "supervisor")
        .map(|c| {
            let (calls, text) = convert_with(&recorded_answer(c), Some(&message_of(c)));
            script(&calls, &text)
        })
        .collect()
}

/// The planner's own calls made (its answer form; the front door's run has none).
fn planner_calls(w: &World) -> usize {
    w.daemon
        .calls
        .lock()
        .unwrap()
        .made
        .iter()
        .filter(|m| m["schema"] == "SUPERVISOR_SCHEMA")
        .count()
}

/// The spec each task was started with (task_context).
fn specs(w: &World) -> Vec<Value> {
    rows(w, "SELECT spec_json FROM task_context ORDER BY rowid")
        .into_iter()
        .map(|r| serde_json::from_str(r[0].as_str().unwrap()).unwrap())
        .collect()
}

fn planning_lines(frames: &[Value]) -> Vec<&Value> {
    frames
        .iter()
        .filter(|f| {
            f["type"] == "progress"
                && ferb_supervisor::words::PLANNING_LINES
                    .iter()
                    .any(|l| f["text"] == json!(l))
        })
        .collect()
}

/// The one_step_task scenario's recorded hand-off as a simple one, its arguments changed by `edit`.
fn simple_call(edit: impl FnOnce(&mut Value)) -> (String, Value) {
    let recorded = support_control::scenario("one_step_task").1["model_calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "supervisor")
        .cloned()
        .unwrap();
    let mut args = simple_args(&recorded_answer(&recorded), &message_of(&recorded), false);
    edit(&mut args);
    ("chat.dispatch_task".into(), args)
}

// decision: ORCH-749 (a simple hand-off: code writes its task from the call's fields, no planner call, no planning
// line; the task starts at once)
#[test]
fn a_simple_hand_off_starts_its_task_with_no_planner_call() {
    let (old, _) = run("one_step_task", false, None);
    let (w, report) = run("one_step_task", true, Some(simple_scripts("one_step_task")));
    assert_eq!(
        results(&w),
        vec![(
            "chat.dispatch_task".to_string(),
            Ok(ferb_supervisor::flow::turn::front_door::ACCEPTED_TASK.to_string())
        )]
    );
    assert_eq!(planner_calls(&old), 1, "the old path's planner call");
    assert_eq!(planner_calls(&w), 0, "no planner call");
    let made = w.daemon.calls.lock().unwrap().made.clone();
    let orchestrator: Vec<&Value> = made
        .iter()
        .filter(|m| m["runtime_role"] == "orchestrator")
        .collect();
    assert_eq!(orchestrator.len(), 1, "the front door's run only: {made:?}");
    let (a, b) = (effects(&old), effects(&w));
    assert_eq!(a["tasks"], b["tasks"], "the same task rows");
    assert!(!b["tasks"].as_array().unwrap().is_empty());
    // the task as code wrote it from the call: the goal, name and quote given, one outcome condition, no plan
    let spec = &specs(&w)[0];
    assert_eq!(spec["goal"], "Say hello in the notes");
    assert_eq!(spec["name"], "Hello note");
    assert_eq!(spec["identity"], "owner");
    assert_eq!(spec["worker_type"], "default");
    assert_eq!(spec["plan"], Value::Null);
    assert_eq!(spec["parts"][0]["quote"], "say hello");
    assert_eq!(spec["parts"][0]["label"], "Hello note");
    assert_eq!(spec["parts"][0]["answer_step"], "worker");
    assert_eq!(spec["parts"][0]["conditions"], json!([0]));
    assert_eq!(
        spec["success_conditions"],
        json!([{"text": "Say hello in the notes", "owner_text": "Say hello in the notes",
                "check": {"kind": "none", "path": null, "text": null, "url_contains": null, "site": null,
                          "expected": null, "tool": null}, "layer": "outcome"}])
    );
    // the ack, then the task's own frames: no planning line
    let frames = frames(&report);
    let ack: String = of_type(&frames, "plan_delta")
        .iter()
        .map(|f| f["text"].as_str().unwrap())
        .collect();
    assert_eq!(ack, "On it.", "{frames:#?}");
    assert!(planning_lines(&frames).is_empty(), "{frames:#?}");
    assert!(!of_type(&frames, "plan").is_empty(), "{frames:#?}");
}

// decision: ORCH-743 (a complex hand-off is planned as before: the planner's call writes its task under a planning
// line), ORCH-749 (a complex one's simple fields are ignored)
#[test]
fn a_complex_hand_off_is_planned_as_before_and_its_simple_fields_are_ignored() {
    let (_, complex) = simple_call(|a| a["complexity"] = json!("complex"));
    let (planned, _) = run("one_step_task", true, None);
    let (w, report) = run(
        "one_step_task",
        true,
        Some(vec![script(&[("chat.dispatch_task".into(), complex)], "")]),
    );
    assert!(results(&w)[0].1.is_ok(), "{:?}", results(&w));
    assert_eq!(planner_calls(&w), 1, "the planner writes the task");
    assert_eq!(
        specs(&w),
        specs(&planned),
        "the planner's task, not one code wrote"
    );
    assert_eq!(specs(&w)[0]["parts"], json!([]));
    assert_eq!(planning_lines(&frames(&report)).len(), 1);
}

// decision: ORCH-749, MEM-430 (a simple hand-off's quote is checked against his messages as the planner's parts are;
// refused in the run, repaired, the owner seeing nothing)
#[test]
fn a_simple_hand_off_quoting_words_he_never_wrote_is_refused_and_repaired() {
    let bad = simple_call(|a| a["quote"] = json!("say goodbye to everyone"));
    let good = simple_call(|_| {});
    let (old, _) = run("one_step_task", false, None);
    let (w, report) = run("one_step_task", true, Some(vec![script(&[bad, good], "")]));
    let got = results(&w);
    assert_eq!(got.len(), 2, "{got:?}");
    let refusal = got[0].1.clone().unwrap_err();
    assert!(
        refusal.contains("part p1: its quote must be the owner's own words"),
        "{refusal}"
    );
    assert!(got[1].1.is_ok(), "{got:?}");
    assert_eq!(planner_calls(&w), 0);
    assert_eq!(
        effects(&old)["tasks"],
        effects(&w)["tasks"],
        "the task started"
    );
    assert_eq!(specs(&w).len(), 1);
    let shown = serde_json::to_string(&frames(&report)).unwrap();
    assert!(!shown.contains("goodbye"), "{shown}");
}

// decision: ORCH-749 (a simple hand-off missing a field code writes its task from is refused naming it)
#[test]
fn a_simple_hand_off_missing_done_when_is_refused_naming_it() {
    let missing = simple_call(|a| {
        a.as_object_mut().unwrap().remove("done_when");
    });
    // the gateway fills a string left out as "" (its schema's default): the same refusal
    let blank = simple_call(|a| a["done_when"] = json!(""));
    let good = simple_call(|_| {});
    let (old, _) = run("one_step_task", false, None);
    let (w, _) = run(
        "one_step_task",
        true,
        Some(vec![script(&[missing, blank, good], "")]),
    );
    let got = results(&w);
    assert_eq!(got.len(), 3, "{got:?}");
    for (_, r) in &got[..2] {
        let told = r.clone().unwrap_err();
        assert!(told.contains("a simple task needs done_when"), "{told}");
    }
    assert!(got[2].1.is_ok(), "{got:?}");
    assert_eq!(effects(&old)["tasks"], effects(&w)["tasks"]);
}

// decision: ORCH-749 (the hand-off's arguments, read alone: its kind, and a simple one's fields, each refused by name)
#[test]
fn a_hand_offs_kind_and_simple_fields_are_checked_by_name() {
    use ferb_supervisor::flow::turn::front_door::part_of;
    let base = json!({"ack": "On it.", "goal": "Say hello in the notes", "complexity": "simple",
                      "quote": "say hello", "name": "Hello note", "done_when": "The notes say hello",
                      "identity": "owner"});
    let with = |k: &str, v: Value| {
        let mut a = base.clone();
        a[k] = v;
        part_of("chat.dispatch_task", &a)
    };
    let part = part_of("chat.dispatch_task", &base).unwrap();
    assert_eq!(part.kind, "task");
    assert_eq!(part.text, "On it.");
    let task = part.task.expect("code wrote the task");
    assert_eq!(task.parts.len(), 1);
    assert_eq!(task.success_conditions.len(), 1);
    for field in ["quote", "name", "done_when", "identity"] {
        let err = with(field, json!(" ")).unwrap_err();
        assert!(
            err.contains(&format!("a simple task needs {field}")),
            "{err}"
        );
    }
    let err = with("complexity", json!("medium")).unwrap_err();
    assert!(err.contains("complexity must be"), "{err}");
    let mut none = base.clone();
    none.as_object_mut().unwrap().remove("complexity");
    assert!(part_of("chat.dispatch_task", &none)
        .unwrap_err()
        .contains("complexity must be"));
    let err = with("folders", json!("~/Documents")).unwrap_err();
    assert!(err.contains("folders must be a list of strings"), "{err}");
    let listed = with("earlier_results", json!(["c3"])).unwrap();
    assert_eq!(listed.task.unwrap().earlier_results, ["c3"]);
    // complex: no task (the planner writes it); a simple field left blank is no problem
    let complex = with("complexity", json!("complex")).unwrap();
    assert!(complex.task.is_none());
    let mut blank = base.clone();
    blank["complexity"] = json!("complex");
    blank["done_when"] = json!("");
    assert!(part_of("chat.dispatch_task", &blank)
        .unwrap()
        .task
        .is_none());
}
