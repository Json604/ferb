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
//! The resumed run is `run_tree(resume_id=..., restored=True, ...)`: its events go out as `task.resumed_event` (each
//! stamped with its request and time), and its outcome is recorded as a result (or handed to its schedule).
//!
//! The shutdown itself (main.py `shutdown`): `suspend_plans`, `cancel_all`, then every chat turn and resumed run is
//! cancelled where it is ([`Supervisor::shut_down`]): a running tree takes Python's `except BaseException` path
//! (`task.interrupted`, its resources released) and records no end; a chat stream sends nothing more and ends.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use super::{Inner, Request, Supervisor, Turn};

impl Supervisor {
    /// The daemon's shutdown (main.py:522-524): `suspend_plans()`, `cancel_all()`, then the drain of every chat turn
    /// and resumed run (each is cancelled where it is and finishes its own clean-up). Returns once they all ended.
    pub async fn shut_down(&self) {
        self.inner.suspend_now();
        self.inner.cancel_all();
        self.inner.shutdown.send_replace(true);
        self.inner.work_open.send_replace(true);
        loop {
            let jobs: Vec<tokio::task::JoinHandle<()>> =
                std::mem::take(&mut *self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner()));
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                let _ = job.await;
            }
        }
        // main.py:526: learning captures are cancelled, journaled as deferred
        self.inner.stop_episodes().await;
    }

    /// supervisor.py:5003 `finish_episodes`: every pending episode capture, awaited (tests); none is cancelled.
    pub async fn finish_episodes(&self) {
        self.inner.finish_episodes().await;
    }

    /// supervisor.py:2251 `hold_new_work` (L34 Phase G): FERB stops for an update now; nothing new starts.
    pub fn hold_new_work(&self) {
        self.inner.work_open.send_replace(false);
    }

    /// What a start recovers before it serves (main.py:418-430): what was running is `interrupted`; the owner is told
    /// of each tree that won't resume. The trees that will resume (`plans.pending()`).
    pub fn recover_after_restart(&self) -> Vec<String> {
        self.inner.recover_after_restart()
    }

    /// main.py:466-468: every intent a restart cut off is of unknown outcome (journaled).
    pub fn mark_unknown_after_restart(&self) -> Vec<String> {
        self.inner.ledger_unknown()
    }

    /// supervisor.py:2324 `resume_plans`: each tree a shutdown or crash cut off runs again in place. Runs them all;
    /// returns when they ended (the daemon spawns it: [`Supervisor::start_resume`]).
    pub async fn resume_saved_plans(&self) -> Vec<String> {
        self.inner.resume_saved_plans().await
    }

    /// main.py:472 `asyncio.create_task(self.supervisor.resume_plans())`: in the background, drained at shutdown.
    pub fn start_resume(&self) {
        let me = self.inner.clone();
        let job = tokio::spawn(async move {
            me.resume_saved_plans().await;
        });
        self.inner.track(job);
    }

    /// supervisor.py:2255 `update_blockers`: the running trees an update must wait for.
    pub fn update_blockers(&self) -> Vec<Value> {
        self.inner.update_blockers()
    }
}

impl Inner {
    /// A turn or run under way, drained at shutdown (finished ones are let go).
    pub(crate) fn track(&self, job: tokio::task::JoinHandle<()>) {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.retain(|j| !j.is_finished());
        jobs.push(job);
    }

    /// The shutdown has begun.
    pub fn shutting_down_now(&self) -> bool {
        *self.shutdown.borrow()
    }

    /// Resolves once the shutdown begins.
    pub async fn shutting_down(&self) {
        let mut rx = self.shutdown.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

    /// `await self._work_open.wait()`: an update's hold (the shutdown ends the wait).
    pub async fn work_opened(&self) {
        let mut rx = self.work_open.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

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

    /// main.py:418-430: a restart's recovery. What was running didn't finish (`interrupted`); a tree whose plan didn't
    /// end resumes by itself, so only the others are told ("pick it up again?") and get their episode. The trees to
    /// resume, in the plan store's order.
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
        resuming
    }

    /// main.py:466-468 `ledger.mark_unknown_after_restart()`, journaled.
    pub fn ledger_unknown(&self) -> Vec<String> {
        let unknown = self.deps.daemon.mark_unknown_after_restart();
        if !unknown.is_empty() {
            self.journal(
                "ledger.unknown_after_restart",
                json!({"intent_ids": unknown}),
                None,
            );
        }
        unknown
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
        // `tree_id not in self._plan_runs`: its plan runner isn't running yet (a planner turn, its plan line)
        if !self.live().plan_runs.contains(tree_id) {
            return false;
        }
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
            // a loop resting between rounds is restart-safe too (L31)
            if step.is_some_and(|s| s.loops())
                && self
                    .live()
                    .resting
                    .contains(&format!("{tree_id}.loop.{key}"))
            {
                continue;
            }
            return false;
        }
        true
    }

