//! What belongs to other parts of FERB and reaches the gateway through a named trait (a seam). Each default is what
//! Python does when the dependency isn't wired (`Observer`'s no-ops, no hooks, no snapshots), never a silent pass of
//! a safety check: the safety stores (confirmations, destinations, spans, the protection lists, recovery copies, the
//! ledger) have no default and must be given.

use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::ActionSpec;
use ferb_rpc::auth::RunGrant;
use serde_json::{Map, Value};

use crate::tool::{Args, Tool, ToolOutput};
use crate::words::LedgerRow;

/// Why the ledger refuses a repeat (P19): an identical action is unknown (reconcile it first) or still in flight.
#[derive(Debug, Clone, PartialEq)]
pub struct DuplicateUnresolved {
    pub message: String,
    pub intent_id: String,
    /// intended, executing or unknown.
    pub state: String,
}

/// A ledger transition that isn't allowed (`BadTransition`).
#[derive(Debug, Clone, PartialEq)]
pub struct BadTransition(pub String);

/// The action ledger (`ferb/orchestrator/ledger.py`): ported by the orchestration part (rust/crates/
/// ferb-orchestrator); the gateway uses exactly these operations. A failed write must stop the action (the row is
/// written before a consequential action runs).
pub trait Ledger: Send + Sync {
    /// `intend(task, capability, target, payload_hash, reconcile_first=True)`: the new row's id, or the refusal.
    fn intend(
        &self,
        task_id: &str,
        capability: &str,
        target: &Map<String, Value>,
        payload_hash: &str,
    ) -> Result<String, DuplicateUnresolved>;
    /// `mark(id, state, evidence)`.
    fn mark(&self, intent_id: &str, state: &str, evidence: &[String]) -> Result<(), BadTransition>;
    /// `refusal(task, capability, hash)`: why `intend` would refuse now, read only.
    fn refusal(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Option<DuplicateUnresolved>;
    /// `approved_retry(task, capability, hash)`: the unknown attempt whose one more try this action would use.
    fn approved_retry(&self, task_id: &str, capability: &str, payload_hash: &str)
        -> Option<String>;
    /// `attempts(id)`: every attempt at the same action, oldest first.
    fn attempts(&self, intent_id: &str) -> Vec<LedgerRow>;
    /// `approve_retry(id, ask_span)`.
    fn approve_retry(&self, intent_id: &str, ask_span: &str) -> Result<(), BadTransition>;
    /// `get(id)`; None for Python's `KeyError`.
    fn get(&self, intent_id: &str) -> Option<LedgerRow>;
}

/// The orchestrator's observer of calls and owner waits (`Observer`, `gateway.py:504`); no-ops by default.
pub trait Observer: Send + Sync {
    fn tool_call(&self, _grant: &RunGrant, _done: bool) {}
    fn tool_started(
        &self,
        _grant: &RunGrant,
        _name: &str,
        _args: &Map<String, Value>,
        _kind: &str,
        _site: Option<&str>,
    ) {
    }
    fn waiting(&self, _worker: &str, _on: bool) {}
    /// ORCH-538: the owner's latest correction hasn't reached this worker's run yet.
    fn correction_pending<'a>(&'a self, _grant: &'a RunGrant) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }
    /// The delegated-conversation guard (`conversation_guard`): a refusal, else None.
    fn conversation_guard(
        &self,
        _grant: &RunGrant,
        _name: &str,
        _tool: &dyn Tool,
        _spec: &ActionSpec,
    ) -> Option<String> {
        None
    }
}

/// The default observer: nothing installed yet.
pub struct NoObserver;
impl Observer for NoObserver {}

/// A worker's question routed to the planner (`route_question`): `{"ask_owner", "answer", "question"}`.
pub trait QuestionRouter: Send + Sync {
    fn route<'a>(
        &'a self,
        task_id: &'a str,
        question: &'a str,
        options: &'a [String],
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>>;
}

/// What only records or shows around a call (`ferb/daemon/call_hooks.py`): never deciding, never failing a call.
/// The defaults are what Python does with no sentinel, evidence store, preview or shadow advice wired.
pub trait CallHooks: Send + Sync {
    fn preview(
        &self,
        _tool: &dyn Tool,
        _args: &Args,
        _grant: &RunGrant,
        _locks: &[String],
        _observed: Option<&Map<String, Value>>,
    ) {
    }
    fn point<'a>(
        &'a self,
        _tool: &'a dyn Tool,
        _args: &'a Args,
        _grant: &'a RunGrant,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
    fn screen_before<'a>(
        &'a self,
        _grant: &'a RunGrant,
        _span_id: &'a str,
        _name: &'a str,
        _arguments: &'a Map<String, Value>,
        _tool: &'a dyn Tool,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
    /// What FERB could have changed on the screen during the call ({} when not sampled).
    fn screen_after<'a>(&'a self, _span_id: &'a str) -> BoxFuture<'a, Map<String, Value>> {
        Box::pin(async { Map::new() })
    }
    /// `_screen_told(screen)` (HAND-212): `{"screen_changed_during_call": ...}` when FERB changed the screen.
    fn told(&self, _screen: &Map<String, Value>) -> Map<String, Value> {
        Map::new()
    }
    #[allow(clippy::too_many_arguments)]
    fn shadow(
        &self,
        _tool: &str,
        _spec: &ActionSpec,
        _target: &Map<String, Value>,
        _ids: &[String],
        _task: &str,
        _words: &str,
        _span: &str,
    ) {
    }
    /// L39: copies of the files a call can change, before it runs; its record's counts.
    fn watch_changes<'a>(
        &'a self,
        _tool: &'a dyn Tool,
        _args: &'a Args,
        _spec: &'a ActionSpec,
        _grant: &'a RunGrant,
        _tree: &'a str,
    ) -> BoxFuture<'a, Map<String, Value>> {
        Box::pin(async { Map::new() })
    }
    /// D6/D26: the working app's first picture (the result changed when one is added).
    fn initial_image<'a>(
        &'a self,
        _name: &'a str,
        _tool: &'a dyn Tool,
        _args: &'a Args,
        _spec: &'a ActionSpec,
        _grant: &'a RunGrant,
        out: ToolOutput,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move { out })
    }
    /// L13: the image through the image sanitizer; facts when that fails.
    fn redact_image<'a>(
        &'a self,
        out: ToolOutput,
    ) -> BoxFuture<'a, (ToolOutput, Map<String, Value>)> {
        Box::pin(async move { (out, Map::new()) })
    }
    /// V1: the image kept as evidence; `{"evidence": id}` or `{"evidence_error": ...}`.
    fn keep_image<'a>(
        &'a self,
        _name: &'a str,
        _grant: &'a RunGrant,
        _span_id: &'a str,
        _content: &'a Map<String, Value>,
        _image: Option<&'a [u8]>,
        _origins: &'a BTreeSet<String>,
    ) -> BoxFuture<'a, Map<String, Value>> {
        Box::pin(async { Map::new() })
    }
}

