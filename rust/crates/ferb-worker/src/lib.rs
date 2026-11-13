//! The worker (rust/PLAN.md stage 1, part 8): `ferb/orchestrator/worker.py`, the supervisor's `WorkerRuns` mixin.
//! One worker attempt is one model run doing one step of a task: it gets a goal, the tools its run token's grant
//! offers, its context and an answer form; it calls tools through the gateway, may ask the owner, may be steered or
//! stopped mid-run, and hands back the report it wrote, or an outcome code wrote. The checker is the same run,
//! read-only. Every other model call (the planner's turn, a summary, attention, the question router) goes through
//! the same shared call with its silence watchdog.
//!
//! - [`worker`]: [`Worker`] with `run_worker` (worker.py:364), `run_call` (`_run`, :502) and the liveness watchdog.
//! - [`progress`]: the live lines code writes for a tool call (`tool_progress`, `_progress_line`, `_tell_tree`).
//! - [`runs`]: what the worker shares with the supervisor about running calls (worker.py's `_active`,
//!   `_worker_runs`, `_tree_of`, `_stalled`, `_retrying`, `_connection`, `_live`, `_last_steps`) and the
//!   connection notice.
//! - [`errors`]: the classes a run fails with and the owner's words for each (`reason`, `couldnt`, ...).
//! - [`ports`]: the seams (the host, the model runtime, the task tree).
//!
//! Checked against what the Python build does (ENG-3): `rust/fixtures/worker/make.py` runs the real worker with
//! recording fakes for every seam, and `rust/fixtures/daemon/chat_scenarios.json` holds the worker and checker
//! prompts of the recorded daemon runs.
//!
//! Portable: nothing here touches the store, a process or the Mac; time comes from the host's clock and the
//! watchdog sleeps on tokio's timer.

pub mod errors;
pub mod ports;
pub mod progress;
pub mod runs;
pub mod worker;

pub use errors::Error;
pub use runs::{ActiveRun, ConnectionNotice, Runs};
pub use worker::{
    deliver, reserve_tokens, Attempt, AttemptOutcome, Call, CallHooks, OutcomeKind, RunOptions,
    Worker, WorkerConfig, NO_TOOLS_LOADED,
};
