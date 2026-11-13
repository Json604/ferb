//! What group A's areas take from parts not ported yet or owned by group B. Each trait is shaped by the calls the
//! Python areas make, named at each method. They run on the daemon's loop thread, so their futures need not be
//! `Send`.

use std::collections::BTreeSet;
use std::path::Path;

use ferb_clock::schedule_store::ClaimedRun;
use ferb_clock::schedules::Schedule;
use serde_json::{Map, Value};

use super::LocalFuture;

/// `persona_scan(home, device, activity, exclude_apps)` (ferb/device/persona.py `discover_persona`): what FERB
/// gathers about this Mac. `activity`: owner-attributed focus counts per app.
pub type PersonaScan =
    dyn Fn(&Path, &str, Vec<Value>, BTreeSet<String>) -> Result<Value, String> + Send + Sync;

/// A scheduled run's failure (`except Exception as e` in `_run_schedule`): `f"{type(e).__name__}: {e}"` and the
/// owner's words for it (`worker.reason(e, redact)`).
#[derive(Clone, Debug, PartialEq)]
pub struct RunFailed {
    pub described: String,
    pub reason: String,
}

/// The supervisor and the task store (ferb-supervisor, in progress), as these areas call them.
pub trait Tasks: Send + Sync {
    /// `supervisor.schedule_running(sid)`: a run of this schedule (a resumed one included) is under way.
    fn schedule_running(&self, schedule_id: &str) -> bool;
    /// The first half of `supervisor.run_scheduled` (supervisor.py:1629): record the run's task on `conn`, inside the
    /// transaction that marks the run's timer handled (`Schedules::start_run`, the fix for the lost scheduled run).
    /// Returns what [`Tasks::run_scheduled`] needs to go on.
    fn create_scheduled(
        &self,
        conn: &rusqlite::Connection,
        run: &ClaimedRun,
        late: bool,
    ) -> Result<Value, String>;
    /// The rest of `supervisor.run_scheduled(s, late=, overdue_s=, retry=)`: the run's final event (`task_id`,
    /// `status`, `answer`, `cards`, `watch_met`, `suspended`...).
    fn run_scheduled(
        &self,
        schedule: Schedule,
        created: Value,
        late: bool,
        overdue_s: f64,
        retry: bool,
    ) -> LocalFuture<Result<Map<String, Value>, RunFailed>>;
    /// `_import_busy`: the owner's chat turn is under way (chat.py's `_turns`; its planner call is the critical path).
    fn turn_running(&self) -> bool;
    /// `supervisor.running_trees()` is not empty.
    fn running(&self) -> bool;
    /// `supervisor.mention_paused()` (ORCH-312): the day's paused-task mentions.
    fn mention_paused(&self) -> LocalFuture<()>;
    /// `self.tasks.column(task_id, column)`.
    fn task_column(&self, task_id: &str, column: &str) -> Option<String>;
    /// `self.tasks.set_status(task_id, status, outcome)`.
    fn set_task_status(&self, task_id: &str, status: &str, outcome: &Value) -> Result<(), String>;
    /// `supervisor.forget_pending(texts)`: a pending clarifying question holding a forgotten item's words.
    fn forget_pending(&self, texts: &[String]);
    /// `supervisor.conversations.forget_sources(refs)` and `daily_activity.forget_sources(trail)`: the copies of
    /// forgotten trail rows outside the memory tables.
    fn forget_trail_copies(&self, trail: &BTreeSet<String>);
}

/// The owner-facing side (ferb/daemon/areas/notify.py, group B).
pub trait Owner: Send + Sync {
    /// `_task_result(ev, shown, alert)`.
    fn task_result(&self, event: Map<String, Value>, shown: bool, alert: bool) -> LocalFuture<()>;
    /// `_notify(title, text, urgent=, category=, target=, ident=, card=)`; an error is swallowed by the caller.
    fn notify(
        &self,
        title: &str,
        text: &str,
        ident: &str,
        target: Value,
        card: Value,
    ) -> LocalFuture<Result<(), String>>;
    /// `_tell_owner(ident, refs, text, origin, cards=)`: a quiet notice in chat.
    fn tell_owner(
        &self,
        ident: &str,
        refs: Vec<String>,
        text: &str,
        origin: &str,
        cards: Option<Value>,
    ) -> LocalFuture<()>;
}

/// FERB's own questions outside tasks (the gateway's approvals, ferb-gateway in progress).
pub trait Questions: Send + Sync {
    /// `_ask_owner`'s `state.approvals.question(...)` with its card parts (`summary`, `detail`, `labels`), and the
    /// `approval.requested` event it publishes. Returns the approval's id.
    fn ask(&self, ask: Ask<'_>) -> Result<String, String>;
    /// `state.approvals.supersede(approval_id)` is not None.
    fn supersede(&self, approval_id: &str) -> bool;
    /// `state.approvals.get(approval_id)` is not None.
    fn waiting(&self, approval_id: &str) -> bool;
}

/// One of FERB's own questions (`_ask_owner`'s arguments, as worked out by it).
#[derive(Clone, Debug, PartialEq)]
pub struct Ask<'a> {
    /// `forget:memory`, `forget:learning`, `downloads:over_cap`, `browsing`.
    pub kind: &'a str,
    pub reason: &'a str,
    pub options: Option<&'a [String]>,
    pub request_id: &'a str,
    pub approval_id: Option<&'a str>,
    pub summary: Option<String>,
    pub detail: Option<String>,
    pub labels: (&'a str, &'a str),
}

/// Why a login-record change didn't happen.
#[derive(Clone, Debug, PartialEq)]
pub enum LoginError {
    /// `Refused`: an unreadable handle file, left as it is.
    Refused(String),
    /// Any other failure (a full disk): `f"{type(e).__name__}: {e}"`.
    Failed(String),
}

/// The owner's saved logins (`ferb.identity.owner_secrets.OwnerSecrets(home)`, behind the Keychain helper).
pub trait Logins: Send + Sync {
    /// `clear_usernames(sources)`: the places whose username came from one of these memory refs.
    fn clear_usernames(&self, sources: &BTreeSet<String>) -> Result<Vec<String>, LoginError>;
}
