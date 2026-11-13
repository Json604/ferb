//! One task's or plan step's attempt loop and its checker: supervisor.py:6135 `_worker`, :6356 `_read_report`, :6384
//! `_judge`, :6417 `_next`, :6470 `_settle_findings` with :6574 `_fix_turn` and :6610 `_recheck`, :7078 `verify`,
//! :7133 `_verifier`, :7248 `_check`, :7281 `_changed_files`, :7344 `_unread`, :6852 `_context`, :7038
//! `_source_block`.

use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_context::assemble::ContextBlock;
use ferb_context::pyvalue::Py;
use ferb_core::types::{Outcome, OutcomeStatus, Task, TaskRole};
use ferb_orchestrator::attempts::Attempts;
use ferb_orchestrator::models::{CheckerOutcome, Condition, Finding};
use ferb_orchestrator::validate::{
    checker_outcome_validate, dump_checker_outcome, dump_finding, dump_fix_outcome, dump_plan,
    dump_recheck_outcome, fix_outcome_validate, recheck_outcome_validate, worker_report_validate,
};
use ferb_orchestrator::verify::{ConditionResult, ACTION_CHECKS, DECLINED, UNCHECKED};
use ferb_sanitize::origin;
use ferb_worker::{Attempt, OutcomeKind, RunOptions};
use serde_json::{json, Map, Value};

use super::tree::{status_of, WORKER};
use super::{Failure, Flow, Inner, Tree};
use crate::words::*;
use crate::worker_context::{self, ContextReads, SourceBlocks, SourceCall, VerifierReads};
use crate::NotPorted;

/// supervisor.py:7676 `_RuntimeDown`: the checker's run ended on an account fact; carried as a raised class.
const RUNTIME_DOWN: &str = "_RuntimeDown";

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn protected(text: impl Into<String>, reference: &str) -> ContextBlock {
    ContextBlock {
        protected: true,
        ..ContextBlock::new(text, reference)
    }
}

fn outcome(task_id: &str, status: &str, summary: &str, artifacts: Vec<String>) -> Outcome {
    Outcome {
        task_id: task_id.to_string(),
        status: status_of(status),
        summary: summary.to_string(),
        artifacts,
        evidence: Vec::new(),
    }
}

/// supervisor.py:7664 `_Next`: what the checker's choice comes to.
#[derive(Clone, Debug, Default)]
pub struct Next {
    pub action: String,
    pub instructions: String,
    pub missing: String,
    pub question: String,
    pub reason: String,
    pub worker_type: Option<String>,
    pub findings: Vec<Finding>,
    pub told: String,
}

impl Next {
    fn of(action: &str) -> Next {
        Next {
            action: action.into(),
            ..Next::default()
        }
    }
}

/// `_Changed`: the change records a check reads and their blocks.
#[derive(Default)]
struct Changed {
    records: Vec<(String, String)>,
    blocks: Vec<ContextBlock>,
}

/// The `context.source` spans of one tree (`_source_block`'s store).
struct Sources<'a> {
    sup: &'a Inner,
    tree: &'a Arc<Tree>,
}

impl SourceBlocks for Sources<'_> {
    fn store(&mut self, call: &SourceCall) -> String {
        self.sup.store_source(self.tree, call)
    }
}

impl Inner {
    /// supervisor.py:7038 `_source_block`'s store: one `context.source` span per label, text and refs in a tree.
    fn store_source(&self, tree: &Arc<Tree>, call: &SourceCall) -> String {
        use sha2::Digest;
        let key = hex::encode(sha2::Sha256::digest(
            format!(
                "{}\n{}{}",
                call.label,
                call.text,
                ferb_py::json::dumps(&call.source_refs)
            )
            .as_bytes(),
        ));
        let known = tree.lock().context_sources.get(&key).cloned();
        if known.is_none() && !call.text.is_empty() {
            let parent = self.task_span(&tree.id);
            let span = self.span_start(
                "context.source",
                Some(&tree.id),
                parent.as_deref(),
                None,
                json!({"label": call.label, "source_refs": crate::lines::stored_refs(&call.source_refs)}),
            );
            self.span_finish(
                &span,
                Some(json!({"text": call.text})),
                None,
                Some(&call.origin),
                None,
            );
            tree.lock().context_sources.insert(key.clone(), span);
        }
        format!(
            "task.evidence span_id={}",
            tree.lock()
                .context_sources
                .get(&key)
                .cloned()
                .unwrap_or_default()
        )
    }

    /// supervisor.py:7038 `_source_block`.
    pub fn source_block(
        &self,
        tree: &Arc<Tree>,
        label: &str,
        text: &str,
        made: &str,
        refs: &[String],
        payload: Option<&str>,
    ) -> ContextBlock {
        worker_context::source_block(
            &mut Sources { sup: self, tree },
            SourceCall {
                label: label.into(),
                text: text.into(),
                origin: made.into(),
                source_refs: refs.to_vec(),
                payload: payload.map(str::to_string),
            },
        )
    }

    /// supervisor.py:6852 `_context`.
    pub async fn context(&self, tree: &Arc<Tree>, learned: bool) -> Flow<Vec<ContextBlock>> {
        let spec = tree.spec();
        let request = self.deps.daemon.request(&tree.worker.request_id);
        let activity = self.deps.daemon.relevant_context(&request.words);
        let (parent_goal, step_context, receipts, reports, schedule_id, schedule_facts, prior) = {
            let st = tree.lock();
            (
                st.parent_goal.clone(),
                st.step_context.clone(),
                st.intake_receipts.clone(),
                st.reports.clone(),
                st.schedule_id.clone(),
                st.schedule_facts.clone(),
                st.prior_task.clone(),
            )
        };
        // a scheduled run's schedule (the root's: `_tree_of[tree.id]`) and what its roles read of it
        let (schedule_id, schedule_facts) = match self.live().tree_of.get(&tree.id).cloned() {
            Some(root) if !Arc::ptr_eq(&root, tree) => {
                let st = root.lock();
                (st.schedule_id.clone(), st.schedule_facts.clone())
            }
            _ => (schedule_id, schedule_facts),
        };
        // ORCH-597: the owner's own messages that made or changed the schedule follow its instructions
        let schedule_messages = match &schedule_id {
            Some(sid) => self.schedule_messages(sid),
            None => String::new(),
        };
        let results = self.result_blocks(tree, &spec);
        let memory = if learned {
            self.worker_memory(tree, &request.words, &parent_goal, &spec)
                .await
        } else {
            Vec::new()
        };
        let mut unresolved = Vec::new();
        for root in std::iter::once(tree.id.clone()).chain(prior) {
            unresolved.extend(self.deps.daemon.unresolved(&root));
        }
        let steps_json = |steps: Vec<(String, ferb_orchestrator::plan_store::StepState)>| -> Value {
            Value::Object(
                steps
                    .into_iter()
                    .map(|(k, s)| (k, json!({"status": s.status, "output": s.output})))
                    .collect(),
            )
        };
        let plan_steps = if spec.plan.is_some() {
            Some(steps_json(
                self.plans(|p| p.steps(&tree.id)).unwrap_or_default(),
            ))
        } else {
            None
        };
        let saved_plan = if spec.plan.is_none() && step_context.is_empty() {
            match self.plans(|p| p.load(&tree.id)) {
                Ok(loaded) => Some(obj(json!({"plan": dump_plan(&loaded.plan).to_value(),
                    "steps": steps_json(self.plans(|p| p.steps(&tree.id)).unwrap_or_default())}))),
                Err(_) => Some(Map::new()),
            }
        } else {
            None
        };
        let confirmations = {
            let db = self.db();
            ferb_orchestrator::sql::rows(
                db.conn(),
                "SELECT id,rule,granted,used,at FROM confirmations WHERE tree_id=?",
                &[rusqlite::types::Value::Text(tree.id.clone())],
            )
            .unwrap_or_default()
        };
        let reads = ContextReads {
            tree_id: tree.id.clone(),
            worker_id: tree.worker.id.clone(),
            learned,
            words: request.words.clone(),
            payload: request.payload.clone(),
            activity: Some(activity),
            schedule_id,
            schedule_messages,
            parent_goal,
            goal: spec.goal.clone(),
            worker_type: spec.worker_type.clone(),
            step_context,
            intake_receipts: receipts.into_iter().map(Value::String).collect(),
            disturbance: self.deps.daemon.disturbance_context(&tree.id),
            schedule_facts,
            reports: reports.into_iter().map(Value::Object).collect(),
            task_made: self.task_made(&tree.id),
            unresolved,
            wait_cut: None,
            conditions: spec
                .success_conditions
                .iter()
                .map(|c| c.text.clone())
                .collect(),
            targets: [
                spec.targets.apps.clone(),
                spec.targets.sites.clone(),
                spec.targets.paths.clone(),
                spec.targets.recipients.clone(),
            ],
            folders: spec.folders.clone(),
            workspace: self.deps.workspace.clone(),
            posted: self.posted_answers(&tree.id),
            plan_steps,
            saved_plan,
            owner_questions: self.owner_questions(&tree.id),
            reopened: self.reopened_answers(&tree.id),
            confirmations,
            memory,
            results,
        };
        Ok(worker_context::worker_context(
            &reads,
            &mut Sources { sup: self, tree },
            &|t| self.wrap(t),
        ))
    }

