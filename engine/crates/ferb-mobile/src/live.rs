//! Live state (ENG-8: "live state is never an entry"): what only shows while it happens, from `chat.entries.watch`'s
//! `{"type": "live", "request_id", "frame"}` frames, layered over the folded transcript and never stored.
//!
//! Ported from what FERB.app does with the same frames on its own turns (DaemonConnection.swift `handleChatEvent`,
//! ConversationArchive.swift `streamPlan`, `abortPlanStream`, `showProgress`, `append`, `abortReply`; CardRowState.swift
//! `RowDisplay`; StepChecklist.swift `ChecklistLine`): the typing dots after the ack, the streamed plan line
//! (`plan_delta`), a streamed reply (`reply_delta`), the one live line under a working turn (`progress` marked
//! `connection` or `planning`), each step task's own line (`progress` with `step_task_id`), each plan step's state
//! (`step`), and the activity line (`subtask`, `verification`, `memory`, `timer`). A turn's live state goes when its
//! plan (the streamed plan and the line) or its answer (everything) is stored, and all of it on a restart of the
//! watch (`reset`): after a restart a phone has none until the next frame.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::entry::{int, ChatEntry};
use crate::fold::model::{s, strs, words, CardRow, PlanStep};
use crate::fold::{Item, Transcript, TurnItem};

/// One plan step's live state (`StepState`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepState {
    pub status: String,
    pub waits_on: Vec<String>,
    pub attempt: i64,
    pub seq: Option<String>,
    #[serde(rename = "loop")]
    pub repeats: bool,
    pub until: Option<String>,
    pub event: Option<String>,
    pub line: Option<String>,
}

impl StepState {
    pub fn read(v: &Value) -> Option<StepState> {
        let m = v.as_object()?;
        Some(StepState {
            status: words(m, "status")?,
            waits_on: strs(m, "waits_on").unwrap_or_default(),
            attempt: m.get("attempt").and_then(int).unwrap_or(0),
            seq: words(m, "seq"),
            repeats: m.get("loop").and_then(Value::as_bool).unwrap_or(false),
            until: words(m, "until").filter(|u| crate::time::parse(u).is_some()),
            event: words(m, "event"),
            line: words(m, "line"),
        })
    }

    /// `StepState.replaces`: a later seq, or any state over none.
    fn replaces(&self, current: Option<&StepState>) -> bool {
        match (current, &self.seq) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(c), Some(seq)) => c.seq.as_ref().is_none_or(|shown| seq > shown),
        }
    }
}

/// A live line: its words and when the step it describes started (ISO text, as the frame carried it).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveLine {
    pub text: String,
    pub started_at: Option<String>,
}

/// What is live for one turn (by its request).
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct LiveTurn {
    pub streaming_plan: String,
    pub streaming_reply: String,
    /// A withdrawn streamed reply's words (`reply_abort`), until the real answer comes.
    pub reply_notice: Option<String>,
    pub line: Option<LiveLine>,
    pub activity: Option<String>,
    /// Each step task's own live line, by step task id.
    pub step_lines: BTreeMap<String, LiveLine>,
    /// Each plan step's state, by task id then step id.
    pub steps: BTreeMap<String, BTreeMap<String, StepState>>,
}

/// The live frames' state for one conversation.
#[derive(Debug, Clone, Default)]
pub struct LiveState {
    turns: BTreeMap<String, LiveTurn>,
}

fn frame_time(f: &serde_json::Map<String, Value>) -> Option<String> {
    words(f, "started_at").or_else(|| words(f, "at"))
}

impl LiveState {
    pub fn new() -> LiveState {
        LiveState::default()
    }

