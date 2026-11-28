//! ORCH-760 (ORCH-719 item 4, `kb/plans/context.md` step 4): the fresh run.
//!
//! When a worker run's conversation passes [`AT_MOST`] tokens, or [`WINDOW_PERCENT`]% of a smaller model's window
//! (the owner's numbers), FERB ends the run and starts a new run of the same attempt with a hand-off note built by
//! code from its records, at most [`NOTE_TOKENS`] tokens. The size is what the run's runtime last reported
//! (`RunControl::context_tokens`, the same shape for Codex, Claude Code and FERB's API loop); the decision is the
//! worker's, the same for every runtime. Never while a question, confirmation or sealed fill is open
//! (`WorkerHost::fresh_run_held`); a run ends this way only after [`MIN_CALLS`] tool calls of its own; it is not one
//! of the step's attempts; past [`ALARM_PAST`] in one attempt FERB journals an alarm and goes on.

use ferb_core::types::ModelLimits;
use serde_json::{json, Value};

/// A conversation past this many tokens is ended for a fresh run (owner, ORCH-719).
pub const AT_MOST: i64 = 200_000;
/// ... or past this share of a smaller model's window (owner, ORCH-719).
pub const WINDOW_PERCENT: i64 = 70;
/// The tool calls a run makes before FERB may end it for a fresh run (owner, ORCH-719: at least 10 between).
pub const MIN_CALLS: u64 = 10;
/// More fresh runs than this in one attempt raise the alarm (owner, ORCH-719).
pub const ALARM_PAST: u32 = 5;
/// The hand-off note's size, o200k tokens (owner, ORCH-719).
pub const NOTE_TOKENS: usize = 1_500;

/// The size, in tokens, at which a run on a model with these limits is ended for a fresh run: 200k, or 70% of a
/// window too small for 200k to be 70% of it.
pub fn threshold(limits: &ModelLimits) -> i64 {
    AT_MOST.min(limits.context_window.saturating_mul(WINDOW_PERCENT) / 100)
}

/// What a run ended for a fresh one had reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreshEnd {
    /// The conversation's size as the runtime last reported it.
    pub context_tokens: i64,
    /// [`threshold`] for the run's model.
    pub threshold: i64,
    pub context_window: i64,
    /// The run's own tool calls the gateway admitted.
    pub calls: u64,
    /// The run's latest words (ORCH-640, redacted), for the note; not in the record.
    pub last_words: Option<String>,
}

impl FreshEnd {
    pub fn as_json(&self) -> Value {
        json!({"context_tokens": self.context_tokens, "threshold": self.threshold,
               "context_window": self.context_window, "calls": self.calls})
    }
}
