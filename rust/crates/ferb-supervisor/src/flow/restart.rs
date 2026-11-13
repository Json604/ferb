//! Restart recovery and suspension (supervisor.py:2291 `suspend_plans`, :2279 `cancel_all` at shutdown, :2324
//! `resume_plans`, :2262 `_restart_safe`, :2255 `update_blockers`; ferb/daemon/main.py:418-430 and :469, what a start
//! recovers; :522-523, what a shutdown does).
//!
//! A clean shutdown suspends every running tree (its plan row `suspended`, its stop words SHUT_DOWN, nothing of the
//! task-end clean-up), and the next start marks what was running `interrupted` (tree.py `interrupt_running`), tells
//! the owner of each tree that won't resume (`task_results.interrupted`, its episode), marks every intent a restart
//! cut off as of unknown outcome (the ledger's `mark_unknown_after_restart`), then resumes each tree whose plan didn't
//! end (`resume_plans`): completed steps are never replayed (the plan store keeps them done), a step that waited for
//! the owner asks again (`restored_wait`), the app hears through `task.resumed_event` and `chat.status`.
//!
//! The resumed run itself is `run_tree(resume_id=..., restored=True, ...)`; the forward path's `run_tree` doesn't take
//! a tree to resume yet, so `resume_plans` restores each tree's request and turn and then answers `NotPorted` for it
//! (journaled `plan.resume_failed`, as Python journals a resume that raised).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_sanitize::origin;
use serde_json::{json, Value};

use super::{Inner, Request, Supervisor, Turn};
use crate::NotPorted;

impl Supervisor {
    /// The daemon's shutdown (main.py:522-523): `suspend_plans()` then `cancel_all()`.
    pub fn shut_down(&self) {
        self.inner.suspend_now();
        self.inner.cancel_all();
    }

    /// What a start recovers before it serves (main.py:418-430, 469); the trees to resume (`plans.pending()`).
    pub fn recover_after_restart(&self) -> Vec<String> {
        self.inner.recover_after_restart()
    }

    /// supervisor.py:2324 `resume_plans`: each tree a shutdown or crash cut off runs again in place.
    pub async fn resume_saved_plans(&self) -> Vec<String> {
        self.inner.resume_saved_plans().await
    }

    /// supervisor.py:2255 `update_blockers`: the running trees an update must wait for.
    pub fn update_blockers(&self) -> Vec<Value> {
        self.inner.update_blockers()
    }
}

impl Inner {
    /// supervisor.py:2291 `suspend_plans`: a daemon shutdown differs from the owner's permanent stop.
    pub fn suspend_now(&self) {
        let roots: Vec<String> = {
            let mut live = self.live();
            live.suspending = true;
            let mut roots: Vec<String> = live
                .tree_of
                .iter()
                .filter(|(k, t)| **k == t.id)
                .map(|(k, _)| k.clone())
                .collect();
            roots.sort();
            roots
        };
        for tree_id in roots {
            let _ = self.plans(|p| p.set_status(&tree_id, "suspended"));
        }
    }

