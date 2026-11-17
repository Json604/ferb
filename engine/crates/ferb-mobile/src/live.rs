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
    /// What the task is doing now (FERB.app's `TaskItem.currentLine`, APP-890, APP-893): the newest step line of any
    /// of its steps, a step's summary, or "Checking the result".
    pub current: Option<LiveLine>,
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
                    let line = LiveLine {
                        text,
                        started_at: words(f, "started_at"),
                    };
                    if !line.text.is_empty() {
                        t.current = Some(line.clone());
                    }
                    t.step_lines.insert(step, line);
                } else if !text.is_empty() {
                    t.current = Some(LiveLine {
                        text,
                        started_at: frame_time(f),
                    });
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
                    t.current = Some(LiveLine {
                        text: text.clone(),
                        started_at: frame_time(f),
                    });
                    t.activity = Some(text);
                }
            }
            Some("verification") => {
                t.activity = Some("Checking the result".into());
                t.current = Some(LiveLine {
                    text: "Checking the result".into(),
                    started_at: frame_time(f),
                });
            }
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
        // APP-895: under a row still waiting, the steps it waits on that answer no row, each with its own line
        let feeders: Vec<LiveFeeder> = t
            .card
            .as_ref()
            .map(|c| {
                let row_steps: std::collections::HashSet<&str> = c
                    .rows
                    .iter()
                    .filter(|r| !r.row.direct)
                    .filter_map(|r| r.row.step.as_deref())
                    .collect();
                let mut out = Vec::new();
                for (r, shown) in c.rows.iter().zip(&rows) {
                    if shown.kind != "waiting" {
                        continue;
                    }
                    for step in feeders_of(r.row.step.as_deref(), &c.plan, &row_steps) {
                        let line = live.step_lines.get(&step.step_task_id);
                        out.push(LiveFeeder {
                            part: r.row.part.clone(),
                            step: step.id.clone(),
                            label: step.label.clone(),
                            step_task_id: step.step_task_id.clone(),
                            line: LiveRow::checklist(step, steps.get(&step.id), line),
                        });
                    }
                }
                out
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
        // the work row's line (FERB.app's `TaskItem.currentLine`): only a turn that became a task has one
        let is_task = t.first_reply.is_some()
            || t.card.is_some()
            || t.checklist.is_some()
            || t.task_id.is_some();
        let current = is_task.then(|| {
            live.current
                .as_ref()
                .map(|l| sentence(&l.text))
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| "Starting task…".into())
        });
        let card_line = t
            .card
            .as_ref()
            .filter(|c| c.line.is_none() && !rows.is_empty())
            .map(|c| card_status_line(c.done, c.total, &rows));
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
            current,
            card_line,
            feeders,
        }
    }
}

/// `ChecklistSummary.feeders` (APP-895): the steps a row's step waits on that no row answers, and theirs, in plan order.
pub fn feeders_of<'a>(
    step: Option<&str>,
    plan: &'a [PlanStep],
    row_steps: &std::collections::HashSet<&str>,
) -> Vec<&'a PlanStep> {
    let Some(step) = step else {
        return Vec::new();
    };
    let by_id: std::collections::HashMap<&str, &PlanStep> =
        plan.iter().rev().map(|p| (p.id.as_str(), p)).collect();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut pending: Vec<String> = by_id
        .get(step)
        .map(|p| p.depends_on.clone())
        .unwrap_or_default();
    while let Some(next) = pending.pop() {
        if row_steps.contains(next.as_str()) || !seen.insert(next.clone()) {
            continue;
        }
        if let Some(p) = by_id.get(next.as_str()) {
            pending.extend(p.depends_on.iter().cloned());
        }
    }
    plan.iter().filter(|p| seen.contains(&p.id)).collect()
}

/// One step a waiting card row waits on that no row answers (APP-895), with its live line.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveFeeder {
    /// The row it shows under.
    pub part: String,
    pub step: String,
    pub label: String,
    pub step_task_id: String,
    pub line: LiveRow,
}

/// `CardStatusLine.text`: a running card's one status line, "1 of 4 done · 1 stopped by you · 2 running · 1 waiting
/// · 1 needs you" (APP-718, APP-752).
pub fn card_status_line(done: i64, total: i64, rows: &[LiveRow]) -> String {
    let count = |kinds: &[&str]| {
        rows.iter()
            .filter(|r| kinds.contains(&r.kind.as_str()))
            .count()
    };
    let mut pieces = vec![format!("{done} of {total} done")];
    let stopped = count(&["stopped"]);
    if stopped > 0 {
        pieces.push(format!("{stopped} stopped by you"));
    }
    let running = count(&[
        "starting", "running", "checking", "retrying", "loop", "working",
    ]);
    let waiting = count(&["waiting"]);
    let asking = count(&["needsYou", "paused"]);
    if running > 0 {
        pieces.push(format!("{running} running"));
    }
    if waiting > 0 {
        pieces.push(format!("{waiting} waiting"));
    }
    if asking > 0 {
        pieces.push(format!(
            "{asking} need{} you",
            if asking == 1 { "s" } else { "" }
        ));
    }
    pieces.join(" · ")
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
    /// What the task is doing now, as FERB.app's work row and short view say it (APP-890, APP-893): its newest step
    /// line, else "Starting task…". None for a turn that is no task (a reply being written).
    pub current: Option<String>,
    /// A running card's status line (`CardStatusLine`), none once the card has its footer.
    pub card_line: Option<String>,
    /// The steps each waiting row waits on that no row answers, with their lines (APP-895).
    pub feeders: Vec<LiveFeeder>,
}
