//! APP-777 (refining APP-1): a task the planner gave to FERB while FERB has no browser profile of its own works as
//! the owner in the profile they picked (supervisor.py:3004 `_browse_as`, :3036 `_await_browsing`), through the
//! daemon's browsing answer ([`Daemon::browsing`](super::Daemon::browsing), areas/browsers.py `_browsing`).
//!
//! The Rust `Task` is fixed: the worker's new identity rides on the tree's state (`as_owner`), read through
//! [`Tree::worker_task`](super::Tree::worker_task) by the worker, its checker and every member made after.
//!
//! decision: APP-777

use std::sync::Arc;

use ferb_orchestrator::browsing;
use serde_json::{json, Map, Value};

use super::{Inner, Tree};

fn text(found: &Map<String, Value>, key: &str) -> String {
    found
        .get(key)
        .filter(|v| ferb_py::value::truthy(v))
        .map(ferb_py::value::py_str)
        .unwrap_or_default()
}

impl Inner {
    /// supervisor.py:3004 `_browse_as`: before the tree's first worker starts.
    pub(crate) async fn browse_as(&self, tree: &Arc<Tree>) {
        if tree.spec().identity != "ferb" {
            return;
        }
        let task = tree.worker_task();
        let attended = !self.unattended_task(&task);
        let mut found = self.deps.daemon.browsing(attended, &task.request_id);
        let asked = text(&found, "ask");
        if !asked.is_empty() {
            self.await_browsing(tree, &asked).await;
            if self.stopped(tree) {
                return;
            }
            // asked once per task: no answer goes on as is
            found = self.deps.daemon.browsing(false, &task.request_id);
        }
        if found.get("as_owner").is_some_and(ferb_py::value::truthy) {
            {
                let mut st = tree.lock();
                if let Some(spec) = st.spec.as_mut() {
                    spec.identity = "owner".into();
                    if let Some(plan) = spec.plan.as_mut() {
                        for step in &mut plan.steps {
                            if let Some(scope) = step.conversation.as_mut() {
                                if scope.identity == "ferb" {
                                    scope.identity = "owner".into();
                                }
                            }
                        }
                    }
                }
                st.as_owner = true;
            }
            let _ = self.tasks(|t| t.set_identity(&tree.worker.id, "owner"));
            self.journal("task.browses_as_owner", json!({"task_id": tree.id}), None);
            if !asked.is_empty() {
                let line = browsing::CHOSEN.replace("{label}", &text(&found, "label"));
                tree.lock().side_lines.push(line);
            }
        } else if found.get("declined").is_some_and(ferb_py::value::truthy) && !asked.is_empty() {
            tree.lock()
                .side_lines
                .push(browsing::NONE_CHOSEN.to_string());
        } else if found.get("note").is_some_and(ferb_py::value::truthy) {
            let note = text(&found, "note");
            tree.lock().side_lines.push(note);
        }
    }

    /// supervisor.py:3036 `_await_browsing`: the task shows paused while FERB's browsing question waits; the answer
    /// is applied once, by whichever of its paths saw it first. A stop leaves the question open for the next task.
    async fn await_browsing(&self, tree: &Arc<Tree>, approval_id: &str) {
        self.set_status(&tree.worker.id, "paused", None);
        self.set_status(&tree.id, "paused", None);
        while !self.stopped(tree) {
            match self.deps.daemon.approval(approval_id) {
                Some(ap) if ap.state == ferb_gateway::questions::State::Pending => {}
                Some(ap) => {
                    self.deps.daemon.question_answered(&ap);
                    break;
                }
                None => break,
            }
            let poll = self.deps.daemon.hold_poll_s();
            tokio::time::sleep(std::time::Duration::from_secs_f64(poll)).await;
        }
        if tree.lock().stop.is_empty() {
            self.set_status(&tree.worker.id, "planned", None);
            self.set_status(&tree.id, "running", None);
        }
    }
}
