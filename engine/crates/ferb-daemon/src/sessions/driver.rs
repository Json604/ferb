//! The session driver (owner 2026-11-23, HAND-707 and HAND-708; how it runs, HAND-709).
//!
//! Hand to FERB starts a driver, not a chat turn: no planner and no checker. Code owns the loop:
//! 1. it waits for the agent's turn to end ([`Sessions::wait_turn`]: code only, no model call, no time limit while the
//!    agent works), watching the owner's Take it back the whole time;
//! 2. a permission prompt is the owner's (HAND-663): the driver waits for his answer and runs no step;
//! 3. after [`ROUNDS_PER_ASK`] rounds it asks him in the session's screen whether to keep going (the owner's number);
//! 4. otherwise it runs exactly one fresh step ([`Driver::step`]: one worker run with no pushed context and the usual
//!    tools), whose decision is one of three: send the next message, done, or ask the owner;
//! 5. a message is typed by code through the gateway like any tool call ([`Driver::deliver`]: GATE-585's question and
//!    every other safety rule unchanged); an open question of the agent's is answered only with its own choices
//!    (`agent_session.answer`), never a permission prompt.
//!
//! HAND-714: only Claude Code and Codex sessions are driven (their replies are exact, from their own records); a step
//! that FERB's guard stopped (ORCH-691) asks the owner "Keep trying?" in the session, as the main chat asks, instead of
//! ending the drive.
//!
//! One round is an agent reply and the driver's next message. The session's note says every step (HAND-702's states);
//! a message FERB sent shows in the session marked as FERB's. When the drive ends by itself (done, a question for the
//! owner, a failure, the session ended) one plain line goes to FERB's chat; when he stops it, nothing does (he knows).

use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use ferb_sessions::model::*;
use ferb_sessions::{clip, codes, one_line};
use serde_json::json;
use tokio::sync::oneshot;

use super::{folder_name, gone, lock, no, off, Refused, Sessions};

/// decision: HAND-708 — the owner's number: after this many rounds the driver stops and asks him whether to keep
/// going; Keep going gives it as many again.
pub const ROUNDS_PER_ASK: u32 = 20;
/// How many of the session's latest items a step is shown (older ones it can read with `agent_session.read`).
pub const RECENT_ITEMS: usize = 24;
/// An item's words a step is shown at most, except the newest reply's.
pub const ITEM_CHARS: usize = 1_500;
/// The newest reply's words a step is shown at most (in full, short of a runaway reply).
pub const NEWEST_CHARS: usize = 40_000;
/// How many earlier decisions a step is reminded of (the latest).
pub const DECIDED_KEPT: usize = 12;
/// How many of FERB's sent messages a session keeps marked (the latest).
pub const FERB_SENT_KEPT: usize = 200;
/// How often the wait looks for the owner's Take it back.
pub const STOP_LOOK_MS: u64 = 250;

/// The session's note while FERB's keep-going question waits (the question itself is its card, just above).
pub const WAITING_FOR_ANSWER: &str = "FERB is waiting for your answer.";
/// The keep-going question's answers.
pub const KEEP_GOING: &str = "keep_going";
pub const STOP: &str = "stop";
/// HAND-714: the answer to FERB's question after its guard stopped a step.
pub const KEEP_TRYING: &str = "keep_trying";
/// HAND-714: the refusal for a session FERB can only watch.
pub const NOT_DRIVEN: &str =
    "FERB keeps only Claude Code and Codex sessions going: their replies are exact. Watch this one and type into it instead.";

/// What one step is given (nothing else of FERB's context).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// 1, 2, …
    pub round: u32,
    pub goal: String,
    /// The session as code knows it.
    pub session: String,
    /// The session's recent exchanges, oldest first, the newest reply in full.
    pub exchanges: String,
    /// The agent's open question with its choices ("" for none).
    pub asking: String,
    /// What was decided before, oldest first.
    pub decided: Vec<String>,
}

