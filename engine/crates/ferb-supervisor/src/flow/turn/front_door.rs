//! The front door (kb/plans/front-door.md, step 5): a chat turn decided by one plain-mode run (RUN-480) in place of
//! the planner's answer form and its repair round. The run streams plain text to the owner through `PlainReply`
//! (ORCH-734) and acts only through typed calls: `chat.dispatch_task`, `chat.control_task`, `chat.ask`,
//! `chat.set_reminder`, `chat.schedule` and `chat.remember` (ferb-orchestrator `FRONT_TOOLS`). Each call's arguments
//! are the matching part of today's answer form, field for field, and the gateway hands each one to
//! [`Inner::front_call`], which runs today's checks (`problems`, `side_quote_problems`) on it: a refusal goes back to
//! the model as the tool's error (the owner sees nothing) and the model repairs it inside the same run; an accepted
//! part is kept on the turn. When the run ends (on an accepted `chat.dispatch_task` or `chat.ask`, ORCH-733, or with
//! its last word), the accepted parts are joined in call order into one `SupervisorOutcome`, the whole passes
//! `problems` once more, and `decide` returns it: routing, side entries, the task's start and the reply are today's
//! code, in today's one apply path at the turn's end.
//!
//! decision: ORCH-735, ORCH-736, ORCH-737

use std::sync::{Arc, Mutex};

use ferb_core::types::{Identity, Task, TaskRole};
use ferb_orchestrator::models::SupervisorOutcome;
use ferb_orchestrator::reply_stream::PlainReply;
use ferb_orchestrator::validate::{dump_json_supervisor_outcome, supervisor_outcome_validate_json};
use ferb_worker::{Call, CallHooks};
use serde_json::{json, Map, Value};

use super::{text_frame, Cancel, Prepare};
use crate::flow::{Emit, Failure, Flow, Inner, Turn};
use crate::words::{FRONT_TOOLS, LOOKUPS, PLANNER_TOOLS};

/// The hand-off: one task per message (today's answer form holds one).
pub const DISPATCH: &str = "chat.dispatch_task";
pub const CONTROL: &str = "chat.control_task";
/// The question: kind `clarify`, so the next message joins it (ORCH-737).
pub const ASK: &str = "chat.ask";
pub const SET_REMINDER: &str = "chat.set_reminder";
pub const SCHEDULE: &str = "chat.schedule";
pub const REMEMBER: &str = "chat.remember";

/// The calls whose acceptance ends the run (ORCH-733 `Call::ends_on`).
pub const ENDS_ON: [&str; 2] = [DISPATCH, ASK];

/// What the model reads back when code accepted its call (never shown to the owner).
pub const ACCEPTED_TASK: &str = "Accepted: the task starts as this message ends.";
pub const ACCEPTED_QUESTION: &str = "Accepted: the owner is asked this question.";
pub const ACCEPTED: &str =
    "Accepted: it takes effect as this message ends, and code tells the owner what it did.";

/// A front door's run of one turn while it runs (on [`Turn::front`]).
pub struct FrontRun {
    /// The parts code accepted, in call order: (tool, the part as today's answer form holds it).
    pub parts: Vec<(String, SupervisorOutcome)>,
    /// Where the checks' live line goes (`problems`' walk of a plan's folders).
    pub emit: Emit,
    /// ORCH-743: the accepted hand-off's goal, which the planner then writes as its task.
    pub handoff: Option<String>,
    /// ORCH-742: the tasks a message stops beyond its first control (every control of it a stop), stopped as the
    /// app's Stop does when the run ends.
    pub stops: Vec<String>,
}

/// The answer form's fields each front tool takes as its arguments (`ack` is the form's `text` for a hand-off, and
/// `question` its `text` for a question).
pub fn arguments_of(tool: &str) -> Option<&'static [&'static str]> {
    Some(match tool {
        DISPATCH => &["ack", "goal", "answers_question"],
        CONTROL => &[
            "control",
            "answer_question",
            "conversation_changes",
            "answers_question",
        ],
        ASK => &["question"],
        SET_REMINDER => &["timers", "reminder_cancels", "answers_question"],
        SCHEDULE => &[
            "schedules",
            "schedule_changes",
            "show_schedules",
            "answers_question",
        ],
        REMEMBER => &["remember", "forget", "answers_question"],
        _ => return None,
    })
}

