//! A step's live state the worker's tools report while it runs: a worker asleep until a time or waiting on a
//! condition (supervisor.py:4856 `task_sleeping`, ORCH-598, ORCH-633), on the task's live line.

use serde_json::{json, Map, Value};

use super::{Inner, Supervisor};

/// supervisor.py:217 `WAIT_REASON_CHARS` (ORCH-598): a sleeping worker's reason in its task's status, at most.
pub const WAIT_REASON_CHARS: usize = 160;

impl Supervisor {
    /// supervisor.py:4856 `task_sleeping`: the wait tools' `status(worker, task, until, line)` (wait.for, and
    /// time.sleep_until's sleep). `until` None: it woke.
    pub fn task_sleeping(
        &self,
        task_id: &str,
        until: Option<&ferb_clock::py::Time>,
        why: &str,
        line: &str,
    ) {
        self.inner.task_sleeping(task_id, until, why, line)
    }
}

impl Supervisor {
    /// supervisor.py:4878 `preview_status`: what a task is doing now, for its live preview (APP-404).
    pub fn preview_status(&self, root: &str, worker: &str) -> Option<(String, String)> {
        self.inner.preview_status(root, worker)
    }

    /// supervisor.py:5682 `loop_statuses`: each running loop's latest `loop.status`.
    pub fn loop_statuses(&self) -> Vec<Value> {
        self.inner.loop_statuses()
    }

    /// supervisor.py:5652 `loop_resting`: a loop's task between its rounds (the browser drivers' `task_resting`).
    pub fn loop_resting(&self, task_id: &str) -> bool {
        self.inner.loop_resting(task_id)
    }

    /// supervisor.py:5677 `wake_loops`.
    pub fn wake_loops(&self, tree_id: &str) {
        self.inner.wake_loops(tree_id)
    }
}

impl Supervisor {
    /// areas/schedules.py:225 `_wait_status` (ORCH-633), the daemon's `WaitDeps.status`: while a worker waits in code
    /// it counts as waiting, not stalled, and its task's live line shows the wait's own line; `line` None: it woke.
    pub fn wait_status(
        &self,
        worker_id: &str,
        task_id: &str,
        until: Option<&ferb_clock::py::Time>,
        line: Option<&str>,
    ) {
        self.inner.activity.waiting(worker_id, line.is_some());
        self.inner.task_sleeping(
            task_id,
            if line.is_some() { until } else { None },
            "",
            line.unwrap_or(""),
        )
    }
}

impl Inner {
    /// supervisor.py:4878 `preview_status`: what a task is doing now, from FERB's own records: done (the task isn't
    /// running), asking (a question of it waits for the owner: its words), paused, waiting (a worker's sleep, a plan
    /// wait, or a loop resting between rounds), or working on another part. None while that step works.
    pub fn preview_status(&self, root: &str, worker: &str) -> Option<(String, String)> {
        let tree = self.live().tree_of.get(root).cloned();
        let Some(tree) = tree.filter(|t| t.id == root) else {
            return Some(("done".into(), "Done".into()));
        };
        let asked = self
            .deps
            .daemon
            .approvals()
            .into_iter()
            .find(|ap| ap.request_id == tree.worker.request_id);
        if let Some(ap) = asked {
            let words = if ap.summary.is_empty() {
                ap.reason.clone()
            } else {
                ap.summary.clone()
            };
            return Some((
                "asking".into(),
                format!("Asking you: {}", ferb_py::text::shorten(&words, 160, true)),
            ));
        }
        if self.task_paused(root) {
            return Some(("paused".into(), "Paused".into()));
        }
        if let Some(asleep) = self.live().sleeping.get(root).cloned() {
            let text = crate::lines::sleeping_line(&asleep);
            let mut chars = text.chars();
            let first: String = chars
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default();
            return Some(("waiting".into(), first + chars.as_str()));
        }
        let (steps, states) = match self.plans(|p| p.load(root).map(|l| l.plan.steps)) {
            Ok(steps) => (steps, self.plans(|p| p.steps(root)).unwrap_or_default()),
            Err(_) => (Vec::new(), Vec::new()),
        };
        for (key, state) in &states {
            let Some(step) = steps.iter().find(|s| &s.id == key) else {
                continue;
            };
            if step.kind == "wait" && state.status == "waiting" {
                let what = [
                    step.goal.clone(),
                    step.until.clone().unwrap_or_default(),
                    step.event.clone().unwrap_or_default(),
                ]
                .into_iter()
                .find(|w| !w.is_empty())
                .unwrap_or_else(|| "the time it waits for".into());
                let what = what.split_whitespace().collect::<Vec<_>>().join(" ");
                return Some((
                    "waiting".into(),
                    format!("Waiting for {}", ferb_py::text::shorten(&what, 160, true)),
                ));
            }
            if step.loops() && self.live().resting.contains(&format!("{root}.loop.{key}")) {
                return Some((
                    "waiting".into(),
                    format!(
                        "Waiting for the next check of {}",
                        ferb_orchestrator::names::short_label(&step.goal, 60)
                    ),
                ));
            }
        }
        if !worker.is_empty() {
            let status = self.task_column(worker, "status");
            let running = match &status {
                None => true,
                Some(v) => v.is_null() || v.as_str() == Some("running"),
            };
            if !running {
                return Some((
                    "working_elsewhere".into(),
                    "Working on another part of this task".into(),
                ));
            }
        }
        None
    }

    /// supervisor.py:4856 `task_sleeping`: the task's live line says "waiting until HH:MM: “why”" for the whole sleep
    /// (a wait.for's own `line` in its place, ORCH-633) and "working" when it wakes; the planner's task state and the
    /// preview read the same record (`_sleeping`).
    pub fn task_sleeping(
        &self,
        task_id: &str,
        until: Option<&ferb_clock::py::Time>,
        why: &str,
        line: &str,
    ) {
        let root = self.tree_of(task_id).unwrap_or_else(|| {
            task_id
                .split_once('.')
                .map(|(r, _)| r.to_string())
                .unwrap_or_else(|| task_id.to_string())
        });
        if !self.live().tree_of.contains_key(&root) {
            return;
        }
        let text = match until {
            None => {
                if self.live().sleeping.remove(&root).is_none() {
                    return;
                }
                ferb_orchestrator::progress::WORKING.to_string()
            }
            Some(until) => {
                let printable = ferb_sanitize::terminal::printable(why);
                let reason = ferb_orchestrator::lines::shorten(&printable, WAIT_REASON_CHARS, true);
                let local = self.deps.clock.zone().local(until);
                let mut asleep = json!({
                    "until": ferb_clock::py::fmt(&local, "%H:%M"),
                    "until_at": ferb_clock::py::isoformat(until, ferb_clock::py::Spec::Auto),
                    "reason": reason,
                });
                if !line.is_empty() {
                    asleep["line"] = json!(ferb_sanitize::terminal::printable(line));
                }
                let asleep = match self.redact(&asleep) {
                    Value::Object(m) => m,
                    _ => Map::new(),
                };
                let text = crate::lines::sleeping_line(&asleep);
                self.live().sleeping.insert(root.clone(), asleep);
                text
            }
        };
        let text = self.redact(&Value::String(text));
        if let Some(worker) = self.worker.get() {
            worker.progress_line(&root, task_id, text);
        }
    }
}