/// One step's decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Send(String),
    Done(String),
    Ask(String),
    /// HAND-714: FERB's guard stopped the step (ORCH-691), in its plain words: the owner is asked whether to keep trying.
    Stuck(String),
}

/// What code types for the drive: a message, or the choice of the agent's open question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Send(String),
    Answer { card: String, choice: String },
}

/// What the area takes from the composition for the driver: its task (rows, spans, Take it back), its fresh step,
/// the gateway's typing, and FERB's chat.
pub trait Driver: Send + Sync {
    /// A drive's task starts: its id ("" never); Err: why it couldn't.
    fn begin(
        &self,
        session: &str,
        goal: &str,
        app: &str,
    ) -> BoxFuture<'static, Result<String, String>>;
    /// One fresh step of the task.
    fn step(&self, task: &str, step: Step) -> BoxFuture<'static, Result<Decision, String>>;
    /// Code's typing for the task, through the gateway (its questions asked and answered there).
    fn deliver(
        &self,
        task: &str,
        session: &str,
        act: Act,
    ) -> BoxFuture<'static, Result<(), String>>;
    /// The owner stopped the task (or it ended).
    fn stopped(&self, task: &str) -> bool;
    /// The task ended: its status (done, needs_user, cancelled, failed) and summary.
    fn end(&self, task: &str, status: &str, summary: &str) -> BoxFuture<'static, ()>;
    /// One plain line in FERB's chat, posted once for `ident`.
    fn say(&self, ident: &str, line: &str);
    /// HAND-714: the owner said keep trying after the guard stopped a step: the guard counts the step afresh.
    fn keep_trying(&self, _task: &str) {}
}

/// A drive under way on one session.
pub(super) struct Drive {
    /// Its task ("" until it has started).
    pub task: String,
    /// FERB's own open question in the session (the keep-going one), and where its answer goes.
    pub card: Option<Card>,
    pub answer: Option<oneshot::Sender<String>>,
}

/// How a drive ended.
enum End {
    Done(String),
    Asked(String),
    Failed(String),
    /// The owner stopped it (Take it back, Stop on the keep-going question, the global stop).
    Stopped,
    /// The session ended (or the drive was replaced).
    Gone,
}

/// What a wait ended with.
enum Waited {
    Over(Box<Session>),
    Stopped,
    Gone,
}

fn quote(t: &str, n: usize) -> String {
    clip(&one_line(t), n)
}

impl Sessions {
    /// The owner hands a session to FERB with his goal (HAND-707): a driver starts on it (no chat turn, no planner, no
    /// checker). Answers his goal as FERB keeps it (redacted).
    pub fn delegate(
        &self,
        id: &str,
        goal: &str,
        _client: Option<String>,
    ) -> Result<String, Refused> {
        if !self.enabled() {
            return Err(off());
        }
        let goal = goal.trim();
        if goal.is_empty() {
            return Err(no(
                codes::SEND_FAILED,
                "Say what FERB should get done in this session.",
            ));
        }
        let s = self.session(id).ok_or_else(gone)?;
        // decision: HAND-714 — only Claude Code and Codex: their replies are exact
        if !matches!(s.agent, Agent::ClaudeCode | Agent::Codex) || s.watch {
            return Err(no(codes::SEND_FAILED, NOT_DRIVEN));
        }
        // nothing can type into it: said now, not after a step
        if s.send_paths.is_empty() {
            return Err(no(codes::NO_SEND_PATH, s.no_send.clone()));
        }
        let goal = self.redact(goal);
        {
            let mut st = lock(&self.state);
            let e = st.sessions.get_mut(id).ok_or_else(gone)?;
            if e.drive.is_some() {
                return Err(no(
                    codes::SEND_FAILED,
                    "FERB is already on this session. Take it back first.",
                ));
            }
            e.drive = Some(Drive {
                task: String::new(),
                card: None,
                answer: None,
            });
            e.s.goal = goal.clone();
            e.s.delegate.clear();
            e.s.ferb_note = format!("Handed to FERB: “{}”", quote(&goal, 160));
        }
        (self.seams.journal)("sessions.delegated", json!({"session": id}));
        self.changed();
        let app = s.host.as_ref().map(|h| h.name.clone()).unwrap_or_default();
        let (me, sid, g) = (self.clone(), id.to_owned(), goal.clone());
        tokio::spawn(async move { me.drive(sid, g, app).await });
        Ok(goal)
    }