    /// main.py:418-430 and :469: a restart's recovery. What was running didn't finish (`interrupted`); a tree whose
    /// plan didn't end resumes by itself, so only the others are told ("pick it up again?") and get their episode;
    /// every intent a restart cut off is of unknown outcome. The trees to resume, in the plan store's order.
    pub fn recover_after_restart(&self) -> Vec<String> {
        let now = self.now_iso();
        let interrupted: Vec<String> = {
            let db = self.db();
            let n = now.clone();
            let tasks =
                ferb_orchestrator::tree::TaskTree::new(Some(&db), Box::new(move || n.clone()));
            tasks.interrupt_running().unwrap_or_default()
        };
        let resuming: Vec<String> = self.plans(|p| p.pending()).unwrap_or_default();
        if !interrupted.is_empty() {
            self.journal(
                "tasks.interrupted_after_restart",
                json!({"task_ids": interrupted}),
                None,
            );
            let roots: BTreeSet<String> = interrupted
                .iter()
                .map(|t| t.split('.').next().unwrap_or("").to_string())
                .filter(|t| !resuming.contains(t))
                .collect();
            for tid in roots {
                let goal = self
                    .task_rows(&tid)
                    .into_iter()
                    .find(|r| r.get("id").and_then(Value::as_str) == Some(tid.as_str()))
                    .and_then(|r| r.get("goal").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
                {
                    let db = self.db();
                    let _ = ferb_store::task_results::TaskResults::new(&db).interrupted(
                        &tid,
                        &now,
                        &goal,
                        &format!(
                            "This was interrupted when FERB restarted: {goal}. Want me to pick it up again?"
                        ),
                    );
                }
                self.record_episode_after_restart(&tid);
            }
        }
        let unknown = self.deps.daemon.mark_unknown_after_restart();
        if !unknown.is_empty() {
            self.journal(
                "ledger.unknown_after_restart",
                json!({"intent_ids": unknown}),
                None,
            );
        }
        resuming
    }

    /// episodes.py `record` for a tree a restart cut off (main.py:427).
    fn record_episode_after_restart(&self, tree_id: &str) {
        let trace = json!({"task_id": tree_id, "spans": self.tree_json(tree_id),
                           "outcome": self.span_outcome(tree_id)});
        let composed = ferb_trace::compose::compose(&trace);
        let now = self.now_iso();
        let failed = {
            let db = self.db();
            let mut failure = None;
            ferb_orchestrator::episodes::record(
                &db,
                || Ok(composed),
                &now,
                tree_id,
                |kind, data| {
                    failure = Some((kind.to_string(), data));
                    Ok(())
                },
            );
            failure
        };
        if let Some((kind, data)) = failed {
            self.journal(&kind, data, None);
        }
    }

    /// supervisor.py:2262 `_restart_safe` / :2255 `update_blockers`: a tree is left out only when each step it has in
    /// progress is a plan wait or a step waiting for the owner (asked again after the restart).
    pub fn update_blockers(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for (tree_id, goal) in self.running_trees() {
            if !self.restart_safe(&tree_id) {
                out.push(json!({"task_id": tree_id, "goal": goal}));
            }
        }
        out
    }

    fn restart_safe(&self, tree_id: &str) -> bool {
        let Ok(loaded) = self.plans(|p| p.load(tree_id)) else {
            return false;
        };
        let Ok(states) = self.plans(|p| p.steps(tree_id)) else {
            return false;
        };
        for (key, state) in states {
            if key.starts_with(ferb_orchestrator::plan_store::EARLIER)
                || !["running", "waiting", "paused"].contains(&state.status.as_str())
            {
                continue;
            }
            let step = loaded.plan.steps.iter().find(|s| s.id == key);
            if state.status == "paused"
                || step.is_some_and(|s| s.kind == "wait" && state.status == "waiting")
            {
                continue;
            }
            // (a loop resting between rounds is restart-safe too: loops are not part of this port)
            return false;
        }
        true
    }

    /// supervisor.py:2324 `resume_plans`: restore exactly the same tree, so completed steps and uncertain-action
    /// checks keep their identity. Each tree's request and turn are restored here; its run (`run_tree` with
    /// `resume_id`) isn't ported yet, so each answers NotPorted, journaled as Python journals a resume that raised.
    /// The trees it tried.
    pub async fn resume_saved_plans(&self) -> Vec<String> {
        if self.deps.daemon.stopped() {
            return Vec::new();
        }
        let pending: Vec<String> = self.plans(|p| p.pending()).unwrap_or_default();
        let mut tried = Vec::new();
        for tree_id in pending {
            if self.live().tree_of.contains_key(&tree_id) {
                continue;
            }
            tried.push(tree_id.clone());
            if let Err(e) = self.restore(&tree_id).await {
                self.journal(
                    "plan.resume_failed",
                    json!({"error": format!("{}: {}", e.class(), e.message())}),
                    None,
                );
            }
        }
        tried
    }

    async fn restore(&self, tree_id: &str) -> super::Flow<()> {
        let saved = self.plans(|p| p.load(tree_id))?;
        let data = saved.request.clone();
        let spec = ferb_orchestrator::validate::task_spec_validate(&ferb_context::Py::from_value(
            data.get("spec").unwrap_or(&Value::Null),
        ))
        .map_err(|e| super::Failure::raised("ValidationError", e.to_string()))?;
        let request_id = data
            .get("request_id")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let words = data
            .get("words")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let payload = data
            .get("payload")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let t = &spec.targets;
        self.deps.daemon.record_request(&Request {
            request_id: request_id.clone(),
            words,
            targets: [
                t.apps.clone(),
                t.sites.clone(),
                t.paths.clone(),
                t.recipients.clone(),
            ],
            payload,
            asked_origins: self.deps.daemon.asked_origins(tree_id),
        });
        let conversation = saved
            .row
            .get("conversation")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let message_origin: BTreeSet<String> = data
            .get("message_origins")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(ferb_py::value::py_str).collect())
            .unwrap_or_default();
        self.live().turns.insert(
            request_id.clone(),
            Arc::new(Mutex::new(Turn {
                request_id: request_id.clone(),
                session: conversation,
                message_origin,
                ..Turn::default()
            })),
        );
        if data.get("unattended").is_some_and(ferb_py::value::truthy) {
            self.deps.daemon.set_unattended(&request_id);
        }
        self.deps.daemon.withdraw_lapsed(tree_id);
        let _ = origin::FERB;
        // `run_tree(spec, 'Continuing your saved plan.', request_id, payload, resume_id=tree_id, restored=True, ...)`
        self.live().turns.remove(&request_id);
        Err(
            NotPorted("Supervisor.run_tree (resume_id, restored: a tree resumed after a restart)")
                .into(),
        )
    }
}