/// A call's arguments as the matching part of today's answer form, validated as the form is (the same model, the same
/// field names, types and errors): kind `task` with the ack as its text for a hand-off, `clarify` with the question as
/// its text for a question, else `reply` with no text. An argument the tool doesn't take, a missing ack or question,
/// or a call that carries nothing is refused. `answers_question` is false unless the call says true.
pub fn part_of(tool: &str, arguments: &Value) -> Result<SupervisorOutcome, String> {
    let Some(takes) = arguments_of(tool) else {
        return Err(format!(
            "{tool} isn't one of the front door's tools: {}.",
            FRONT_TOOLS.join(", ")
        ));
    };
    let given = match arguments {
        Value::Object(m) => m.clone(),
        Value::Null => Map::new(),
        _ => return Err(format!("{tool}: its arguments must be an object.")),
    };
    let unknown: Vec<&str> = given
        .keys()
        .map(String::as_str)
        .filter(|k| !takes.contains(k))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "{tool} takes only {}; not: {}.",
            takes.join(", "),
            unknown.join(", ")
        ));
    }
    let said = |key: &str| -> Result<String, String> {
        match given.get(key) {
            Some(Value::String(s)) if !ferb_orchestrator::py::strip(s).is_empty() => Ok(s.clone()),
            _ => Err(format!("{tool}: {key} must be a non-empty string.")),
        }
    };
    let mut form = Map::new();
    let (kind, text) = match tool {
        // ORCH-743: the form's own check wants a task for kind "task"; the hand-off has none yet (the planner writes
        // it), so its part is checked as a reply and marked a hand-off after
        DISPATCH => {
            said("goal")?;
            ("reply", said("ack")?)
        }
        ASK => ("clarify", said("question")?),
        _ => ("reply", String::new()),
    };
    form.insert("kind".into(), json!(kind));
    form.insert("text".into(), json!(text));
    form.insert("answers_question".into(), json!(false));
    for (k, v) in &given {
        if k != "ack" && k != "question" && k != "goal" {
            form.insert(k.clone(), v.clone());
        }
    }
    let mut part = supervisor_outcome_validate_json(&Value::Object(form).to_string())
        .map_err(|e| format!("{tool}: {e}"))?;
    if tool == DISPATCH {
        part.kind = "task".into();
    }
    let carries = match tool {
        DISPATCH => true,
        ASK => true,
        CONTROL => {
            part.control.is_some()
                || part.answer_question.is_some()
                || !part.conversation_changes.is_empty()
        }
        SET_REMINDER => !part.timers.is_empty() || !part.reminder_cancels.is_empty(),
        SCHEDULE => {
            !part.schedules.is_empty() || !part.schedule_changes.is_empty() || part.show_schedules
        }
        _ => !part.remember.is_empty() || !part.forget.is_empty(),
    };
    if !carries {
        return Err(format!(
            "{tool}: the call carries nothing to do; give at least one of {}.",
            takes
                .iter()
                .filter(|k| **k != "answers_question" && **k != "ack")
                .copied()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(part)
}

/// An empty answer: a plain reply with no text and nothing else.
fn empty_outcome() -> SupervisorOutcome {
    SupervisorOutcome {
        control: None,
        kind: "reply".into(),
        text: String::new(),
        answers_question: false,
        answer_question: None,
        task: None,
        remember: Vec::new(),
        forget: Vec::new(),
        timers: Vec::new(),
        schedules: Vec::new(),
        schedule_changes: Vec::new(),
        show_schedules: false,
        reminder_cancels: Vec::new(),
        conversation_changes: Vec::new(),
    }
}

/// `into` with `part`'s side entries added after its own (call order), its control or card answer if it holds one.
/// An entry identical to one already in `into` is dropped (ORCH-739): a model that resends a whole call after part of
/// it was accepted never doubles a reminder, a schedule or a memory.
fn add(into: &mut SupervisorOutcome, part: &SupervisorOutcome) {
    fn new_ones<T: Clone + PartialEq>(into: &mut Vec<T>, from: &[T]) {
        for e in from {
            if !into.contains(e) {
                into.push(e.clone());
            }
        }
    }
    into.answers_question |= part.answers_question;
    if part.control.is_some() {
        into.control = part.control.clone();
    }
    if part.answer_question.is_some() {
        into.answer_question = part.answer_question.clone();
    }
    new_ones(&mut into.remember, &part.remember);
    new_ones(&mut into.forget, &part.forget);
    new_ones(&mut into.timers, &part.timers);
    new_ones(&mut into.schedules, &part.schedules);
    new_ones(&mut into.schedule_changes, &part.schedule_changes);
    into.show_schedules |= part.show_schedules;
    new_ones(&mut into.reminder_cancels, &part.reminder_cancels);
    new_ones(&mut into.conversation_changes, &part.conversation_changes);
}

/// A side call's entries, each alone (ORCH-739): (its name in the call, the index its schedule is named by, a part
/// holding only it, with the call's `answers_question`). A control and a card answer are one entry each;
/// `show_schedules` is no entry (it rides on whatever of the call is accepted). Empty for a hand-off or a question,
/// which are checked whole.
fn entries(tool: &str, part: &SupervisorOutcome) -> Vec<(String, usize, SupervisorOutcome)> {
    if !matches!(tool, CONTROL | SET_REMINDER | SCHEDULE | REMEMBER) {
        return Vec::new();
    }
    let base = || SupervisorOutcome {
        answers_question: part.answers_question,
        ..empty_outcome()
    };
    let mut out = Vec::new();
    if let Some(c) = &part.control {
        out.push((
            "control".to_string(),
            0,
            SupervisorOutcome {
                control: Some(c.clone()),
                ..base()
            },
        ));
    }
    if let Some(a) = &part.answer_question {
        out.push((
            "answer_question".to_string(),
            0,
            SupervisorOutcome {
                answer_question: Some(a.clone()),
                ..base()
            },
        ));
    }
    macro_rules! each {
        ($field:ident) => {
            for (i, e) in part.$field.iter().enumerate() {
                out.push((
                    format!("{}[{i}]", stringify!($field)),
                    i,
                    SupervisorOutcome {
                        $field: vec![e.clone()],
                        ..base()
                    },
                ));
            }
        };
    }
    each!(remember);
    each!(forget);
    each!(timers);
    each!(schedules);
    each!(schedule_changes);
    each!(reminder_cancels);
    each!(conversation_changes);
    out
}

/// The accepted parts joined in call order into one answer (kb/plans/front-door.md, "After the run"): kind `task`
/// with the hand-off's task, else `clarify` with a question, else `reply`; its text is the streamed text, else the ack
/// or the question. None when nothing was accepted and nothing was said.
pub fn joined(parts: &[(String, SupervisorOutcome)], streamed: &str) -> Option<SupervisorOutcome> {
    let mut out = empty_outcome();
    let mut said = String::new();
    for (tool, part) in parts {
        add(&mut out, part);
        match tool.as_str() {
            DISPATCH => {
                out.kind = "task".into();
                out.task = part.task.clone();
                said = part.text.clone();
            }
            ASK if out.kind != "task" => {
                out.kind = "clarify".into();
                said = part.text.clone();
            }
            _ => {}
        }
    }
    out.text = if ferb_orchestrator::py::strip(streamed).is_empty() {
        said
    } else {
        streamed.to_string()
    };
    if parts.is_empty() && ferb_orchestrator::py::strip(&out.text).is_empty() {
        return None;
    }
    Some(out)
}

impl Inner {
    /// ORCH-736: one call of the front door's tools, as the gateway hands it over (`task_id` is the run grant's task,
    /// `{request}.lookups`). An answer to FERB's earlier question joins it first (`answers_question`, once per turn);
    /// the arguments are parsed as today's answer form's part; today's checks run on it (`problems` with the side
    /// entries already accepted in this message beside a hand-off, so a part an entry answers is backed;
    /// `side_quote_problems` on the part alone). Err: the problems, one a line, for the model as the tool's error (the
    /// owner sees nothing). Ok: the part is kept on the turn and the model reads a short line. ORCH-739: a side call
    /// (reminders, schedules, memory, a task's control) that fails is checked again entry by entry; when some entries
    /// pass, they are kept and the error names what was taken and what was refused, so the model resends only those.
    pub async fn front_call(
        &self,
        task_id: &str,
        tool: &str,
        arguments: &Value,
    ) -> Result<String, String> {
        let turn = task_id
            .strip_suffix(LOOKUPS)
            .and_then(|rid| self.turn_of(rid))
            .ok_or_else(|| "No chat message is being answered for this call.".to_string())?;
        let (emit, accepted) = {
            let t = turn.lock().unwrap();
            let Some(front) = t.front.as_ref() else {
                return Err("No chat message is being answered for this call.".into());
            };
            (front.emit.clone(), front.parts.clone())
        };
        let mut part = part_of(tool, arguments)?;
        let had = |name: &str| accepted.iter().any(|(t, _)| t == name);
        if tool == DISPATCH && had(DISPATCH) {
            return Err(
                "One task per message: a task was already handed off for this message.".into(),
            );
        }
        // the same control or card answer sent again (a whole call resent after part of it was refused, ORCH-739) is
        // the one already taken, not a second one
        // decision: ORCH-742 — several stops in one message ("stop all tasks") each stop their task; any other
        // control stays one per message
        let mut extra_stop = None;
        if let Some(c) = &part.control {
            let earlier: Vec<&ferb_orchestrator::models::TaskControl> = accepted
                .iter()
                .filter_map(|(_, p)| p.control.as_ref())
                .collect();
            if earlier.iter().any(|e| *e != c) {
                if c.action == "stop" && earlier.iter().all(|e| e.action == "stop") {
                    extra_stop = Some(c.task_id.clone());
                } else {
                    return Err(
                        "One control per message, except stops: a task was already controlled for this message."
                            .into(),
                    );
                }
            }
        }
        if part.answer_question.is_some()
            && accepted.iter().any(|(_, p)| {
                p.answer_question.is_some() && p.answer_question != part.answer_question
            })
        {
            return Err(
                "One card answer per message: a question was already answered for this message."
                    .into(),
            );
        }
        // decision: ORCH-737 — the earlier question joins the message before its words are checked
        self.join_question(&turn, &part);
        let given = part.clone();
        let mut problems = if tool == DISPATCH {
            let mut backed = part.clone();
            for (_, p) in &accepted {
                add(&mut backed, p);
            }
            let found = self
                .problems(&turn, &mut backed, Some(&emit))
                .map_err(|f| f.message())?;
            part.task = backed.task;
            found
        } else {
            self.problems(&turn, &mut part, Some(&emit))
                .map_err(|f| f.message())?
        };
        problems.extend(
            self.side_quote_problems(&turn, &part)
                .await
                .map_err(|f| f.message())?,
        );
        let rid = turn.lock().unwrap().request_id.clone();
        let mut answer = Ok(match tool {
            DISPATCH => ACCEPTED_TASK,
            ASK => ACCEPTED_QUESTION,
            _ => ACCEPTED,
        }
        .to_string());
        if !problems.is_empty() {
            // decision: ORCH-739 — a side call's entries are checked one at a time: the ones that pass are kept, the
            // model is told which were refused and why, and resends only those
            let Some((kept, taken, refused)) = self.entries_apart(&turn, tool, &given).await?
            else {
                self.journal(
                    "task.repair",
                    json!({"request_id": rid, "tool": tool, "problems": problems}),
                    None,
                );
                return Err(problems.join("\n"));
            };
            self.journal(
                "task.repair",
                json!({"request_id": rid, "tool": tool, "problems": refused, "accepted": taken}),
                None,
            );
            part = kept;
            answer = Err(format!(
                "Accepted, and already taken (don't send these again): {}. They take effect as this message ends.\n\
                 Refused (send only these again, corrected, or leave them out):\n{}",
                taken.join(", "),
                refused.join("\n")
            ));
        }
        let mut t = turn.lock().unwrap();
        let Some(front) = t.front.as_mut() else {
            return Err("No chat message is being answered for this call.".into());
        };
        if let Some(task_id) = extra_stop {
            part.control = None;
            front.stops.push(task_id);
        }
        if tool == DISPATCH {
            front.handoff = arguments["goal"].as_str().map(str::to_string);
        }
        front.parts.push((tool.to_string(), part));
        answer
    }

    /// ORCH-739: a side call that failed its checks, its entries checked one at a time (each alone, as the whole call
    /// was). Some((the call with only the entries that passed, their names, the refused ones' lines: each name, then
    /// its problems)) when some passed and some didn't; None when the call is checked whole (a hand-off, a question,
    /// one entry), when no entry passed, or when every entry passed alone (the whole call's problems stand, as
    /// before).
    async fn entries_apart(
        &self,
        turn: &Arc<Mutex<Turn>>,
        tool: &str,
        given: &SupervisorOutcome,
    ) -> Result<Option<(SupervisorOutcome, Vec<String>, Vec<String>)>, String> {
        let each = entries(tool, given);
        if each.len() < 2 {
            return Ok(None);
        }
        let mut kept = SupervisorOutcome {
            answers_question: given.answers_question,
            show_schedules: given.show_schedules,
            ..empty_outcome()
        };
        let (mut accepted, mut refused, mut advised) = (Vec::new(), Vec::new(), false);
        for (name, first, mut one) in each {
            let mut found = self
                .problems(turn, &mut one, None)
                .map_err(|f| f.message())?;
            found.extend(
                self.side_quote_problems_at(turn, &one, first)
                    .await
                    .map_err(|f| f.message())?,
            );
            if found.is_empty() {
                add(&mut kept, &one);
                accepted.push(name);
                continue;
            }
            refused.push(format!("{name}:"));
            for line in found {
                if line == super::QUOTE_ADVICE {
                    advised = true;
                } else {
                    refused.push(line);
                }
            }
        }
        if accepted.is_empty() || refused.is_empty() {
            return Ok(None);
        }
        if advised {
            refused.push(super::QUOTE_ADVICE.into());
        }
        Ok(Some((kept, accepted, refused)))
    }

    /// ORCH-735: the front door's run of a turn, in place of `supervise` and its repair: the planner's blocks, the
    /// shared rules with `concierge.md`, no answer form, the lookups and the front tools on a grant that may act,
    /// text through `PlainReply`, a lookup's start shown as the turn's `progress` line, and the run ended on an
    /// accepted hand-off or question. The accepted parts joined; the one cross-part check (`unbacked_parts`) runs on the whole.
    pub(super) async fn front_decide(
        &self,
        turn: &Arc<Mutex<Turn>>,
        emit: &Emit,
        cancel: &Cancel,
    ) -> Flow<SupervisorOutcome> {
        let rid = turn.lock().unwrap().request_id.clone();
        let held = turn.lock().unwrap().prepare.take();
        let mut prep = held.unwrap_or_else(|| Prepare::start(self, &rid, false));
        let (rt, ctx, sourced, lookups) = match self.planner_blocks(turn, None, "", &mut prep, true)
        {
            Ok(b) => b,
            Err(e) => {
                prep.finish(self, Some(&e));
                return Err(e);
            }
        };
        prep.finish(self, None);
        let span = self.span_start(
            "model.turn",
            None,
            None,
            None,
            json!({"role": "supervisor", "request_id": rid, "repair": false, "front_door": true}),
        );
        // decision: ORCH-732 — the time to the owner's first word, as the planner's call records it
        let first_text: Arc<Mutex<Option<f64>>> = Arc::default();
        let plain = {
            let emit = emit.clone();
            let first = first_text.clone();
            let clock = self.deps.clock.clone();
            Arc::new(Mutex::new(PlainReply::new(
                self.deps.redactor.clone(),
                self.deps.policy.clone(),
                Arc::new(move |v: Value| {
                    if text_frame(&v) {
                        first
                            .lock()
                            .unwrap()
                            .get_or_insert_with(|| clock.monotonic());
                    }
                    let _ = emit.send(v);
                }),
            )))
        };
        let streamed: Arc<Mutex<String>> = Arc::default();
        let usage: Arc<Mutex<Map<String, Value>>> = Arc::default();
        let call_timing: Arc<Mutex<Map<String, Value>>> = Arc::default();
        let mut clocked = {
            let clock = self.deps.clock.clone();
            ferb_core::timing::Steps::new(Box::new(move || clock.monotonic()))
        };
        let offered: Vec<String> = lookups
            .iter()
            .cloned()
            .chain(FRONT_TOOLS.iter().map(|n| n.to_string()))
            .collect();
        turn.lock().unwrap().front = Some(FrontRun {
            parts: Vec::new(),
            stops: Vec::new(),
            handoff: None,
            emit: emit.clone(),
        });
        let (token, looker) = self.lookup_grant(turn, &span, &offered, true);
        let system = self.system_for_request("concierge.md", false, &[], &rid);
        clocked.mark("ferb_before");
        let task = Task {
            id: rid.clone(),
            parent_id: None,
            goal: "chat turn".into(),
            device_id: self.deps.device_id.clone(),
            identity: Identity::Owner,
            scopes: Default::default(),
            role: TaskRole::Mixed,
            request_id: rid.clone(),
            success_conditions: Vec::new(),
            deadline: None,
        };
        let images = turn.lock().unwrap().planner_images();
        let on_tool_event = {
            let me = self.me();
            let plain = plain.clone();
            let turn = turn.clone();
            let emit = emit.clone();
            Arc::new(move |data: &Value| {
                let name = data.get("name").and_then(Value::as_str).unwrap_or("");
                match data.get("phase").and_then(Value::as_str) {
                    // a lookup's start: the turn's live line, as the walk of a plan's folders shows it (ORCH-625)
                    Some("started") if PLANNER_TOOLS.contains(&name) => {
                        let args = data
                            .get("arguments")
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        let text = ferb_orchestrator::progress::describe(
                            me.deps.policy.as_ref(),
                            name,
                            &args,
                            "read",
                            None,
                            None,
                        );
                        let _ = emit
                            .send(json!({"type": "progress", "text": me.redact_text(&text),
                            "planning": true, "started_at": me.now_iso()}));
                    }
                    // the hand-off's ack, shown only once code accepted the call; a refused one is withdrawn
                    Some("completed") if name == DISPATCH => {
                        if data.get("error") == Some(&Value::Bool(false)) {
                            let ack = turn
                                .lock()
                                .unwrap()
                                .front
                                .as_ref()
                                .and_then(|f| f.parts.iter().rev().find(|(t, _)| t == DISPATCH))
                                .map(|(_, p)| p.text.clone());
                            if let Some(ack) = ack {
                                let mut p = plain.lock().unwrap();
                                p.ack(&ack);
                                p.ack_end(&ack);
                            }
                        } else {
                            plain.lock().unwrap().withdraw();
                        }
                    }
                    _ => {}
                }
            }) as ferb_worker::worker::OnEvent
        };
        let hooks = CallHooks {
            on_delta: Some({
                let plain = plain.clone();
                let streamed = streamed.clone();
                Arc::new(move |t: &str| {
                    streamed.lock().unwrap().push_str(t);
                    plain.lock().unwrap().feed(t);
                }) as ferb_worker::worker::OnText
            }),
            on_usage: Some({
                let usage = usage.clone();
                Arc::new(move |m: Map<String, Value>| usage.lock().unwrap().extend(m))
            }),
            notify: None,
            on_tool: Some({
                let plain = plain.clone();
                Arc::new(move || plain.lock().unwrap().restart()) as Arc<dyn Fn() + Send + Sync>
            }),
            on_timing: Some({
                let t = call_timing.clone();
                Arc::new(move |m: Map<String, Value>| t.lock().unwrap().extend(m))
            }),
            on_tool_event: Some(on_tool_event),
        };
        let ctx = self.context_final(&rid, ctx);
        let worker = self.worker.get().expect("the worker").clone();
        let call = Call {
            task: &task,
            system,
            context: ctx.clone(),
            token: &token,
            parent_span: &span,
            tree: None,
            worker_id: "",
            images,
            hooks,
            retry_silent: false,
            ends_on: ENDS_ON.iter().map(|n| n.to_string()).collect(),
        };
        let ran = tokio::select! {
            r = worker.run_call(&rt, call) => Some(r),
            _ = cancel.wait() => None,
        };
        clocked.mark("model_call");
        // the run is over: a call still on its way finds no run open
        let (parts, stops, handoff) = turn
            .lock()
            .unwrap()
            .front
            .take()
            .map(|f| (f.parts, f.stops, f.handoff))
            .unwrap_or_default();
        // ORCH-743: the planner writes the hand-off's task (decide), its tail naming this goal
        turn.lock().unwrap().handoff = handoff;
        let streamed = streamed.lock().unwrap().clone();
        let timing = |clocked: &ferb_core::timing::Steps| -> Value {
            let mut t = clocked.facts();
            let call = call_timing.lock().unwrap().clone();
            if !call.is_empty() {
                t.insert("call".into(), Value::Object(call));
            }
            // decision: ORCH-732 — once per turn, on the call that sent its first text frame: ms from its intake
            let sent = *first_text.lock().unwrap();
            if let Some(sent) = sent {
                let mut held = turn.lock().unwrap();
                if let (false, Some(intake)) = (held.first_text_told, held.intake_at) {
                    held.first_text_told = true;
                    let ms = ((sent - intake) * 1000.0).round_ties_even() as i64;
                    t.insert("first_text_ms".into(), json!(ms));
                }
            }
            Value::Object(t)
        };
        let calls: Vec<&str> = parts.iter().map(|(t, _)| t.as_str()).collect();
        let failure = match &ran {
            None => Some(Failure::Cancelled),
            Some(Err(e)) => Some(Failure::Worker(e.clone())),
            Some(Ok(_)) => None,
        };
        // a cancelled run (a newer message restarts the turn) or a failed one that left nothing accepted: today's
        // failure; a failed run keeps the parts code already accepted
        if let Some(f) = failure
            .clone()
            .filter(|f| *f == Failure::Cancelled || parts.is_empty())
        {
            plain.lock().unwrap().abort();
            if matches!(f, Failure::Cancelled) {
                let mut call = call_timing.lock().unwrap();
                if call.is_empty() {
                    let mut steps = Map::new();
                    for step in ["prepare", "start", "stream", "close"] {
                        steps.insert(step.into(), json!(0));
                    }
                    call.insert("steps_ms".into(), Value::Object(steps));
                    call.insert("total_ms".into(), json!(0));
                    call.insert("model_calls".into(), json!(0));
                }
            }
            self.span_finish(
                &span,
                Some(json!({"text": "", "timing": timing(&clocked)})),
                Some(&format!("{}: {}", f.class(), f.message())),
                None,
                None,
            );
            self.lookups_read(turn);
            self.lookups_end(&rid, &looker);
            if let Failure::Worker(we @ ferb_worker::Error::Service(s)) = &f {
                if matches!(s.kind, ferb_worker::errors::ServiceKind::Image)
                    && !turn.lock().unwrap().planner_images().is_empty()
                {
                    // ORCH-524, L12: a model that won't take the screenshot must not fail the message: once more
                    // without it, as the planner's call does
                    self.journal(
                        "screen.image_refused",
                        json!({"request_id": rid, "error": we.to_string()}),
                        None,
                    );
                    self.withhold_images(turn);
                    return Box::pin(self.front_decide(turn, emit, cancel)).await;
                }
            }
            return Err(f);
        }
        // the plain answer: what streamed, else (a runtime that streamed nothing) the run's own answer, unless the
        // run ended on its hand-off or question (its answer is then that call's record)
        let ended = parts.iter().any(|(t, _)| ENDS_ON.contains(&t.as_str()));
        let final_ = match &ran {
            Some(Ok(text)) => text.clone(),
            _ => String::new(),
        };
        let text = if !streamed.is_empty() || ended {
            streamed.clone()
        } else {
            final_.clone()
        };
        plain.lock().unwrap().finish(&text);
        self.after_decision(turn, &ctx, &sourced);
        self.lookups_read(turn);
        clocked.mark("ferb_after");
        let held = usage.lock().unwrap().clone();
        let mut kept = Map::new();
        for k in [
            "input_tokens",
            "cached_input_tokens",
            "cache_write_tokens",
            "output_tokens",
            "reasoning_tokens",
        ] {
            if let Some(v) = held.get(k) {
                kept.insert(k.into(), v.clone());
            }
        }
        let error = failure.map(|f| format!("{}: {}", f.class(), f.message()));
        // decision: ORCH-742 — the message's further stops, each as the app's Stop (its end is told in its own task's
        // answer); the first control goes through `route` as any control does
        for task_id in &stops {
            if self.restart_pending(task_id) {
                self.call_off_restart(task_id, Some(&rid));
            } else {
                self.stop_tree(task_id);
            }
        }
        let decided = joined(&parts, &text);
        let mut output = Map::new();
        output.insert("text".into(), json!(final_));
        output.insert("calls".into(), json!(calls));
        if let Some(d) = &decided {
            // what the run decided, in the planner's answer form as its span holds it (`ferb trace`, episodes)
            output.insert("decided".into(), json!(dump_json_supervisor_outcome(d)));
        }
        output.insert("usage".into(), Value::Object(kept));
        output.insert("timing".into(), timing(&clocked));
        self.span_finish(
            &span,
            Some(Value::Object(output)),
            error.as_deref(),
            None,
            None,
        );
        self.lookups_end(&rid, &looker);
        let Some(out) = decided else {
            // nothing accepted and nothing said: today's empty answer
            return Err(Failure::Worker(ferb_worker::Error::value(
                "runtime finished without an answer",
            )));
        };
        // the one check that looks across parts (a task part an entry of this message answers, ORCH-736): every other
        // check already ran on its own call, so a hand-off's folders, inputs and earlier results aren't read twice
        let problems = self.unbacked_parts(turn, &out);
        if !problems.is_empty() {
            self.journal(
                "chat.failed",
                json!({"request_id": rid, "why": "the front door's accepted calls are invalid together",
                       "problems": problems}),
                None,
            );
            return Err(Failure::PlanRejected(problems));
        }
        Ok(out)
    }
}

/// ORCH-743: the front door's answer with the task the planner wrote for its hand-off. The front door's own effects
/// (side entries, a control, a card answer) and its ack stand; the planner's side entries are dropped (they were the
/// front door's to make). When the planner asks or replies instead of writing a task, its words stand.
pub fn with_task(front: SupervisorOutcome, planned: SupervisorOutcome) -> SupervisorOutcome {
    if planned.kind == "task" && planned.task.is_some() {
        return SupervisorOutcome {
            kind: "task".into(),
            task: planned.task,
            ..front
        };
    }
    SupervisorOutcome {
        kind: planned.kind,
        text: planned.text,
        task: planned.task,
        control: front.control.or(planned.control),
        answer_question: front.answer_question.or(planned.answer_question),
        ..front
    }
}