    /// supervisor.py:6838 `_worker_memory`.
    async fn worker_memory(
        &self,
        tree: &Arc<Tree>,
        words: &str,
        parent_goal: &str,
        spec: &ferb_orchestrator::models::TaskSpec,
    ) -> Vec<ContextBlock> {
        let targets = ferb_orchestrator::context_layers::target_words(
            self.deps.policy.as_ref(),
            &spec.targets,
        );
        let query: Vec<&str> = [words, parent_goal, spec.goal.as_str(), targets.as_str()]
            .into_iter()
            .filter(|p| !ferb_orchestrator::py::strip(p).is_empty())
            .collect();
        let query = query.join("\n");
        let _ = tree;
        if ferb_memory::terms::query_terms(&query).is_empty() {
            return Vec::new();
        }
        self.deps.daemon.memory_push(&query, &targets).await
    }

    /// supervisor.py:6311 `_new_attempt`.
    fn new_attempt(&self, tree: &Arc<Tree>) {
        let mut st = tree.lock();
        st.resources.clear();
        st.cleanup_errors.clear();
        st.verification.clear();
        st.check_problem.clear();
        st.unconfirmed.clear();
        st.owner_answer.clear();
        st.reason.clear();
        st.how.clear();
        st.worker_question.clear();
        st.owner_question.clear();
        st.answer_unmarked.clear();
        st.input_notes.clear();
        st.conversation_proposals.clear();
        st.part_updates.clear();
        st.watch_met = None;
        st.answer_settled = false;
    }

    /// supervisor.py:6324 `_steered`.
    async fn steered(&self, tree: &Arc<Tree>) -> bool {
        let revision = *tree.result_revision.lock().unwrap();
        revision < tree.steering.lock().await.messages.len()
    }

    /// supervisor.py:6328 `_report_roles`.
    fn report_roles(&self, tree: &Arc<Tree>) -> Flow<(Value, Vec<String>)> {
        let (context, inputs, watch) = {
            let st = tree.lock();
            (
                st.step_context.clone(),
                !st.required_inputs.is_empty(),
                st.watch,
            )
        };
        let step: Map<String, Value> = serde_json::from_str::<Value>(&context)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        let is_step = step.get("step").is_some_and(ferb_py::value::truthy);
        let mut proposals = false;
        if is_step {
            if let Ok(loaded) = self.plans(|p| p.load(&tree.id)) {
                proposals = loaded.plan.steps.iter().any(|s| {
                    s.kind == "converse"
                        && s.conversation.is_none()
                        && s.depends_on.contains(&tree.step_id)
                });
            }
        }
        let root = self.root(&tree.id);
        let scheduled = !is_step
            && !watch
            && root
                .as_ref()
                .is_some_and(|r| r.lock().schedule_id.is_some());
        if step.contains_key("loop") || step.contains_key("conversation") {
            return Err(NotPorted(
                "Supervisor._report_roles (a loop round or a conversation turn)",
            )
            .into());
        }
        let parts = is_step && root.as_ref().is_some_and(|r| r.lock().card.is_some());
        let flags = ferb_context::schemas::WorkerFlags {
            inputs,
            proposals,
            conversation: false,
            watch,
            loop_: false,
            parts,
        };
        let mut roles = Vec::new();
        if is_step {
            roles.push("step".to_string());
        }
        for (name, on) in [
            ("inputs", inputs),
            ("proposals", proposals),
            ("watch", watch),
            ("parts", parts),
            ("scheduled", scheduled),
        ] {
            if on {
                roles.push(name.to_string());
            }
        }
        Ok((ferb_context::schemas::worker_schema(flags), roles))
    }

    /// supervisor.py:6135 `_worker`: the attempt loop.
    pub async fn worker_loop(&self, tree: &Arc<Tree>, span: &str) -> Flow<Outcome> {
        let task = tree.worker.clone();
        let mut instructions = String::new();
        let mut attempts = Attempts::default();
        tree.lock().confirmed.clear();
        let mut retrying = false;
        let worker = self.worker.get().expect("the worker").clone();
        let fix = std::mem::take(&mut tree.lock().restored_fix);
        if !fix.is_empty() {
            // a restart came during the step's fix round: code settles it (ORCH-545)
            return self.restored_fix(tree, &task, &fix, span).await;
        }
        if !tree.lock().restored_wait.is_empty() {
            match self.restored_wait(tree, &task, &mut attempts, span).await {
                Ok(more) => instructions = more,
                Err(end) => return Ok(end),
            }
        }
        while !self.stopped(tree) {
            match self.hold_if_paused(tree, &task).await {
                super::owner_wait::Held::Go => {}
                super::owner_wait::Held::Stop => break,
                super::owner_wait::Held::Resume(more) => {
                    instructions = more;
                    continue;
                }
            }
            self.new_attempt(tree);
            self.set_status(&task.id, "running", None);
            let mut ctx = self.context(tree, true).await?;
            ctx.push(protected(instructions.clone(), "current continuation"));
            let (schema, roles) = self.report_roles(tree)?;
            tree.lock().attempt += 1;
            self.attempt_state(tree, if retrying { "retrying" } else { "running" });
            let wtree: Arc<dyn ferb_worker::ports::WorkerTree> = tree.clone();
            let (raw, _) = worker
                .run_worker(
                    &wtree,
                    &task,
                    "worker.md",
                    ctx,
                    &schema,
                    span,
                    &RunOptions {
                        effort: None,
                        roles,
                        observe_only: false,
                    },
                )
                .await
                .map_err(Failure::Worker)?;
            if self.is_paused(tree) && !self.stopped(tree) {
                continue; // the pause ended this attempt: its report is dropped (owner_wait.rs)
            }
            if let Attempt::Outcome(o) = &raw {
                if let OutcomeKind::ServiceDown { runtime, problem } = &o.kind {
                    if !self.stopped(tree) {
                        let _ = self.read_report(tree, &task, &raw);
                        match self
                            .service_down_step(tree, &task, &o.outcome, runtime, problem)
                            .await
                        {
                            Ok(more) => {
                                instructions = more;
                                continue;
                            }
                            Err(None) => break,
                            Err(Some(failed)) => return Ok(failed),
                        }
                    }
                }
            }
            let out = self.read_report(tree, &task, &raw);
            let mut choice: Option<Next> = None;
            let mut down: Option<Value> = None;
            if !self.steered(tree).await && !self.stopped(tree) {
                self.attempt_state(tree, "checking");
                match self.judge(tree, &out, span).await {
                    Ok(next) => {
                        if !next.findings.is_empty()
                            && !self.stopped(tree)
                            && !self.steered(tree).await
                        {
                            self.settle_findings(tree, &task, &next.findings, span)
                                .await;
                            if out.status != OutcomeStatus::Done && !tree.lock().reason.is_empty() {
                                return Err(
                                    NotPorted("Supervisor._worker (findings on a reason)").into()
                                );
                            }
                        }
                        choice = Some(next);
                    }
                    // the checker's runtime hit the same kind of account fact
                    Err(e @ Failure::Raised { .. }) if e.class() == RUNTIME_DOWN => {
                        down = serde_json::from_str(&e.message()).ok();
                    }
                    Err(e) => return Err(e),
                }
            }
            if let Some(d) = down.filter(|_| !self.stopped(tree)) {
                let text = |k: &str| d[k].as_str().unwrap_or("").to_string();
                let failed = outcome(&text("task_id"), "failed", &text("summary"), Vec::new());
                match self
                    .service_down_step(tree, &task, &failed, &text("runtime"), &text("problem"))
                    .await
                {
                    Ok(more) => {
                        instructions = more;
                        continue;
                    }
                    Err(None) => break,
                    Err(Some(failed)) => return Ok(failed),
                }
            }
            let (worker_type, answer, reason) = {
                let st = tree.lock();
                (
                    st.spec
                        .as_ref()
                        .map(|s| s.worker_type.clone())
                        .unwrap_or_default(),
                    st.owner_answer.clone(),
                    st.reason.clone(),
                )
            };
            let mut report = obj(json!({"worker_type": worker_type,
                "status": if out.status == OutcomeStatus::Done { "done" } else { "not_done" },
                "answer": answer, "reason": reason, "artifacts": out.artifacts}));
            if let Some(c) = &choice {
                report.insert(
                    "check".into(),
                    json!({"next": c.action, "missing": c.missing, "instructions": c.instructions, "reason": c.reason}),
                );
            }
            tree.lock().reports.push(report);
            let accepted = choice
                .as_ref()
                .is_some_and(|c| c.action == "accept" || c.action == "accept_unconfirmed")
                && !self.stopped(tree);
            if accepted {
                let mine = self
                    .live()
                    .tree_of
                    .get(&tree.id)
                    .is_some_and(|t| Arc::ptr_eq(t, tree));
                if mine {
                    tree.lock().work_done = true;
                }
                self.release_resources(tree, span).await;
            }
            if (self.steered(tree).await || self.is_paused(tree)) && !self.stopped(tree) {
                // the attempt's one steering checkpoint (owner_wait.rs): a done report is kept, the next attempt
                // starts from the correction
                self.superseded_attempt(tree, out.status == OutcomeStatus::Done);
                instructions.clear();
                continue;
            }
            let Some(choice) = choice else { break };
            if self.stopped(tree) || out.status == OutcomeStatus::Cancelled {
                break;
            }
            if accepted {
                let mut st = tree.lock();
                st.stands_unconfirmed =
                    choice.action == "accept_unconfirmed" || st.kept_contradicted;
                let answer = st.owner_answer.clone();
                let done = outcome(&task.id, "done", &answer, out.artifacts.clone());
                st.outcome = Some(done.clone());
                return Ok(done);
            }
            if choice.action == "fail" {
                let mut st = tree.lock();
                st.stop_reason = if !choice.reason.is_empty() {
                    choice.reason.clone()
                } else if !st.reason.is_empty() {
                    st.reason.clone()
                } else {
                    NOT_ACCEPTED.into()
                };
                st.stop_by_code = false;
                st.check_told = choice.told.clone();
                return Ok(outcome(
                    &task.id,
                    "failed",
                    &st.stop_reason,
                    out.artifacts.clone(),
                ));
            }
            if choice.action == "ask" {
                tree.lock().owner_question = choice.question.clone();
                if tree.asks_in_place {
                    match self.ask_in_place(tree, &task, &choice.question).await {
                        Ok(more) => {
                            instructions = more;
                            continue;
                        }
                        Err(true) => break,
                        Err(false) => {}
                    }
                }
                return Ok(outcome(
                    &task.id,
                    "needs_user",
                    &choice.question,
                    out.artifacts.clone(),
                ));
            }
            if let Some(t) = choice
                .worker_type
                .as_ref()
                .filter(|t| self.deps.daemon.worker_types().contains(t))
            {
                if let Some(spec) = tree.lock().spec.as_mut() {
                    spec.worker_type = t.clone();
                }
            }
            instructions = if choice.missing.is_empty() {
                String::new()
            } else {
                format!("\nWhat the check found missing: {}", choice.missing)
            } + &format!("\nInstructions for this attempt: {}", choice.instructions);
            retrying = true;
            if attempts.failed_one() {
                match self
                    .check_in_step(tree, &task, &mut attempts, out.artifacts.clone())
                    .await
                {
                    Ok(more) => instructions.push_str(&more),
                    Err(None) => break,
                    Err(Some(failed)) => return Ok(failed),
                }
            }
        }
        let stop = tree.lock().stop.clone();
        Ok(outcome(
            &task.id,
            "cancelled",
            if stop.is_empty() {
                STOPPED_BY_YOU
            } else {
                &stop
            },
            Vec::new(),
        ))
    }