    /// One live frame; returns whether anything a phone shows changed.
    pub fn frame(&mut self, request_id: Option<&str>, frame: &Value) -> bool {
        let Some(f) = frame.as_object() else {
            return false;
        };
        let Some(request) = request_id
            .map(str::to_string)
            .or_else(|| words(f, "request_id"))
        else {
            return false;
        };
        let before = self.turns.get(&request).cloned();
        let t = self.turns.entry(request.clone()).or_default();
        match s(f, "type").as_deref() {
            Some("plan_delta") => t.streaming_plan.push_str(&s(f, "text").unwrap_or_default()),
            // the streamed plan goes back for repair: its walk's line is over, the typing dots show again
            Some("plan_abort") => {
                t.streaming_plan.clear();
                t.line = None;
            }
            Some("reply_delta") => {
                t.streaming_reply
                    .push_str(&s(f, "text").unwrap_or_default());
                t.reply_notice = None;
            }
            Some("reply_abort") => {
                t.streaming_reply.clear();
                let reason = s(f, "reason").unwrap_or_default();
                t.reply_notice = Some(if reason.is_empty() {
                    "The draft was interrupted. Waiting for FERB’s final response.".into()
                } else {
                    reason
                });
                t.line = None;
            }
            Some("progress") => {
                let text = s(f, "text").unwrap_or_default();
                if f.contains_key("connection")
                    || f.get("planning").and_then(Value::as_bool) == Some(true)
                {
                    if !text.is_empty() {
                        t.line = Some(LiveLine {
                            text,
                            started_at: frame_time(f),
                        });
                    }
                } else if let Some(step) = words(f, "step_task_id") {
                    t.step_lines.insert(
                        step,
                        LiveLine {
                            text,
                            started_at: words(f, "started_at"),
                        },
                    );
                }
            }
            Some("step") => {
                if let (Some(task), Some(key), Some(state)) = (
                    words(f, "task_id"),
                    words(f, "step"),
                    StepState::read(frame),
                ) {
                    let steps = t.steps.entry(task).or_default();
                    if state.replaces(steps.get(&key)) && steps.get(&key) != Some(&state) {
                        steps.insert(key, state);
                    }
                }
            }
            // a step's summary is a line; its bare state ("started") is not (APP-893)
            Some("subtask") => {
                if let Some(text) = words(f, "summary") {
                    t.activity = Some(text);
                }
            }
            Some("verification") => t.activity = Some("Checking the result".into()),
            Some("memory") | Some("timer") => {
                t.activity = Some(s(f, "text").unwrap_or_else(|| "Task updated".into()))
            }
            _ => {}
        }
        let now = self.turns.get(&request).cloned();
        if now == Some(LiveTurn::default()) && before.is_none() {
            self.turns.remove(&request);
            return false;
        }
        now != before
    }

    /// A stored entry arrived: a plan ends the streamed plan and the live line; a turn's answer (one that `follows`
    /// nothing) or a merged or interrupted message ends everything live for it. Returns whether anything changed.
    pub fn entry(&mut self, e: &ChatEntry) -> bool {
        let Some(request) = &e.request_id else {
            return false;
        };
        let Some(t) = self.turns.get_mut(request) else {
            return false;
        };
        match e.kind.as_str() {
            "plan" => {
                let changed = !t.streaming_plan.is_empty()
                    || t.line.is_some()
                    || !t.streaming_reply.is_empty();
                t.streaming_plan.clear();
                t.streaming_reply.clear();
                t.line = None;
                changed
            }
            "answer" | "error" if !e.body.get("follows").is_some_and(Value::is_string) => {
                self.turns.remove(request);
                true
            }
            "message" if e.deleted || e.state == "merged" || e.state == "interrupted" => {
                self.turns.remove(request);
                true
            }
            _ if e.deleted => {
                self.turns.remove(request);
                true
            }
            _ => false,
        }
    }

    /// The watch restarted (a reconnect, `behind`, `reset_required`): nothing live is known until the next frame.
    pub fn reset(&mut self) -> bool {
        let had = !self.turns.is_empty();
        self.turns.clear();
        had
    }

    pub fn turn(&self, request_id: &str) -> Option<&LiveTurn> {
        self.turns.get(request_id)
    }

