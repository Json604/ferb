//! What the gateway tells the supervisor about tool calls (supervisor.py:1063 `_Watch`, attached with
//! `gateway.attach(_Watch(self), ...)` at construction): a call is the worker's liveness, a call starting is its
//! task's live line (`tool_progress`), an owner wait pauses the stall timer, and a run that lacks the owner's latest
//! correction gets no new question (ORCH-538).
//!
//! The daemon attaches [`Supervisor::watch`] to its gateway (`Gateway::attach`) once, as Python does.

use std::sync::{Arc, Weak};

use ferb_core::interfaces::BoxFuture;
use ferb_rpc::auth::RunGrant;
use ferb_worker::ports::Grant;
use serde_json::{Map, Value};

use super::{Inner, Supervisor};

/// supervisor.py:1063 `_Watch`: the gateway's `Observer`, held weakly (the daemon's gateway outlives no supervisor).
pub struct Watch {
    sup: Weak<Inner>,
}

/// The gateway's grant as the worker reads it.
pub fn worker_grant(grant: &RunGrant) -> Grant {
    Grant {
        task_id: grant.task_id.clone(),
        worker_id: grant.worker_id.clone(),
        scopes: grant.scopes.clone(),
        identity: match grant.identity {
            ferb_rpc::auth::Identity::Owner => ferb_core::types::Identity::Owner,
            ferb_rpc::auth::Identity::Ferb => ferb_core::types::Identity::Ferb,
        },
        observe_only: grant.observe_only,
    }
}

impl Supervisor {
    /// The observer the daemon attaches to its gateway (`gateway.attach(_Watch(self), ...)`).
    pub fn watch(&self) -> Arc<Watch> {
        Arc::new(Watch {
            sup: Arc::downgrade(&self.inner),
        })
    }
}

impl ferb_gateway::seams::Observer for Watch {
    /// `tool_call`: a call is the worker's liveness.
    fn tool_call(&self, grant: &RunGrant, _done: bool) {
        if let Some(sup) = self.sup.upgrade() {
            sup.activity.beat(&grant.worker_id);
        }
    }

    /// `tool_started`: the task's live line (worker.py:315 `tool_progress`). Python lets an exception out of the
    /// observer into the gateway's call; ferb-worker's `tool_progress` already journals a line it couldn't build, and
    /// what is left (the redactor failing) has nowhere to go here, as the gateway ignores an observer's result.
    fn tool_started(
        &self,
        grant: &RunGrant,
        name: &str,
        args: &Map<String, Value>,
        kind: &str,
        site: Option<&str>,
    ) {
        let Some(sup) = self.sup.upgrade() else {
            return;
        };
        if let Some(worker) = sup.worker.get() {
            let _ = worker.tool_progress(&worker_grant(grant), name, args, kind, site);
        }
    }

    /// `waiting`: an owner wait pauses the stall timer.
    fn waiting(&self, worker: &str, on: bool) {
        if let Some(sup) = self.sup.upgrade() {
            sup.activity.waiting(worker, on);
        }
    }

    /// supervisor.py:1791 `correction_pending` (ORCH-538): whether the owner's latest correction to this worker's task
    /// hasn't reached the worker's run. A correction being applied now (its tree's steering lock held) is waited for.
    fn correction_pending<'a>(&'a self, grant: &'a RunGrant) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let Some(sup) = self.sup.upgrade() else {
                return false;
            };
            sup.correction_pending(&grant.worker_id).await
        })
    }
}

impl Inner {
    /// supervisor.py:1791 `correction_pending`.
    pub async fn correction_pending(&self, worker_id: &str) -> bool {
        let Some(run) = self.runs.worker_run(worker_id) else {
            return false;
        };
        let tree = self.live().tree_of.get(&run.handle.task_id).cloned();
        let Some(tree) = tree else {
            return false;
        };
        // the tree's steering state, held while a correction is applied (`async with state.lock`)
        let steering = tree.steering.lock().await;
        let n = steering.messages.len();
        steering.delivered.get(&run.handle.id).copied().unwrap_or(n) < n
    }
}
