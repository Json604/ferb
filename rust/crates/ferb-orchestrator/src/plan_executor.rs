//! ferb/orchestrator/plan_executor.py: runs a stored plan, every step whose dependencies finished, independent steps
//! at once, with durable step state and waits; every action still goes through ordinary workers and the gateway.
//! A single task is a one-step plan.
//!
//! decision: ORCH-322, ORCH-323, ORCH-531, ORCH-532

use std::sync::Arc;

use futures_util::stream::{FuturesUnordered, StreamExt};
use futures_util::FutureExt;
use serde_json::{json, Map, Value};

use crate::loops::{Wake, POLL_S};
use crate::models::Step;
use crate::plan_store::{PlanStore, StepState};
use crate::plans::{DONE, ENDED};
use crate::ports::BoxFuture;

/// A step's work: (step, its inputs, what earlier steps gave) -> its result, or the exception's (type name, text).
pub type Execute = Arc<
    dyn Fn(
            Step,
            Vec<String>,
            Map<String, Value>,
        ) -> BoxFuture<'static, Result<Value, (String, String)>>
        + Send
        + Sync,
>;
/// The step lifecycle (ORCH-531): (task id, step id, status, facts).
pub type OnStep = Arc<dyn Fn(&str, &str, &str, Map<String, Value>) + Send + Sync>;

/// A step running: its id and how it ended.
type StepJob<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = (String, crate::sql::Result<Value>)> + 'a>>;

/// plan_executor.py:17 `_ENDED_AS`.
fn ended_as(status: &str) -> &'static str {
    match status {
        "done" | "done_unverified" => "done",
        "cancelled" | "needs_user" => "not_finished",
        _ => "failed",
    }
}

/// `recheck_in(due, now)` (ferb/clock/timers.py:31).
pub fn recheck_in(due: f64, now: f64) -> f64 {
    (due - now).clamp(0.0, POLL_S)
}

/// plan_executor.py:25 `PlanExecutor`.
pub struct PlanExecutor<'a> {
    pub store: &'a PlanStore<'a>,
    pub execute: Execute,
    pub stopped: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The injected wall clock, as Python's aware `datetime`.
    pub now: Arc<dyn Fn() -> ferb_py::time::DateTime + Send + Sync>,
    pub on_step: Option<OnStep>,
    pub wake: Arc<Wake>,
}

fn state_of<'s>(states: &'s [(String, StepState)], id: &str) -> Option<&'s StepState> {
    states.iter().find(|(k, _)| k == id).map(|(_, s)| s)
}

fn status_of<'s>(states: &'s [(String, StepState)], id: &str) -> &'s str {
    state_of(states, id)
        .map(|s| s.status.as_str())
        .unwrap_or("")
}

fn steps_json(states: &[(String, StepState)]) -> Value {
    let map: Map<String, Value> = states
        .iter()
        .map(|(k, s)| (k.clone(), json!({"status": s.status, "output": s.output})))
        .collect();
    Value::Object(map)
}

/// Marks the steps still running `interrupted` when the run ends (Python's CancelledError path).
struct InFlight<'s, 'a> {
    store: &'s PlanStore<'a>,
    task_id: String,
    steps: Vec<String>,
}

impl Drop for InFlight<'_, '_> {
    fn drop(&mut self) {
        for step in &self.steps {
            let _ = self.store.begin(&self.task_id, step, "interrupted");
        }
    }
}

impl<'a> PlanExecutor<'a> {
    fn tell(&self, task_id: &str, step: &Step, status: &str, facts: Vec<(&str, Value)>) {
        if let Some(on_step) = &self.on_step {
            let mut map = Map::new();
            map.insert("loop".into(), Value::Bool(step.loops()));
            for (k, v) in facts {
                map.insert(k.into(), v);
            }
            on_step(task_id, &step.id, status, map);
        }
    }

    pub fn wake(&self) {
        self.wake.set();
    }