    /// The session's drive is this task's, still.
    fn drives(&self, id: &str, task: &str) -> bool {
        lock(&self.state)
            .sessions
            .get(id)
            .and_then(|e| e.drive.as_ref())
            .is_some_and(|d| d.task == task)
    }

    fn drive_note(&self, id: &str, task: &str, note: String) {
        {
            let mut st = lock(&self.state);
            let Some(e) = st.sessions.get_mut(id) else {
                return;
            };
            if !e.drive.as_ref().is_some_and(|d| d.task == task) {
                return;
            }
            e.s.ferb_note = note;
        }
        self.changed();
    }

    /// Resolves once the owner stops the task or the drive is no longer this one.
    async fn until_stopped(&self, id: &str, task: &str) {
        loop {
            if self.seams.driver.stopped(task) || !self.drives(id, task) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(STOP_LOOK_MS)).await;
        }
    }

    /// The agent's turn is over (code only, no time limit), or the owner stopped it, or the session went.
    async fn turn_over(&self, id: &str, task: &str) -> Waited {
        tokio::select! {
            s = self.wait_turn(id, f64::INFINITY) => match s {
                Some(s) if self.drives(id, task) => Waited::Over(Box::new(s)),
                _ => Waited::Gone,
            },
            _ = self.until_stopped(id, task) => {
                if self.drives(id, task) { Waited::Stopped } else { Waited::Gone }
            }
        }
    }

    /// The owner answered the agent's permission prompt (it's his: HAND-663), or stopped, or the session went.
    async fn permission_answered(&self, id: &str, task: &str) -> Option<Waited> {
        loop {
            if self.seams.driver.stopped(task) {
                return Some(Waited::Stopped);
            }
            match self.session(id) {
                None => return Some(Waited::Gone),
                Some(_) if !self.drives(id, task) => return Some(Waited::Gone),
                Some(s) if s.waiting != Waiting::Permission => return None,
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    /// FERB's keep-going question in the session's screen (HAND-708's words), answered with `sessions.answer`:
    /// Some(true) keep going, Some(false) stop, None stopped or gone meanwhile.
    async fn keep_going(&self, id: &str, task: &str, goal: &str, rounds: u32) -> Option<bool> {
        let question = format!(
            "I've gone {rounds} rounds on “{}”. Keep going?",
            quote(goal, 160)
        );
        let banner =
            |folder: &str| format!("FERB has gone {rounds} rounds in {folder}. Keep going?");
        self.ask_in_session(
            id,
            task,
            &format!("ferb-{task}-{rounds}"),
            question,
            (KEEP_GOING, "Keep going"),
            &banner,
            json!({"session": id, "task": task, "rounds": rounds}),
        )
        .await
    }

    /// HAND-714: FERB's guard stopped a step (its plain words in `why`): the same kind of question, "Keep trying?".
    async fn keep_trying(&self, id: &str, task: &str, why: &str, n: u32) -> Option<bool> {
        let why = quote(why.trim_end_matches('.'), 300);
        let question = format!("{why}. Keep trying?");
        let banner = |folder: &str| format!("FERB stopped in {folder}: {why}. Keep trying?");
        self.ask_in_session(
            id,
            task,
            &format!("ferb-{task}-stuck-{n}"),
            question,
            (KEEP_TRYING, "Keep trying"),
            &banner,
            json!({"session": id, "task": task, "stuck": n}),
        )
        .await
    }

    /// FERB's own question in the session's screen, with one Mac banner: `go` (its id and words) or Stop.
    #[allow(clippy::too_many_arguments)]
    async fn ask_in_session(
        &self,
        id: &str,
        task: &str,
        card_id: &str,
        question: String,
        go: (&str, &str),
        banner: &(dyn Fn(&str) -> String + Sync),
        journal: serde_json::Value,
    ) -> Option<bool> {
        let (tx, rx) = oneshot::channel();
        let folder = {
            let mut st = lock(&self.state);
            let e = st.sessions.get_mut(id)?;
            let d = e.drive.as_mut().filter(|d| d.task == task)?;
            d.card = Some(Card {
                id: card_id.to_owned(),
                session: id.to_owned(),
                kind: CardKind::Ferb,
                tool: String::new(),
                detail: String::new(),
                question: question.clone(),
                choices: vec![
                    Choice {
                        id: go.0.into(),
                        label: go.1.into(),
                    },
                    Choice {
                        id: STOP.into(),
                        label: "Stop".into(),
                    },
                ],
                asked_at: (self.seams.now)(),
                approval_id: String::new(),
            });
            d.answer = Some(tx);
            e.s.ferb_note = WAITING_FOR_ANSWER.into();
            folder_name(&e.s.cwd)
        };
        (self.seams.journal)("sessions.driver_asked", journal);
        (self.seams.publish)("sessions.card", json!({"session": id}));
        (self.seams.tell)(&format!("sessions.driver:{id}"), &banner(&folder));
        self.changed();
        let answer = tokio::select! {
            a = rx => a.ok(),
            _ = self.until_stopped(id, task) => None,
        };
        if let Some(e) = lock(&self.state).sessions.get_mut(id) {
            if let Some(d) = e.drive.as_mut().filter(|d| d.task == task) {
                d.card = None;
                d.answer = None;
            }
        }
        self.changed();
        answer.map(|a| a == go.0)
    }

    /// An answer to FERB's own question in the session: Some(result) when `card` is it, None when it isn't.
    pub(super) fn answer_drive_card(
        &self,
        id: &str,
        card: &str,
        choice: &str,
    ) -> Option<Result<(), Refused>> {
        let tx = {
            let mut st = lock(&self.state);
            let d = st.sessions.get_mut(id)?.drive.as_mut()?;
            if d.card.as_ref().map(|c| c.id.as_str()) != Some(card) {
                return None;
            }
            let offered = d
                .card
                .as_ref()
                .is_some_and(|c| c.choices.iter().any(|x| x.id == choice));
            if !offered {
                return Some(Err(no(
                    codes::CARD_GONE,
                    "That isn't one of the question's answers.",
                )));
            }
            d.card = None;
            d.answer.take()
        };
        (self.seams.journal)(
            "sessions.answered",
            json!({"session": id, "card": card, "choice": choice, "by": "owner"}),
        );
        if let Some(tx) = tx {
            let _ = tx.send(choice.to_owned());
        }
        (self.seams.publish)("sessions.card_gone", json!({"session": id, "card": card}));
        self.changed();
        Some(Ok(()))
    }

    /// What one step is shown: the goal, the session as code knows it, its recent exchanges (the newest reply in
    /// full), its open question, and what was decided before.
    fn step_of(&self, id: &str, goal: &str, round: u32, allowed: u32, decided: &[String]) -> Step {
        let st = lock(&self.state);
        let Some(e) = st.sessions.get(id) else {
            return Step::default();
        };
        let s = &e.s;
        let agent = match (s.agent, s.host.as_ref()) {
            (Agent::AppChat, Some(h)) => format!("{} chat", h.name),
            (Agent::Terminal, _) if !s.program.is_empty() => {
                format!("terminal running {}", s.program)
            }
            _ => s.agent.name().to_owned(),
        };
        let mut session = vec![format!("Agent: {agent}"), format!("Session id: {}", s.id)];
        if !s.cwd.is_empty() {
            session.push(format!("Folder: {}", s.cwd));
        }
        if let Some(h) = s.host.as_ref().filter(|h| !h.name.is_empty()) {
            session.push(format!("App: {}", h.name));
        }
        if !s.title.is_empty() {
            session.push(format!("Title: {}", s.title));
        }
        session.push(format!(
            "State: {}",
            match (s.status, s.waiting) {
                (_, Waiting::Question) => "asking a question",
                (Status::Idle, _) => "its turn is over",
                (Status::WaitingForYou, _) => "waiting for a reply",
                (Status::Working, _) => "working",
                (Status::Ended, _) => "ended",
            }
        ));
        session.push(format!(
            "Rounds so far: {} (FERB asks the owner whether to keep going after {allowed})",
            round - 1
        ));
        let marked = super::ferb_marks(e);
        let start = e.items.len().saturating_sub(RECENT_ITEMS);
        let newest = e
            .items
            .iter()
            .rposition(|i| i.kind == ItemKind::Assistant)
            .unwrap_or(usize::MAX);
        let name = s.agent.name();
        let mut lines = Vec::new();
        if start > 0 {
            lines.push(format!(
                "({start} earlier items: read them with agent_session.read if you need them)"
            ));
        }
        for (n, i) in e.items.iter().enumerate().skip(start) {
            let words = if n == newest {
                clip(&i.text, NEWEST_CHARS)
            } else {
                clip(&i.text, ITEM_CHARS)
            };
            lines.push(match i.kind {
                ItemKind::User if marked.contains(&n) => {
                    format!("FERB (you, for the owner): {words}")
                }
                ItemKind::User => format!("The owner: {words}"),
                ItemKind::Assistant => format!("{name}: {words}"),
                ItemKind::Tool => format!("{name} ran {}: {}", i.tool, one_line(&words)),
                ItemKind::Interrupted => "(the turn was stopped)".to_owned(),
                ItemKind::Error => format!("{name}'s error: {words}"),
            });
        }
        let asking = e
            .cards
            .iter()
            .find(|h| h.card.kind == CardKind::Question)
            .map(|h| {
                let labels: Vec<&str> = h.card.choices.iter().map(|c| c.label.as_str()).collect();
                format!("{}\nChoices: {}", h.card.question, labels.join(" | "))
            })
            .unwrap_or_default();
        let from = decided.len().saturating_sub(DECIDED_KEPT);
        Step {
            round,
            goal: goal.to_owned(),
            session: session.join("\n"),
            exchanges: lines.join("\n"),
            asking,
            decided: decided[from..].to_vec(),
        }
    }

    /// The step's message as the agent's open question's choice, when it asks one (by label or id, any case).
    fn choice_of(&self, id: &str, text: &str) -> Option<Option<Act>> {
        let st = lock(&self.state);
        let held = st
            .sessions
            .get(id)?
            .cards
            .iter()
            .find(|h| h.card.kind == CardKind::Question)?;
        let want = text.trim().to_lowercase();
        Some(
            held.card
                .choices
                .iter()
                .find(|c| c.label.trim().to_lowercase() == want || c.id.to_lowercase() == want)
                .map(|c| Act::Answer {
                    card: held.card.id.clone(),
                    choice: c.id.clone(),
                }),
        )
    }

    /// The drive itself: its task starts, then wait, step, act until it ends.
    async fn drive(self, id: String, goal: String, app: String) {
        let task = match self.seams.driver.begin(&id, &goal, &app).await {
            Ok(t) => t,
            Err(why) => {
                if let Some(e) = lock(&self.state).sessions.get_mut(&id) {
                    e.drive = None;
                    e.s.goal.clear();
                    e.s.delegate.clear();
                    e.s.ferb_note = format!("FERB couldn't start on it: “{}”", quote(&why, 300));
                }
                self.changed();
                return;
            }
        };
        let started = lock(&self.state)
            .sessions
            .get_mut(&id)
            .and_then(|e| {
                let d = e.drive.as_mut()?;
                d.task = task.clone();
                e.s.delegate = task.clone();
                e.s.ferb_note = format!("FERB is on it: “{}”", quote(&goal, 160));
                Some(())
            })
            .is_some();
        if !started {
            self.seams
                .driver
                .end(&task, "cancelled", "The session went before FERB started.")
                .await;
            return;
        }
        (self.seams.journal)(
            "sessions.driver_started",
            json!({"session": id, "task": task}),
        );
        self.changed();
        // how FERB's chat names the session when the drive ends (it may be gone by then)
        let who = self
            .session(&id)
            .map(|s| match (s.agent, s.host.as_ref()) {
                (Agent::AppChat, Some(h)) => format!("{} chat", h.name),
                _ => format!("{} session in {}", s.agent.name(), folder_name(&s.cwd)),
            })
            .unwrap_or_default();
        let mut rounds: u32 = 0;
        let mut allowed = ROUNDS_PER_ASK;
        let mut decided: Vec<String> = Vec::new();
        let mut stuck: u32 = 0;
        let end = loop {
            // 1. the agent's turn ends: code only, no time limit
            let s = match self.turn_over(&id, &task).await {
                Waited::Over(s) => s,
                Waited::Stopped => break End::Stopped,
                Waited::Gone => break End::Gone,
            };
            // 2. a permission prompt is the owner's (HAND-663): no step until he answers it
            if s.waiting == Waiting::Permission {
                self.drive_note(
                    &id,
                    &task,
                    format!(
                        "FERB is waiting for you to answer {}'s permission prompt.",
                        s.agent.name()
                    ),
                );
                match self.permission_answered(&id, &task).await {
                    Some(Waited::Stopped) => break End::Stopped,
                    Some(_) => break End::Gone,
                    None => continue,
                }
            }
            // 3. the rounds the owner set (HAND-708), then his say
            if rounds >= allowed {
                match self.keep_going(&id, &task, &goal, allowed).await {
                    Some(true) => allowed += ROUNDS_PER_ASK,
                    Some(false) => break End::Stopped,
                    None if self.drives(&id, &task) => break End::Stopped,
                    None => break End::Gone,
                }
                self.drive_note(
                    &id,
                    &task,
                    format!("FERB is on it: “{}”", quote(&goal, 160)),
                );
            }
            // 4. one fresh step (again, after the guard stopped one and the owner said keep trying)
            let step = self.step_of(&id, &goal, rounds + 1, allowed, &decided);
            let decision = tokio::select! {
                d = self.seams.driver.step(&task, step) => d,
                _ = self.until_stopped(&id, &task) => {
                    break if self.drives(&id, &task) { End::Stopped } else { End::Gone };
                }
            };
            if self.seams.driver.stopped(&task) {
                break End::Stopped;
            }
            if !self.drives(&id, &task) {
                break End::Gone;
            }
            let text = match decision {
                // decision: HAND-714 — the guard stopped the step: the owner is asked, never just ended
                Ok(Decision::Stuck(why)) => {
                    stuck += 1;
                    match self.keep_trying(&id, &task, &why, stuck).await {
                        Some(true) => {
                            self.seams.driver.keep_trying(&task);
                            self.drive_note(
                                &id,
                                &task,
                                format!("FERB is on it: “{}”", quote(&goal, 160)),
                            );
                            continue;
                        }
                        Some(false) => break End::Stopped,
                        None if self.drives(&id, &task) => break End::Stopped,
                        None => break End::Gone,
                    }
                }
                Err(why) => break End::Failed(why),
                Ok(Decision::Done(summary)) => break End::Done(summary),
                Ok(Decision::Ask(q)) => break End::Asked(q),
                Ok(Decision::Send(text)) => text,
            };
            // 5. code types it, through the gateway (GATE-585); the agent's question only with its own choices
            let act = match self.choice_of(&id, &text) {
                None => Act::Send(text.clone()),
                Some(Some(act)) => act,
                Some(None) => {
                    break End::Asked(format!(
                        "{} is asking something FERB couldn't answer with one of its choices.",
                        s.agent.name()
                    ))
                }
            };
            if let Act::Send(t) = &act {
                // decision: HAND-709 — what FERB sends shows in the session as FERB's
                if let Some(e) = lock(&self.state).sessions.get_mut(&id) {
                    e.ferb_sent.push(one_line(&self.redact(t)));
                    // the marks a session keeps: its latest messages from FERB
                    let over = e.ferb_sent.len().saturating_sub(FERB_SENT_KEPT);
                    e.ferb_sent.drain(..over);
                }
            }
            if let Err(why) = self.seams.driver.deliver(&task, &id, act).await {
                if self.seams.driver.stopped(&task) {
                    break End::Stopped;
                }
                break End::Failed(why);
            }
            rounds += 1;
            decided.push(format!(
                "Round {rounds}: sent “{}”",
                quote(&self.redact(&text), 300)
            ));
            self.drive_note(
                &id,
                &task,
                format!(
                    "FERB is on it: “{}” · {rounds} of {allowed} rounds",
                    quote(&goal, 160)
                ),
            );
        };
        self.drive_ended(&id, &task, &who, end, rounds).await;
    }

    /// How a drive ended: its task's outcome, the session's note, one line in FERB's chat unless the owner stopped it.
    async fn drive_ended(&self, id: &str, task: &str, who: &str, end: End, rounds: u32) {
        let r = |t: &str| quote(&self.redact(t), 300);
        let (status, summary, note, line) = match &end {
            End::Done(sum) => (
                "done",
                sum.clone(),
                format!("FERB finished: “{}” The session is yours again.", r(sum)),
                Some(format!("Done with your {who}: {}", r(sum))),
            ),
            End::Asked(q) => (
                "needs_user",
                q.clone(),
                format!(
                    "FERB stopped to ask you: “{}” The session is yours again; hand it back with your answer.",
                    r(q)
                ),
                Some(format!("Your {who} needs you: {}", r(q))),
            ),
            End::Failed(why) => (
                "failed",
                why.clone(),
                format!(
                    "FERB stopped without finishing: “{}” The session is yours again.",
                    r(why)
                ),
                Some(format!("I stopped keeping your {who} going: {}", r(why))),
            ),
            End::Stopped => (
                "cancelled",
                "You took it back.".to_owned(),
                "FERB stopped. The session is yours again.".to_owned(),
                None,
            ),
            End::Gone => (
                "failed",
                "The session ended.".to_owned(),
                String::new(),
                Some(format!("Your {who} ended before I finished.")),
            ),
        };
        self.seams.driver.end(task, status, &summary).await;
        let mine = {
            let mut st = lock(&self.state);
            match st.sessions.get_mut(id) {
                Some(e) if e.drive.as_ref().is_some_and(|d| d.task == task) => {
                    e.drive = None;
                    e.s.delegate.clear();
                    e.s.goal.clear();
                    if !note.is_empty() {
                        e.s.ferb_note = note;
                    }
                    true
                }
                _ => false,
            }
        };
        (self.seams.journal)(
            "sessions.driver_ended",
            json!({"session": id, "task": task, "status": status, "rounds": rounds}),
        );
        if mine || matches!(end, End::Gone) {
            if let Some(l) = line.filter(|_| !who.is_empty()) {
                self.seams
                    .driver
                    .say(&format!("sessions.driver:{task}"), &l);
            }
        }
        self.changed();
    }
}

/// A composition's driver, shared.
pub type DriverRef = Arc<dyn Driver>;