    /// What a phone draws live over the transcript: one view per turn still waiting for its answer (working or
    /// asking), in the transcript's order. Live state of a turn that has its answer is dropped here too.
    pub fn view(&self, transcript: &Transcript) -> Vec<LiveView> {
        transcript
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Turn(t) if t.status == "working" || t.status == "needs_user" => Some(t),
                _ => None,
            })
            .map(|t| self.turn_view(t, transcript))
            .collect()
    }

    fn turn_view(&self, t: &TurnItem, transcript: &Transcript) -> LiveView {
        let empty = LiveTurn::default();
        let live = t
            .request_id
            .as_ref()
            .and_then(|r| self.turns.get(r))
            .unwrap_or(&empty);
        // the task the steps belong to: the turn's, else its card's or checklist's (a plan binds the task)
        let task = t
            .task_id
            .clone()
            .or_else(|| t.card.as_ref().map(|c| c.task_id.clone()))
            .or_else(|| t.checklist.as_ref().map(|c| c.task_id.clone()));
        let task_steps = task.as_ref().and_then(|task| live.steps.get(task));
        let no_steps = BTreeMap::new();
        let steps = task_steps.unwrap_or(&no_steps);
        let open: Vec<String> = transcript
            .questions
            .iter()
            .filter(|q| q.open)
            .filter(|q| {
                (q.request_id.is_some() && q.request_id == t.request_id)
                    || (q.task_id.is_some() && q.task_id == t.task_id)
            })
            .filter_map(|q| q.approval_id.clone())
            .collect();
        let rows: Vec<LiveRow> = t
            .card
            .as_ref()
            .map(|c| {
                let all: Vec<CardRow> = c.rows.iter().map(|r| r.row.clone()).collect();
                all.iter()
                    .map(|r| {
                        let asking = open
                            .iter()
                            .any(|a| r.question.as_ref().is_some_and(|q| &q.approval_id == a));
                        let line = r.step_task_id.as_ref().and_then(|s| live.step_lines.get(s));
                        LiveRow::of(r, steps, &all, &c.plan, asking, line)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let checklist: Vec<LiveRow> = t
            .checklist
            .as_ref()
            .filter(|c| !c.finished)
            .map(|c| {
                c.steps
                    .iter()
                    .map(|step| {
                        let line = live.step_lines.get(&step.step_task_id);
                        LiveRow::checklist(step, steps.get(&step.id), line)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let has_plan = t.first_reply.is_some();
        let typing = t.status == "working"
            && t.answer.is_none()
            && !has_plan
            && live.streaming_plan.is_empty()
            && live.streaming_reply.is_empty()
            && t.merged_into.is_none()
            && t.card.is_none();
        let paused = rows.iter().any(|r| r.kind == "paused");
        let all_live: Vec<&LiveRow> = rows.iter().chain(checklist.iter()).collect();
        let sleeping = !all_live.is_empty()
            && all_live.iter().any(|r| r.until.is_some())
            && !all_live.iter().any(|r| r.spinner);
        let state = if !open.is_empty() || t.status == "needs_user" {
            "needs_you"
        } else if paused {
            "paused"
        } else if sleeping {
            "sleeping"
        } else {
            "running"
        };
        LiveView {
            entry: t.entry.clone(),
            request_id: t.request_id.clone(),
            state: state.into(),
            typing,
            streaming_plan: (!live.streaming_plan.is_empty()).then(|| live.streaming_plan.clone()),
            streaming_reply: (!live.streaming_reply.is_empty())
                .then(|| live.streaming_reply.clone()),
            reply_notice: live.reply_notice.clone(),
            line: live.line.clone(),
            activity: live.activity.clone(),
            rows,
            checklist,
            open_questions: open,
        }
    }
}

/// A card row's or checklist line's live look (`RowDisplay`, `ChecklistLine`): kind, words, spinner; `until` when it
/// waits for a time (the app writes the time in the phone's own format).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveRow {
    /// The row's part, or the checklist step's id.
    pub id: String,
    pub kind: String,
    pub words: Option<String>,
    pub spinner: bool,
    pub until: Option<String>,
}

fn list(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
    }
}

fn sentence(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

impl LiveRow {
    fn new(id: &str, kind: &str, words: Option<String>, spinner: bool) -> LiveRow {
        LiveRow {
            id: id.into(),
            kind: kind.into(),
            words,
            spinner,
            until: None,
        }
    }

    /// `RowDisplay.forStep`: what a step's live state says (loop rounds come from the task's loop status, which a
    /// phone reads elsewhere: a loop row shows as running here).
    fn for_step(
        id: &str,
        step: &StepState,
        waiting_for: &[String],
        line: Option<&LiveLine>,
    ) -> LiveRow {
        match step.status.as_str() {
            "waiting" => LiveRow::new(
                id,
                "waiting",
                Some(if waiting_for.is_empty() {
                    "Waiting for an earlier step".into()
                } else {
                    format!("Waiting for {}", list(waiting_for))
                }),
                false,
            ),
            "starting" => LiveRow::new(id, "starting", Some("Starting…".into()), true),
            "running" => LiveRow::new(
                id,
                "running",
                Some(
                    line.map(|l| sentence(&l.text))
                        .unwrap_or_else(|| "Working".into()),
                ),
                true,
            ),
            "retrying" => {
                let attempt = format!("Trying again (attempt {})", step.attempt.max(2));
                let words = match line {
                    Some(l) => format!("{attempt} · {}", l.text),
                    None => attempt,
                };
                LiveRow::new(id, "retrying", Some(words), true)
            }
            "needs_you" => LiveRow::new(id, "needsYou", Some("Needs you".into()), false),
            "not_finished" => LiveRow::new(id, "notFinished", Some("Not finished".into()), false),
            "wont_run" => LiveRow::new(
                id,
                "wontRun",
                Some("Won't run: an earlier step couldn't be done".into()),
                false,
            ),
            "done_with_gaps" => LiveRow::new(
                id,
                "gaps",
                Some("Delivered, with parts not done".into()),
                false,
            ),
            _ => LiveRow::new(id, "checking", Some("Checking the result".into()), true),
        }
    }

    /// `RowDisplay.make` for a turn still running.
    fn of(
        row: &CardRow,
        steps: &BTreeMap<String, StepState>,
        rows: &[CardRow],
        plan: &[PlanStep],
        asking: bool,
        line: Option<&LiveLine>,
    ) -> LiveRow {
        let id = row.part.as_str();
        match row.state.as_str() {
            "preview" => return LiveRow::new(id, "previewRunning", None, false),
            "asks" => return LiveRow::new(id, "needsYou", None, false),
            _ if asking => return LiveRow::new(id, "needsYou", None, false),
            "paused" => return LiveRow::new(id, "paused", None, false),
            "updated" => return LiveRow::new(id, "updated", None, false),
            "done" => return LiveRow::new(id, "done", None, false),
            "done_with_gaps" => {
                return LiveRow::new(
                    id,
                    "gaps",
                    Some("Delivered, with parts not done".into()),
                    false,
                )
            }
            "stopped" => return LiveRow::new(id, "stopped", None, false),
            _ => {}
        }
        let step = row.step.as_ref().and_then(|s| steps.get(s));
        if row.state == "failed" {
            return match step {
                Some(s) if s.status == "wont_run" => LiveRow::new(
                    id,
                    "wontRun",
                    Some("Won't run: an earlier step couldn't be done".into()),
                    false,
                ),
                _ => LiveRow::new(id, "failed", None, false),
            };
        }
        let Some(step) = step else {
            return if steps.is_empty() {
                LiveRow::new(id, "working", Some("Working".into()), true)
            } else {
                LiveRow::new(id, "starting", Some("Starting…".into()), true)
            };
        };
        let mut seen = std::collections::HashSet::new();
        let names: Vec<String> = step
            .waits_on
            .iter()
            .filter_map(|dep| {
                rows.iter()
                    .find(|r| r.step.as_deref() == Some(dep) && !r.direct && r.part != row.part)
                    .map(|r| r.label.clone())
                    .or_else(|| plan.iter().find(|p| &p.id == dep).map(|p| p.label.clone()))
            })
            .filter(|n| !n.is_empty() && seen.insert(n.clone()))
            .collect();
        LiveRow::for_step(id, step, &names, line)
    }

    /// `ChecklistLine.make` for a task still running.
    fn checklist(step: &PlanStep, state: Option<&StepState>, line: Option<&LiveLine>) -> LiveRow {
        let id = step.id.as_str();
        let Some(state) = state else {
            return LiveRow::new(id, "upcoming", None, false);
        };
        match state.status.as_str() {
            "done" => return LiveRow::new(id, "done", None, false),
            "done_with_gaps" => {
                return LiveRow::new(id, "gaps", Some("Parts not done".into()), false)
            }
            "failed" => return LiveRow::new(id, "failed", Some("Couldn't do".into()), false),
            "wont_run" => {
                return LiveRow::new(
                    id,
                    "wontRun",
                    Some("Won't run: an earlier step couldn't be done".into()),
                    false,
                )
            }
            "stopped" => return LiveRow::new(id, "stopped", Some("Stopped by you".into()), false),
            "not_finished" => {
                return LiveRow::new(id, "notFinished", Some("Not finished".into()), false)
            }
            "needs_you" => return LiveRow::new(id, "needsYou", Some("Asking you".into()), false),
            "waiting" => {
                if let Some(until) = &state.until {
                    let mut r = LiveRow::new(id, "waiting", Some("Waiting until".into()), false);
                    r.until = Some(until.clone());
                    return r;
                }
                if let Some(said) = &state.line {
                    return LiveRow::new(id, "waiting", Some(said.clone()), false);
                }
                if state.event.is_some() {
                    return LiveRow::new(
                        id,
                        "waiting",
                        Some("Waiting for its event".into()),
                        false,
                    );
                }
                return LiveRow::new(id, "upcoming", None, false);
            }
            _ => {}
        }
        let shown = LiveRow::for_step(id, state, &[], line);
        match shown.kind.as_str() {
            "needsYou" => LiveRow::new(id, "needsYou", Some("Asking you".into()), false),
            "notFinished" | "wontRun" => shown,
            _ => LiveRow::new(id, "running", shown.words, shown.spinner),
        }
    }
}

/// What a phone draws live for one turn.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveView {
    /// The turn's item (`Item::entry`).
    pub entry: String,
    pub request_id: Option<String>,
    /// running, paused (a row waits on the owner's answer to go on), sleeping (only waits for a time), needs_you.
    pub state: String,
    /// The typing dots: acknowledged, nothing said yet.
    pub typing: bool,
    pub streaming_plan: Option<String>,
    pub streaming_reply: Option<String>,
    pub reply_notice: Option<String>,
    pub line: Option<LiveLine>,
    pub activity: Option<String>,
    /// The card's rows with their live states, in the card's order.
    pub rows: Vec<LiveRow>,
    /// A running checklist's lines with their live states.
    pub checklist: Vec<LiveRow>,
    /// The open questions of this turn (their approval ids), drawn by the questions' `shown`.
    pub open_questions: Vec<String>,
}
