//! The planner: ferb/orchestrator/supervisor.py (rust/PLAN.md stage 1, part 8), split by responsibility.
//!
//! Ported and checked against the Python build (rust/fixtures/supervisor, rust/fixtures/catalog,
//! rust/fixtures/daemon):
//! - [`prompts`]: every role's instructions (`_system`) and the worker's runtime-tools block.
//! - [`history`]: the planner's conversation (`_timeline`, `_self_post`, `_conversation_blocks`, `_recent_entries`).
//! - [`planner`]: the planner's context blocks around it (`_supervise`'s block list, the open questions, the latest
//!   result, the previous scheduled run, the front window) and its budgeted assembly.
//! - [`lines`] and [`words`]: the owner-facing lines composed in code and the fixed words.
//! - [`chat_line`]: a chat's messages in arrival order (ORCH-529).
//!
//! - [`flow`] (feature `store`): the forward path a chat message takes, from its arrival to its result: intake and
//!   the planner's decision, routing (a reply, a question, a task), the plan run, the worker and checker round and the
//!   task's end, on FERB's store, reaching the worker (ferb-worker), the runtimes and the rest of FERB through
//!   [`flow::Daemon`]: control of running work, suspend and resume, restart recovery, scheduled runs, waits for the
//!   owner, loops and the side entries. What is not ported (delegated conversations among it) answers [`NotPorted`]
//!   by its Python name.
//!
//! The composition is portable and touches no store; the flows read and write the Mac's SQLite store through
//! ferb-orchestrator's stores, as the Python supervisor does.

pub mod chat_line;
#[cfg(feature = "store")]
pub mod flow;
pub mod history;
pub mod intake;
pub mod lines;
pub mod planner;
pub mod prompts;
pub mod seams;
pub mod words;
pub mod worker_context;

/// A path of supervisor.py this crate doesn't run yet, by its Python name. Returned, never panicked, so a caller
/// can tell an unported path from a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotPorted(pub &'static str);

impl std::fmt::Display for NotPorted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not ported yet: supervisor.py {}", self.0)
    }
}

impl std::error::Error for NotPorted {}
