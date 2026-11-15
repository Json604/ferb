//! FERB's own questions outside any task (Python: `areas/memory.py` `_ask_owner` and `_close_memory_questions` on
//! `state.approvals`): a forget's Yes / No, the downloads cap, which profile FERB browses in. The daemon's areas ask
//! through these calls; the question store is the gateway's, so the owner's answer (`resolve`, chat or the notch's
//! buttons) reaches them the same way as a task's.

use serde_json::{json, Map, Value};

use super::{lock, CallError, Cancel, Gateway};
use crate::questions::{card_parts, Approval, Asker, State};

/// One of FERB's own questions (`_ask_owner`'s `ActionRequest(capability, "owner_question", Identity.OWNER, ...)`
/// with its card).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwnQuestion {
    /// `MEMORY_QUESTIONS[kind][0]`: `memory.forget`, `downloads.delete`, `browsing.choose`, ...
    pub capability: String,
    /// The question's words (`reason`).
    pub reason: String,
    /// The buttons' answers.
    pub options: Vec<String>,
    pub request_id: String,
    /// The id of a question asked again after a restart (`_restore_memory_questions`), else a new one.
    pub approval_id: Option<String>,
    /// GATE-566: the card's first line and the rest under it (`reason.partition("\n")`), when the caller split them.
    pub summary: Option<String>,
    pub detail: Option<String>,
    /// The buttons' action words (`OWN_LABELS`).
    pub labels: Vec<String>,
    /// A task's question (the supervisor's `_ask_and_wait`: a plan's ask step, a check-in, a runtime offer, a paused
    /// loop): the task that asks ("" for none: the event's `task_id` is then null, as for FERB's own questions).
    pub task_id: String,
    /// ORCH-319: the card row the question sits in; "" for none.
    pub part: String,
    /// ORCH-315: the runtime a Yes switches to (the offer's `runtime_switch`).
    pub runtime_switch: Option<Map<String, Value>>,
    /// ORCH-536: FERB's own option question takes only one of its options.
    pub options_only: bool,
    /// D33: a model-written question, tagged as the task's.
    pub from_task: bool,
    /// GATE-510: the card's box takes the owner's words as a correction of the task.
    pub corrects: bool,
    /// GATE-566: the exact words the card shows under its summary, when code wrote them.
    pub exact: String,
    /// What is published: [`OwnEvent::Publish`] (the default) publishes `approval.requested` as FERB's own questions
    /// always have; [`OwnEvent::Extend`] adds fields to it; [`OwnEvent::Quiet`] publishes nothing (the caller
    /// publishes its own, as the supervisor does with `question_kind` and the offer).
    pub event: OwnEvent,
}

/// What [`Gateway::ask_own`] publishes for a question.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum OwnEvent {
    /// `approval.requested` as FERB's own questions outside a task publish it.
    #[default]
    Publish,
    /// The same, with these fields added (or replaced) last.
    Extend(Map<String, Value>),
    /// Nothing: the caller publishes its own event.
    Quiet,
}

impl Gateway {
    /// `_ask_owner`: the question is added to the store with its card, and `approval.requested` is published as
    /// Python's daemon publishes it (`task_id` None, `summary` when set, the card's parts). Returns the question as
    /// it stands (its `id` is what the answer names).
    pub fn ask_own(&self, q: &OwnQuestion) -> Approval {
        let ap = {
            let mut i = lock(&self.inner);
            let ap = i.approvals.question(
                &Asker {
                    capability: &q.capability,
                    task_id: &q.task_id,
                    request_id: &q.request_id,
                },
                &q.reason,
                &q.options,
                q.approval_id.as_deref(),
            );
            if let Some(summary) = &q.summary {
                ap.summary = summary.clone();
            }
            if let Some(detail) = &q.detail {
                ap.detail = detail.clone();
            }
            ap.labels = q.labels.clone();
            ap.part = q.part.clone();
            ap.runtime_switch = q.runtime_switch.clone();
            ap.options_only = q.options_only;
            ap.from_task = q.from_task;
            ap.corrects = q.corrects;
            ap.exact = q.exact.clone();
            ap.clone()
        };
        if q.event == OwnEvent::Quiet {
            return ap;
        }
        let mut event = Map::new();
        event.insert("approval_id".into(), json!(ap.id));
        event.insert("capability".into(), json!(ap.capability));
        event.insert("kind".into(), json!(ap.kind));
        event.insert("reason".into(), json!(ap.reason));
        event.insert(
            "task_id".into(),
            if ap.task_id.is_empty() {
                Value::Null
            } else {
                json!(ap.task_id)
            },
        );
        event.insert("options".into(), json!(ap.options));
        if !ap.summary.is_empty() {
            event.insert("summary".into(), json!(ap.summary));
        }
        event.extend(card_parts(&ap));
        if let OwnEvent::Extend(more) = &q.event {
            event.extend(more.clone());
        }
        self.publish("approval.requested", event);
        ap
    }

    /// `ap.hidden = hidden` after an answer (areas/chat.py): the kinds of secret FERB hid in the owner's typed
    /// answer, by name, so the waiting step tells its worker. False when the question is unknown.
    pub fn set_hidden(&self, approval_id: &str, hidden: &[String]) -> bool {
        let mut i = lock(&self.inner);
        match i.approvals.get_mut(approval_id) {
            Some(ap) => {
                ap.hidden = hidden.to_vec();
                true
            }
            None => false,
        }
    }

    /// `card_parts(ap) | card_flags(ap)`: what a question's card shows besides its words; empty when unknown.
    pub fn question_card(&self, approval_id: &str) -> Map<String, Value> {
        let i = lock(&self.inner);
        let Some(ap) = i.approvals.get(approval_id) else {
            return Map::new();
        };
        let mut m = card_parts(ap);
        m.extend(crate::questions::card_flags(ap));
        m
    }

    /// `state.approvals.get(id) is not None`: the question is still kept (open, or answered and not yet forgotten).
    pub fn waiting(&self, approval_id: &str) -> bool {
        lock(&self.inner).approvals.get(approval_id).is_some()
    }

    /// `_close_memory_questions`' close of one question: set aside (superseded, never a No) and, when it was still
    /// open, `approval.resolved` published with state `superseded`. True when it was open.
    pub fn close_own(&self, approval_id: &str) -> bool {
        let closed = self.supersede(approval_id).is_some();
        if closed {
            let mut event = Map::new();
            event.insert("approval_id".into(), json!(approval_id));
            event.insert("state".into(), json!("superseded"));
            self.publish("approval.resolved", event);
        }
        closed
    }

    /// The owner's answer to one of FERB's own questions, once given: the question as answered (granted, denied or
    /// set aside); `Ok(None)` when it went before an answer (its request was forgotten). `cancel` ends the wait
    /// (`CallError::Cancelled`); the question stays open.
    pub async fn await_own(
        &self,
        approval_id: &str,
        cancel: &Cancel,
    ) -> Result<Option<Approval>, CallError> {
        loop {
            let n = self.answers.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            match lock(&self.inner).approvals.get(approval_id) {
                None => return Ok(None),
                Some(ap) if ap.state != State::Pending => return Ok(Some(ap.clone())),
                Some(_) => {}
            }
            cancel.guard(n).await?;
        }
    }
}