    /// supervisor.py:6356 `_read_report`.
    fn read_report(&self, tree: &Arc<Tree>, task: &Task, raw: &Attempt) -> Outcome {
        let report = match raw {
            Attempt::Outcome(o) => {
                tree.lock().reason = o.outcome.summary.clone();
                return Outcome {
                    task_id: task.id.clone(),
                    status: o.outcome.status,
                    summary: o.outcome.summary.clone(),
                    artifacts: Vec::new(),
                    evidence: o.outcome.evidence.clone(),
                };
            }
            Attempt::Report(py) => py,
        };
        let w = match worker_report_validate(report) {
            Ok(w) => w,
            Err(_) => {
                let reason = "The report from this attempt was incomplete.";
                tree.lock().reason = reason.into();
                return outcome(&task.id, "failed", reason, Vec::new());
            }
        };
        let mut st = tree.lock();
        st.owner_answer = w.answer.clone();
        st.how = w.how.clone();
        st.input_notes = w.input_notes.iter().map(Py::to_value).collect();
        st.conversation_proposals = w.conversation_proposals.iter().map(Py::to_value).collect();
        st.watch_met = w.watch_met;
        st.part_updates = w
            .part_updates
            .iter()
            .filter(|u| matches!(u, Py::Dict(_)))
            .map(Py::to_value)
            .collect();
        st.reason = if w.status == "not_done" {
            w.reason.clone()
        } else {
            String::new()
        };
        st.worker_question = if w.status == "not_done" {
            w.question.clone().unwrap_or_default()
        } else {
            String::new()
        };
        if w.status == "done" && !st.required_inputs.is_empty() {
            let covered: Vec<String> = st
                .input_notes
                .iter()
                .filter(|n| {
                    n.get("note").is_some_and(ferb_py::value::truthy)
                        && n.get("evidence").is_some_and(ferb_py::value::truthy)
                })
                .map(|n| {
                    n.get("input")
                        .map(ferb_py::value::py_str)
                        .unwrap_or_default()
                })
                .collect();
            let missing: Vec<String> = st
                .required_inputs
                .iter()
                .filter(|p| covered.iter().filter(|c| c == p).count() != 1)
                .cloned()
                .collect();
            let extra = covered.iter().any(|c| !st.required_inputs.contains(c));
            if !missing.is_empty() || extra {
                st.reason = format!(
                    "The complete evidence notes are missing or duplicated for: {}",
                    ferb_py::json::dumps(&missing)
                );
                return outcome(&task.id, "failed", &st.reason.clone(), w.artifacts.clone());
            }
        }
        if w.status == "done" {
            return outcome(&task.id, "done", &w.answer, w.artifacts.clone());
        }
        let summary = if st.reason.is_empty() {
            w.answer.clone()
        } else {
            st.reason.clone()
        };
        outcome(&task.id, "failed", &summary, w.artifacts.clone())
    }

