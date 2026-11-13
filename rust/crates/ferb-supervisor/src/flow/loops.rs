//! Loops (L31): supervisor.py:5641-6000 `_loop_step` and what it reads and writes (`wake_loops`, `loop_statuses`,
//! `loop_resting`, `_loop_facts`, `_paused_words`, `_pause_question`, `_HeldTabs`, `_loop_end_reached`, `_judged`,
//! `_answer_record`, `_with_answer_check`, `_kept_record`, `_probe_exposure`, `_look_only`), on ferb-orchestrator's
//! `loops::LoopRunner` (its `Send` form, [`LoopRunnerShared`], so a loop runs inside a task that moves between
//! threads).
//!
//! Every round is an ordinary worker run with its check under one task id (`<tree>.loop.<step>`), so what a round
//! starts is still the loop's in the next round; between rounds the runner reads the round's probe through the gateway
//! without a model ([`Daemon::probe_read`](super::Daemon::probe_read)). A loop's own question waits through the one
//! wait for the owner (owner_wait.rs).
//!
//! Python's cancellation of a round (FERB's shutdown, a path not ported) has no place in the runner's `round`, which
//! answers a result: the round records why it ended and never answers, and the step ends with that failure, dropping
//! the runner where it stands (nothing more of the loop's row is written, as Python's cancelled round writes nothing).
//!
//! decision: ORCH-505, ORCH-322, ORCH-323, ORCH-536

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use ferb_core::types::{Outcome, OutcomeStatus, Task};
use ferb_orchestrator::loops::{
    self, LoopHost, LoopRows, LoopRunnerShared, Observed, ProbeRead, Reason, RoundResult, Wake,
};
use ferb_orchestrator::models::{Check, Condition, Probe, Step};
use ferb_orchestrator::ports::BoxFuture;
use ferb_orchestrator::verify::ConditionResult;
use ferb_sanitize::origin;
use serde_json::{json, Map, Value};

use super::owner_wait::{OwnerReply, Wait};
use super::{Failure, Flow, Inner, Tree, TreeState};
use crate::words::*;

/// `reason | {key: value}`.
fn with(mut reason: Reason, key: &str, value: Value) -> Reason {
    reason.fields.insert(key.into(), value);
    reason
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// `Condition(text=t, owner_text=t, check=Check(kind='none', ...))`.
fn plain_condition(text: &str) -> Condition {
    Condition {
        text: text.to_string(),
        owner_text: text.to_string(),
        check: Check::none(),
        layer: "outcome".into(),
    }
}

/// plans.py `step_results(spec, step)`: the task's earlier results and the step's own, each once.
pub(crate) fn step_results(spec: &ferb_orchestrator::models::TaskSpec, step: &Step) -> Vec<String> {
    let mut all = spec.earlier_results.clone();
    for r in &step.earlier_results {
        if !all.contains(r) {
            all.push(r.clone());
        }
    }
    all
}

/// loops.py:81 `LoopStore`: the `loop_state` rows (migration 0047) on the supervisor's store.
pub struct LoopStoreRows {
    inner: Arc<Inner>,
}

const LOOP_FIELDS: [&str; 10] = [
    "state",
    "rounds",
    "probe_json",
    "baseline_json",
    "reads",
    "pacing_json",
    "recent_json",
    "note",
    "event_cursor",
    "steer_seen",
];

fn sql_of(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(b) => rusqlite::types::Value::Integer(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => rusqlite::types::Value::Integer(i),
            None => rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        other => rusqlite::types::Value::Text(ferb_py::json::dumps(other)),
    }
}

impl LoopRows for LoopStoreRows {
    fn load(&self, task_id: &str, step_id: &str, steers: i64, cursor: i64) -> Map<String, Value> {
        let now = self.inner.now_iso();
        let db = self.inner.db();
        let _ = db.conn().execute(
            "INSERT OR IGNORE INTO loop_state(task_id,step_id,steer_seen,event_cursor,updated_at) VALUES(?,?,?,?,?)",
            rusqlite::params![task_id, step_id, steers, cursor, now],
        );
        ferb_orchestrator::sql::row(
            db.conn(),
            "SELECT * FROM loop_state WHERE task_id=? AND step_id=?",
            &[
                rusqlite::types::Value::Text(task_id.into()),
                rusqlite::types::Value::Text(step_id.into()),
            ],
        )
        .ok()
        .flatten()
        .unwrap_or_default()
    }

    fn save(&self, task_id: &str, step_id: &str, values: &[(&str, Value)]) {
        let known: Vec<&(&str, Value)> = values
            .iter()
            .filter(|(k, _)| LOOP_FIELDS.contains(k))
            .collect();
        if known.is_empty() {
            return;
        }
        let sets: Vec<String> = known.iter().map(|(k, _)| format!("{k}=?")).collect();
        let mut params: Vec<rusqlite::types::Value> =
            known.iter().map(|(_, v)| sql_of(v)).collect();
        params.push(rusqlite::types::Value::Text(self.inner.now_iso()));
        params.push(rusqlite::types::Value::Text(task_id.into()));
        params.push(rusqlite::types::Value::Text(step_id.into()));
        let db = self.inner.db();
        let _ = db.conn().execute(
            &format!(
                "UPDATE loop_state SET {}, updated_at=? WHERE task_id=? AND step_id=?",
                sets.join(", ")
            ),
            rusqlite::params_from_iter(params),
        );
    }

    fn texts(&self, task_id: &str, step_id: &str) -> (String, String) {
        let db = self.inner.db();
        db.conn()
            .query_row(
                "SELECT recent_json, note FROM loop_state WHERE task_id=? AND step_id=?",
                rusqlite::params![task_id, step_id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?
                            .unwrap_or_else(|| "[]".into()),
                        r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    ))
                },
            )
            .unwrap_or_else(|_| ("[]".into(), String::new()))
    }
}

/// supervisor.py:1040 `_HeldTabs`: the owner's own tabs a loop holds that the browser reports closed, each asked
/// about once.
#[derive(Default)]
struct HeldTabs {
    known: BTreeSet<String>,
}

impl HeldTabs {
    fn closed(inner: &Inner, task_id: &str) -> Vec<Value> {
        inner
            .deps
            .daemon
            .task_tabs(task_id)
            .into_iter()
            .filter(|t| {
                t.get("closed").is_some_and(ferb_py::value::truthy)
                    && t.get("taken_over_from").is_some_and(ferb_py::value::truthy)
            })
            .collect()
    }

    fn tab(t: &Value) -> String {
        t.get("tab").map(ferb_py::value::py_str).unwrap_or_default()
    }