    /// plan_executor.py:47 `_wait`.
    async fn wait(&self, task_id: &str, step: &Step) -> crate::sql::Result<Value> {
        let states = self.store.steps(task_id)?;
        let state = state_of(&states, &step.id)
            .cloned()
            .ok_or_else(|| crate::sql::Error::Key(step.id.clone()))?;
        let cursor = match state.output.get("event_cursor") {
            Some(v) => v.as_i64().unwrap_or(0),
            None => self.store.event_cursor()?,
        };
        self.store.finish(
            task_id,
            &step.id,
            &json!({"status": "waiting", "event_cursor": cursor}),
        )?;
        let fact = match &step.event {
            Some(e) if !e.is_empty() => ("event", json!(e)),
            _ => (
                "until",
                step.until.clone().map(Value::String).unwrap_or(Value::Null),
            ),
        };
        self.tell(task_id, step, "waiting", vec![fact]);
        while !(self.stopped)() {
            self.wake.clear();
            if step.event.as_deref().is_some_and(|e| !e.is_empty()) {
                let event = step.event.as_deref().unwrap_or("");
                if let Some(payload) = self.store.event_after(event, cursor, &step.event_match)? {
                    return Ok(json!({"status": "done", "summary": step.goal, "event": payload}));
                }
                self.wake.wait().await;
            } else {
                let text = step
                    .until
                    .clone()
                    .unwrap_or_else(|| "None".into())
                    .replace('Z', "+00:00");
                let at = ferb_py::time::DateTime::from_isoformat(&text).map_err(|_| {
                    crate::sql::Error::Value(format!(
                        "Invalid isoformat string: {}",
                        crate::py::repr_str(&text)
                    ))
                })?;
                if at.offset.is_none() {
                    return Err(crate::sql::Error::Value(
                        "A wait time must include its timezone".into(),
                    ));
                }
                let now = (self.now)();
                let (due, current) = (
                    crate::py::epoch_seconds(&at),
                    crate::py::epoch_seconds(&now),
                );
                if due <= current {
                    return Ok(json!({"status": "done", "summary": step.goal}));
                }
                let wait = recheck_in(due, current);
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs_f64(wait),
                    self.wake.wait(),
                )
                .await;
            }
        }
        Ok(json!({"status": "cancelled", "summary": "stopped by you"}))
    }

    /// plan_executor.py:72 `_step`.
    async fn step(
        &self,
        task_id: &str,
        step: Step,
        prior: Map<String, Value>,
    ) -> crate::sql::Result<Value> {
        self.store.begin(task_id, &step.id, "running")?;
        self.tell(task_id, &step, "starting", vec![]);
        let result = if step.kind == "wait" {
            match self.wait(task_id, &step).await {
                Ok(v) => v,
                Err(e) => {
                    json!({"status": "failed", "summary": format!("{}: {e}", error_type(&e))})
                }
            }
        } else {
            match (self.execute)(step.clone(), step.inputs.clone(), prior).await {
                Ok(v) => v,
                Err((kind, text)) => {
                    json!({"status": "failed", "summary": format!("{kind}: {text}")})
                }
            }
        };
        self.store.finish(task_id, &step.id, &result)?;
        let status = result
            .get("status")
            .map(crate::py::str_of)
            .unwrap_or_else(|| "None".into());
        self.tell(task_id, &step, ended_as(&status), vec![]);
        Ok(result)
    }

    /// plan_executor.py:86 `_blocked`: steps that can never run in this run, with the dependencies that stop each.
    fn blocked(steps: &[Step], states: &[(String, StepState)]) -> Vec<(String, Vec<String>)> {
        let ended: Vec<&str> = states
            .iter()
            .filter(|(_, s)| {
                ENDED.contains(&s.status.as_str()) && !DONE.contains(&s.status.as_str())
            })
            .map(|(k, _)| k.as_str())
            .collect();
        let mut blocked: Vec<(String, Vec<String>)> = Vec::new();
        let mut changed = true;
        while changed {
            changed = false;
            for step in steps {
                if blocked.iter().any(|(k, _)| *k == step.id)
                    || ENDED.contains(&status_of(states, &step.id))
                {
                    continue;
                }
                let stops: Vec<String> = step
                    .depends_on
                    .iter()
                    .filter(|d| ended.contains(&d.as_str()) || blocked.iter().any(|(k, _)| k == *d))
                    .cloned()
                    .collect();
                if !stops.is_empty() {
                    blocked.push((step.id.clone(), stops));
                    changed = true;
                }
            }
        }
        blocked
    }

    /// plan_executor.py:103 `run`: `{"status", "steps"}`.
    pub async fn run(&self, task_id: &str) -> crate::sql::Result<Value> {
        let plan = self.store.load(task_id)?.plan;
        self.store.set_status(task_id, "running")?;
        let mut jobs: FuturesUnordered<StepJob<'_>> = FuturesUnordered::new();
        let mut guard = InFlight {
            store: self.store,
            task_id: task_id.to_string(),
            steps: Vec::new(),
        };
        while !(self.stopped)() {
            let states = self.store.steps(task_id)?;
            let unfinished: Vec<&Step> = plan
                .steps
                .iter()
                .filter(|s| !DONE.contains(&status_of(&states, &s.id)))
                .collect();
            if unfinished.is_empty() {
                guard.steps.clear();
                return Ok(json!({"status": "done", "steps": steps_json(&states)}));
            }
            let blocked = Self::blocked(&plan.steps, &states);
            for step in &unfinished {
                if guard.steps.contains(&step.id) || ENDED.contains(&status_of(&states, &step.id)) {
                    continue;
                }
                if let Some((_, stops)) = blocked.iter().find(|(k, _)| *k == step.id) {
                    self.tell(task_id, step, "wont_run", vec![("waits_on", json!(stops))]);
                } else if step
                    .depends_on
                    .iter()
                    .all(|d| DONE.contains(&status_of(&states, d)))
                {
                    let mut prior = Map::new();
                    for d in &step.depends_on {
                        prior.insert(
                            d.clone(),
                            state_of(&states, d)
                                .map(|s| s.output.clone())
                                .unwrap_or(Value::Null),
                        );
                    }
                    prior.insert(
                        "previous_attempt".into(),
                        state_of(&states, &step.id)
                            .map(|s| s.output.clone())
                            .unwrap_or(Value::Null),
                    );
                    let id = step.id.clone();
                    let owned = (*step).clone();
                    let task = task_id.to_string();
                    guard.steps.push(id.clone());
                    jobs.push(Box::pin(async move {
                        let r = self.step(&task, owned, prior).await;
                        (id, r)
                    }));
                } else {
                    let waits: Vec<String> = step
                        .depends_on
                        .iter()
                        .filter(|d| !DONE.contains(&status_of(&states, d)))
                        .cloned()
                        .collect();
                    self.tell(task_id, step, "waiting", vec![("waits_on", json!(waits))]);
                }
            }
            if jobs.is_empty() {
                let waiting = unfinished
                    .iter()
                    .any(|s| status_of(&states, &s.id) == "needs_user");
                return Ok(
                    json!({"status": if waiting { "needs_user" } else { "blocked" }, "steps": steps_json(&states)}),
                );
            }
            // asyncio.wait(FIRST_COMPLETED): every step that finishes without waiting finishes in the same pass
            let Some(first) = jobs.next().await else {
                continue;
            };
            let mut done = vec![first];
            while let Some(Some(more)) = jobs.next().now_or_never() {
                done.push(more);
            }
            for (id, result) in done {
                guard.steps.retain(|s| *s != id);
                result?;
            }
        }
        drop(jobs);
        drop(guard);
        Ok(json!({"status": "cancelled", "steps": steps_json(&self.store.steps(task_id)?)}))
    }
}

fn error_type(e: &crate::sql::Error) -> &'static str {
    match e {
        crate::sql::Error::Value(_) => "ValueError",
        crate::sql::Error::Key(_) => "KeyError",
        crate::sql::Error::Json(_) => "JSONDecodeError",
        _ => "OperationalError",
    }
}