    /// supervisor.py:6384 `_judge`.
    async fn judge(&self, tree: &Arc<Tree>, out: &Outcome, span: &str) -> Flow<Next> {
        tree.lock().outcome = Some(out.clone());
        let (conditions, watch, watch_met, reason) = {
            let st = tree.lock();
            let spec = st.spec.clone().expect("a spec");
            (
                spec.success_conditions,
                st.watch,
                st.watch_met,
                st.reason.clone(),
            )
        };
        let checked = crate::lines::checked_conditions(&conditions, watch, watch_met);
        if checked.is_empty() {
            let mut next = Next::of(if out.status == OutcomeStatus::Done {
                "accept"
            } else {
                "fail"
            });
            next.reason = reason;
            return Ok(next);
        }
        let verdict = match self.verify(tree, span).await {
            Ok((results, verdict)) => {
                tree.lock().verification = results;
                verdict
            }
            Err(Failure::NotPorted(n)) => return Err(Failure::NotPorted(n)),
            // the checker's runtime hit an account fact: the attempt loop offers another runtime
            Err(e @ Failure::Raised { .. }) if e.class() == RUNTIME_DOWN => return Err(e),
            Err(e) => {
                self.journal(
                    "task.inspection_failed",
                    json!({"task_id": tree.worker.id, "error": format!("{}: {}", e.class(), e.message())}),
                    None,
                );
                let err = match &e {
                    Failure::Worker(w) => w.clone(),
                    f => ferb_worker::Error::other(&f.class(), f.message()),
                };
                let problem = self.worker.get().expect("the worker").reason(&err);
                let mut st = tree.lock();
                st.check_problem = problem;
                st.verification = checked
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        ConditionResult::new(
                            i as i64,
                            &c.text,
                            None,
                            UNCHECKED,
                            &format!("The check couldn't run: {}: {}", e.class(), e.message()),
                        )
                    })
                    .collect();
                None
            }
        };
        let Some(verdict) = verdict else {
            if out.status == OutcomeStatus::Done {
                let mut st = tree.lock();
                let problem = st.check_problem.clone();
                st.unconfirmed = if problem.is_empty() {
                    "the check couldn't run".into()
                } else {
                    format!("the check couldn't run: {}", problem.trim_end_matches('.'))
                };
                return Ok(Next::of("accept_unconfirmed"));
            }
            let mut next = Next::of("fail");
            next.reason = tree.lock().reason.clone();
            return Ok(next);
        };
        Ok(self.next(tree, out, &verdict))
    }

    /// supervisor.py:6643 `_found`.
    fn found(&self, tree: &Arc<Tree>, r: &ConditionResult) -> String {
        let said = self.owner_text(tree, r);
        let said = ferb_orchestrator::py::strip(&said)
            .trim_end_matches('.')
            .to_string();
        let found = ferb_orchestrator::py::strip(&r.evidence).to_string();
        let lead = if r.met == Some(false) {
            "Not done"
        } else {
            "Couldn't confirm"
        };
        let mut chars = found.chars();
        let first: String = chars
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default();
        format!("{lead}: {said}. {first}{}", chars.as_str())
    }

    /// supervisor.py:6417 `_next`.
    fn next(&self, tree: &Arc<Tree>, out: &Outcome, checked: &CheckerOutcome) -> Next {
        let (planned, verification, reason, worker_question, judged) = {
            let st = tree.lock();
            let spec = st.spec.clone().expect("a spec");
            (
                spec.success_conditions.len() as i64 + i64::from(st.watch),
                st.verification.clone(),
                st.reason.clone(),
                st.worker_question.clone(),
                st.checker_judged.clone(),
            )
        };
        let outcomes: Vec<ConditionResult> = verification
            .into_iter()
            .filter(|r| r.index < planned)
            .collect();
        let mut action = checked.next.clone();
        let verified = planned > 0
            && outcomes.len() as i64 == planned
            && outcomes.iter().all(|r| r.met == Some(true));
        if !checked.dropped_edits.is_empty() {
            self.journal(
                "task.answer_edit_dropped",
                json!({"task_id": tree.worker.id, "finds": checked.dropped_edits}),
                None,
            );
        }
        if action == "retry" && verified {
            action = "accept".into();
        }
        if action == "accept" || action == "accept_unconfirmed" {
            let refuted: Vec<&ConditionResult> = outcomes
                .iter()
                .filter(|r| r.met == Some(false) && r.path == "code")
                .collect();
            let unknown: Vec<&ConditionResult> = outcomes
                .iter()
                .filter(|r| r.met != Some(true) && r.path != DECLINED)
                .collect();
            if !refuted.is_empty() || (!unknown.is_empty() && out.status != OutcomeStatus::Done) {
                let why = if !checked.reason.is_empty() {
                    checked.reason.clone()
                } else if refuted.is_empty() && !reason.is_empty() {
                    reason.clone()
                } else {
                    let from = if refuted.is_empty() {
                        &unknown
                    } else {
                        &refuted
                    };
                    from.iter()
                        .map(|r| self.found(tree, r))
                        .collect::<Vec<_>>()
                        .join("; ")
                };
                let mut next = Next::of("fail");
                next.reason = why;
                return next;
            }
            let lines: Vec<String> = unknown
                .iter()
                .map(|r| {
                    let text = self.owner_text(tree, r);
                    let base = ferb_orchestrator::py::strip(&text)
                        .trim_end_matches('.')
                        .to_string();
                    if !r.how.is_empty() && judged.contains(&r.index) {
                        format!("{base} ({})", r.how.trim_end_matches('.'))
                    } else {
                        base
                    }
                })
                .filter(|l| !l.is_empty())
                .collect();
            tree.lock().unconfirmed = lines.join("; ");
            let findings = if out.status == OutcomeStatus::Done {
                checked
                    .findings
                    .iter()
                    .filter(|f| !ferb_orchestrator::py::strip(&f.quote).is_empty())
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };
            let mut next = Next::of(if unknown.is_empty() {
                "accept"
            } else {
                "accept_unconfirmed"
            });
            next.findings = findings;
            return next;
        }
        if action == "retry" {
            let instructions = [
                checked.instructions.trim(),
                checked.missing.trim(),
                reason.as_str(),
            ]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or("")
            .to_string();
            let mut next = Next::of("retry");
            next.instructions = instructions;
            next.missing = checked.missing.trim().to_string();
            next.worker_type = checked.worker_type.clone();
            return next;
        }
        if action == "ask" {
            let question = if checked.question.trim().is_empty() {
                worker_question.clone()
            } else {
                checked.question.trim().to_string()
            };
            if !question.is_empty() {
                let mut next = Next::of("ask");
                next.question = question;
                return next;
            }
        }
        let found = outcomes
            .iter()
            .filter(|r| r.met != Some(true))
            .map(|r| self.found(tree, r))
            .collect::<Vec<_>>()
            .join("; ");
        let findings = if out.status != OutcomeStatus::Done {
            checked
                .findings
                .iter()
                .filter(|f| !ferb_orchestrator::py::strip(&f.quote).is_empty())
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let reason_said = [
            checked.reason.trim(),
            reason.as_str(),
            found.as_str(),
            NOT_ACCEPTED,
        ]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string();
        let mut next = Next::of("fail");
        next.reason = reason_said;
        next.findings = findings;
        next.told = if action == "fail" {
            checked.reason.trim().to_string()
        } else {
            String::new()
        };
        next
    }

    /// supervisor.py:7078 `verify`.
    async fn verify(
        &self,
        tree: &Arc<Tree>,
        span: &str,
    ) -> Flow<(Vec<ConditionResult>, Option<CheckerOutcome>)> {
        let task = tree.worker.clone();
        let (conditions, watch, watch_met) = {
            let st = tree.lock();
            (
                st.spec.clone().expect("a spec").success_conditions,
                st.watch,
                st.watch_met,
            )
        };
        tree.lock().checker_judged.clear();
        let judged: Vec<(usize, Condition)> =
            crate::lines::checked_conditions(&conditions, watch, watch_met)
                .into_iter()
                .enumerate()
                .collect();
        let mut observations = Vec::new();
        for (i, c) in &judged {
            if self.deps.daemon.code_checkable(c, &task.id) {
                observations.push(self.deps.daemon.code_check(*i as i64, c, &task.id).await);
            }
        }
        let (results, verdict) = self
            .verifier(tree, &task, &judged, span, &observations)
            .await?;
        let idle: BTreeSet<i64> = verdict
            .as_ref()
            .map(|v| {
                v.results
                    .iter()
                    .filter(|r| !r.applies)
                    .filter_map(|r| r.index.as_i64())
                    .collect()
            })
            .unwrap_or_default();
        let judged_set = tree.lock().checker_judged.clone();
        let unconfirmed: BTreeSet<i64> = if verdict.is_some() {
            results
                .iter()
                .filter(|r| r.met.is_none() && !judged_set.contains(&r.index))
                .map(|r| r.index)
                .collect()
        } else {
            BTreeSet::new()
        };
        let judged_kind = |i: i64| {
            judged
                .iter()
                .find(|(n, _)| *n as i64 == i)
                .map(|(_, c)| c.check.kind.clone())
                .unwrap_or_default()
        };
        let refuted: Vec<&ConditionResult> = observations
            .iter()
            .filter(|r| {
                r.met == Some(false)
                    && !idle.contains(&r.index)
                    && !unconfirmed.contains(&r.index)
                    && ACTION_CHECKS.contains(&judged_kind(r.index).as_str())
            })
            .collect();
        let mut merged: Vec<ConditionResult> = results
            .into_iter()
            .map(|r| {
                refuted
                    .iter()
                    .find(|x| x.index == r.index)
                    .map(|x| (*x).clone())
                    .unwrap_or(r)
            })
            .collect();
        for r in merged.iter_mut() {
            if r.met == Some(false) && r.path != DECLINED {
                if let Some((_, c)) = judged.iter().find(|(n, _)| *n as i64 == r.index) {
                    if let Some(owner) = self.deps.daemon.declined(r.index, c, &task.id) {
                        *r = owner;
                    }
                }
            }
        }
        Ok((merged, verdict))
    }

    /// supervisor.py:7116 `_carry`.
    fn carry(&self, tree: &Arc<Tree>, results: Vec<ConditionResult>) -> Vec<ConditionResult> {
        let mut st = tree.lock();
        let planned = st.spec.as_ref().expect("a spec").success_conditions.len() as i64;
        let mut kept = Vec::new();
        for mut r in results {
            let earlier = if r.index < planned {
                st.confirmed.get(&r.index).cloned()
            } else {
                None
            };
            if r.index < planned && r.met == Some(true) {
                st.confirmed.insert(r.index, r.clone());
            } else if r.index < planned && r.met == Some(false) {
                st.confirmed.remove(&r.index);
            } else if let Some(e) = earlier.filter(|_| r.met.is_none()) {
                r = ConditionResult {
                    evidence: format!("Confirmed in an earlier attempt: {}", e.evidence),
                    ..e
                };
            }
            kept.push(r);
        }
        kept
    }

    /// supervisor.py:7133 `_verifier`.
    async fn verifier(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        judged: &[(usize, Condition)],
        span: &str,
        observations: &[ConditionResult],
    ) -> Flow<(Vec<ConditionResult>, Option<CheckerOutcome>)> {
        let (reads, calls_since) = {
            let st = tree.lock();
            let spec = st.spec.clone().expect("a spec");
            let planned = spec.success_conditions.len() as i64;
            let mut confirmed: Vec<(i64, String)> = st
                .confirmed
                .iter()
                .filter(|(i, _)| **i < planned)
                .map(|(i, r)| (*i, r.evidence.clone()))
                .collect();
            confirmed.sort();
            let observations: Vec<Value> = observations
                .iter()
                .map(|r| {
                    let c = &spec.success_conditions[r.index as usize];
                    json!({"index": r.index, "predicate_met": r.met, "observation": r.evidence,
                           "predicate": {"kind": c.check.kind, "path": c.check.path, "text": c.check.text, "url_contains": c.check.url_contains, "site": c.check.site, "expected": c.check.expected, "tool": c.check.tool}})
                })
                .collect();
            (
                VerifierReads {
                    owner_answer: st.owner_answer.clone(),
                    judged: judged
                        .iter()
                        .map(|(i, c)| (*i, c.text.clone(), c.layer.clone()))
                        .collect(),
                    confirmed: confirmed
                        .into_iter()
                        .map(|(i, e)| (i.to_string(), json!(e)))
                        .collect(),
                    done: st
                        .outcome
                        .as_ref()
                        .is_some_and(|o| o.status == OutcomeStatus::Done),
                    artifacts: st
                        .outcome
                        .as_ref()
                        .map(|o| o.artifacts.clone())
                        .unwrap_or_default(),
                    reason: st.reason.clone(),
                    worker_question: st.worker_question.clone(),
                    part_updates: st.part_updates.clone(),
                    worker_types: self.deps.daemon.worker_types(),
                    availability: self.deps.daemon.availability(),
                    observations,
                    calls_since: 0,
                    began: None,
                },
                0i64,
            )
        };
        let began = self.request_began(&tree.id, &task.request_id, &task.id);
        let mut reads = reads;
        reads.began = began.as_ref().map(|b| b.0.clone());
        let report = worker_context::verifier_report(&reads, &|t| self.wrap(t));
        let mut context = self.context(tree, false).await?;
        context.push(self.code_facts(task, calls_since));
        let above =
            |blocks: &[ContextBlock]| 1 + blocks.iter().map(|b| b.priority).max().unwrap_or(0);
        let mut ctx = context.clone();
        ctx.push(protected(
            report.clone(),
            "complete report and inspection evidence",
        ));
        let calls = {
            let secrets: Vec<String> = Vec::new();
            let redactor = self.deps.redactor.clone();
            let redact: ferb_orchestrator::ports::Redact =
                Arc::new(move |v: &Value| redactor.value(v));
            let wrap = self.deps.wrap.clone();
            let wrap: ferb_orchestrator::ports::Wrap = Arc::new(move |t: &str| wrap(t));
            let daemon = self.deps.daemon.clone();
            let observes = move |tool: &str| daemon.observes(tool);
            let priority = above(&ctx);
            self.with_spans(|s| {
                ferb_orchestrator::evidence::recorded_calls(
                    s,
                    &task.id,
                    &tree.id,
                    &observes,
                    &redact,
                    &secrets,
                    &wrap,
                    priority,
                    began.as_ref().map(|b| b.1).unwrap_or(0),
                    calls_since,
                )
            })
            .map_err(Failure::from)?
        };
        ctx.extend(calls);
        let tail = "\nThe worker's recorded tool calls and their results follow. More: task_evidence; memory.read; \
                    chat.history";
        let plain_report = format!(
            "{}\nThis check's context holds no recorded tool calls: with them, it overflowed the model's window; list \
             and read them with task_evidence. Full observations: task_evidence; memory.read; chat.history",
            report.strip_suffix(tail).unwrap_or(&report)
        );
        let mut plain = context.clone();
        plain.push(protected(
            plain_report,
            "complete report and inspection evidence",
        ));
        let vtask = Task {
            id: format!("{}.verify", task.id),
            parent_id: Some(task.id.clone()),
            goal: "verify".into(),
            device_id: self.deps.device_id.clone(),
            identity: task.identity,
            scopes: task.scopes.clone(),
            role: TaskRole::Verifier,
            request_id: task.request_id.clone(),
            success_conditions: Vec::new(),
            deadline: task.deadline,
        };
        let changed = self.changed_files(tree, task, above(&ctx));
        ctx.extend(changed.blocks.clone());
        plain.extend(changed.blocks.clone());
        let effort = if changed.records.is_empty() {
            None
        } else {
            Some("high".to_string())
        };
        let cursor: i64 = {
            let db = self.db();
            db.conn()
                .query_row("SELECT COALESCE(MAX(rowid), 0) FROM spans", [], |r| {
                    r.get(0)
                })
                .unwrap_or(0)
        };
        let checked = self
            .check(
                tree,
                &vtask,
                judged,
                span,
                ctx.clone(),
                plain.clone(),
                effort.clone(),
            )
            .await?;
        let mut verdict = match checked {
            Checked::Unchecked(results) => return Ok((results, None)),
            Checked::Verdict(v) => v,
        };
        tree.lock().checker_judged = verdict
            .results
            .iter()
            .filter(|r| r.met.is_some())
            .filter_map(|r| r.index.as_i64())
            .collect();
        if !changed.records.is_empty() && verdict.results.iter().any(|r| r.met == Some(true)) {
            let unread = self.unread(&vtask.id, cursor, &changed.records);
            if !unread.is_empty() {
                let refused = self.span_start(
                    "verify.refused",
                    Some(&vtask.id),
                    Some(span),
                    None,
                    json!({"unread": unread}),
                );
                self.span_finish(
                    &refused,
                    Some(json!({"rerun": "once, with the reason"})),
                    None,
                    None,
                    None,
                );
                let files: Vec<String> = unread
                    .iter()
                    .map(|path| {
                        let id = changed
                            .records
                            .iter()
                            .find(|(p, _)| p == path)
                            .map(|(_, s)| s.clone())
                            .unwrap_or_default();
                        format!(
                            "{} (task_evidence span_id={id})",
                            ferb_py::json::dumps_utf8(path)
                        )
                    })
                    .collect();
                let why = protected(
                    REFUSED_PASS.replace("{files}", &files.join("; ")),
                    "FERB refused this check's pass",
                );
                let mut ctx2 = ctx.clone();
                ctx2.push(why.clone());
                let mut plain2 = plain.clone();
                plain2.push(why);
                match self
                    .check(tree, &vtask, judged, span, ctx2, plain2, effort)
                    .await?
                {
                    Checked::Unchecked(results) => return Ok((results, None)),
                    Checked::Verdict(v) => verdict = v,
                }
                tree.lock().checker_judged = verdict
                    .results
                    .iter()
                    .filter(|r| r.met.is_some())
                    .filter_map(|r| r.index.as_i64())
                    .collect();
                if verdict.results.iter().any(|r| r.met == Some(true)) {
                    let unread = self.unread(&vtask.id, cursor, &changed.records);
                    if !unread.is_empty() {
                        let again = self.span_start(
                            "verify.refused",
                            Some(&vtask.id),
                            Some(span),
                            None,
                            json!({"unread": unread}),
                        );
                        self.span_finish(
                            &again,
                            Some(json!({"result": "met=null: a pass without a look is never a pass"})),
                            None,
                            None,
                            None,
                        );
                        let shown = unread
                            .iter()
                            .map(|p| ferb_fs::changes::shown(p, &self.deps.home))
                            .collect::<Vec<_>>()
                            .join(", ");
                        for r in verdict.results.iter_mut() {
                            if r.met == Some(true) {
                                r.met = None;
                                r.evidence = format!(
                                    "{} {}",
                                    NO_LOOK.replace("{files}", &shown),
                                    r.evidence
                                );
                                r.how = Some(NO_LOOK_HOW.replace("{files}", &shown));
                            }
                        }
                    }
                }
            }
        }
        self.set_status(
            &vtask.id,
            "done",
            Some(&dump_checker_outcome(&verdict).to_value()),
        );
        let results: Vec<ConditionResult> = judged
            .iter()
            .map(|(i, c)| {
                let got = verdict
                    .results
                    .iter()
                    .find(|r| r.index.as_i64() == Some(*i as i64));
                match got {
                    Some(g) if !g.applies => ConditionResult::new(
                        *i as i64,
                        &c.text,
                        Some(true),
                        "verifier",
                        &format!("{DIDNT_APPLY}{}", g.evidence),
                    ),
                    Some(g) => {
                        let mut r = ConditionResult::new(
                            *i as i64,
                            &c.text,
                            g.met,
                            "verifier",
                            &g.evidence,
                        );
                        if g.met.is_none() {
                            r.how = g.how.clone().unwrap_or_default();
                        }
                        r
                    }
                    None => ConditionResult::new(
                        *i as i64,
                        &c.text,
                        None,
                        "verifier",
                        "The check gave no answer for this.",
                    ),
                }
            })
            .collect();
        Ok((self.carry(tree, results), Some(verdict)))
    }

    /// supervisor.py:7234 `_code_facts`.
    fn code_facts(&self, task: &Task, since: i64) -> ContextBlock {
        let facts = {
            let db = self.db();
            ferb_orchestrator::evidence::how_facts(&db, &task.id, since).unwrap_or(Value::Null)
        };
        let tabs = self.deps.daemon.task_tabs(&task.id);
        worker_context::code_facts(&obj(facts), &tabs, &|t| self.wrap(t))
    }

    /// supervisor.py:7248 `_check`.
    #[allow(clippy::too_many_arguments)] // _check's own arguments
    async fn check(
        &self,
        tree: &Arc<Tree>,
        vtask: &Task,
        judged: &[(usize, Condition)],
        span: &str,
        ctx: Vec<ContextBlock>,
        plain: Vec<ContextBlock>,
        effort: Option<String>,
    ) -> Flow<Checked> {
        let worker = self.worker.get().expect("the worker").clone();
        let wtree: Arc<dyn ferb_worker::ports::WorkerTree> = tree.clone();
        let schema = ferb_context::schemas::checker_schema();
        let options = RunOptions {
            effort: effort.clone(),
            roles: Vec::new(),
            observe_only: false,
        };
        let ran = worker
            .run_worker(&wtree, vtask, "verifier.md", ctx, &schema, span, &options)
            .await;
        let (mut raw, _) = match ran {
            Ok(r) => r,
            Err(e) => {
                self.set_status(
                    &vtask.id,
                    "failed",
                    Some(&json!({"summary": format!("Inspection unavailable: {}", e.class())})),
                );
                return Err(Failure::Worker(e));
            }
        };
        if let Attempt::Outcome(o) = &raw {
            if o.kind == OutcomeKind::Overflowed {
                let note = self.span_start(
                    "verify.rerun",
                    Some(&vtask.id),
                    Some(span),
                    None,
                    json!({"reason": o.outcome.summary}),
                );
                self.span_finish(
                    &note,
                    Some(json!({"rerun": "once, without the recorded calls"})),
                    None,
                    None,
                    None,
                );
                self.journal(
                    "task.check_rerun",
                    json!({"task_id": vtask.parent_id, "reason": o.outcome.summary}),
                    None,
                );
                raw = worker
                    .run_worker(&wtree, vtask, "verifier.md", plain, &schema, span, &options)
                    .await
                    .map_err(Failure::Worker)?
                    .0;
            }
        }
        match raw {
            Attempt::Outcome(o) => {
                if matches!(o.kind, OutcomeKind::ServiceDown { .. }) {
                    self.set_status(
                        &vtask.id,
                        "failed",
                        Some(&json!({"summary": o.outcome.summary})),
                    );
                    // `_RuntimeDown(raw)`: the attempt loop offers another runtime (`_service_down`)
                    let OutcomeKind::ServiceDown { runtime, problem } = &o.kind else {
                        unreachable!()
                    };
                    return Err(Failure::raised(
                        RUNTIME_DOWN,
                        ferb_py::json::dumps(&json!({"task_id": o.outcome.task_id,
                            "runtime": runtime, "problem": problem, "summary": o.outcome.summary})),
                    ));
                }
                let status = if o.outcome.status == OutcomeStatus::Cancelled {
                    "cancelled"
                } else {
                    "failed"
                };
                self.set_status(
                    &vtask.id,
                    status,
                    Some(&json!({"summary": o.outcome.summary})),
                );
                let detail = if o.outcome.evidence.is_empty() {
                    o.outcome.summary.clone()
                } else {
                    o.outcome.evidence.join("; ")
                };
                tree.lock().check_problem = o.outcome.summary.clone();
                Ok(Checked::Unchecked(
                    judged
                        .iter()
                        .map(|(i, c)| {
                            ConditionResult::new(
                                *i as i64,
                                &c.text,
                                None,
                                UNCHECKED,
                                &format!("The check couldn't finish: {detail}"),
                            )
                        })
                        .collect(),
                ))
            }
            Attempt::Report(py) => match checker_outcome_validate(&py) {
                Ok(v) => Ok(Checked::Verdict(v)),
                Err(e) => {
                    let class = e.raised().map(|r| r.exception).unwrap_or("ValidationError");
                    self.set_status(
                        &vtask.id,
                        "failed",
                        Some(&json!({"summary": format!("Inspection unavailable: {class}")})),
                    );
                    Err(Failure::raised(class, e.to_string()))
                }
            },
        }
    }

    /// supervisor.py:7281 `_changed_files` (L39).
    fn changed_files(&self, tree: &Arc<Tree>, task: &Task, above: i64) -> Changed {
        let Some(found) = self.deps.daemon.changes(&task.id, &tree.id) else {
            return Changed::default();
        };
        let found = match found {
            Ok(f) => f,
            Err(e) => {
                self.journal(
                    "task.changes_failed",
                    json!({"task_id": task.id, "error": e}),
                    None,
                );
                return Changed::default();
            }
        };
        if found.is_empty() {
            return Changed::default();
        }
        let untrusted = self.deps.daemon.untrusted_roots();
        let named = found.iter().filter(|c| c.content).count() as i64;
        let mut records: Vec<(String, String)> = Vec::new();
        let (mut listed, mut diffs, mut facts) = (Vec::new(), Vec::new(), Vec::new());
        for change in &found {
            let text = ferb_py::value::py_str(&self.redact(&json!({"t": change.text}))["t"]);
            if !change.content && !change.named {
                facts.push(text);
                continue;
            }
            let n = listed.len() + 1;
            if !change.content {
                let rest = text
                    .split_once(": ")
                    .map(|(_, r)| r.to_string())
                    .unwrap_or_default();
                listed.push(format!(
                    "{n}. {} (named by the call; a fact, nothing to read): {}",
                    ferb_py::json::dumps_utf8(&change.path),
                    ferb_py::json::dumps_utf8(&rest)
                ));
                continue;
            }
            let outside = untrusted.iter().any(|u| {
                change.path == *u
                    || change
                        .path
                        .starts_with(&format!("{}/", u.trim_end_matches('/')))
            });
            let made = if outside {
                origin::external("download", "")
            } else {
                origin::PRIVATE.to_string()
            };
            let block =
                self.source_block(tree, &format!("Change record {n}"), &text, &made, &[], None);
            let span_id = block
                .reference
                .strip_prefix("task.evidence span_id=")
                .unwrap_or(&block.reference)
                .to_string();
            records.push((change.path.clone(), span_id.clone()));
            let page = ferb_orchestrator::evidence::PAGE_CHARS;
            let cut = if text.chars().count() <= page {
                text.clone()
            } else {
                text.chars().take(page).collect::<String>()
                    + &format!("\n[… the record holds the rest: task_evidence span_id={span_id}, following next_offset]")
            };
            let source = change
                .source
                .as_ref()
                .map(|s| format!(", compared with {}", ferb_py::json::dumps_utf8(s)))
                .unwrap_or_default();
            let what = match change.what.as_str() {
                "modified" => "changed".to_string(),
                "removed" => "removed".to_string(),
                "renamed" => format!(
                    "renamed from {}",
                    ferb_py::json::dumps_utf8(change.source.as_deref().unwrap_or(""))
                ),
                _ => format!("new{source}"),
            };
            listed.push(format!(
                "{n}. {} ({what}) — change record: task.evidence span_id={span_id}",
                ferb_py::json::dumps_utf8(&change.path)
            ));
            diffs.push(ContextBlock {
                priority: above + named - records.len() as i64 + 1,
                origin: made,
                payload: Some(cut.clone()),
                ..ContextBlock::new(
                    format!("Change record {n} (data): {}", self.wrap(&cut)),
                    block.reference.clone(),
                )
            });
        }
        let mut blocks = Vec::new();
        if !listed.is_empty() {
            blocks.push(protected(
                format!("{CHANGED_FILES}\n{}", listed.join("\n")),
                "files this step changed",
            ));
        }
        if !facts.is_empty() {
            let (shown, more) =
                crate::lines::within(&facts, ferb_orchestrator::evidence::PAGE_CHARS);
            let tail = if more > 0 {
                format!("\n… and {more} more changed during this step")
            } else {
                String::new()
            };
            blocks.push(ContextBlock {
                protected: true,
                origin: origin::PRIVATE.into(),
                ..ContextBlock::new(
                    format!(
                        "{OTHER_CHANGES}{}",
                        self.wrap(&format!("{}{tail}", shown.join("\n")))
                    ),
                    "other changes during this step",
                )
            });
        }
        blocks.extend(diffs);
        Changed { records, blocks }
    }

    /// supervisor.py:7344 `_unread`.
    fn unread(&self, check_id: &str, cursor: i64, records: &[(String, String)]) -> Vec<String> {
        let rows = {
            let db = self.db();
            ferb_orchestrator::sql::rows(
                db.conn(),
                "SELECT input_json, output_json FROM spans WHERE task_id = ? AND kind = 'tool.call' AND rowid > ? \
                 AND error IS NULL",
                &[
                    rusqlite::types::Value::Text(check_id.into()),
                    rusqlite::types::Value::Integer(cursor),
                ],
            )
            .unwrap_or_default()
        };
        let mut seen_records = BTreeSet::new();
        let mut seen_paths = BTreeSet::new();
        for row in rows {
            let call = ferb_py::json::loads(
                row.get("input_json")
                    .and_then(Value::as_str)
                    .unwrap_or("{}"),
            )
            .unwrap_or(Value::Null);
            let target = call
                .get("target")
                .and_then(|t| t.get("path"))
                .map(ferb_py::value::py_str);
            let tool = call
                .get("tool")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            if tool == "task.evidence" {
                let got = row
                    .get("output_json")
                    .and_then(Value::as_str)
                    .and_then(|o| ferb_py::json::loads(o).ok())
                    .unwrap_or(Value::Null);
                if let Some(pages) = got.get("records").and_then(Value::as_array) {
                    for r in pages {
                        if r.get("span_id").is_some_and(ferb_py::value::truthy)
                            && r.get("text").is_some()
                        {
                            seen_records.insert(ferb_py::value::py_str(&r["span_id"]));
                        }
                    }
                }
                if got.get("span_id").is_some_and(ferb_py::value::truthy)
                    && got.get("text").is_some()
                {
                    seen_records.insert(ferb_py::value::py_str(&got["span_id"]));
                }
            } else if let Some(t) = target.filter(|t| !t.is_empty()) {
                if self.deps.daemon.reads_content(&tool) {
                    if let Some(real) = ferb_fs::changes::real(&t, &self.deps.home) {
                        seen_paths.insert(real);
                    }
                }
            }
        }
        records
            .iter()
            .filter(|(path, record)| !seen_records.contains(record) && !seen_paths.contains(path))
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// supervisor.py:6470 `_settle_findings` (ORCH-545).
    async fn settle_findings(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        findings: &[Finding],
        span: &str,
    ) {
        let findings: Vec<Finding> = findings
            .iter()
            .filter(|f| !ferb_orchestrator::py::strip(&f.quote).is_empty())
            .cloned()
            .collect();
        if findings.is_empty() || tree.lock().answer_settled {
            return;
        }
        tree.lock().answer_settled = true;
        let original = tree.lock().owner_answer.clone();
        let dumped: Vec<Value> = findings
            .iter()
            .map(|f| dump_finding(f).to_value())
            .collect();
        self.journal(
            "task.answer_findings",
            json!({"task_id": task.id, "findings": dumped}),
            None,
        );
        self.keep_fixing(tree, task, Some(&original), &dumped);
        let quotes: Vec<String> = findings.iter().map(|f| f.quote.clone()).collect();
        let mut revised = self.fix_turn(tree, task, &original, &dumped, span).await;
        if let Some(r) = &revised {
            if !ferb_orchestrator::answer_marks::only_quoted_changed(&original, r, &quotes) {
                self.journal(
                    "task.answer_fix_discarded",
                    json!({"task_id": task.id}),
                    None,
                );
                revised = None;
            }
        }
        let remaining = match &revised {
            Some(r) => self.recheck(tree, task, &dumped, r, span).await,
            None => None,
        };
        let pairs = |fs: &[Finding]| -> Vec<(String, String)> {
            fs.iter()
                .map(|f| (f.quote.clone(), f.kind.clone()))
                .collect()
        };
        let (done, how) = match (&revised, &remaining) {
            (Some(r), Some(rem)) if rem.is_empty() => {
                tree.lock().owner_answer = r.clone();
                (ferb_orchestrator::answer_marks::settle(r, &[]), "fixed")
            }
            (Some(r), Some(rem)) if rem.iter().all(|f| r.contains(&f.quote)) => {
                let done = ferb_orchestrator::answer_marks::settle(r, &pairs(rem));
                let mut st = tree.lock();
                st.owner_answer = done.answer.clone();
                st.answer_unmarked = ferb_orchestrator::answer_marks::unmarked(r, &pairs(rem));
                (done, "marked")
            }
            _ => {
                let done = ferb_orchestrator::answer_marks::settle(&original, &pairs(&findings));
                let mut st = tree.lock();
                st.owner_answer = done.answer.clone();
                st.answer_unmarked =
                    ferb_orchestrator::answer_marks::unmarked(&original, &pairs(&findings));
                (done, "marked")
            }
        };
        let mut data = obj(
            json!({"task_id": task.id, "how": how, "removed": done.removed, "marked": done.marked,
            "missing": done.missing}),
        );
        if !done.kept_contradicted.is_empty() {
            data.insert("kept_contradicted".into(), json!(done.kept_contradicted));
        }
        self.journal("task.answer_settled", Value::Object(data), None);
        if !done.kept_contradicted.is_empty() {
            let mut st = tree.lock();
            st.kept_contradicted = true;
            st.stands_unconfirmed = true;
            if !st.unconfirmed.contains(REPORT_UNCONFIRMED) {
                st.unconfirmed = [st.unconfirmed.as_str(), REPORT_UNCONFIRMED]
                    .iter()
                    .filter(|p| !p.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join("; ");
            }
        }
        self.keep_fixing(tree, task, None, &[]);
    }

    /// supervisor.py:6541 `_restored_fix`: a restart came during the step's fix round (ORCH-545): the work and its
    /// check stand as saved, and code settles the saved answer's quoted sentences (never run twice, nor redone).
    async fn restored_fix(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        saved: &Map<String, Value>,
        span: &str,
    ) -> Flow<Outcome> {
        let planned = {
            let st = tree.lock();
            st.spec.as_ref().map_or(0, |s| s.success_conditions.len()) + usize::from(st.watch)
        } as i64;
        let verification = saved_results(saved.get("verification"), planned)?;
        let list = |k: &str| -> Vec<Value> {
            saved
                .get(k)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter(|v| v.is_object()).cloned().collect())
                .unwrap_or_default()
        };
        let mut findings = Vec::new();
        for f in list("findings") {
            let f =
                ferb_orchestrator::validate::finding_validate(&ferb_context::Py::from_value(&f))
                    .map_err(|e| Failure::raised("ValidationError", e.to_string()))?;
            findings.push((f.quote.clone(), f.kind.clone()));
        }
        let answer = saved
            .get("answer")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let done = ferb_orchestrator::answer_marks::settle(&answer, &findings);
        {
            let mut st = tree.lock();
            st.stands_unconfirmed =
                verification.iter().any(|r| r.met != Some(true)) || verification.is_empty();
            st.verification = verification;
            st.unconfirmed = saved
                .get("unconfirmed")
                .filter(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            st.input_notes = list("input_notes");
            st.conversation_proposals = list("conversation_proposals");
            st.part_updates = list("part_updates");
            st.how = saved
                .get("how")
                .filter(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            st.owner_answer = done.answer.clone();
            st.answer_settled = true;
            st.answer_unmarked = ferb_orchestrator::answer_marks::unmarked(&answer, &findings);
        }
        let mut data = obj(json!({"task_id": task.id, "how": "marked", "restart": true,
            "removed": done.removed, "marked": done.marked, "missing": done.missing}));
        if !done.kept_contradicted.is_empty() {
            data.insert("kept_contradicted".into(), json!(done.kept_contradicted));
        }
        self.journal("task.answer_settled", Value::Object(data), None);
        if !done.kept_contradicted.is_empty() {
            let mut st = tree.lock();
            st.kept_contradicted = true;
            st.stands_unconfirmed = true;
            if !st.unconfirmed.contains(REPORT_UNCONFIRMED) {
                st.unconfirmed = [st.unconfirmed.as_str(), REPORT_UNCONFIRMED]
                    .iter()
                    .filter(|p| !p.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join("; ");
            }
        }
        self.keep_fixing(tree, task, None, &[]);
        let mine = self
            .live()
            .tree_of
            .get(&tree.id)
            .is_some_and(|t| Arc::ptr_eq(t, tree));
        if mine {
            tree.lock().work_done = true;
        }
        self.release_resources(tree, span).await;
        let strs = |k: &str| -> Vec<String> {
            saved
                .get(k)
                .and_then(Value::as_array)
                .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                .unwrap_or_default()
        };
        let out = Outcome {
            task_id: task.id.clone(),
            status: OutcomeStatus::Done,
            summary: done.answer,
            artifacts: strs("artifacts"),
            evidence: strs("evidence"),
        };
        tree.lock().outcome = Some(out.clone());
        Ok(out)
    }

    /// supervisor.py:6527 `_keep_fixing`.
    fn keep_fixing(&self, tree: &Arc<Tree>, task: &Task, answer: Option<&str>, findings: &[Value]) {
        let fixing_step = (task.id == format!("{}.{WORKER}", tree.id)
            || task.id == format!("{}.step.{}", tree.id, tree.step_id))
            && self
                .plans(|p| p.steps(&tree.id))
                .unwrap_or_default()
                .iter()
                .any(|(k, _)| *k == tree.step_id);
        if !fixing_step {
            return;
        }
        let mut output = obj(json!({"status": "running"}));
        if let Some(answer) = answer {
            let st = tree.lock();
            output.insert(
                "fixing".into(),
                json!({"answer": answer, "findings": findings,
                    "artifacts": st.outcome.as_ref().map(|o| o.artifacts.clone()).unwrap_or_default(),
                    "evidence": st.outcome.as_ref().map(|o| o.evidence.clone()).unwrap_or_default(),
                    "verification": st.verification.iter().map(ConditionResult::as_dict).collect::<Vec<_>>(),
                    "unconfirmed": st.unconfirmed, "input_notes": st.input_notes,
                    "conversation_proposals": st.conversation_proposals, "part_updates": st.part_updates,
                    "how": st.how}),
            );
        }
        let _ = self.plans(|p| p.finish(&tree.id, &tree.step_id, &Value::Object(output)));
    }

    /// supervisor.py:6574 `_fix_turn`.
    async fn fix_turn(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        answer: &str,
        findings: &[Value],
        span: &str,
    ) -> Option<String> {
        let fix = Task {
            id: format!("{}.fix", task.id),
            parent_id: Some(task.id.clone()),
            goal: task.goal.clone(),
            device_id: self.deps.device_id.clone(),
            identity: task.identity,
            scopes: task.scopes.clone(),
            role: TaskRole::Mixed,
            request_id: task.request_id.clone(),
            success_conditions: Vec::new(),
            deadline: task.deadline,
        };
        let mut ctx = match self.context(tree, false).await {
            Ok(c) => c,
            Err(_) => return None,
        };
        ctx.push(worker_context::fix_block(answer, findings, &|t| {
            self.wrap(t)
        }));
        let worker = self.worker.get().expect("the worker").clone();
        let wtree: Arc<dyn ferb_worker::ports::WorkerTree> = tree.clone();
        let ran = worker
            .run_worker(
                &wtree,
                &fix,
                "worker.md",
                ctx,
                &ferb_context::schemas::fix_schema(),
                span,
                &RunOptions {
                    effort: None,
                    roles: vec!["fix".into()],
                    observe_only: true,
                },
            )
            .await;
        let raw = match ran {
            Ok((raw, _)) => raw,
            Err(e) => {
                self.journal(
                    "task.answer_fix_failed",
                    json!({"task_id": task.id, "error": e.failure()}),
                    None,
                );
                self.set_status(
                    &fix.id,
                    "failed",
                    Some(&json!({"summary": format!("The fix turn couldn't run: {}", e.class())})),
                );
                return None;
            }
        };
        let py = match raw {
            Attempt::Report(py) => py,
            Attempt::Outcome(o) => {
                self.journal(
                    "task.answer_fix_failed",
                    json!({"task_id": task.id, "error": o.outcome.summary}),
                    None,
                );
                let status = if o.outcome.status == OutcomeStatus::Cancelled {
                    "cancelled"
                } else {
                    "failed"
                };
                self.set_status(
                    &fix.id,
                    status,
                    Some(&json!({"summary": o.outcome.summary})),
                );
                return None;
            }
        };
        match fix_outcome_validate(&py) {
            Err(_) => {
                self.set_status(
                    &fix.id,
                    "failed",
                    Some(&json!({"summary": "The fix turn's report wasn't usable."})),
                );
                None
            }
            Ok(fixed) => {
                let ok = !ferb_orchestrator::py::strip(&fixed.answer).is_empty();
                self.set_status(
                    &fix.id,
                    if ok { "done" } else { "failed" },
                    Some(&dump_fix_outcome(&fixed).to_value()),
                );
                ok.then_some(fixed.answer)
            }
        }
    }

    /// supervisor.py:6610 `_recheck`.
    async fn recheck(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        findings: &[Value],
        revised: &str,
        span: &str,
    ) -> Option<Vec<Finding>> {
        let vtask = Task {
            id: format!("{}.recheck", task.id),
            parent_id: Some(task.id.clone()),
            goal: "recheck".into(),
            device_id: self.deps.device_id.clone(),
            identity: task.identity,
            scopes: task.scopes.clone(),
            role: TaskRole::Verifier,
            request_id: task.request_id.clone(),
            success_conditions: Vec::new(),
            deadline: task.deadline,
        };
        let block = worker_context::recheck_block(findings, revised, &|t| self.wrap(t));
        let worker = self.worker.get().expect("the worker").clone();
        let wtree: Arc<dyn ferb_worker::ports::WorkerTree> = tree.clone();
        let ran = worker
            .run_worker(
                &wtree,
                &vtask,
                "recheck.md",
                vec![block],
                &ferb_context::schemas::recheck_schema(),
                span,
                &RunOptions::default(),
            )
            .await;
        let raw = match ran {
            Ok((raw, _)) => raw,
            Err(e) => {
                self.journal(
                    "task.answer_recheck_failed",
                    json!({"task_id": task.id, "error": e.failure()}),
                    None,
                );
                self.set_status(
                    &vtask.id,
                    "failed",
                    Some(&json!({"summary": format!("The re-check couldn't run: {}", e.class())})),
                );
                return None;
            }
        };
        let py = match raw {
            Attempt::Report(py) => py,
            Attempt::Outcome(o) => {
                let status = if o.outcome.status == OutcomeStatus::Cancelled {
                    "cancelled"
                } else {
                    "failed"
                };
                self.set_status(
                    &vtask.id,
                    status,
                    Some(&json!({"summary": o.outcome.summary})),
                );
                return None;
            }
        };
        match recheck_outcome_validate(&py) {
            Err(_) => {
                self.set_status(
                    &vtask.id,
                    "failed",
                    Some(&json!({"summary": "The re-check's answer wasn't usable."})),
                );
                None
            }
            Ok(checked) => {
                self.set_status(
                    &vtask.id,
                    "done",
                    Some(&dump_recheck_outcome(&checked).to_value()),
                );
                Some(
                    checked
                        .findings
                        .into_iter()
                        .filter(|f| !ferb_orchestrator::py::strip(&f.quote).is_empty())
                        .collect(),
                )
            }
        }
    }
}

/// What one run of the checker gave: its verdict, or each condition unchecked with why.
enum Checked {
    Verdict(CheckerOutcome),
    Unchecked(Vec<ConditionResult>),
}

/// A saved check's results (`_restored_fix`, `_adopt_saved`): those whose index is below `planned`, as Python rebuilds
/// each `ConditionResult(int(r['index']), ...)`.
pub fn saved_results(v: Option<&Value>, planned: i64) -> Flow<Vec<ConditionResult>> {
    let mut out = Vec::new();
    for r in v.and_then(Value::as_array).cloned().unwrap_or_default() {
        let index = match r.get("index") {
            Some(Value::Number(n)) => n
                .as_i64()
                .or_else(|| n.as_f64().map(|f| f as i64))
                .unwrap_or(planned),
            Some(Value::String(t)) => t.trim().parse::<i64>().map_err(|_| {
                Failure::raised(
                    "ValueError",
                    format!("invalid literal for int() with base 10: '{t}'"),
                )
            })?,
            Some(Value::Bool(b)) => i64::from(*b),
            None => planned,
            Some(other) => {
                return Err(Failure::raised(
                    "TypeError",
                    format!("int() argument must be a string, a bytes-like object or a real number, not '{}'",
                        if other.is_null() { "NoneType" } else { "list" }),
                ))
            }
        };
        if index >= planned {
            continue;
        }
        let text = |k: &str, d: &str| {
            r.get(k)
                .map(ferb_py::value::py_str)
                .unwrap_or_else(|| d.to_string())
        };
        out.push(ConditionResult {
            index,
            text: text("condition", ""),
            met: r.get("met").and_then(Value::as_bool),
            path: text("path", "verifier"),
            evidence: text("evidence", ""),
            how: text("how", ""),
        });
    }
    Ok(out)
}
