//! ORCH-691 (owner, 2026-11-22: "8 failed actions or 4 min"): FERB's guard inside one step. A worker attempt has
//! no other limit; this one stops it when it isn't getting anywhere, and the supervisor then asks the owner ORCH-679's
//! question ("Keep trying, or go on with what I have?"). Code reads only structure, never words (rule 4):
//!
//! - **Failed or unchanged on-screen actions on one app**: a call whose tool can change the owner's screen
//!   (`on_screen`) into one app (its `app` argument, one key however it is named) that was refused or errored, says
//!   it didn't happen (`ok` or `performed` false), or looked at the app afterwards and saw no change (`after.read` other
//!   than "none" and `after.changed` not true). [`FAILED_ON_ONE_APP`] of them in a row on one app, with no progress
//!   on that app in between, stop the attempt: progress on an app (a call into it that went through and showed a
//!   change, or observed something of it not observed before) starts its count again, so a long healthy task with a
//!   few failures here and there never adds up to a stop.
//! - **Time on the step without progress**: progress is a call that succeeded and either changed something it could
//!   see (an on-screen action with `after.changed` true), acted off the screen (any other non-observation call), or
//!   observed something not observed before in the step (the tool and its arguments). [`NO_PROGRESS_S`] without
//!   progress stop the attempt. Only the time the step's worker is thinking counts: time inside a call (a wait, a
//!   long command) and the time between two attempts (the check) don't.
//!
//! Nothing counts calls as such: a long task that keeps making progress is never stopped for its length (ORCH-706,
//! owner 2026-11-23, retiring the 40-call trial of ORCH-693).
//!
//! Each stop is latched once for the supervisor ([`Steps::take_tripped`]) and the step's counts start again, so a
//! "keep trying" gets a fresh 8 actions and 4 minutes. decision: ORCH-691, ORCH-692

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{json, Map, Value};

// The guard's numbers, in one place (each with its decision).
/// ORCH-691: failed or unchanged on-screen actions in a row on one app (no progress there in between) that stop an
/// attempt.
pub const FAILED_ON_ONE_APP: u32 = 8;
/// ORCH-691: seconds of the step without progress that stop an attempt.
pub const NO_PROGRESS_S: f64 = 240.0;

/// Arguments that annotate a call rather than say what it reaches.
const NOTES: [&str; 4] = ["why", "effect", "purpose", "mechanism"];

/// One finished call as the guard reads it.
pub struct Done<'a> {
    pub tool: &'a str,
    /// The tool can change the owner's screen (`Facts::on_screen`).
    pub on_screen: bool,
    /// The tool only observes (`Facts::observation`).
    pub observation: bool,
    /// The app the call goes to (its `app` argument), one key however it was named ("" when none).
    pub app: &'a str,
    /// The app as the owner knows it, for the stop's words.
    pub app_label: &'a str,
    pub args: &'a Map<String, Value>,
    pub content: &'a Value,
    pub is_error: bool,
}

#[derive(Default, Debug)]
struct Step {
    /// app key -> (failed or unchanged actions, its label).
    failed: BTreeMap<String, (u32, String)>,
    /// Seconds of the worker's own time without progress.
    idle: f64,
    /// The worker and the moment the step's time was last counted up to.
    last: Option<(String, f64)>,
    in_flight: u32,
    seen: BTreeSet<String>,
    tripped: Option<Value>,
}

/// The guard's count for every running step (keyed by the step's task id).
#[derive(Default, Debug)]
pub struct Steps {
    by_task: HashMap<String, Step>,
    task_of: HashMap<String, String>,
}

fn truthy_false(content: &Value, key: &str) -> bool {
    content.get(key) == Some(&Value::Bool(false))
}

/// The call didn't happen or went wrong.
fn went_wrong(d: &Done) -> bool {
    d.is_error
        || d.content.get("error").is_some_and(|e| !e.is_null())
        || truthy_false(d.content, "ok")
        || truthy_false(d.content, "performed")
}

/// The call looked at the app afterwards and saw no change.
fn unchanged(content: &Value) -> bool {
    let Some(after) = content.get("after") else {
        return false;
    };
    let read = after.get("read").and_then(Value::as_str).unwrap_or("none");
    read != "none" && after.get("changed") != Some(&Value::Bool(true))
}

fn changed(content: &Value) -> bool {
    content
        .get("after")
        .and_then(|a| a.get("changed"))
        .is_some_and(|c| c == &Value::Bool(true))
}

