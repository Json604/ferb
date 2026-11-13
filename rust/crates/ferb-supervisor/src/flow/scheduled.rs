//! Scheduled runs (supervisor.py:1629 `run_scheduled`, :1728 `_previous_run`, :2369 `schedule_running`), as the
//! daemon's schedules area drives them (rust/crates/ferb-daemon `areas_a::seams::Tasks`: `schedule_running`,
//! `create_scheduled`, `run_scheduled`).
//!
//! The daemon claims a due timer and starts the run in one transaction (ferb-clock `claim_run` / `start_run`): its
//! first half here, [`Inner::create_scheduled`], writes the run's record (`schedule.run`, with the run's request id)
//! on the connection the daemon hands in, so a crash between the timer and the task never loses the run (the fix
//! rust/PLAN.md part 9 names). A run of a schedule whose previous run is still under way is skipped by the daemon
//! (`schedule_running`).
//!
//! The run itself ([`Inner::run_scheduled_rest`]): an unattended planner turn on the schedule's own session with its
//! previous run's answer, then `run_tree(..., unattended_scopes=, start_grant=, watch=, schedule=)`.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_clock::schedules::Schedule;
use ferb_orchestrator::models::{SendScope, TaskSpec};
use ferb_orchestrator::sql;
use ferb_sanitize::origin;
use rusqlite::types::Value as Sql;
use serde_json::{json, Map, Value};

use super::{Failure, Flow, Inner, Request, Tree, Turn};
use crate::words::{
    NO_OUTCOME, PAUSED_MENTION, PAUSED_SINCE, RESUME, SCHEDULE_MESSAGES, STOP, STOP_LABEL, TASK_ASK,
};
use crate::NotPorted;