    /// A round that didn't pause went past whatever closed before it.
    fn carried_on(&mut self, inner: &Inner, task_id: &str) {
        for t in Self::closed(inner, task_id) {
            self.known.insert(Self::tab(&t));
        }
    }

    /// Closed since the last round or question; known from now on.
    fn gone(&mut self, inner: &Inner, task_id: &str) -> Vec<Value> {
        let fresh: Vec<Value> = Self::closed(inner, task_id)
            .into_iter()
            .filter(|t| !self.known.contains(&Self::tab(t)))
            .collect();
        for t in &fresh {
            self.known.insert(Self::tab(t));
        }
        fresh
    }
}

/// A correction's hand-over to a resting loop. Python runs the turn and the loop on one thread: the correction wakes
/// the loop (`wake_loops`), the woken round runs when the turn next waits, starts its `loop.round` span, and waits in
/// turn (its change records are read in a thread) while the turn writes its reply; then the round goes on. On several
/// threads the round and the turn would race, so the turn waits until the round has started (`started`, or the loop
/// went back to sleep or ended without one) and the round waits until the reply is recorded (`released`).
#[derive(Default)]
pub struct SteerHold {
    started: Wake,
    released: Wake,
}

/// The longest a turn waits for a woken loop to start its round (a loop busy elsewhere: the reply goes on).
const HAND_OVER_S: f64 = 5.0;