/// No hooks wired.
pub struct NoHooks;
impl CallHooks for NoHooks {}

/// S1: the recovery copies and records around a call that changes files (`ferb/act/fs/recovery.py`). Python's
/// `Recovery` is never None in the daemon (the gateway refuses to start without it).
pub trait Recovery: Send + Sync {
    /// A copy of the file before a change: its record id, None when there is no file to copy (a new file); Err(why)
    /// when no copy could be made (the call must not run).
    fn preserve<'a>(
        &'a self,
        path: &'a str,
        reason: &'a str,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>>;
    /// The copy is dropped when the file is still identical to it (nothing changed); true when dropped.
    fn discard_if_unchanged<'a>(
        &'a self,
        record: &'a str,
        path: &'a str,
    ) -> BoxFuture<'a, Result<bool, String>>;
    /// The file as FERB left it, recorded.
    fn mark_written<'a>(
        &'a self,
        record: &'a str,
        path: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// A trash's record: where the item was, where it went, the identity the tool verified.
    fn record_trash(
        &self,
        original: &str,
        trashed_to: Option<&str>,
        device: Option<i64>,
        inode: Option<i64>,
        reason: &str,
        task_id: &str,
    ) -> Result<String, String>;
}

/// S1: a local snapshot before a permanent delete or a shell command (`recovery.Snapshots`); None in the daemon
/// when snapshots are off.
pub trait Snapshots: Send + Sync {
    fn request<'a>(&'a self, task_id: &'a str, why: &'a str) -> BoxFuture<'a, Map<String, Value>>;
}

/// S4 (c): a folder's count before a permanent delete (`ferb/act/fs/usage.py` `usage(path, closed, True)`):
/// `{"complete", "files", "bytes", "sample"...}`.
pub trait Usage: Send + Sync {
    fn count<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Map<String, Value>>;
}

/// What `progress.describe` says of an action in a few plain words (orchestration's), for a retry question.
pub trait Describe: Send + Sync {
    fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        kind: &str,
        site: Option<&str>,
        app: Option<&str>,
    ) -> String;
    /// `_app_name`: a Mac app by the name the Mac knows it by, "an app" when none matches.
    fn app_name(&self, app: &str) -> String;
}

/// Shared handle to a seam.
pub type Shared<T> = Arc<T>;
