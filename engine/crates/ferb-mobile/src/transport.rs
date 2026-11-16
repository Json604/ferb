//! How the core reaches the Mac: a trait, so the tests use a fake Mac built from the fixtures and the apps plug the
//! real link (ferb-link's `LinkClient`, slice S8/S9; `door.rs` adapts its JSON door). JSON travels as text, so the
//! trait is the same at an FFI boundary.
//!
//! Every call is bounded by the transport itself (the link answers `mac_unreachable` after its call deadline); the
//! core never waits on the network with its state locked.

use std::sync::Arc;

/// A failure the link or the Mac reported, with the link's stable code (ferb-link `errors.rs`, CONTRACT §6). The
/// core matches on `code`, never on the words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFailure {
    pub code: String,
    pub message: String,
    pub allow: Option<String>,
    pub retry_after_s: Option<u64>,
}

impl LinkFailure {
    pub fn new(code: &str, message: impl Into<String>) -> LinkFailure {
        LinkFailure {
            code: code.into(),
            message: message.into(),
            allow: None,
            retry_after_s: None,
        }
    }

    /// From the link's error JSON (`{"code", "message", "allow"?, "retry_after_s"?}`).
    pub fn from_json(v: &serde_json::Value) -> LinkFailure {
        LinkFailure {
            code: v["code"]
                .as_str()
                .unwrap_or(codes::LINK_INTERNAL_ERROR)
                .to_string(),
            message: v["message"].as_str().unwrap_or_default().to_string(),
            allow: v["allow"].as_str().map(str::to_string),
            retry_after_s: v["retry_after_s"].as_u64(),
        }
    }

    /// What this failure means for a message waiting to go (the outbox) or a read.
    pub fn kind(&self) -> FailureKind {
        codes::kind(&self.code)
    }
}

impl std::fmt::Display for LinkFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// The link's codes the core acts on (the same strings as ferb-link `errors.rs`; this crate does not depend on it).
pub mod codes {
    use super::FailureKind;

    pub const NEEDS_BETTER_PATH: &str = "needs_direct_or_own_relay";
    pub const TRANSFER_TOO_LARGE: &str = "transfer_too_large";
    pub const MAC_UNREACHABLE: &str = "mac_unreachable";
    pub const LINK_CLOSED: &str = "link_closed";
    pub const LINK_INTERNAL_ERROR: &str = "link_internal_error";
    pub const FERB_NOT_RUNNING: &str = "ferb_not_running";
    pub const FERB_ERROR: &str = "ferb_error";
    pub const LOCKED: &str = "locked";
    pub const UNLOCK_REQUIRED: &str = "unlock_required";
    pub const UNLOCK_INVALID: &str = "unlock_invalid";
    pub const FRESH_UNLOCK: &str = "fresh_unlock_required";
    pub const QUESTION_NOT_WAITING: &str = "question_not_waiting";

    /// The link could not carry it, or FERB could not take it now: nothing is known to have been done, or the
    /// outcome is unknown. A message stays waiting and goes again (with the same submission id) when the link returns.
    const TRANSIENT: [&str; 17] = [
        MAC_UNREACHABLE,
        LINK_CLOSED,
        LINK_INTERNAL_ERROR,
        FERB_NOT_RUNNING,
        "relay_unreachable",
        "own_relay_unreachable",
        "own_relay_dns",
        "own_relay_tls",
        "own_relay_not_a_relay",
        "own_relay_access_denied",
        "own_relay_version",
        "own_relay_bad_url",
        "stalled",
        "too_many_requests",
        "relay_rate_limited",
        "daily_relay_cap",
        "bulk_budget_exhausted",
    ];
    /// The phone must be unlocked toward the Mac first (PHN-125): the message waits for the owner's unlock.
    const UNLOCK: [&str; 4] = [LOCKED, UNLOCK_REQUIRED, UNLOCK_INVALID, FRESH_UNLOCK];

    pub fn kind(code: &str) -> FailureKind {
        if TRANSIENT.contains(&code) {
            FailureKind::Transient
        } else if UNLOCK.contains(&code) {
            FailureKind::NeedsUnlock
        } else if code == NEEDS_BETTER_PATH {
            FailureKind::NeedsBetterPath
        } else {
            FailureKind::Refused
        }
    }
}

/// What a failure means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The link could not carry it now; try again when it returns.
    Transient,
    /// The phone must be unlocked toward the Mac.
    NeedsUnlock,
    /// The public relay refuses it (PHN-444): available when connected directly or through the owner's relay.
    NeedsBetterPath,
    /// The Mac (or the link's rules) refused this very request: trying again changes nothing.
    Refused,
}

/// Frames of a running `chat.entries.watch`, delivered on the transport's own thread.
pub trait WatchSink: Send + Sync {
    /// One frame's JSON (`change`, `caught_up`, `live`, `behind`, `reset_required`, `cursor_ahead`).
    fn frame(&self, frame_json: String);
    /// The stream ended: by itself (None) or with a failure.
    fn ended(&self, failure: Option<LinkFailure>);
}

/// A running watch; `close` stops it (idempotent, bounded).
pub trait WatchHandle: Send + Sync {
    fn close(&self);
}

pub trait Transport: Send + Sync {
    /// One call (`chat.entries.page`, `changes`, `get`, `mark`, `chat.approve`, `chat.answer`, `chat.correct`,
    /// `task.stop`, `link.challenge`): the reply's JSON, or the failure. `proof_json`: `{payload, sig}` for a call the
    /// Mac checks against the phone's key.
    fn call(
        &self,
        method: &str,
        params_json: &str,
        proof_json: Option<&str>,
    ) -> Result<String, LinkFailure>;

    /// `chat.send`: the stream's `ack` frame as JSON once it arrives (the rest of the stream reaches the phone as
    /// entries and live frames through the watch), or the refusal.
    fn send(&self, params_json: &str) -> Result<String, LinkFailure>;

    /// Starts `chat.entries.watch` with these params; frames go to `sink` until it ends or the handle is closed.
    fn watch(
        &self,
        params_json: &str,
        sink: Arc<dyn WatchSink>,
    ) -> Result<Box<dyn WatchHandle>, LinkFailure>;
}