impl Inner {
    /// After a correction is applied (`steering.apply`'s `wake_loops`): each corrected tree's loops are woken; a
    /// loop resting between rounds is waited for until its round has started (`SteerHold`).
    pub(crate) async fn hand_over(&self, turn: &Arc<Mutex<super::Turn>>) {
        let trees = std::mem::take(&mut turn.lock().unwrap().wake_after);
        for tree_id in trees {
            if !self.loop_resting_in(&tree_id) || self.live().steer_holds.contains_key(&tree_id) {
                self.wake_loops(&tree_id);
                continue;
            }
            let hold = Arc::new(SteerHold::default());
            self.live()
                .steer_holds
                .insert(tree_id.clone(), hold.clone());
            self.wake_loops(&tree_id);
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs_f64(HAND_OVER_S),
                hold.started.wait(),
            )
            .await;
            turn.lock().unwrap().held.push((tree_id, hold));
        }
    }

    /// A loop of this tree rests between rounds in its own sleep (watching or backing off), not waiting for the owner.
    fn loop_resting_in(&self, tree_id: &str) -> bool {
        let live = self.live();
        live.loop_status.iter().any(|((t, step), status)| {
            t == tree_id
                && live.resting.contains(&format!("{t}.loop.{step}"))
                && matches!(
                    status.get("state").and_then(Value::as_str),
                    Some("watching" | "backing_off")
                )
        })
    }

    /// The hand-over's round may go on (the turn's reply is recorded).
    pub(crate) fn release_hold(&self, tree_id: &str, hold: &Arc<SteerHold>) {
        let mut live = self.live();
        if live
            .steer_holds
            .get(tree_id)
            .is_some_and(|h| Arc::ptr_eq(h, hold))
        {
            live.steer_holds.remove(tree_id);
        }
        drop(live);
        hold.started.set();
        hold.released.set();
    }

    /// The woken loop of this tree will start no round now (it sleeps again, or ended): the turn goes on.
    fn steer_passed(&self, tree_id: &str) {
        if let Some(hold) = self.live().steer_holds.get(tree_id) {
            hold.started.set();
        }
    }

    /// A round's span is written: under a hand-over, it waits here (Python's thread read) until the reply is.
    async fn steer_round_started(&self, tree_id: &str) {
        let hold = self.live().steer_holds.get(tree_id).cloned();
        if let Some(hold) = hold.filter(|h| !h.started.is_set()) {
            hold.started.set();
            hold.released.wait().await;
        }
    }

    /// supervisor.py:5677 `wake_loops`: a stop, a pause or a plan event for this tree: its loops look now.
    pub fn wake_loops(&self, tree_id: &str) {
        let wakes = self
            .live()
            .loop_wakes
            .get(tree_id)
            .cloned()
            .unwrap_or_default();
        for wake in wakes {
            wake.set();
        }
    }

    /// supervisor.py:5682 `loop_statuses`: each running loop's latest `loop.status` (the notch's standing work and
    /// chat.status).
    pub fn loop_statuses(&self) -> Vec<Value> {
        self.live()
            .loop_status
            .iter()
            .map(|(_, v)| Value::Object(v.clone()))
            .collect()
    }

    /// supervisor.py:5652 `loop_resting`: a loop's task between its rounds (the owner's held tab is theirs again).
    pub fn loop_resting(&self, task_id: &str) -> bool {
        self.live().resting.contains(task_id)
    }

    /// supervisor.py:5686 `_loop_failed`: journaled (the journal redacts) and swallowed, so the loop runs on.
    fn loop_failed(&self, tree_id: &str, step_id: &str, what: &str, error: &str) {
        self.journal(
            "loop.failed",
            json!({"task_id": tree_id, "step": step_id, "what": what, "error": error}),
            None,
        );
    }

    /// supervisor.py:2198 `_paused_words`: the question of a loop's latest round as journaled, its first line.
    pub(crate) fn paused_words(&self, tree_id: &str, step_id: &str) -> String {
        let data: Option<String> = self
            .db()
            .conn()
            .query_row(
                "SELECT data_json FROM journal WHERE kind = 'loop.round' AND json_extract(data_json, '$.task_id') \
                 = ? AND json_extract(data_json, '$.step') = ? ORDER BY seq DESC LIMIT 1",
                rusqlite::params![tree_id, step_id],
                |r| r.get(0),
            )
            .ok();
        let data: Value = data
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or(Value::Null);
        let text = ["question", "summary"]
            .iter()
            .filter_map(|k| data.get(*k).filter(|v| ferb_py::value::truthy(v)))
            .map(ferb_py::value::py_str)
            .next()
            .unwrap_or_default();
        let first = ferb_orchestrator::py::strip(&text)
            .split('\n')
            .next()
            .unwrap_or("")
            .to_string();
        ferb_orchestrator::py::prefix(&first, 300).to_string()
    }

    /// supervisor.py:2224 `_loop_facts`: each running loop of this tree as the planner's task status shows it.
    pub(crate) fn loop_facts(&self, tree_id: &str) -> Vec<Value> {
        let statuses: Vec<((String, String), Map<String, Value>)> = self.live().loop_status.clone();
        let mut out = Vec::new();
        for ((tree, step), status) in statuses {
            if tree != tree_id {
                continue;
            }
            let goal = status
                .get("goal")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            let goal = goal.split_whitespace().collect::<Vec<_>>().join(" ");
            let state = status
                .get("state")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            let mut fact = obj(json!({"goal": ferb_orchestrator::py::prefix(&goal, 300),
                "state": state.replace('_', " "),
                "rounds": status.get("rounds").cloned().unwrap_or(json!(0))}));
            if state == "paused" {
                let question = self.paused_words(&tree, &step);
                if !question.is_empty() {
                    fact.insert("question".into(), json!(question));
                }
            }
            out.push(Value::Object(fact));
        }
        out
    }

    /// supervisor.py:2205 `_pause_question`: a paused loop's question, its options and its origin.
    fn pause_question(
        &self,
        tree_id: &str,
        step: &Step,
        result: Option<&RoundResult>,
        unknown: bool,
        gone: &[Value],
    ) -> (String, Vec<String>, String) {
        if let Some(tab) = gone.first().filter(|_| !(result.is_some() && unknown)) {
            let page = tab
                .get("taken_over_from")
                .and_then(|t| t.get("url"))
                .filter(|u| ferb_py::value::truthy(u))
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            let host = if page.is_empty() {
                String::new()
            } else {
                ferb_py::url::hostname(&page)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            };
            let what = if host.is_empty() {
                "The tab".to_string()
            } else {
                format!("The {host} tab")
            };
            return (
                format!("{what} FERB was watching was closed. Stop watching?"),
                vec![LOOP_STOP.into(), LOOP_KEEP.into()],
                origin::FERB.into(),
            );
        }
        let said = result
            .map(|r| {
                r.question
                    .clone()
                    .filter(|q| !q.is_empty())
                    .unwrap_or_else(|| r.summary.clone())
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.paused_words(tree_id, &step.id));
        let said = if said.is_empty() {
            format!(
                "I paused keeping this going: {}.",
                ferb_orchestrator::names::short_label(&step.goal, 60)
            )
        } else {
            said
        };
        let words = said.split_whitespace().collect::<Vec<_>>().join(" ");
        (
            ferb_policy::confirm::task_question(&ferb_sanitize::terminal::printable(&words)),
            vec![LOOP_STOP.into()],
            self.task_made(tree_id),
        )
    }

    /// supervisor.py:5692 `_unknown_actions`: the ledger rows this task wrote whose outcome is unknown (P19).
    fn unknown_actions(&self, tree_id: &str, task_id: &str) -> BTreeSet<String> {
        self.deps
            .daemon
            .unresolved(tree_id)
            .into_iter()
            .filter(|row| {
                row.get("task_id").and_then(Value::as_str) == Some(task_id)
                    && row.get("state").and_then(Value::as_str) == Some("unknown")
            })
            .filter_map(|row| row.get("id").map(ferb_py::value::py_str))
            .collect()
    }

    /// supervisor.py:5697 `_span_cursor`: a span's place in the record (its rowid).
    fn span_cursor(&self, span_id: &str) -> i64 {
        self.db()
            .conn()
            .query_row("SELECT rowid FROM spans WHERE id = ?", [span_id], |r| {
                r.get(0)
            })
            .unwrap_or(0)
    }

    /// supervisor.py:5702 `_probe_exposure`: what this round is told from the loop's probe is outside content the
    /// task read (MP2, S4): one context exposure with the text shown and the origins of the probe reads since the
    /// loop's last round.
    fn probe_exposure(&self, task_id: &str, cause: &Reason) {
        let mut shown: Vec<String> = Vec::new();
        if let Some(Observed::Value(v)) = &cause.observed {
            shown.push(ferb_context::pyvalue::dumps(
                v,
                ferb_context::pyvalue::Dumps::UTF8,
            ));
        } else if let Some(Observed::Missing) = &cause.observed {
            // MISSING is never shown (`cause.get('observed', MISSING) is not MISSING`)
        }
        for key in ["error", "probe_refused"] {
            if let Some(v) = cause.get(key).filter(|v| ferb_py::value::truthy(v)) {
                shown.push(ferb_py::value::py_str(v));
            }
        }
        if shown.is_empty() {
            return;
        }
        let read: BTreeSet<String> = {
            let db = self.db();
            let last: i64 = db
                .conn()
                .query_row(
                    "SELECT COALESCE(MAX(rowid), 0) FROM spans WHERE kind = 'loop.round' AND task_id = ?",
                    [task_id],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let mut read = BTreeSet::new();
            if let Ok(mut stmt) = db.conn().prepare(
                "SELECT input_json, origin FROM spans WHERE kind = 'loop.probe_read' AND task_id = ? AND rowid > ?",
            ) {
                let rows = stmt.query_map(rusqlite::params![task_id, last], |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                    ))
                });
                if let Ok(rows) = rows {
                    for (input, made) in rows.flatten() {
                        let input: Value = input
                            .and_then(|i| serde_json::from_str(&i).ok())
                            .unwrap_or(Value::Null);
                        if input.get("read").and_then(Value::as_str) != Some("baseline") {
                            if let Some(m) = made.filter(|m| !m.is_empty()) {
                                read.extend(origin::split(Some(&m)));
                            }
                        }
                    }
                }
            }
            read
        };
        if !read.is_empty() {
            let exposure = self.span_start(
                "context.exposure",
                Some(task_id),
                None,
                None,
                json!({"source_refs": [], "reference": "loop probe read"}),
            );
            self.span_finish(
                &exposure,
                Some(json!({"payload": shown.join("\n")})),
                None,
                Some(&origin::join(read.iter().map(String::as_str))),
                None,
            );
        }
    }

    /// supervisor.py:5723 `_resolved_probe`: the probe's tool resolved to its registered name once.
    fn resolved_probe(&self, probe: Option<Probe>) -> Option<Probe> {
        let mut probe = probe?;
        let (name, _) =
            ferb_orchestrator::probes::resolve_tool(&probe.tool, &self.deps.daemon.tool_names());
        probe.tool = name;
        Some(probe)
    }

    /// supervisor.py:6814 `_look_only`: a loop round that reported `continue` with a probe and only looked: why its
    /// model check may be skipped; "" when the check runs. `_look_only_note` (supervisor.py:6825) is inlined: a
    /// probe that won't arm keeps the check; the round's calls since its cursor are read with their outputs.
    pub(crate) fn look_only(&self, tree: &Tree) -> String {
        let (since, action, probe) = {
            let st = tree.lock();
            (st.calls_since, st.loop_action.clone(), st.probe.clone())
        };
        if since == 0 || action.as_deref() != Some("continue") || probe.is_none() {
            return String::new();
        }
        let Some(probe) = self.resolved_probe(probe) else {
            return String::new();
        };
        let task = &tree.worker;
        let names = self.deps.daemon.tool_names();
        let scopes: Vec<String> = task.scopes.iter().cloned().collect();
        let cap = self.deps.daemon.probe_cap(&probe.tool);
        if ferb_orchestrator::probes::resolve_tool(&probe.tool, &names)
            .1
            .is_some()
            || ferb_orchestrator::probes::refusal(
                &probe,
                cap.as_deref(),
                &scopes,
                task.identity.as_str(),
            )
            .is_some()
        {
            return String::new();
        }
        let calls: Vec<Map<String, Value>> = {
            let db = self.db();
            let Ok(mut stmt) = db.conn().prepare(
                "SELECT input_json, output_json FROM spans WHERE task_id = ? AND kind = 'tool.call' AND rowid > ? \
                 ORDER BY rowid",
            ) else {
                return String::new();
            };
            let Ok(rows) = stmt.query_map(rusqlite::params![task.id, since], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                ))
            }) else {
                return String::new();
            };
            let mut calls = Vec::new();
            for (input, output) in rows.flatten() {
                let Some(mut call) = input
                    .and_then(|i| serde_json::from_str::<Value>(&i).ok())
                    .and_then(|v| v.as_object().cloned())
                else {
                    return String::new();
                };
                call.insert(
                    "output".into(),
                    output
                        .filter(|o| !o.is_empty())
                        .and_then(|o| serde_json::from_str(&o).ok())
                        .unwrap_or(Value::Null),
                );
                calls.push(call);
            }
            calls
        };
        let held: Vec<String> = self
            .deps
            .daemon
            .task_tabs(&task.id)
            .iter()
            .flat_map(|t| [t.get("tab"), t.get("tab_ref")])
            .flatten()
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .collect();
        let daemon = self.deps.daemon.clone();
        let caps = move |tool: &str| daemon.probe_cap(tool);
        match loops::look_only_round(&calls, Some(&probe), &caps, &held) {
            Some(note) if !note.is_empty() => {
                let shown = self.redact(&json!({"note": note}));
                shown
                    .get("note")
                    .map(ferb_py::value::py_str)
                    .unwrap_or_default()
            }
            _ => String::new(),
        }
    }

    /// supervisor.py:5992 `_judged`: one check of `out` against `conditions`; None when it couldn't run (journaled).
    /// Its findings about the answer get the member's one fix round (ORCH-545).
    pub(crate) async fn judged(
        &self,
        member: &Arc<Tree>,
        out: &Outcome,
        conditions: Vec<Condition>,
        span: &str,
    ) -> Flow<Option<Vec<ConditionResult>>> {
        let state: TreeState = {
            let mut st = member.lock().clone();
            if let Some(spec) = st.spec.as_mut() {
                spec.success_conditions = conditions;
            }
            st.outcome = Some(out.clone());
            st.verification.clear();
            st.confirmed.clear();
            st
        };
        let judge = Arc::new(Tree {
            id: member.id.clone(),
            worker: member.worker.clone(),
            step_id: member.step_id.clone(),
            asks_in_place: member.asks_in_place,
            st: Mutex::new(state),
            steering: member.steering.clone(),
            result_revision: Mutex::new(*member.result_revision.lock().unwrap()),
        });
        let (results, checked) = match self.verify_tree(&judge, span).await {
            Ok(v) => v,
            Err(Failure::Cancelled) => return Err(Failure::Cancelled),
            Err(Failure::NotPorted(n)) => return Err(Failure::NotPorted(n)),
            Err(e) => {
                self.journal(
                    "task.inspection_failed",
                    json!({"task_id": member.worker.id, "error": format!("{}: {}", e.class(), e.message())}),
                    None,
                );
                return Ok(None);
            }
        };
        if let Some(checked) = checked {
            if !checked.findings.is_empty()
                && out.status == OutcomeStatus::Done
                && member.lock().stop.is_empty()
                && !results.iter().any(|r| r.met == Some(false))
            {
                self.settle_findings_of(member, &member.worker, &checked.findings, span)
                    .await;
            }
        }
        Ok(Some(results))
    }

    /// supervisor.py:6020 `_answer_record`: what an answering loop's or conversation's end check confirmed, indexed
    /// like the answered conditions.
    pub(crate) fn answer_record(
        results: &[ConditionResult],
        answers: &[Condition],
        offset: i64,
    ) -> Map<String, Value> {
        let unmet: Vec<String> = results
            .iter()
            .filter(|r| r.met != Some(true))
            .filter_map(|r| answers.get((r.index - offset) as usize))
            .map(|c| {
                if c.owner_text.is_empty() {
                    c.text.clone()
                } else {
                    c.owner_text.clone()
                }
            })
            .collect();
        let verified = unmet.is_empty() && results.len() == answers.len();
        let note = if !unmet.is_empty() {
            unmet.join("; ")
        } else if verified {
            String::new()
        } else {
            NOTHING_TO_CHECK.to_string()
        };
        let verification: Vec<Value> = results
            .iter()
            .map(|r| {
                ConditionResult {
                    index: r.index - offset,
                    ..r.clone()
                }
                .as_dict()
            })
            .collect();
        obj(json!({"verified": verified,
            "note": if note.is_empty() { String::new() } else { crate::lines::clipped(&note) },
            "verification": verification}))
    }

    /// supervisor.py:5980 `_kept_record`: an answer record of a member whose fix round kept a contradicted sentence
    /// marked: never verified, with code's standard "Couldn't confirm" line (ORCH-547).
    pub(crate) fn kept_record(member: &Tree, record: Map<String, Value>) -> Map<String, Value> {
        if !member.lock().kept_contradicted {
            return record;
        }
        let note = record
            .get("note")
            .filter(|n| ferb_py::value::truthy(n))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let note = if note.contains(REPORT_UNCONFIRMED) {
            note
        } else {
            crate::lines::clipped(
                &[note.as_str(), REPORT_UNCONFIRMED]
                    .iter()
                    .filter(|p| !p.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        };
        let mut out = record;
        out.insert("verified".into(), json!(false));
        out.insert("note".into(), json!(note));
        out
    }

    /// supervisor.py:6040 `_with_answer_check`: a finished loop's or conversation's output with its end check of the
    /// task's conditions (several prepared conversations: confirmed only when each is), and its answer.
    pub(crate) fn with_answer_check(
        &self,
        tree_id: &str,
        step_id: &str,
        result: Map<String, Value>,
    ) -> Map<String, Value> {
        let checks: Vec<Map<String, Value>> = {
            let mut live = self.live();
            let mut taken = Vec::new();
            live.answer_checks.retain(|((t, s), record)| {
                let mine = t == tree_id && (s == step_id || s.starts_with(&format!("{step_id}-")));
                if mine {
                    taken.push(record.clone());
                }
                !mine
            });
            taken
        };
        if result.get("status").and_then(Value::as_str) != Some("done") || checks.is_empty() {
            return result;
        }
        let answer = result
            .get("summary")
            .filter(|s| ferb_py::value::truthy(s))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let mut out = result;
        if checks.len() == 1 {
            out.extend(checks[0].clone());
            out.insert("answer".into(), json!(answer));
            return out;
        }
        let mut notes: Vec<String> = Vec::new();
        for c in &checks {
            let n = c
                .get("note")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            if !n.is_empty() && !notes.contains(&n) {
                notes.push(n);
            }
        }
        out.insert("answer".into(), json!(answer));
        out.insert(
            "verified".into(),
            json!(checks
                .iter()
                .all(|c| c.get("verified").is_some_and(ferb_py::value::truthy))),
        );
        out.insert(
            "note".into(),
            json!(crate::lines::clipped(&notes.join("; "))),
        );
        out
    }

    /// supervisor.py:5956 `_loop_end_reached`: a round reported the owner's end condition met: a separate check of
    /// that condition decides; when the loop is an answer step it also judges the task's conditions it answers.
    async fn loop_end_reached(
        &self,
        member: &Arc<Tree>,
        step: &Step,
        out: &Outcome,
        span: &str,
        answers: &[Condition],
    ) -> Flow<bool> {
        let mut conditions = vec![plain_condition(step.end_when())];
        conditions.extend(answers.iter().cloned());
        let Some(results) = self.judged(member, out, conditions, span).await? else {
            return Ok(false);
        };
        let finished = results.iter().any(|r| r.index == 0)
            && results
                .iter()
                .filter(|r| r.index == 0)
                .all(|r| r.met == Some(true));
        if finished && !answers.is_empty() {
            let kept: Vec<ConditionResult> = results
                .iter()
                .filter(|r| 1 <= r.index && r.index <= answers.len() as i64)
                .cloned()
                .collect();
            let record = Self::kept_record(member, Self::answer_record(&kept, answers, 1));
            let key = (member.id.clone(), step.id.clone());
            let mut live = self.live();
            live.answer_checks.retain(|(k, _)| *k != key);
            live.answer_checks.push((key, record));
        }
        Ok(finished)
    }

    /// supervisor.py:5643 `_loop_rest`: the browser drivers hear a loop's round starts or ends.
    async fn loop_rest(&self, task_id: &str, resting: bool) {
        if let Err(e) = self.deps.daemon.loop_rest(task_id, resting).await {
            self.journal(
                "loop.failed",
                json!({"task_id": task_id, "what": "rest", "error": e}),
                None,
            );
        }
    }

    /// supervisor.py:5735 `_loop_step`: a loop step, on LoopRunner.
    pub(crate) async fn loop_step(
        &self,
        tree: &Arc<Tree>,
        step: &Step,
        span: &str,
        prior: Map<String, Value>,
        answers: Vec<Condition>,
    ) -> Flow<Map<String, Value>> {
        let inner = self.me.upgrade().expect("the supervisor");
        let spec = tree.spec();
        let conditions: Vec<Condition> = if step.conditions.is_empty() {
            vec![plain_condition(&step.goal)]
        } else {
            step.conditions.iter().map(|t| plain_condition(t)).collect()
        };
        let mut child = tree.worker_task();
        child.id = format!("{}.loop.{}", tree.id, step.id);
        child.goal = step.goal.clone();
        child.success_conditions = conditions.iter().map(|c| c.text.clone()).collect();
        if !self.task_known(&child.id) {
            self.task_add(&child, span, &tree.id, "planned");
        }
        let holder = Arc::new(Tree {
            id: tree.id.clone(),
            worker: child.clone(),
            step_id: super::tree::WORKER.into(),
            asks_in_place: true,
            st: Mutex::new(TreeState {
                spec: Some(spec.clone()),
                accepting_updates: true,
                ..TreeState::default()
            }),
            steering: tree.steering.clone(),
            result_revision: Mutex::new(0),
        });
        // a stop before the first round reaches the loop's task id too
        self.live().tree_of.insert(child.id.clone(), holder.clone());
        self.runs.set_tree(&child.id, holder.clone());
        let mut handed = obj(json!({"prior": prior}));
        let earlier = self.plans(|p| p.earlier(&tree.id)).unwrap_or_default();
        if !earlier.is_empty() {
            handed.insert(
                "earlier_plan_results".into(),
                Value::Object(earlier.into_iter().collect()),
            );
        }
        let host = RoundHost {
            inner: inner.clone(),
            tree: tree.clone(),
            step: step.clone(),
            child: child.clone(),
            span: span.to_string(),
            conditions,
            answers,
            handed,
            closed: Mutex::new(HeldTabs::default()),
            steers: AtomicI64::new(0),
            unknown: AtomicBool::new(false),
            abort: Mutex::new(None),
            aborted: tokio::sync::Notify::new(),
            wake: std::sync::OnceLock::new(),
        };
        let rows = LoopStoreRows {
            inner: inner.clone(),
        };
        let cursor = self.plans(|p| p.event_cursor()).unwrap_or(0);
        let mut runner = LoopRunnerShared::new(
            &rows as &(dyn LoopRows + Send + Sync),
            &host as &(dyn LoopHost + Send + Sync),
            &tree.id,
            step.clone(),
            cursor,
        );
        let wake: Arc<Wake> = runner.wake.clone();
        let _ = host.wake.set(wake.clone());
        self.live()
            .loop_wakes
            .entry(tree.id.clone())
            .or_default()
            .push(wake.clone());
        let ended = {
            let run = runner.run();
            tokio::pin!(run);
            tokio::select! {
                biased;
                failure = host.aborted() => Err(failure),
                ended = &mut run => Ok(ended),
            }
        };
        self.steer_passed(&tree.id); // a hand-over waiting for this loop's round gets none
        {
            let mut live = self.live();
            if let Some(wakes) = live.loop_wakes.get_mut(&tree.id) {
                wakes.retain(|w| !Arc::ptr_eq(w, &wake));
                if wakes.is_empty() {
                    live.loop_wakes.remove(&tree.id);
                }
            }
            live.resting.remove(&child.id);
        }
        if !self.live().suspending {
            let key = (tree.id.clone(), step.id.clone());
            self.live().loop_status.retain(|(k, _)| *k != key);
            self.release_resources(&holder, span).await;
            let (resources, errors) = {
                let st = holder.lock();
                (st.resources.clone(), st.cleanup_errors.clone())
            };
            let mut st = tree.lock();
            st.resources.extend(resources);
            st.cleanup_errors.extend(errors);
        }
        let ended = ended?;
        let status = ended
            .get("status")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        self.set_status(
            &child.id,
            &status,
            Some(&json!({"summary": ended.get("summary").cloned().unwrap_or(Value::Null)})),
        );
        Ok(ended)
    }

    /// One round (`_loop_step`'s `round_play`): an ordinary worker run with its check, under the loop's task id.
    async fn round_play(&self, h: &RoundHost, cause: Reason) -> Flow<RoundResult> {
        let (tree, step, child) = (&h.tree, &h.step, &h.child);
        let base = tree.spec();
        let mut spec = base.clone();
        spec.plan = None;
        spec.goal = step.goal.clone();
        spec.worker_type = step.worker_type.clone();
        spec.earlier_results = step_results(&base, step);
        spec.success_conditions = h.conditions.clone();
        let wrap = self.deps.wrap.clone();
        let wrap: ferb_orchestrator::ports::Wrap = Arc::new(move |t: &str| wrap(t));
        let why = loops::round_reason(&cause, &wrap);
        let tabs = self.deps.daemon.task_tabs(&child.id);
        let digest = self.loop_digest(&tree.id, &step.id);
        let mut context = Map::new();
        context.insert(
            "loop".into(),
            json!({"goal": step.goal, "end_when": step.end_when(), "why_this_round": Value::Object(why)}),
        );
        context.insert("earlier_rounds".into(), digest);
        if !tabs.is_empty() {
            context.insert("held_tabs".into(), Value::Array(tabs));
        }
        context.extend(h.handed.clone());
        context.insert("instruction".into(), json!(loops::LOOP_ROUND));
        let prior_task = tree.lock().prior_task.clone();
        let member = Arc::new(Tree {
            id: tree.id.clone(),
            worker: child.clone(),
            step_id: step.id.clone(),
            asks_in_place: false,
            st: Mutex::new(TreeState {
                spec: Some(spec),
                prior_task,
                parent_goal: base.goal.clone(),
                hold_resources: true,
                step_context: ferb_py::json::dumps(&Value::Object(context)),
                accepting_updates: true,
                ..TreeState::default()
            }),
            steering: tree.steering.clone(),
            result_revision: Mutex::new(0),
        });
        self.live().tree_of.insert(child.id.clone(), member.clone());
        self.runs.set_tree(&child.id, member.clone());
        self.probe_exposure(&child.id, &cause);
        let round_span = self.span_start(
            "loop.round",
            Some(&child.id),
            Some(&h.span),
            None,
            json!({"step": step.id, "number": cause.get("number").cloned().unwrap_or(Value::Null),
                   "why": cause.get("why").cloned().unwrap_or(Value::Null)}),
        );
        // its check judges this round's calls only, and only the files this round changed (L39)
        member.lock().calls_since = self.span_cursor(&round_span);
        let stuck = self.deps.daemon.changes_reset(&child.id);
        // Python reads the change records in a thread (`asyncio.to_thread`): the round yields here, and the thread's
        // answer comes back on a later turn of the event loop (what was ready meanwhile runs first); a correction's
        // turn that woke this round writes its reply meanwhile (`SteerHold`)
        self.steer_round_started(&tree.id).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        if !stuck.is_empty() {
            self.journal(
                "changes.cleanup_failed",
                json!({"task_id": child.id, "paths": stuck.len()}),
                None,
            );
        }
        let before = self.unknown_actions(&tree.id, &child.id);
        let ran: Flow<(Outcome, bool, bool)> = async {
            let out = self.worker_loop(&member, &round_span).await?;
            let unknown = !self.unknown_actions(&tree.id, &child.id).is_subset(&before);
            let finished = !unknown
                && member.lock().loop_action.as_deref() == Some("finished")
                && !ferb_orchestrator::py::strip(step.end_when()).is_empty()
                && out.status == OutcomeStatus::Done
                && member.lock().stop.is_empty()
                && tree.lock().stop.is_empty()
                && self
                    .loop_end_reached(&member, step, &out, &round_span, &h.answers)
                    .await?;
            if self.live().suspending {
                // FERB is shutting down: the round reports nothing; it stays 'acting' and re-runs after the restart
                return Err(Failure::Cancelled);
            }
            Ok((out, unknown, finished))
        }
        .await;
        let (out, unknown, finished) = match ran {
            Ok(v) => v,
            Err(e) => {
                let error = if self.live().suspending {
                    SHUT_DOWN.to_string()
                } else {
                    format!("{}: {}", e.class(), e.message())
                };
                self.span_finish(&round_span, None, Some(&error), None, None);
                return Err(e);
            }
        };
        // the round's checker (its check, and any end check) is done: its tabs and processes go now, into the loop's
        // lists; what the round's worker started stays the loop's until it ends
        let holder = self
            .live()
            .tree_of
            .get(&child.id)
            .cloned()
            .unwrap_or_else(|| member.clone());
        for tid in self.helper_ids(&child.id) {
            self.release_task(&holder, &tid, &round_span).await;
        }
        let (mut status, mut question, answer, action, probe, unchecked) = {
            let st = member.lock();
            (
                out.status.as_str().to_string(),
                st.owner_question.clone(),
                st.owner_answer.clone(),
                st.loop_action.clone(),
                st.probe.clone(),
                !st.unchecked.is_empty(),
            )
        };
        let probe = self.resolved_probe(probe);
        if unknown && status != "needs_user" && status != "blocked" {
            status = "blocked".into();
            question = LOOP_UNKNOWN.into(); // the loop waits for the owner
        }
        let summary = if answer.is_empty() {
            out.summary.clone()
        } else {
            answer
        };
        let result = obj(
            json!({"status": status, "summary": summary, "evidence": out.evidence,
            "loop_action": action, "finished_verified": finished, "question": question,
            "unknown": unknown, "unchecked": unchecked}),
        );
        if !(action.as_deref() == Some("blocked") || status == "needs_user" || status == "blocked")
        {
            h.closed.lock().unwrap().carried_on(self, &child.id);
        }
        let mut shown = result.clone();
        shown.insert(
            "probe".into(),
            probe
                .as_ref()
                .map(|p| ferb_orchestrator::validate::dump_probe(p).to_value())
                .unwrap_or(Value::Null),
        );
        self.span_finish(&round_span, Some(Value::Object(shown)), None, None, None);
        let mut data = obj(json!({"task_id": tree.id, "step": step.id,
            "number": cause.get("number").cloned().unwrap_or(Value::Null)}));
        data.extend(result);
        let made = self.task_made(&tree.id);
        self.journal("loop.round", Value::Object(data), Some(&made));
        h.unknown.store(unknown, Ordering::SeqCst);
        let revision = *member.result_revision.lock().unwrap() as i64;
        Ok(RoundResult {
            status,
            summary,
            loop_action: action,
            steers_seen: revision,
            question: Some(question),
            finished_verified: finished,
            evidence: out.evidence.iter().map(|e| json!(e)).collect(),
            probe,
            unchecked,
        })
    }

    /// loops.py:113 `LoopStore.digest`: the rounds so far and the latest results.
    pub(crate) fn loop_digest(&self, tree_id: &str, step_id: &str) -> Value {
        let row: Option<(i64, String)> = self
            .db()
            .conn()
            .query_row(
                "SELECT rounds, recent_json FROM loop_state WHERE task_id=? AND step_id=?",
                rusqlite::params![tree_id, step_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        match row {
            Some((rounds, recent)) => json!({"rounds": rounds,
                "recent": serde_json::from_str::<Value>(&recent).unwrap_or(json!([]))}),
            None => json!({"rounds": 0, "recent": []}),
        }
    }

    /// supervisor.py:6006 `_helper_ids`: the task ids that run around one worker and hold what they open: its
    /// checker, and the fix round's turn and re-check when they ran (ORCH-548).
    pub(crate) fn helper_ids(&self, task_id: &str) -> Vec<String> {
        let mut ids = vec![format!("{task_id}.verify")];
        for extra in ["fix", "recheck"] {
            let tid = format!("{task_id}.{extra}");
            if self.task_known(&tid) {
                ids.push(tid);
            }
        }
        ids
    }
}

/// `_loop_step`'s callbacks, as the runner's host.
struct RoundHost {
    inner: Arc<Inner>,
    tree: Arc<Tree>,
    step: Step,
    child: Task,
    span: String,
    conditions: Vec<Condition>,
    answers: Vec<Condition>,
    handed: Map<String, Value>,
    closed: Mutex<HeldTabs>,
    /// The tree's corrections as last read (the steering lock is held only briefly; a busy one reads this).
    steers: AtomicI64,
    /// The last round left an action whose outcome is unknown (P19).
    unknown: AtomicBool,
    /// Why a round ended without a result (a shutdown, a path not ported): the step ends with it.
    abort: Mutex<Option<Failure>>,
    aborted: tokio::sync::Notify,
    /// The runner's wake (its own sleep, kept as the runner's: `sleep` below).
    wake: std::sync::OnceLock<Arc<Wake>>,
}

impl RoundHost {
    async fn aborted(&self) -> Failure {
        loop {
            let notified = self.aborted.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(f) = self.abort.lock().unwrap().clone() {
                return f;
            }
            notified.await;
        }
    }

    fn abort_with(&self, failure: Failure) {
        *self.abort.lock().unwrap() = Some(failure);
        self.aborted.notify_waiters();
    }

    fn failed(&self, what: &str, error: &str) {
        self.inner
            .loop_failed(&self.tree.id, &self.step.id, what, error);
    }

    /// `play`: the round drives the loop's tabs; between rounds the owner's held tab is theirs again.
    async fn play(&self, cause: Reason) -> Flow<RoundResult> {
        let inner = &self.inner;
        inner.work_opened().await; // L34 Phase G: no round starts during an update's hold
        inner.live().resting.remove(&self.child.id);
        inner.loop_rest(&self.child.id, false).await;
        let out = inner.round_play(self, cause).await;
        inner.live().resting.insert(self.child.id.clone());
        inner.loop_rest(&self.child.id, true).await;
        out
    }
}

impl LoopHost for RoundHost {
    fn round(&self, reason: Reason) -> BoxFuture<'_, RoundResult> {
        Box::pin(async move {
            match self.play(reason).await {
                Ok(r) => r,
                Err(f @ (Failure::Cancelled | Failure::NotPorted(_))) => {
                    // Python's cancellation (or a path not ported): the round never answers; the step ends with it
                    self.abort_with(f);
                    std::future::pending::<RoundResult>().await
                }
                Err(e) => {
                    // a round that crashed is a failed round: the loop runs on
                    let words = format!("{}: {}", e.class(), e.message());
                    self.failed("round", &words);
                    let err = ferb_worker::Error::other(&e.class(), e.message());
                    let route = || self.inner.deps.daemon.has_route();
                    let summary = ferb_worker::errors::couldnt(
                        "finish this round",
                        &err,
                        Some(&|t| self.inner.redact_text(t)),
                        &route,
                    );
                    RoundResult {
                        status: "failed".into(),
                        summary,
                        question: Some(String::new()),
                        ..RoundResult::default()
                    }
                }
            }
        })
    }

    fn arm(&self, probe: &Probe) -> Option<String> {
        let names = self.inner.deps.daemon.tool_names();
        if let (_, Some(why)) = ferb_orchestrator::probes::resolve_tool(&probe.tool, &names) {
            return Some(why);
        }
        let cap = self.inner.deps.daemon.probe_cap(&probe.tool);
        let scopes: Vec<String> = self.child.scopes.iter().cloned().collect();
        ferb_orchestrator::probes::refusal(
            probe,
            cap.as_deref(),
            &scopes,
            self.child.identity.as_str(),
        )
    }

    fn probe<'a>(&'a self, probe: &'a Probe) -> BoxFuture<'a, ProbeRead> {
        Box::pin(async move {
            let ferb = vec![origin::FERB.to_string()];
            let args = match probe.args() {
                Ok(a) => Value::Object(
                    a.into_iter()
                        .map(|(k, v)| (k, v.to_value()))
                        .collect::<Map<String, Value>>(),
                ),
                Err(e) => {
                    let words = e.to_string();
                    self.failed("probe", &words);
                    return (Observed::Missing, Some(words), ferb);
                }
            };
            let args = args.as_object().cloned().unwrap_or_default();
            let read = self
                .inner
                .deps
                .daemon
                .probe_read(
                    &self.child.id,
                    &format!("{}.probe", self.child.id),
                    &self.child.scopes,
                    self.child.identity.as_str(),
                    &probe.tool,
                    &args,
                    &self.span,
                )
                .await;
            let got = match read {
                Ok(got) => got,
                Err(e) => {
                    self.failed("probe", &e);
                    return (Observed::Missing, Some(self.inner.redact_text(&e)), ferb);
                }
            };
            let origins: Vec<String> = got
                .get("origins")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                .unwrap_or_default();
            let content = got.get("content").cloned().unwrap_or(Value::Null);
            if got.get("is_error").is_some_and(ferb_py::value::truthy) {
                let error = match &content {
                    Value::Object(m) => m.get("error").cloned().unwrap_or(Value::Null),
                    other => other.clone(),
                };
                let error = if ferb_py::value::truthy(&error) {
                    ferb_py::value::py_str(&error)
                } else {
                    "the probe failed".into()
                };
                return (Observed::Missing, Some(error), origins);
            }
            let content = ferb_context::Py::from_value(&content);
            let value = match ferb_orchestrator::probes::field_value(&content, &probe.field) {
                Some(v) => Observed::Value(v.clone()),
                None => Observed::Missing,
            };
            (value, None, origins)
        })
    }

    fn record(
        &self,
        kind: &str,
        probe: &Probe,
        value: &Observed,
        error: Option<&str>,
        origins: &[String],
        reads: i64,
    ) {
        let inner = &self.inner;
        let s = inner.span_start(
            "loop.probe_read",
            Some(&self.child.id),
            Some(&self.span),
            None,
            json!({"step": self.step.id, "read": kind,
                   "probe": ferb_orchestrator::validate::dump_probe(probe).to_value(),
                   "reads_since_armed": reads}),
        );
        let shown = match value {
            Observed::Missing => Value::Null,
            Observed::Value(v) => v.to_value(),
        };
        let made = if origins.is_empty() {
            origin::FERB.to_string()
        } else {
            origin::join(origins.iter().map(String::as_str))
        };
        inner.span_finish(&s, Some(json!({"value": shown})), error, Some(&made), None);
    }

    fn event(&self, cursor: i64) -> Option<(i64, Value)> {
        let on = self.step.wakes_on().unwrap_or("").to_string();
        match self
            .inner
            .plans(|p| p.event_seq_after(&on, cursor, self.step.wakes_match()))
        {
            Ok(found) => found,
            Err(e) => {
                self.failed("event", &e.to_string());
                None
            }
        }
    }

    fn has_event(&self) -> bool {
        self.step.wakes_on().is_some()
    }

    /// The runner's own wakeable sleep (loops.py `_sleep`: never longer than POLL_S); a loop going back to sleep
    /// starts no round for a correction that woke it (`SteerHold`).
    fn sleep(&self, seconds: f64) -> Option<BoxFuture<'_, ()>> {
        let wake = self.wake.get()?.clone();
        Some(Box::pin(async move {
            if !wake.is_set() {
                self.inner.steer_passed(&self.tree.id);
            }
            let wait = seconds.clamp(0.0, loops::POLL_S);
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs_f64(wait), wake.wait()).await;
            wake.clear();
        }))
    }

    fn steers(&self) -> i64 {
        if let Ok(steering) = self.tree.steering.try_lock() {
            self.steers
                .store(steering.messages.len() as i64, Ordering::SeqCst);
        }
        self.steers.load(Ordering::SeqCst)
    }

    fn notify<'a>(&'a self, text: &'a str, model_written: bool) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let inner = &self.inner;
            // a pause quotes the round's own words: model-written from what the task read (MP2)
            let made = inner.task_made(&self.tree.id);
            inner.journal(
                "loop.notice",
                json!({"task_id": self.tree.id, "step": self.step.id, "text": text}),
                Some(&made),
            );
            let told = if model_written {
                origin::join([origin::FERB, origin::INFERRED])
            } else {
                origin::FERB.to_string()
            };
            if let Err(e) = inner
                .deps
                .daemon
                .loop_notice(&self.tree.id, &self.step.id, text, &told)
                .await
            {
                self.failed("notice", &e);
            }
        })
    }

    fn status(&self, status: Value) {
        let mut data =
            obj(json!({"task_id": self.tree.id, "step": self.step.id, "goal": self.step.goal}));
        if let Value::Object(view) = status {
            data.extend(view);
        }
        {
            let key = (self.tree.id.clone(), self.step.id.clone());
            let mut live = self.inner.live();
            match live.loop_status.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => *v = data.clone(),
                None => live.loop_status.push((key, data.clone())),
            }
        }
        self.inner
            .deps
            .daemon
            .publish("loop.status", Value::Object(data));
    }

    fn stopped(&self) -> bool {
        self.inner.stopped(&self.tree) || self.abort.lock().unwrap().is_some()
    }

    fn now(&self) -> f64 {
        let t = self.inner.now();
        t.timestamp() as f64 + f64::from(t.timestamp_subsec_micros()) / 1e6
    }

    fn wait_owner(&self, result: Option<RoundResult>) -> BoxFuture<'_, Option<Reason>> {
        Box::pin(async move {
            let inner = &self.inner;
            let tree = &self.tree;
            let reply: OwnerReply = if result.is_none() && inner.is_paused(tree) {
                // a tab closed before this pause is known from now on
                let _ = self.closed.lock().unwrap().gone(inner, &self.child.id);
                inner.hold_pause(tree, &self.child, &self.step.id).await
            } else {
                let gone = self.closed.lock().unwrap().gone(inner, &self.child.id);
                let unknown = self.unknown.load(Ordering::SeqCst);
                let (question, options, made) =
                    inner.pause_question(&tree.id, &self.step, result.as_ref(), unknown, &gone);
                let options: Vec<&str> = options.iter().map(String::as_str).collect();
                let wait = Wait::new(&tree.id, &question, &options, &made, LOOP_ASK);
                let kind = if options.contains(&LOOP_KEEP) {
                    "closed_tab"
                } else {
                    "loop"
                };
                wait.lock()
                    .extra
                    .insert("question_kind".into(), json!(kind));
                let wait = Arc::new(wait);
                let reply = inner
                    .wait_for_owner(tree, &self.child, &wait, &self.step.id, true, true)
                    .await;
                if reply.kind == "denied"
                    || (reply.kind == "answer" && reply.choice.as_deref() == Some(LOOP_STOP))
                {
                    inner.stop_tree(&tree.id); // the owner's stop: the loop and its task end
                    return None;
                }
                reply
            };
            match reply.kind.as_str() {
                "declined" => Some(Reason {
                    fields: obj(json!({"declined": true, "summary": reply.question})),
                    observed: None,
                }),
                "stopped" => None,
                "steer" => Some(Reason::why("your message")),
                _ => {
                    let choice = reply.choice.clone().unwrap_or_default();
                    if reply.choice.is_some() && (choice == LOOP_KEEP || choice == CONTINUE) {
                        return Some(with(
                            Reason::why(&format!("you chose \"{choice}\"")),
                            "owner",
                            json!(true),
                        ));
                    }
                    let r = with(
                        Reason::why("your answer to my question"),
                        "question",
                        json!(reply.question),
                    );
                    let r = with(r, "answer", json!(reply.words));
                    Some(with(r, "owner", json!(true)))
                }
            }
        })
    }

    fn paused(&self) -> bool {
        self.inner.is_paused(&self.tree)
    }
}
