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
                    task_id: "",
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
            ap.clone()
        };
        let mut event = Map::new();
        event.insert("approval_id".into(), json!(ap.id));
        event.insert("capability".into(), json!(ap.capability));
        event.insert("kind".into(), json!(ap.kind));
        event.insert("reason".into(), json!(ap.reason));
        event.insert("task_id".into(), Value::Null);
        event.insert("options".into(), json!(ap.options));
        if !ap.summary.is_empty() {
            event.insert("summary".into(), json!(ap.summary));
        }
        event.extend(card_parts(&ap));
        self.publish("approval.requested", event);
        ap
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
