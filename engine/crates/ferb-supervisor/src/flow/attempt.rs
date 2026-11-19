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
use ferb_orchestrator::verify::{ConditionResult, ACTION_CHECKS, DECLINED, DIFFERENT, UNCHECKED};
use ferb_sanitize::origin;
use ferb_worker::{Attempt, OutcomeKind, RunOptions};
use serde_json::{json, Map, Value};

use super::tree::{status_of, WORKER};
use super::{Failure, Flow, Gap, Inner, Tree};
use crate::words::*;
use crate::worker_context::{self, ContextReads, SourceBlocks, SourceCall, VerifierReads};

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

/// HAND-580: the worker's fact for a task whose owner's words named browser profiles.
pub fn task_profiles_fact(named: &[(String, String)]) -> String {
    let list = named
        .iter()
        .map(|(b, f)| format!("{b}/{f}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Browser profiles for this task (the owner's words named them): {list}. The browser tools work there, in the \
         background, with the profile argument (one of these, or none when there is one)."
    )
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
    /// ORCH-652: with accept_with_gaps, the conditions accepted as not done.
    pub gaps: Vec<Gap>,
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
    /// supervisor.py:5657 `_wait_cut` (ORCH-633): the latest wait.for of this tree that FERB's restart ended before it
    /// woke (`wait.ended`, journaled by the daemon), as a plain fact for the step's next run; None when there is none.
    pub(crate) fn wait_cut(&self, tree_id: &str) -> Option<String> {
        let rows = self.of_kind("wait.ended", "tree", tree_id, None);
        let last = rows.last()?;
        crate::lines::wait_cut(&last.at, &last.data, &self.deps.clock.zone())
    }

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
        // HAND-625: the owner's projects as one fact line (the store's lock released before the reads below)
        let projects = {
            let folders = ferb_policy::projects::store::folders(&self.db()).unwrap_or_default();
            ferb_policy::projects::fact_line(&self.deps.home, &folders)
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
            wait_cut: self.wait_cut(&tree.id),
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
            projects,
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
        let mut blocks =
            worker_context::worker_context(&reads, &mut Sources { sup: self, tree }, &|t| {
                self.wrap(t)
            });
        // HAND-580: a code fact (no prompt line): the profiles the owner's words named for this task, after the task
        let named = self
            .task_profiles(&tree.id)
            .map(|(_, n)| n)
            .unwrap_or_default();
        if !named.is_empty() {
            let at = blocks
                .iter()
                .position(|b| b.reference == "task specification")
                .map_or(blocks.len(), |i| i + 1);
            blocks.insert(at, protected(task_profiles_fact(&named), "task.profiles"));
        }
        // ORCH-669: a file on the Mac the conditions name for upload, as a code fact before the work (the stop before a
        // send with another file is the worker's; code tells it after when it went on)
        let named_files: Vec<String> = spec
            .success_conditions
            .iter()
            .filter(|c| c.check.kind == "uploaded")
            .filter_map(|c| c.check.path.clone())
            .filter(|p| !p.is_empty())
            .collect();
        if !named_files.is_empty() {
            let at = blocks
                .iter()
                .position(|b| b.reference == "task specification")
                .map_or(blocks.len(), |i| i + 1);
            blocks.insert(
                at,
                protected(
                    crate::endings::upload_fact(&named_files),
                    "task.named_files",
                ),
            );
        }
        Ok(blocks)
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
        st.unchecked.clear();
        st.worker_gaps.clear();
        st.reported_not_done = false;
        st.gaps.clear();
        st.conversation_action = None;
        st.loop_action = None;
        st.probe = None;
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
        let conversation = step.contains_key("conversation");
        let looping = step.contains_key("loop");
        let parts = is_step && root.as_ref().is_some_and(|r| r.lock().card.is_some());
        let flags = ferb_context::schemas::WorkerFlags {
            inputs,
            proposals,
            conversation,
            watch,
            loop_: looping,
            parts,
        };
        let mut roles = Vec::new();
        if is_step {
            roles.push("step".to_string());
        }
        for (name, on) in [
            ("inputs", inputs),
            ("proposals", proposals),
            ("converse", conversation),
            ("watch", watch),
            ("loop", looping),
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
        let mut task = tree.worker_task();
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
            // Python's `tree.worker` is the one object a correction changes: each attempt reads it as it is now
            task = tree.worker_task();
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
                            let said = tree.lock().reason.clone();
                            let mut next = next;
                            if out.status != OutcomeStatus::Done && !said.is_empty() {
                                // ORCH-562: its reason's quoted sentences, by code
                                let quoted: Vec<(String, String)> = next
                                    .findings
                                    .iter()
                                    .filter(|f| {
                                        !ferb_orchestrator::py::strip(&f.quote).is_empty()
                                            && said.contains(&f.quote)
                                    })
                                    .map(|f| (f.quote.clone(), f.kind.clone()))
                                    .collect();
                                let settled =
                                    ferb_orchestrator::answer_marks::settle(&said, &quoted).answer;
                                tree.lock().reason = settled.clone();
                                if next.reason == said {
                                    // the fail carried the worker's own reason
                                    next.reason = settled;
                                }
                            }
                            choice = Some(next);
                        } else {
                            choice = Some(next);
                        }
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
            let accepted = choice.as_ref().is_some_and(|c| {
                c.action == "accept"
                    || c.action == "accept_unconfirmed"
                    || c.action == "accept_with_gaps"
            }) && !self.stopped(tree);
            // a loop round's resources stay the loop's until the loop ends (L31): released once, by the loop step
            if accepted && !tree.lock().hold_resources {
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
                // ORCH-652: done with gaps: what was delivered stands, and each gap is told as not done
                st.gaps = choice.gaps.clone();
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
                if let Some(answer) = self.answered_in_step(tree, &choice.question) {
                    // ORCH-650: the owner already answered this very question in this step: not asked again; the
                    // next attempt has their answer (and every later check reads it in the step's context)
                    self.journal(
                        "plan.question_answered_before",
                        json!({"task_id": tree.id, "step": tree.step_id, "question": choice.question}),
                        None,
                    );
                    instructions =
                        super::owner_wait::owner_continuation(&super::owner_wait::OwnerReply {
                            kind: "answer".into(),
                            words: answer,
                            question: choice.question.clone(),
                            ..Default::default()
                        });
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
                    continue;
                }
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

    /// ORCH-650: the owner's latest answer in this step to `question` asked in the same words (as the card showed it,
    /// case and spacing aside), if the step asked it before.
    fn answered_in_step(&self, tree: &Arc<Tree>, question: &str) -> Option<String> {
        let norm = |t: &str| {
            t.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let plain = ferb_sanitize::terminal::printable(question);
        let forms = [
            norm(&plain),
            norm(&ferb_policy::confirm::task_question(&plain)),
        ];
        if forms[0].is_empty() {
            return None;
        }
        self.step_answers(&tree.id, &tree.step_id)
            .iter()
            .rev()
            .find(|qa| forms.contains(&norm(qa["question"].as_str().unwrap_or(""))))
            .and_then(|qa| qa["answer"].as_str().map(str::to_string))
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
        st.reported_not_done = w.status == "not_done";
        st.worker_gaps = w
            .gaps
            .iter()
            .map(|g| (g.condition.clone(), g.reason.clone()))
            .collect();
        st.input_notes = w.input_notes.iter().map(Py::to_value).collect();
        st.conversation_proposals = w.conversation_proposals.iter().map(Py::to_value).collect();
        st.watch_met = w.watch_met;
        st.conversation_action = w.conversation_action.clone();
        st.loop_action = w.loop_action.clone();
        st.probe = w.probe.clone();
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
        if self.not_done_without_acting(tree, out) {
            // ORCH-654: the worker reported not done and its records show it neither acted nor changed a file: the
            // checker isn't run and the step ends as reported, as a check confirming the failure would end it
            let skipped = self.span_start(
                "verify.skipped",
                Some(&tree.worker.id),
                Some(span),
                None,
                json!({"reason": NOT_DONE_UNCHECKED}),
            );
            self.span_finish(
                &skipped,
                Some(json!({"checker": "not run"})),
                None,
                None,
                None,
            );
            let mut next = Next::of("fail");
            next.reason = reason;
            return Ok(next);
        }
        if out.status == OutcomeStatus::Done {
            let skip = self.look_only(tree);
            if !skip.is_empty() {
                // owner 2026-10-25: the probe's first read is this round's check (L31)
                tree.lock().unchecked = skip.clone();
                let skipped = self.span_start(
                    "verify.skipped",
                    Some(&tree.worker.id),
                    Some(span),
                    None,
                    json!({"reason": skip}),
                );
                self.span_finish(
                    &skipped,
                    Some(json!({"checker": "not run"})),
                    None,
                    None,
                    None,
                );
                return Ok(Next::of("accept"));
            }
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
        // ORCH-668: FERB acted on something other than what a condition names: the step ends its own way (D3) instead
        // of going on or trying again by itself, unless code proved another condition false
        if out.status == OutcomeStatus::Done
            && outcomes.iter().any(|r| r.verdict == DIFFERENT)
            && !outcomes.iter().any(ConditionResult::proved_false)
            && ["accept", "accept_unconfirmed", "accept_with_gaps", "retry"]
                .contains(&action.as_str())
        {
            return self.different_ending(tree, out, &outcomes, checked);
        }
        if action == "accept_with_gaps" {
            match self.gapped(tree, out, &outcomes, checked) {
                Ok(gaps) => {
                    tree.lock().unconfirmed.clear();
                    let mut next = Next::of("accept_with_gaps");
                    next.gaps = gaps;
                    next.findings = checked
                        .findings
                        .iter()
                        .filter(|f| !ferb_orchestrator::py::strip(&f.quote).is_empty())
                        .cloned()
                        .collect();
                    return next;
                }
                Err(why) => {
                    // ORCH-652: code's limits refused the gapped accept: it reads as the checker's plain accept
                    self.journal(
                        "task.gaps_refused",
                        json!({"task_id": tree.worker.id, "why": why}),
                        None,
                    );
                    action = "accept".into();
                }
            }
        }
        if action == "accept" || action == "accept_unconfirmed" {
            // ORCH-664: only code's own facts (the Mac's state, nothing done) prove a condition false
            let refuted: Vec<&ConditionResult> =
                outcomes.iter().filter(|r| r.proved_false()).collect();
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

    /// ORCH-668 (D3's default, `endings::DIFFERENT_ENDS_DELIVERED`): a step whose work FERB did on something other
    /// than what a condition names. Each such condition is a gap whose reason is code's sentence of what was done
    /// instead; a condition the check accepted as a gap keeps its reason; any other unmet one is unconfirmed. Off: the
    /// step fails with those sentences.
    fn different_ending(
        &self,
        tree: &Arc<Tree>,
        out: &Outcome,
        outcomes: &[ConditionResult],
        checked: &CheckerOutcome,
    ) -> Next {
        let conditions = tree.spec().success_conditions;
        let mut gaps = Vec::new();
        let mut unconfirmed = Vec::new();
        for r in outcomes
            .iter()
            .filter(|r| r.met != Some(true) && r.path != DECLINED)
        {
            let condition = ferb_orchestrator::py::strip(&self.owner_text(tree, r))
                .trim_end_matches('.')
                .to_string();
            let reason = if r.verdict == DIFFERENT {
                conditions
                    .get(r.index as usize)
                    .map(|c| crate::endings::different_reason(c, r))
            } else {
                checked
                    .gaps
                    .iter()
                    .filter(|g| g.index.as_i64() == Some(r.index))
                    .map(|g| ferb_orchestrator::py::strip(&g.reason).to_string())
                    .find(|t| !t.is_empty())
            };
            // ORCH-677: a gap's line is its reason alone; a "different" reason names what FERB did, so it carries its
            // condition and its line reads as ORCH-668 wrote it
            let reason = reason.map(|reason| {
                if r.verdict == DIFFERENT {
                    format!("{condition} — {reason}")
                } else {
                    reason
                }
            });
            match reason {
                Some(reason) => gaps.push(Gap {
                    index: r.index,
                    condition,
                    reason,
                    evidence: r.evidence.clone(),
                }),
                None => unconfirmed.push(condition),
            }
        }
        if !crate::endings::DIFFERENT_ENDS_DELIVERED {
            let mut next = Next::of("fail");
            next.reason = gaps
                .iter()
                .map(|g| format!("{}.", g.reason))
                .collect::<Vec<_>>()
                .join(" ");
            return next;
        }
        tree.lock().unconfirmed = unconfirmed.join("; ");
        let mut next = Next::of("accept_with_gaps");
        next.gaps = gaps;
        if out.status == OutcomeStatus::Done {
            next.findings = checked
                .findings
                .iter()
                .filter(|f| !ferb_orchestrator::py::strip(&f.quote).is_empty())
                .cloned()
                .collect();
        }
        next
    }

    /// ORCH-652: the check's gapped accept within code's limits: the worker reported done; at least one condition is
    /// met on this check's own evidence (not carried from an earlier attempt, not one that didn't apply); every
    /// condition left unmet (but one the owner declined) is named in the check's gaps with a reason; none of them is
    /// one code's facts proved false (ORCH-664). The gaps in condition order, or why not.
    fn gapped(
        &self,
        tree: &Arc<Tree>,
        out: &Outcome,
        outcomes: &[ConditionResult],
        checked: &CheckerOutcome,
    ) -> Result<Vec<Gap>, &'static str> {
        if out.status != OutcomeStatus::Done {
            return Err("the worker didn't report done");
        }
        let fresh = outcomes.iter().any(|r| {
            r.met == Some(true)
                && !r.evidence.starts_with(DIDNT_APPLY)
                && !r.evidence.starts_with(CONFIRMED_EARLIER)
        });
        if !fresh {
            return Err("no condition is met on this check's evidence");
        }
        let unmet: Vec<&ConditionResult> = outcomes
            .iter()
            .filter(|r| r.met != Some(true) && r.path != DECLINED)
            .collect();
        if unmet.is_empty() {
            return Err("every condition is met");
        }
        if unmet.iter().any(|r| r.proved_false()) {
            return Err("code's facts proved a condition false");
        }
        let mut gaps = Vec::new();
        for r in unmet {
            let reason = checked
                .gaps
                .iter()
                .filter(|g| g.index.as_i64() == Some(r.index))
                .map(|g| ferb_orchestrator::py::strip(&g.reason).to_string())
                .find(|t| !t.is_empty());
            let Some(reason) = reason else {
                return Err("an unmet condition has no reason");
            };
            let condition = ferb_orchestrator::py::strip(&self.owner_text(tree, r))
                .trim_end_matches('.')
                .to_string();
            gaps.push(Gap {
                index: r.index,
                condition,
                reason,
                evidence: r.evidence.clone(),
            });
        }
        Ok(gaps)
    }

    /// ORCH-654: a valid not_done report, with no question for the owner, from an attempt whose records show no acting
    /// call (every recorded call of this task, earlier attempts' included, is an observation or one of FERB's
    /// context reads) and no file changed during the step.
    fn not_done_without_acting(&self, tree: &Arc<Tree>, out: &Outcome) -> bool {
        let (reported, question, since) = {
            let st = tree.lock();
            (
                st.reported_not_done,
                st.worker_question.clone(),
                st.calls_since,
            )
        };
        if !reported
            || out.status == OutcomeStatus::Done
            || !question.trim().is_empty()
            || self.deps.daemon.checks_unacted_not_done()
        {
            return false;
        }
        let task_id = tree.worker_task().id;
        let rows = {
            let db = self.db();
            ferb_orchestrator::sql::rows(
                db.conn(),
                "SELECT input_json FROM spans WHERE task_id = ? AND kind = 'tool.call' AND rowid > ?",
                &[
                    rusqlite::types::Value::Text(task_id.clone()),
                    rusqlite::types::Value::Integer(since),
                ],
            )
        };
        let Ok(rows) = rows else {
            return false;
        };
        let acted = rows.iter().any(|row| {
            let call = row
                .get("input_json")
                .and_then(Value::as_str)
                .and_then(|t| ferb_py::json::loads(t).ok())
                .unwrap_or(Value::Null);
            let tool = call
                .get("tool")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            !ferb_orchestrator::evidence::CONTEXT_READS.contains(&tool.as_str())
                && !self.deps.daemon.observes(&tool)
        });
        if acted {
            return false;
        }
        match self.deps.daemon.changes(&task_id, &tree.id) {
            None => true,
            Some(Ok(found)) => found.is_empty(),
            Some(Err(_)) => false,
        }
    }

    /// supervisor.py:7078 `verify`.
    async fn verify(
        &self,
        tree: &Arc<Tree>,
        span: &str,
    ) -> Flow<(Vec<ConditionResult>, Option<CheckerOutcome>)> {
        let task = tree.worker_task();
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
        let judged_kind = |i: i64| {
            judged
                .iter()
                .find(|(n, _)| *n as i64 == i)
                .map(|(_, c)| c.check.kind.clone())
                .unwrap_or_default()
        };
        // ORCH-664, ORCH-665: each action condition settled from code's fact and the checker's result in one order;
        // a disagreement between them is journaled (ORCH-673)
        let acted = if observations.is_empty() {
            None
        } else {
            self.step_acted(tree, &task)
        };
        let unknown = if observations.is_empty() {
            Vec::new()
        } else {
            let db = self.db();
            ferb_orchestrator::verify::unknown_outcomes(&db, &task.id).unwrap_or_default()
        };
        let mut merged: Vec<ConditionResult> = Vec::new();
        for r in results {
            let kind = judged_kind(r.index);
            if !ACTION_CHECKS.contains(&kind.as_str()) {
                merged.push(r);
                continue;
            }
            let step = ferb_orchestrator::verdicts::Step {
                kind: kind.clone(),
                applies: !idle.contains(&r.index),
                acted,
                unknown: unknown.clone(),
            };
            let code = observations.iter().find(|o| o.index == r.index);
            let settled = ferb_orchestrator::verdicts::settle(&step, code, &r);
            if let Some(Value::Object(mut d)) = settled.disagreement {
                d.insert("task_id".into(), json!(task.id));
                self.journal("task.verdict_disagreement", Value::Object(d), None);
            }
            merged.push(settled.result);
        }
        // ORCH-613: the owner's No on the action's own card beats every other source
        for r in merged.iter_mut() {
            if r.met != Some(true) && r.path != DECLINED {
                if let Some((_, c)) = judged.iter().find(|(n, _)| *n as i64 == r.index) {
                    if !ACTION_CHECKS.contains(&c.check.kind.as_str()) {
                        continue;
                    }
                    if let Some(owner) = self.deps.daemon.declined(r.index, c, &task.id) {
                        *r = owner;
                    }
                }
            }
        }
        Ok((merged, verdict))
    }

    /// ORCH-666: whether the step acted: an acting call of its worker (not an observation, not one of FERB's context
    /// reads, not refused at the boundary, not one its tool says it didn't perform) since the step (or loop round)
    /// began, or a file changed during it. None when the store can't be read.
    fn step_acted(&self, tree: &Arc<Tree>, task: &Task) -> Option<bool> {
        let since = tree.lock().calls_since;
        let rows = {
            let db = self.db();
            ferb_orchestrator::sql::rows(
                db.conn(),
                "SELECT input_json, output_json FROM spans WHERE task_id = ? AND kind = 'tool.call' AND rowid > ?",
                &[
                    rusqlite::types::Value::Text(task.id.clone()),
                    rusqlite::types::Value::Integer(since),
                ],
            )
            .ok()?
        };
        let acted = rows.iter().any(|row| {
            let load = |k: &str| {
                row.get(k)
                    .and_then(Value::as_str)
                    .and_then(|t| ferb_py::json::loads(t).ok())
                    .unwrap_or(Value::Null)
            };
            let call = load("input_json");
            let refused = ["boundary_refused", "ferb_question"]
                .iter()
                .any(|k| call.get(*k).is_some_and(ferb_py::value::truthy));
            let tool = call
                .get("tool")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            !refused
                && load("output_json").get("performed") != Some(&Value::Bool(false))
                && !ferb_orchestrator::evidence::CONTEXT_READS.contains(&tool.as_str())
                && !self.deps.daemon.observes(&tool)
        });
        if acted {
            return Some(true);
        }
        match self.deps.daemon.changes(&task.id, &tree.id) {
            None => Some(false),
            Some(Ok(found)) => Some(!found.is_empty()),
            Some(Err(_)) => None,
        }
    }

    /// `verify` for another flow (a loop's end check, `_judged`).
    pub(crate) async fn verify_tree(
        &self,
        tree: &Arc<Tree>,
        span: &str,
    ) -> Flow<(Vec<ConditionResult>, Option<CheckerOutcome>)> {
        self.verify(tree, span).await
    }

    /// `_settle_findings` for another flow (a loop's end check, `_judged`).
    pub(crate) async fn settle_findings_of(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        findings: &[Finding],
        span: &str,
    ) {
        self.settle_findings(tree, task, findings, span).await
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
                    evidence: format!("{CONFIRMED_EARLIER}{}", e.evidence),
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
                    gaps: st.worker_gaps.clone(),
                    part_updates: st.part_updates.clone(),
                    worker_types: self.deps.daemon.worker_types(),
                    availability: self.deps.daemon.availability(),
                    observations,
                    calls_since: st.calls_since,
                    began: None,
                },
                st.calls_since,
            )
        };
        // a loop round: its own calls only; earlier rounds share the task id (L31)
        let began = if calls_since != 0 {
            None
        } else {
            self.request_began(&tree.id, &task.request_id, &task.id)
        };
        let mut reads = reads;
        reads.began = began.as_ref().map(|b| b.0.clone());
        let report = worker_context::verifier_report(&reads, &|t| self.wrap(t));
        let mut context = self.context(tree, false).await?;
        // ORCH-655: what changes between checks of one step comes after what doesn't, so a later check's first call
        // can reuse the earlier one's cached prefix: the previous attempts' block goes last among the task's blocks
        worker_context::attempts_last(&mut context);
        context.push(self.code_facts(task, calls_since));
        // ORCH-650: the owner's answers to this step's questions, the check's own included
        context.extend(worker_context::step_answers_block(
            &self.step_answers(&tree.id, &tree.step_id),
            &|t| self.wrap(t),
        ));
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
            Checked::Verdict(v) => *v,
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
                    .check(tree, &vtask, judged, span, ctx2, plain2, effort.clone())
                    .await?
                {
                    Checked::Unchecked(results) => return Ok((results, None)),
                    Checked::Verdict(v) => verdict = *v,
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
        // ORCH-671: no look, no pass, for pages and apps too: a met on an outward condition stands only with a look of
        // the check's own at the tab, app or site of the step's last action; once refused with the reason, then
        // can't tell, never false
        if let Some(at) = self.last_action_at(tree, task) {
            let outward = |i: i64| {
                judged
                    .iter()
                    .find(|(n, _)| *n as i64 == i)
                    .is_some_and(|(_, c)| c.layer == "outcome" || c.layer == "external")
            };
            let needs = |v: &CheckerOutcome| {
                v.results.iter().any(|r| {
                    r.met == Some(true) && r.applies && r.index.as_i64().is_some_and(outward)
                })
            };
            if needs(&verdict) && !self.looked_at(&vtask.id, cursor, &at) {
                let refused = self.span_start(
                    "verify.refused",
                    Some(&vtask.id),
                    Some(span),
                    None,
                    json!({"no_look": at.said}),
                );
                self.span_finish(
                    &refused,
                    Some(json!({"rerun": "once, with the reason"})),
                    None,
                    None,
                    None,
                );
                let why = protected(
                    crate::endings::refused_look(&at.said),
                    "FERB refused this check's pass",
                );
                let mut ctx2 = ctx.clone();
                ctx2.push(why.clone());
                let mut plain2 = plain.clone();
                plain2.push(why);
                match self
                    .check(tree, &vtask, judged, span, ctx2, plain2, effort.clone())
                    .await?
                {
                    Checked::Unchecked(results) => return Ok((results, None)),
                    Checked::Verdict(v) => verdict = *v,
                }
                tree.lock().checker_judged = verdict
                    .results
                    .iter()
                    .filter(|r| r.met.is_some())
                    .filter_map(|r| r.index.as_i64())
                    .collect();
                if needs(&verdict) && !self.looked_at(&vtask.id, cursor, &at) {
                    let again = self.span_start(
                        "verify.refused",
                        Some(&vtask.id),
                        Some(span),
                        None,
                        json!({"no_look": at.said}),
                    );
                    self.span_finish(
                        &again,
                        Some(json!({"result": "met=null: a pass without a look is never a pass"})),
                        None,
                        None,
                        None,
                    );
                    for r in verdict.results.iter_mut() {
                        if r.met == Some(true) && r.applies && r.index.as_i64().is_some_and(outward)
                        {
                            r.met = None;
                            r.evidence =
                                format!("{} {}", crate::endings::no_look_at(&at.said), r.evidence);
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

    /// ORCH-671: where the step's last acting call on a page or an app acted (its tab, its site or app), when the rule
    /// applies; None when the step made none (a step that only changed files has the file rule).
    fn last_action_at(&self, tree: &Arc<Tree>, task: &Task) -> Option<ActedAt> {
        if !self.deps.daemon.fresh_look_rule() {
            return None;
        }
        let since = tree.lock().calls_since;
        let rows = {
            let db = self.db();
            ferb_orchestrator::sql::rows(
                db.conn(),
                "SELECT input_json, output_json FROM spans WHERE task_id = ? AND kind = 'tool.call' AND rowid > ? \
                 ORDER BY rowid",
                &[
                    rusqlite::types::Value::Text(task.id.clone()),
                    rusqlite::types::Value::Integer(since),
                ],
            )
            .ok()?
        };
        let mut last = None;
        for row in rows {
            let load = |k: &str| {
                row.get(k)
                    .and_then(Value::as_str)
                    .and_then(|t| ferb_py::json::loads(t).ok())
                    .unwrap_or(Value::Null)
            };
            let call = load("input_json");
            if ["boundary_refused", "ferb_question"]
                .iter()
                .any(|k| call.get(*k).is_some_and(ferb_py::value::truthy))
                || load("output_json").get("performed") == Some(&Value::Bool(false))
            {
                continue;
            }
            let tool = call
                .get("tool")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            if ferb_orchestrator::evidence::CONTEXT_READS.contains(&tool.as_str())
                || self.deps.daemon.observes(&tool)
            {
                continue;
            }
            let at = ActedAt::of(&call);
            if at.tab.is_none() && at.site.is_none() {
                continue;
            }
            last = Some(at);
        }
        last
    }

    /// ORCH-671: whether this check made an observation of its own (no error, not a context read) at the place since
    /// `cursor`: the same tab, else the same site or app.
    fn looked_at(&self, check_id: &str, cursor: i64, at: &ActedAt) -> bool {
        let rows = {
            let db = self.db();
            ferb_orchestrator::sql::rows(
                db.conn(),
                "SELECT input_json FROM spans WHERE task_id = ? AND kind = 'tool.call' AND rowid > ? AND error IS NULL",
                &[
                    rusqlite::types::Value::Text(check_id.into()),
                    rusqlite::types::Value::Integer(cursor),
                ],
            )
            .unwrap_or_default()
        };
        rows.iter().any(|row| {
            let call = row
                .get("input_json")
                .and_then(Value::as_str)
                .and_then(|t| ferb_py::json::loads(t).ok())
                .unwrap_or(Value::Null);
            let tool = call
                .get("tool")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            if ferb_orchestrator::evidence::CONTEXT_READS.contains(&tool.as_str()) {
                return false;
            }
            let seen = ActedAt::of(&call);
            (at.tab.is_some() && seen.tab == at.tab)
                || (at.site.is_some()
                    && seen.site.as_deref().map(site_key) == at.site.as_deref().map(site_key))
        })
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
                Ok(v) => Ok(Checked::Verdict(Box::new(v))),
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
            // supervisor.py:6510 `_kept_contradiction` (ORCH-547)
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
            // supervisor.py:6510 `_kept_contradiction` (ORCH-547)
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
        // supervisor.py:6521 `_fixing_step`
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

/// ORCH-671: where an acting call acted, as its record says: the browser tab alias, the site (or, for the Mac tools,
/// the app); `said` names it for the check and the record.
struct ActedAt {
    tab: Option<String>,
    site: Option<String>,
    said: String,
}

impl ActedAt {
    fn of(call: &Value) -> ActedAt {
        let text = |v: Option<&Value>| {
            v.filter(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
        };
        let tab = text(call.get("args").and_then(|a| a.get("tab")));
        let site = text(call.get("target").and_then(|t| t.get("site")));
        let said = match (&tab, &site) {
            (Some(t), Some(s)) => format!("tab {t} on {s}"),
            (Some(t), None) => format!("tab {t}"),
            (None, Some(s)) => s.clone(),
            (None, None) => String::new(),
        };
        ActedAt { tab, site, said }
    }
}

/// A site or app as compared for a look: lowercased, without a leading "www.".
fn site_key(s: &str) -> String {
    let l = s.trim().to_lowercase();
    l.strip_prefix("www.").map(str::to_string).unwrap_or(l)
}

/// What one run of the checker gave: its verdict, or each condition unchecked with why.
enum Checked {
    Verdict(Box<CheckerOutcome>),
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
        out.push(
            ConditionResult {
                index,
                text: text("condition", ""),
                met: r.get("met").and_then(Value::as_bool),
                path: text("path", "verifier"),
                evidence: text("evidence", ""),
                how: text("how", ""),
                ..ConditionResult::default()
            }
            .with_stored(&r),
        );
    }
    Ok(out)
}