    /// supervisor.py:2324 `resume_plans`: restore exactly the same tree, so completed steps and uncertain-action
    /// checks keep their identity; all at once (`gather`). The trees it tried.
    pub async fn resume_saved_plans(&self) -> Vec<String> {
        if self.deps.daemon.stopped() {
            return Vec::new();
        }
        let pending: Vec<String> = self.plans(|p| p.pending()).unwrap_or_default();
        let tried: Vec<String> = pending
            .into_iter()
            .filter(|t| !self.live().tree_of.contains_key(t))
            .collect();
        let runs = tried.iter().map(|t| self.restore(t));
        let results = futures_util::future::join_all(runs).await;
        for result in results {
            match result {
                Ok(()) | Err(super::Failure::Cancelled) => {}
                Err(e) => {
                    self.journal(
                        "plan.resume_failed",
                        json!({"error": format!("{}: {}", e.class(), e.message())}),
                        None,
                    );
                }
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
        let text = |k: &str| data.get(k).map(ferb_py::value::py_str).unwrap_or_default();
        let request_id = text("request_id");
        let payload = text("payload");
        let t = &spec.targets;
        self.deps.daemon.record_request(&Request {
            request_id: request_id.clone(),
            words: text("words"),
            targets: [
                t.apps.clone(),
                t.sites.clone(),
                t.paths.clone(),
                t.recipients.clone(),
            ],
            payload: payload.clone(),
            asked_origins: self.deps.daemon.asked_origins(tree_id),
        });
        let conversation = saved
            .row
            .get("conversation")
            .and_then(Value::as_str)
            .map(str::to_string);
        let set_of = |k: &str| -> BTreeSet<String> {
            data.get(k)
                .and_then(Value::as_array)
                .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                .unwrap_or_default()
        };
        self.live().turns.insert(
            request_id.clone(),
            Arc::new(Mutex::new(Turn {
                request_id: request_id.clone(),
                session: conversation.clone().unwrap_or_default(),
                message_origin: set_of("message_origins"),
                ..Turn::default()
            })),
        );
        let schedule = data.get("schedule").and_then(Value::as_object).cloned();
        if data.get("unattended").is_some_and(ferb_py::value::truthy) {
            self.deps.daemon.set_unattended(&request_id);
        }
        // resuming a loop posts nothing to chat: no plan line (L31)
        let quiet = saved.plan.steps.iter().any(|s| s.loops());
        // nobody started this from chat: only the scope already granted to this tree, and still in force, carries
        self.deps.daemon.withdraw_lapsed(tree_id);
        let watch = schedule
            .as_ref()
            .and_then(|s| s.get("watch"))
            .is_some_and(ferb_py::value::truthy);
        let opts = super::tree::RunOpts {
            conversation,
            origins: set_of("origins"),
            resume_id: Some(tree_id.to_string()),
            unattended_scopes: Some(self.deps.daemon.standing_scopes(tree_id)),
            restored: true,
            schedule: schedule.clone(),
            watch,
            ..super::tree::RunOpts::default()
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let run = self.run_tree(
            spec,
            "Continuing your saved plan.",
            &request_id,
            &payload,
            opts,
            &tx,
            &|| {},
        );
        tokio::pin!(run);
        let mut take = Vec::new();
        let result = loop {
            tokio::select! {
                biased;
                Some(ev) = rx.recv() => take.push(ev),
                r = &mut run => break r,
            }
            for ev in std::mem::take(&mut take) {
                self.resumed_event(ev, quiet, &request_id, schedule.as_ref())
                    .await;
            }
        };
        while let Ok(ev) = rx.try_recv() {
            self.resumed_event(ev, quiet, &request_id, schedule.as_ref())
                .await;
        }
        // the tree's per-request context goes when it ends (final review M4)
        self.live().turns.remove(&request_id);
        result
    }

    /// One event of a resumed run: no chat stream stamps it, so it names its request and when it was sent.
    async fn resumed_event(
        &self,
        ev: Value,
        quiet: bool,
        request_id: &str,
        schedule: Option<&Map<String, Value>>,
    ) {
        let kind = ev.get("type").and_then(Value::as_str).unwrap_or("");
        if (quiet && kind == "plan") || kind == "part" || kind == "step" {
            return; // a row's or step's change is on the bus already (task.part, task.step)
        }
        let outcome = kind == "outcome";
        let mut ev = match ev {
            Value::Object(m) => m,
            _ => return,
        };
        ev.insert("request_id".into(), json!(request_id));
        ev.insert("at".into(), json!(self.now_iso()));
        if outcome {
            if let Some(s) = schedule {
                ev.insert("schedule".into(), Value::Object(s.clone()));
            }
        }
        let ev = Value::Object(ev);
        self.deps.daemon.publish("task.resumed_event", ev.clone());
        if outcome && !self.deps.daemon.resumed_outcome(&ev).await {
            self.task_result(&ev, false);
        }
    }
}