fn what(tool: &str, args: &Map<String, Value>) -> String {
    let kept: BTreeMap<&String, &Value> = args
        .iter()
        .filter(|(k, _)| !NOTES.contains(&k.as_str()))
        .collect();
    format!(
        "{tool} {}",
        serde_json::to_string(&kept).unwrap_or_default()
    )
}

impl Step {
    /// The worker's time up to `now` (not while a call is in flight, nor across two workers).
    fn tick(&mut self, worker: &str, now: f64) {
        if self.in_flight == 0 {
            if let Some((w, t)) = &self.last {
                if w == worker {
                    self.idle += (now - t).max(0.0);
                }
            }
        }
        self.last = Some((worker.to_string(), now));
    }
}

impl Steps {
    /// A call of `worker` (step `task`) begins.
    pub fn started(&mut self, task: &str, worker: &str, now: f64) {
        self.task_of.insert(worker.to_string(), task.to_string());
        let step = self.by_task.entry(task.to_string()).or_default();
        step.tick(worker, now);
        step.in_flight += 1;
    }

    /// The call ended: what it says about the step.
    pub fn ended(&mut self, task: &str, worker: &str, d: &Done, now: f64) {
        let step = self.by_task.entry(task.to_string()).or_default();
        step.in_flight = step.in_flight.saturating_sub(1);
        step.last = Some((worker.to_string(), now));
        let wrong = went_wrong(d);
        if d.on_screen && !d.app.is_empty() && (wrong || unchanged(d.content)) {
            let slot = step
                .failed
                .entry(d.app.to_string())
                .or_insert((0, d.app_label.to_string()));
            slot.0 += 1;
        }
        let progress = !wrong
            && if d.on_screen {
                changed(d.content)
            } else if d.observation {
                step.seen.insert(what(d.tool, d.args))
            } else {
                true
            };
        if progress {
            step.idle = 0.0;
            // progress on this app: its failures in a row start again
            if !d.app.is_empty() {
                step.failed.remove(d.app);
            }
        }
    }

    /// Whether `worker`'s attempt should stop now: the stop's facts the first time, then None (the step counts
    /// afresh). Asked by the worker's run as it goes.
    pub fn check(&mut self, worker: &str, now: f64) -> Option<Value> {
        let task = self.task_of.get(worker)?.clone();
        let step = self.by_task.get_mut(&task)?;
        step.tick(worker, now);
        let failed = step
            .failed
            .values()
            .find(|(n, _)| *n >= FAILED_ON_ONE_APP)
            .cloned();
        let why = if let Some((n, label)) = failed {
            json!({"why": "failed_actions", "app": label, "count": n})
        } else if step.in_flight == 0 && step.idle >= NO_PROGRESS_S {
            json!({"why": "no_progress", "seconds": NO_PROGRESS_S})
        } else {
            return None;
        };
        step.failed.clear();
        step.idle = 0.0;
        step.tripped = Some(why.clone());
        Some(why)
    }

    /// The stop latched for the step, once (the supervisor reads it when it judges the attempt).
    pub fn take_tripped(&mut self, task: &str) -> Option<Value> {
        self.by_task.get_mut(task).and_then(|s| s.tripped.take())
    }

    /// The owner said "keep trying": the step counts afresh.
    pub fn keep_trying(&mut self, task: &str) {
        if let Some(s) = self.by_task.get_mut(task) {
            s.failed.clear();
            s.idle = 0.0;
            s.tripped = None;
        }
    }

    pub fn forget(&mut self, tasks: &BTreeSet<String>) {
        self.by_task.retain(|t, _| !tasks.contains(t));
        self.task_of.retain(|_, t| !tasks.contains(t));
    }

    /// A run ended: its worker id goes (the step's counts stay for its next attempt).
    pub fn forget_worker(&mut self, worker: &str) {
        self.task_of.remove(worker);
    }
}

/// The stop's plain words: what the owner may read as why this attempt ended.
pub fn words(why: &Value) -> String {
    match why.get("why").and_then(Value::as_str) {
        Some("failed_actions") => format!(
            "{} of FERB's actions in {} failed or changed nothing, so FERB stopped to ask you",
            why.get("count")
                .and_then(Value::as_u64)
                .unwrap_or(FAILED_ON_ONE_APP as u64),
            why.get("app").and_then(Value::as_str).unwrap_or("one app")
        ),
        _ => format!(
            "{} minutes went by on this step without progress, so FERB stopped to ask you",
            (NO_PROGRESS_S / 60.0).round() as i64
        ),
    }
}