/// Python's `t.overdue_s or 0` (areas/schedules.py:153): a timer due now carries the int 0, a late one its float.
fn or_zero(overdue_s: f64) -> Value {
    if overdue_s == 0.0 {
        json!(0)
    } else {
        json!(overdue_s)
    }
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// What `create_scheduled` hands the rest of the run.
#[derive(Clone, Debug, PartialEq)]
pub struct Created {
    pub request_id: String,
    /// `_previous_run(sid)`, read before this run's own record: (ran_at, status, its answer's text, that entry's id
    /// and origin, how many later runs left no answer).
    pub previous: Option<Previous>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Previous {
    pub ran_at: String,
    pub status: String,
    pub entry_id: String,
    pub text: String,
    pub origin: String,
    pub skipped: usize,
}

impl Created {
    /// What [`Created::to_json`] wrote (the daemon hands it back for the run).
    pub fn from_json(v: &Value) -> Created {
        let text = |v: &Value, k: &str| v.get(k).map(ferb_py::value::py_str).unwrap_or_default();
        Created {
            request_id: text(v, "request_id"),
            previous: v
                .get("previous")
                .filter(|p| p.is_object())
                .map(|p| Previous {
                    ran_at: text(p, "ran_at"),
                    status: text(p, "status"),
                    entry_id: text(p, "entry_id"),
                    text: text(p, "text"),
                    origin: text(p, "origin"),
                    skipped: p.get("skipped").and_then(Value::as_u64).unwrap_or(0) as usize,
                }),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"request_id": self.request_id, "previous": self.previous.as_ref().map(|p| json!({
            "ran_at": p.ran_at, "status": p.status, "entry_id": p.entry_id, "text": p.text, "origin": p.origin,
            "skipped": p.skipped}))})
    }
}

impl Inner {
    /// supervisor.py:2369 `schedule_running`: a run of this schedule (a resumed one included) is under way.
    pub fn schedule_running(&self, schedule_id: &str) -> bool {
        self.live()
            .tree_of
            .iter()
            .any(|(k, t)| *k == t.id && t.lock().schedule_id.as_deref() == Some(schedule_id))
    }

    /// The first half of `run_scheduled` (supervisor.py:1632-1635): the run's request id, its previous run (read
    /// before this run's own record), and its `schedule.run` record, written on `conn` (the daemon's transaction that
    /// marks the run's timer handled).
    pub fn create_scheduled(
        &self,
        conn: &rusqlite::Connection,
        schedule_id: &str,
        late: bool,
        retry: bool,
    ) -> Result<Created, String> {
        let request_id = self.new_id();
        let previous = previous_run(conn, schedule_id).map_err(|e| e.to_string())?;
        let mut data = json!({"schedule_id": schedule_id, "request_id": request_id, "late": late});
        if retry {
            data["retry"] = json!(true);
        }
        self.journal_on(conn, "schedule.run", &data)
            .map_err(|e| e.to_string())?;
        Ok(Created {
            request_id,
            previous,
        })
    }

    /// journal.py `append` on a connection the caller holds (a transaction of its own).
    fn journal_on(
        &self,
        conn: &rusqlite::Connection,
        kind: &str,
        data: &Value,
    ) -> rusqlite::Result<String> {
        let body = self.redact(data);
        let made = super::store::journal_origin(kind, &body);
        let id = self.new_id();
        let at = self.now_iso();
        conn.execute(
            "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![id, at, kind, ferb_py::json::dumps_utf8(&body), made],
        )?;
        Ok(id)
    }

    /// The rest of `run_scheduled` (supervisor.py:1636-1706): decide an unattended run from its recorded request; the
    /// run's final event (`suspended` when FERB's shutdown cut it: it resumes, and reports, after the restart).
    pub async fn run_scheduled_rest(
        &self,
        sched: &Schedule,
        created: &Created,
        late: bool,
        overdue_s: f64,
        retry: bool,
    ) -> Flow<Map<String, Value>> {
        let request_id = created.request_id.clone();
        let previous = created.previous.clone();
        let label = ferb_orchestrator::names::shown_name(Some(&sched.name), &sched.request);
        let mut recorded_scopes: Vec<SendScope> = Vec::new();
        for scope in sched
            .targets
            .get("standing_sends")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let s = ferb_orchestrator::validate::send_scope_validate(
                &ferb_context::Py::from_value(&scope),
            )
            .map_err(|e| Failure::raised("ValidationError", e.to_string()))?;
            recorded_scopes.push(s);
        }
        let dumped: Vec<Value> = recorded_scopes
            .iter()
            .map(super::schedules_chat::dump_scope)
            .collect();
        // a watch: what its latest finished check that reported the condition found
        let mut watched = Map::new();
        if sched.watch != "none" {
            watched.insert("last_reported_met".into(), json!(sched.last_reported_met));
        }
        // ORCH-313: what every run uses, recorded when the schedule was made
        let mut recorded_run = Map::new();
        if let Some(scope) = sched.run_scope() {
            recorded_run.insert("recorded_scope".into(), Value::Object(scope));
        }
        let ran_before = previous.as_ref().map(|p| {
            json!({"ran_at": p.ran_at, "status": p.status, "later_runs_without_answer": p.skipped})
        });
        let mut facts = obj(
            json!({"rule": sched.when_said(), "watch": sched.watch, "late": late,
            "overdue_s": or_zero(overdue_s)}),
        );
        if retry {
            facts.insert("retry_of_failed_run".into(), json!(true));
        }
        facts.insert("consecutive_failures".into(), json!(sched.failures));
        facts.insert("standing_sends".into(), Value::Array(dumped.clone()));
        facts.insert(
            "previous_run".into(),
            ran_before.clone().unwrap_or(Value::Null),
        );
        facts.extend(watched.clone());
        facts.extend(recorded_run.clone());
        let scheduled = self
            .deps
            .prompts
            .load(&format!("{}/scheduled.md", crate::prompts::APPROACH), &[])
            .map_err(|e| Failure::raised("FileNotFoundError", e.to_string()))?;
        let note = format!(
            "\n\n{scheduled}\nSchedule facts (data): {}",
            ferb_py::json::dumps(&Value::Object(facts))
        );
        // what the run's worker reads of them
        let mut worker_facts = obj(json!({"rule": sched.when_said(), "watch": sched.watch}));
        worker_facts.extend(watched);
        worker_facts.extend(recorded_run);
        worker_facts.insert(
            "previous_run".into(),
            match (&ran_before, &previous) {
                (Some(Value::Object(r)), Some(p)) => {
                    let mut r = r.clone();
                    r.insert("answer".into(), json!(p.text));
                    r.insert("answer_origin".into(), json!(p.origin));
                    Value::Object(r)
                }
                _ => Value::Null,
            },
        );
        let session = format!("schedule:{}", sched.id);
        let said = self.schedule_messages(&sched.id);
        let payload = if said.is_empty() {
            format!("{}{note}", sched.request)
        } else {
            format!("{}\n\n{SCHEDULE_MESSAGES}{said}{note}", sched.request)
        };
        let turn = Arc::new(Mutex::new(Turn {
            request_id: request_id.clone(),
            session: session.clone(),
            words: sched.request.clone(),
            payload,
            message_origin: self.schedule_origins(&sched.id),
            request_origins: self.schedule_sink_origins(&sched.id),
            schedule_previous: previous,
            ..Turn::default()
        }));
        self.live().turns.insert(request_id.clone(), turn.clone());
        let out = self
            .scheduled_run(
                sched,
                &turn,
                &request_id,
                &label,
                recorded_scopes,
                dumped,
                overdue_s,
                retry,
                worker_facts,
            )
            .await;
        // the run's turn goes when it ends, however it ends (final review M4)
        self.live().turns.remove(&request_id);
        out
    }

    #[allow(clippy::too_many_arguments)] // run_scheduled's own values
    async fn scheduled_run(
        &self,
        sched: &Schedule,
        turn: &Arc<Mutex<Turn>>,
        request_id: &str,
        label: &str,
        recorded_scopes: Vec<SendScope>,
        dumped: Vec<Value>,
        overdue_s: f64,
        retry: bool,
        worker_facts: Map<String, Value>,
    ) -> Flow<Map<String, Value>> {
        let failed = |status: &str, answer: &str| {
            obj(
                json!({"task_id": request_id, "request_id": request_id, "status": status, "answer": answer,
                       "goal": sched.request, "name": label}),
            )
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let cancel = super::turn::Cancel::default();
        let env = self.deps.daemon.environment(&sched.request).await;
        turn.lock().unwrap().environment = env;
        let decided = self.decide(turn, &tx, &cancel).await;
        let mut out = match decided {
            Ok(out) => out,
            Err(e) => {
                // before its task started: the cause is kept, and told by its reason (L21)
                self.journal(
                    "chat.failed",
                    json!({"request_id": request_id, "why": e.class(), "detail": e.message()}),
                    None,
                );
                let err = ferb_worker::Error::other(&e.class(), e.message());
                let route = || self.deps.daemon.has_route();
                let answer = self.redact_text(&ferb_worker::errors::couldnt(
                    "start this scheduled run",
                    &err,
                    Some(&|t| self.redact_text(t)),
                    &route,
                ));
                return Ok(failed("failed", &answer));
            }
        };
        let Some(mut task) = out.task.take().filter(|_| out.kind == "task") else {
            let status = if out.kind == "reply" {
                "done"
            } else {
                "needs_user"
            };
            // in the schedule's conversation, so the next run (and its chat.history) has this answer
            let mut made = turn.lock().unwrap().turn_origins.clone();
            made.insert(origin::INFERRED.into());
            let kind = if out.kind == "clarify" {
                "clarify"
            } else {
                "reply"
            };
            self.record_reply(turn, kind, &self.redact_text(&out.text), &made, "", None)
                .await;
            return Ok(failed(status, &out.text));
        };
        if sched.run_scope().is_some() {
            self.apply_run_scope(sched, &mut task);
        }
        // T-1055: a scheduled run is headed by its schedule's name
        task.name = label.to_string();
        let recorded = |k: &str| -> Vec<String> {
            let mut v: Vec<String> = sched
                .targets
                .get(k)
                .and_then(Value::as_array)
                .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                .unwrap_or_default();
            v.sort();
            v.dedup();
            v
        };
        self.deps.daemon.record_request(&Request {
            request_id: request_id.to_string(),
            words: sched.own_words.clone(),
            targets: [
                recorded("apps"),
                recorded("sites"),
                recorded("paths"),
                recorded("recipients"),
            ],
            payload: sched.request.clone(),
            asked_origins: BTreeSet::new(),
        });
        self.deps.daemon.set_unattended(request_id);
        let mut final_ = failed("failed", NO_OUTCOME);
        if self.live().suspending {
            // FERB is shutting down: the run starts after the restart's resume, if at all
            final_.insert("suspended".into(), json!(true));
            return Ok(final_);
        }
        let origins = turn.lock().unwrap().turn_origins.clone();
        let schedule = obj(
            json!({"id": sched.id, "overdue_s": or_zero(overdue_s), "retry": retry,
            "watch": sched.watch != "none", "facts": worker_facts}),
        );
        let (etx, mut erx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let opts = super::tree::RunOpts {
            // the next run sees this one's answer
            conversation: Some(format!("schedule:{}", sched.id)),
            origins,
            unattended_scopes: Some(dumped.clone()),
            start_grant: Some((dumped, format!("schedule:{}", sched.id))),
            watch: sched.watch != "none",
            schedule: Some(schedule),
            ..super::tree::RunOpts::default()
        };
        let _ = recorded_scopes;
        let ran = self
            .run_tree(
                task,
                &out.text,
                request_id,
                &sched.request,
                opts,
                &etx,
                &|| {},
            )
            .await;
        while let Ok(ev) = erx.try_recv() {
            if ev.get("type").and_then(Value::as_str) == Some("outcome") {
                if let Value::Object(m) = ev {
                    final_ = m;
                }
            }
        }
        match ran {
            Ok(()) => {}
            Err(Failure::Cancelled) => {}
            Err(e) => return Err(e),
        }
        if final_.get("answer").and_then(Value::as_str) == Some(NO_OUTCOME)
            && self.live().suspending
        {
            final_.insert("suspended".into(), json!(true));
        }
        Ok(final_)
    }

    /// supervisor.py:1708 `_apply_run_scope` (ORCH-313): a scheduled run's task uses what the schedule recorded when it
    /// was made (its folders, identity and worker type, for every step too), whatever this run's plan proposed.
    fn apply_run_scope(&self, sched: &Schedule, spec: &mut TaskSpec) {
        let scope = sched.run_scope().unwrap_or_default();
        let worker_type = scope
            .get("worker_type")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .unwrap_or_else(|| "default".into());
        let mut kinds: BTreeSet<String> = BTreeSet::from([spec.worker_type.clone()]);
        if let Some(plan) = &spec.plan {
            kinds.extend(plan.steps.iter().map(|s| s.worker_type.clone()));
        }
        let proposed = json!({"folders": spec.folders, "identity": spec.identity,
            "worker_type": kinds.into_iter().collect::<Vec<_>>()});
        spec.folders = scope
            .get("folders")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(ferb_py::value::py_str).collect())
            .unwrap_or_default();
        if let Some(i) = scope
            .get("identity")
            .and_then(Value::as_str)
            .filter(|i| *i == "owner" || *i == "ferb")
        {
            spec.identity = i.to_string();
        }
        spec.worker_type = worker_type.clone();
        if let Some(plan) = spec.plan.as_mut() {
            for step in plan.steps.iter_mut() {
                step.worker_type = worker_type.clone();
            }
        }
        let applied = json!({"folders": spec.folders, "identity": spec.identity, "worker_type": [worker_type]});
        if proposed != applied {
            self.journal(
                "schedule.scope_applied",
                json!({"schedule_id": sched.id, "proposed": proposed, "recorded": applied}),
                None,
            );
        }
    }

    /// supervisor.py:4153 `_schedule_origins`: a scheduled run's request: what the turn that made the schedule was
    /// written from, as recorded on its rows (always holding `inferred`); else unlabeled.
    pub fn schedule_origins(&self, schedule_id: &str) -> BTreeSet<String> {
        let mut held = BTreeSet::new();
        for kind in ["schedule.created", "schedule.edited"] {
            for e in self.of_kind(kind, "schedule_id", schedule_id, None) {
                held.extend(origin::split(Some(&e.origin)));
            }
        }
        if held.contains(origin::INFERRED) {
            held
        } else {
            BTreeSet::from([origin::external("unlabeled", "")])
        }
    }

    /// supervisor.py:4331 `_schedule_sink_origins`: a scheduled run's request origins for the sink rule (S4, L4), as
    /// recorded; a row recorded without them gives every label the turn saw (fail-closed).
    pub fn schedule_sink_origins(&self, schedule_id: &str) -> BTreeSet<String> {
        let mut rows = self.of_kind("schedule.created", "schedule_id", schedule_id, None);
        rows.extend(self.of_kind("schedule.edited", "schedule_id", schedule_id, None));
        let recorded: Vec<Option<Vec<Value>>> = rows
            .iter()
            .map(|e| e.get("request_origins").and_then(Value::as_array).cloned())
            .collect();
        if !rows.is_empty()
            && recorded
                .iter()
                .all(|r| r.as_ref().is_some_and(|r| !r.is_empty()))
        {
            return recorded
                .into_iter()
                .flatten()
                .flatten()
                .map(|o| ferb_py::value::py_str(&o))
                .collect();
        }
        self.schedule_origins(schedule_id)
    }

    /// supervisor.py:4445 `_schedule_messages` (ORCH-597): the owner's own chat messages that made or changed a
    /// schedule, oldest first, each with its time; "" when none are recorded.
    pub fn schedule_messages(&self, schedule_id: &str) -> String {
        let mut requests: Vec<String> = Vec::new();
        for kind in ["schedule.created", "schedule.edited"] {
            for e in self.of_kind(kind, "schedule_id", schedule_id, None) {
                let rid = e.text("request_id");
                if e.get("request_id").is_some_and(ferb_py::value::truthy)
                    && !requests.contains(&rid)
                {
                    requests.push(rid);
                }
            }
        }
        let mut found: Vec<super::store::Entry> = Vec::new();
        for rid in &requests {
            for field in ["request_id", "read_with"] {
                for e in self.of_kind("chat.message", field, rid, None) {
                    if !found.iter().any(|f| f.id == e.id) {
                        found.push(e);
                    }
                }
            }
        }
        found.sort_by(|a, b| a.at.cmp(&b.at));
        let zone = self.deps.clock.zone();
        found
            .iter()
            .map(|e| {
                let at = ferb_clock::py::parse(&e.at)
                    .map(|t| {
                        let l = zone.local(&t);
                        ferb_clock::py::strftime(
                            &l.naive_local(),
                            l.offset().local_minus_utc(),
                            "",
                            "%a %d %b %H:%M",
                        )
                    })
                    .unwrap_or_default();
                format!("({at}) {}", e.text("text"))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The daemon's `_import_busy`: an owner's chat turn is under way.
    pub fn turn_running(&self) -> bool {
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|j| !j.is_finished())
    }

    /// supervisor.py:1840 `mention_paused` (ORCH-312): when the owner is back at the Mac, each task they paused on an
    /// earlier day, not mentioned yet today, is mentioned once on its pause's one wait (Resume / Stop). At most once a
    /// day per task (journaled `task.pause_mentioned`); an unanswered mention is replaced, never stacked. A pause that
    /// already asks something, or a tree with another open question, is shown already.
    pub async fn mention_paused(&self) -> Result<(), NotPorted> {
        let zone = self.deps.clock.zone();
        let now = self.now();
        let today = zone.date(&now);
        let pauses: Vec<(String, Arc<super::owner_wait::Wait>)> = self
            .live()
            .pauses
            .iter()
            .map(|(k, w)| (k.clone(), w.clone()))
            .collect();
        for (tree_id, wait) in pauses {
            let tree = self.live().tree_of.get(&tree_id).cloned();
            let Some(tree) = tree else { continue };
            let opened = wait.lock().opened;
            if !tree.lock().stop.is_empty()
                || !wait.question.is_empty()
                || !opened
                || self.live().suspending
            {
                continue;
            }
            let mentioned = wait.lock().extra.get("mention").map(ferb_py::value::py_str);
            let asked = self.live().questions.get(&tree_id).cloned();
            if asked.as_ref().is_some_and(|q| !q.is_empty()) && asked != mentioned {
                continue; // another question of this task is open: it shows
            }
            let marks: Vec<ferb_clock::py::Time> = ["task.paused", "task.pause_mentioned"]
                .iter()
                .flat_map(|k| self.of_kind(k, "task_id", &tree_id, None))
                .filter_map(|e| ferb_clock::py::parse(&e.at))
                .collect();
            let Some(last) = marks.iter().max() else {
                continue;
            };
            if zone.date(last) >= today {
                continue; // paused today, or already mentioned today
            }
            self.mention_pause(&tree, &wait);
        }
        Ok(())
    }

    /// supervisor.py:1863 `_mention_pause`: ask on a paused tree's wait whether to resume, replacing an earlier
    /// unanswered mention (the new question goes on the wait first).
    fn mention_pause(&self, tree: &Arc<Tree>, wait: &Arc<super::owner_wait::Wait>) {
        let Some(questions) = self.deps.daemon.questions() else {
            return;
        };
        let task = &tree.worker;
        let goal = tree.spec().goal;
        let question =
            PAUSED_MENTION.replace("{goal}", &ferb_orchestrator::names::short_label(&goal, 60));
        let options: Vec<String> = [RESUME, STOP]
            .iter()
            .map(|o| ferb_policy::confirm::from_task(o))
            .collect();
        let paused: Vec<ferb_clock::py::Time> = self
            .of_kind("task.paused", "task_id", &tree.id, None)
            .iter()
            .filter_map(|e| ferb_clock::py::parse(&e.at))
            .collect();
        let detail = paused
            .iter()
            .max()
            .map(|at| {
                PAUSED_SINCE.replace(
                    "{day}",
                    &crate::lines::day(at, &self.now(), &self.deps.clock.zone()),
                )
            })
            .unwrap_or_default();
        let row = self.part_of(&tree.id, &tree.step_id);
        let ask = super::owner_wait::Ask {
            capability: TASK_ASK.into(),
            task_id: task.id.clone(),
            request_id: task.request_id.clone(),
            question: question.clone(),
            options: options.clone(),
            part: row.as_ref().map(|(_, p)| p.id.clone()).unwrap_or_default(),
            options_only: true, // ORCH-536: Resume or Stop, never typed words
            detail,
            labels: vec![RESUME.into(), STOP_LABEL.into()],
            ..super::owner_wait::Ask::default()
        };
        let id = questions.ask(&ask);
        let earlier = {
            let mut st = wait.lock();
            st.extra.insert("mention".into(), json!(question));
            st.approval.replace(id.clone())
        };
        self.live()
            .questions
            .insert(tree.id.clone(), question.clone());
        if let Some(earlier) = earlier {
            if questions.supersede(&earlier) {
                self.deps.daemon.publish(
                    "approval.resolved",
                    json!({"approval_id": earlier, "state": "superseded"}),
                );
            }
        }
        self.journal(
            "task.pause_mentioned",
            json!({"task_id": tree.id, "approval_id": id, "question": question}),
            Some(origin::FERB),
        );
        let mut data = obj(
            json!({"approval_id": id, "capability": TASK_ASK, "kind": "owner_question",
            "reason": question, "options": options, "task_id": task.id, "request_id": task.request_id,
            "question_kind": "paused"}),
        );
        data.extend(questions.card(&id));
        if let Some((_, part)) = &row {
            data.insert("part".into(), json!(part.id));
        }
        self.deps
            .daemon
            .publish("approval.requested", Value::Object(data));
        if let Some((card, part)) = &row {
            let shown = json!({"approval_id": id, "text": question, "options": options});
            self.with_card(&tree.id, card, |c| {
                c.show(part, "asks", vec![("question", shown)])
            });
        }
    }

    /// supervisor.py:4294 `forget_pending`: a chat whose pending clarifying question still holds a forgotten item's
    /// words drops that pending question. The chats cleared.
    pub fn forget_pending(&self, texts: &[String]) -> Vec<String> {
        let norm = |t: &str| {
            t.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let wanted: Vec<String> = texts
            .iter()
            .filter(|t| !ferb_orchestrator::py::strip(t).is_empty())
            .map(|t| norm(t))
            .collect();
        if wanted.is_empty() {
            return Vec::new();
        }
        let holds = |parts: &[String]| {
            let joined = norm(
                &parts
                    .iter()
                    .filter(|p| !p.is_empty())
                    .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            wanted.iter().any(|w| joined.contains(w.as_str()))
        };
        let rows = {
            let db = self.db();
            sql::rows(
                db.conn(),
                "SELECT id, pending, asked, pending_payload FROM conversations WHERE pending != '' OR asked != ''",
                &[],
            )
            .unwrap_or_default()
        };
        let mut cleared = Vec::new();
        for row in rows {
            let parts: Vec<String> = ["pending", "asked", "pending_payload"]
                .iter()
                .map(|k| sql::col(&row, k))
                .collect();
            if holds(&parts) {
                let id = sql::col(&row, "id");
                let _ = self.continuity(|c| c.save_pending(&id, "", "", ""));
                cleared.push(id);
            }
        }
        cleared
    }

    /// The daemon's forget of trail rows' copies outside memory (`conversations.forget_sources`): delegated
    /// conversations are not ported.
    pub fn forget_trail_copies(&self, _trail: &BTreeSet<String>) -> Result<(), NotPorted> {
        Err(NotPorted("ConversationStore.forget_sources"))
    }
}

/// supervisor.py:1728 `_previous_run`: this schedule's most recent earlier run that left an answer (a task's answer, or
/// the planner's reply), with how many later runs left none. Read on `conn`.
pub fn previous_run(
    conn: &rusqlite::Connection,
    schedule_id: &str,
) -> Result<Option<Previous>, sql::Error> {
    let of_kind = |kind: &str,
                   field: &str,
                   equals: &str|
     -> Result<Vec<Map<String, Value>>, sql::Error> {
        sql::rows(
            conn,
            "SELECT * FROM journal WHERE kind = ? AND COALESCE(json_extract(data_json, ?), ?) = ? ORDER BY seq",
            &[
                Sql::Text(kind.into()),
                Sql::Text(format!("$.{field}")),
                Sql::Null,
                Sql::Text(equals.into()),
            ],
        )
    };
    let runs = of_kind("schedule.run", "schedule_id", schedule_id)?;
    let mut skipped = 0;
    for run in runs.iter().rev() {
        let data: Value = serde_json::from_str(&sql::col(run, "data_json")).unwrap_or(json!({}));
        let rid = data
            .get("request_id")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let mut answers = Vec::new();
        for kind in ["task.answer", "chat.reply", "chat.clarify"] {
            answers.extend(of_kind(kind, "request_id", &rid)?);
        }
        if answers.is_empty() {
            skipped += 1;
            continue;
        }
        // `max(answers, key=lambda e: e.at)`: the first of the latest
        let mut best = &answers[0];
        for a in &answers[1..] {
            if sql::col(a, "at") > sql::col(best, "at") {
                best = a;
            }
        }
        let entry: Value = serde_json::from_str(&sql::col(best, "data_json")).unwrap_or(json!({}));
        let kind = sql::col(best, "kind");
        let status = match entry.get("status").filter(|s| ferb_py::value::truthy(s)) {
            Some(s) => ferb_py::value::py_str(s),
            None if kind == "chat.clarify" => "needs_user".into(),
            None => "done".into(),
        };
        return Ok(Some(Previous {
            ran_at: sql::col(run, "at"),
            status,
            entry_id: sql::col(best, "id"),
            text: entry
                .get("text")
                .map(ferb_py::value::py_str)
                .unwrap_or_default(),
            origin: sql::col(best, "origin"),
            skipped,
        }));
    }
    Ok(None)
}
