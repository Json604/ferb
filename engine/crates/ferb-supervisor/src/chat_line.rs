//! One chat's messages in arrival order (supervisor.py:606 `_ChatLine`, ORCH-529, ORCH-529): a message sent while
//! the chat's turn is still thinking restarts it once, read together with it; any later message waits, and the
//! messages that waited are read together in the next turn. Code-enforced, never by a prompt.
//!
//! Python resolves each arrival's `role` future; here each change of role is returned as a [`Decision`] in the order
//! Python sets them, and a restart says the running think must be cancelled ([`Arrived::cancel_think`]).

use crate::words::MAX_RESTARTS;

/// What an arrival became: the lead of a turn, or read with the turn led by that request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    Lead,
    Merged(String),
}

/// One role set, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub request_id: String,
    pub role: Role,
}

/// A chat's turn while it holds the chat (supervisor.py:591 `_Run`, the part the line reads).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub lead: String,
    pub arrivals: Vec<String>,
    pub thinking: bool,
    pub restarts: usize,
    pub restart: bool,
}

/// What one arrival did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arrived {
    pub decisions: Vec<Decision>,
    /// The holder was thinking: its think task is cancelled (Python's `run.think.cancel()`), to restart with this
    /// message read too.
    pub cancel_think: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatLine {
    pub holder: Option<Run>,
    pub queued: Vec<String>,
}

impl ChatLine {
    pub fn new() -> ChatLine {
        ChatLine::default()
    }

    /// supervisor.py:615 `arrive`.
    pub fn arrive(&mut self, request_id: &str) -> Arrived {
        if let Some(run) = self.holder.as_mut() {
            if run.thinking && run.restarts < MAX_RESTARTS {
                run.restarts += 1;
                run.arrivals.push(request_id.to_string());
                run.restart = true;
                return Arrived {
                    decisions: vec![Decision {
                        request_id: request_id.to_string(),
                        role: Role::Merged(run.lead.clone()),
                    }],
                    cancel_think: true,
                };
            }
        }
        self.queued.push(request_id.to_string());
        Arrived {
            decisions: self.next(),
            cancel_think: false,
        }
    }

    /// supervisor.py:628 `withdraw`: a queued arrival leaves (its sender went away).
    pub fn withdraw(&mut self, request_id: &str) {
        if let Some(at) = self.queued.iter().position(|q| q == request_id) {
            self.queued.remove(at);
        }
    }

    /// supervisor.py:632 `release`: the turn led by `lead` lets the chat go.
    pub fn release(&mut self, lead: &str) -> Vec<Decision> {
        if self.holder.as_ref().is_some_and(|h| h.lead == lead) {
            self.holder = None;
            return self.next();
        }
        Vec::new()
    }

    /// The holder replied (`run.thinking = False`): later messages wait for the next turn.
    pub fn replied(&mut self) {
        if let Some(run) = self.holder.as_mut() {
            run.thinking = false;
        }
    }

    fn next(&mut self) -> Vec<Decision> {
        if self.holder.is_some() || self.queued.is_empty() {
            return Vec::new();
        }
        let batch = std::mem::take(&mut self.queued);
        let lead = batch[0].clone();
        self.holder = Some(Run {
            lead: lead.clone(),
            arrivals: batch.clone(),
            thinking: true,
            restarts: 0,
            restart: false,
        });
        let mut out = vec![Decision {
            request_id: lead.clone(),
            role: Role::Lead,
        }];
        out.extend(batch[1..].iter().map(|other| Decision {
            request_id: other.clone(),
            role: Role::Merged(lead.clone()),
        }));
        out
    }
}
