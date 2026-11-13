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
