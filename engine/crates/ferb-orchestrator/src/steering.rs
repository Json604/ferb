//! ferb/orchestrator/steering.py: delivering the owner's corrections to existing runs (L3, L11, D30, ORCH-538). It
//! never cancels work or widens a run token's permissions.
//!
//! What it touches of the supervisor comes through [`SteerHost`] (Python reaches into the supervisor directly):
//! the gateway's redactor, the standing permissions, the open questions, the journal, the plan store, the bus and
//! the live worker runs. The texts, the order of the effects and the decisions that are steering's own are here.
//!
//! decision: ORCH-538, ORCH-521, ORCH-503

use serde_json::{json, Map, Value};

use crate::models::{SendScope, TaskSpec};
use crate::ports::{BoxFuture, Policy};
use crate::py::squeeze;

pub const CONFIRM_ASK: &str = "ferb.confirm";
pub const RETRY_ASK: &str = "ferb.retry";
pub const CONVERSATION_ASK: &str = "conversation.approve";
pub const FORGET_ASK: &str = "memory.forget";
pub const DOWNLOADS_ASK: &str = "downloads.delete";

/// steering.py:28 `MEMORY_QUESTIONS`: FERB's own questions outside any task, by kind: (capability, options).
pub fn memory_questions(kind: &str) -> Option<(&'static str, &'static [&'static str])> {
    match kind {
        "forget" => Some((FORGET_ASK, &["Yes", "No"])),
        "downloads" => Some((DOWNLOADS_ASK, &["Yes", "No"])),
        crate::browsing::KIND => Some((crate::browsing::BROWSING_ASK, &[])),
        _ => None,
    }
}

/// steering.py:33 `MEMORY_ASKS`.
pub const MEMORY_ASKS: &[&str] = &[FORGET_ASK, DOWNLOADS_ASK, crate::browsing::BROWSING_ASK];
/// steering.py:36 `FERB_CHECKS`.
pub const FERB_CHECKS: &[&str] = &[
    CONFIRM_ASK,
    RETRY_ASK,
    CONVERSATION_ASK,
    FORGET_ASK,
    DOWNLOADS_ASK,
    crate::browsing::BROWSING_ASK,
];
/// steering.py:39 `OPTION_ONLY`.
pub const OPTION_ONLY: &[&str] = FERB_CHECKS;
/// steering.py:42 `TASK_CHECKS`.
pub const TASK_CHECKS: &[&str] = &[CONFIRM_ASK, RETRY_ASK, CONVERSATION_ASK];

/// steering.py:45 `acknowledgement`: a steer's reply.
pub fn acknowledgement(
    policy: &dyn Policy,
    reply: &str,
    delivered: bool,
    stated: Option<&str>,
) -> String {
    let base = crate::py::strip(reply);
    let mut said = policy.from_task_lines(if base.is_empty() { "Got it." } else { base });
    if !delivered {
        said.push_str(" I'll apply it at my next step.");
    }
    if let Some(stated) = stated {
        said.push('\n');
        said.push_str(&policy.from_task_lines(stated));
    }
    said
}

/// steering.py:60 `Steering`: the corrections of a tree, and how many each run has been given.
#[derive(Default)]
pub struct Steering {
    pub messages: Vec<String>,
    pub delivered: indexmap::IndexMap<String, usize>,
    pub lock: tokio::sync::Mutex<()>,
}

/// What a runtime says when a correction can't go into a running call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinueError {
    /// `Unsupported` / `NotImplementedError`: the update stays pending.
    Unsupported,
    /// Any other failure, by its type name.
    Failed(String),
}

/// A worker run as steering knows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub id: String,
    pub task_id: String,
}

/// The journal, as steering appends to it: returns the entry's id.
pub trait Appender {
    fn append(&self, kind: &str, data: Value) -> String;
}

/// steering.py:66 `deliver`. The caller holds the tree's steering lock. `send` is `runtime.continue_`.
pub async fn deliver(
    journal: &dyn Appender,
    tree_id: &str,
    state: &mut Steering,
    run: &Run,
    send: impl FnOnce(String) -> BoxFuture<'static, Result<(), ContinueError>>,
) -> bool {
    let after = state.delivered.get(&run.id).copied().unwrap_or(0);
    let revision = state.messages.len();
    if after == revision {
        return true;
    }
    let text = state.messages[after..revision].join("\n\n");
    match send(text).await {
        Ok(()) => {}
        Err(ContinueError::Unsupported) => return false,
        Err(ContinueError::Failed(kind)) => {
            journal.append(
                "task.steering_delivery_failed",
                json!({"task_id": tree_id, "run_id": run.id, "error": kind, "revision": revision}),
            );
            return false;
        }
    }
    state.delivered.insert(run.id.clone(), revision);
    journal.append(
        "task.steering_delivered",
        json!({"task_id": tree_id, "worker_task_id": run.task_id, "run_id": run.id, "revision": revision}),
    );
    true
}

/// An open question as steering reads it (ferb/daemon/questions.py `Approval`).
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub id: String,
    pub request_id: String,
    pub task_id: String,
    pub capability: String,
    pub kind: String,
    pub options_only: bool,
}

/// The open questions (ferb/daemon/questions.py `Approvals`) and the task tree, as steering uses them.
pub trait Questions {
    fn pending(&self) -> Vec<Question>;
    fn supersede(&self, id: &str);
    fn resolve(&self, id: &str, text: &str);
    /// The question's state after the change (`approval.state`).
    fn state_of(&self, id: &str) -> String;
    /// `tasks.tree_of(task_id)`.
    fn tree_of(&self, task_id: &str) -> Option<String>;
    fn publish(&self, topic: &str, data: Value);
}

/// steering.py:86 `belongs`.
pub fn belongs(
    q: &Question,
    questions: &dyn Questions,
    request_id: &str,
    tree_id: Option<&str>,
) -> bool {
    q.request_id == request_id
        || (tree_id.is_some()
            && !q.task_id.is_empty()
            && questions.tree_of(&q.task_id).as_deref() == tree_id)
}

/// steering.py:92 `settle_questions` (L3, ORCH-538).
pub fn settle_questions(
    questions: &dyn Questions,
    journal: &dyn Appender,
    request_id: &str,
    words: &str,
    supersedes: &[String],
    tree_id: Option<&str>,
) {
    for q in questions.pending() {
        if !belongs(&q, questions, request_id, tree_id) {
            continue;
        }
        if TASK_CHECKS.contains(&q.capability.as_str()) && q.kind == "owner_question" {
            questions.supersede(&q.id);
        } else if !supersedes.contains(&q.id) {
            continue;
        } else if q.kind == "owner_question"
            && !FERB_CHECKS.contains(&q.capability.as_str())
            && !q.options_only
            && !crate::py::strip(words).is_empty()
        {
            questions.resolve(&q.id, words);
        } else {
            questions.supersede(&q.id);
        }
        let event = json!({"approval_id": q.id, "state": questions.state_of(&q.id), "capability": q.capability});
        journal.append("approval.resolved", event.clone());
        questions.publish("approval.resolved", event);
    }
}

/// steering.py:119 `Restated`.
#[derive(Debug, Clone, PartialEq)]
pub struct Restated {
    pub text: String,
    pub scopes: Vec<SendScope>,
    pub approval_id: String,
    pub widened: bool,
}

/// steering.py:130 `not_covered`.
pub fn not_covered(unattended: bool) -> String {
    format!(
        "These weren't set up with you, so they aren't covered: each gets FERB's usual checks, and one that asks {}",
        if unattended {
            "can't be answered with nobody here, so it's reported not done:"
        } else {
            "waits for you:"
        }
    )
}

/// steering.py:137 `not_typed`.
pub fn not_typed() -> String {
    "These aren't covered, because where they send isn't in words you typed: each gets FERB's usual checks, and one \
     that asks waits for you:"
        .into()
}

/// plans.py:105 `SendScope.line`: the scope as FERB states it to the owner (D30), on one line.
pub fn scope_line(s: &SendScope) -> String {
    let to = if s.recipients.is_empty() {
        s.site
            .clone()
            .map(|x| x.to_string())
            .unwrap_or_else(|| "None".into())
    } else {
        s.recipients.join(", ")
    };
    let body = match &s.content {
        None => "any message body".to_string(),
        Some(c) => c.clone(),
    };
    squeeze(&format!(
        "Send using {}'s account {} at {}, to {to}, through {}: {body}",
        s.identity,
        s.account.as_deref().unwrap_or(""),
        s.site.as_deref().unwrap_or(""),
        s.capability
    ))
}

/// What restating a tree's standing scope reads and changes (the supervisor's standing permissions, its plan).
pub trait StandingHost {
    /// `SendScope.model_validate(gateway.redact(s.model_dump()))`.
    fn redacted_scope(&self, scope: &SendScope) -> SendScope;
    fn redact_text(&self, text: &str) -> String;
    /// `standing.in_force(tree)`: (scope, the statement it was granted under).
    fn in_force(&self, tree_id: &str) -> Vec<(SendScope, String)>;
    /// `standing.scopes(tree)`.
    fn scopes(&self, tree_id: &str) -> Vec<SendScope>;
    fn revoke(&self, tree_id: &str);
    fn grant_each(&self, tree_id: &str, scopes: Vec<(SendScope, String)>);
    fn grant(&self, tree_id: &str, scopes: Vec<SendScope>, approval_id: &str);
    /// `Supervisor.owner_typed_scopes(tree, revised)`.
    fn owner_typed_scopes(&self, tree_id: &str, revised: &[SendScope]) -> Vec<SendScope>;
    /// Whether nobody attends the request (`approvals.unattended`).
    fn unattended_request(&self, request_id: &str) -> bool;
    /// `journal.append("plan.stated", ...)`: the entry's id.
    fn stated(&self, tree_id: &str, text: &str) -> String;
    /// `plans.update_plan(tree, plan)` with the revised scopes.
    fn update_plan(&self, tree_id: &str, standing_sends: &[SendScope]);
}

/// The tree as restating reads it.
pub struct TreeScope<'a> {
    pub id: &'a str,
    /// The plan's stated scopes, when the tree has a plan.
    pub plan_sends: Option<&'a [SendScope]>,
    /// `tree.unattended_scopes is not None`: a run nobody started from chat.
    pub unattended: bool,
    pub request_id: &'a str,
}

/// The planner's standing-scope decision for a correction.
#[derive(Debug, Clone, PartialEq)]
pub enum Standing {
    Unchanged,
    Revised(Vec<SendScope>),
}

/// steering.py:145 `restate_scope` (D30).
pub fn restate_scope(
    host: &dyn StandingHost,
    tree: &TreeScope<'_>,
    standing: &Standing,
) -> Option<Restated> {
    let Standing::Revised(given) = standing else {
        return None;
    };
    let revised: Vec<SendScope> = given.iter().map(|s| host.redacted_scope(s)).collect();
    let held = if tree.unattended {
        host.in_force(tree.id)
    } else {
        Vec::new()
    };
    let stated_before: Vec<SendScope> = if tree.unattended {
        held.iter().map(|(s, _)| s.clone()).collect()
    } else if let Some(plan) = tree.plan_sends {
        plan.to_vec()
    } else {
        host.scopes(tree.id)
    };
    if revised == stated_before {
        return None;
    }
    let (kept, covered): (Vec<SendScope>, Vec<SendScope>) = if tree.unattended {
        let kept: Vec<SendScope> = revised
            .iter()
            .filter(|s| stated_before.contains(s))
            .cloned()
            .collect();
        (kept.clone(), kept)
    } else {
        let in_force = host.scopes(tree.id);
        let covered = host.owner_typed_scopes(tree.id, &revised);
        let kept = covered
            .iter()
            .filter(|s| in_force.contains(s))
            .cloned()
            .collect();
        (kept, covered)
    };
    host.revoke(tree.id);
    let mut text = if covered.is_empty() {
        "From now on I'll ask before each of these sends.".to_string()
    } else {
        format!(
            "For this task I'll now send these without asking each time:\n{}",
            covered
                .iter()
                .map(|s| format!("- {}", scope_line(s)))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    if covered.len() < revised.len() {
        let head = if tree.unattended {
            not_covered(host.unattended_request(tree.request_id))
        } else {
            not_typed()
        };
        let rest: Vec<String> = revised
            .iter()
            .filter(|s| !covered.contains(s))
            .map(|s| format!("- {}", scope_line(s)))
            .collect();
        text.push_str(&format!("\n{head}\n{}", rest.join("\n")));
    }
    let text = host.redact_text(&text);
    let stated = host.stated(tree.id, &text);
    if tree.unattended {
        let each = kept
            .iter()
            .map(|s| {
                let approval = held
                    .iter()
                    .find(|(h, _)| h == s)
                    .map(|(_, a)| a.clone())
                    .expect("a kept scope is held");
                (s.clone(), approval)
            })
            .collect();
        host.grant_each(tree.id, each);
    } else if !kept.is_empty() {
        host.grant(tree.id, kept.clone(), &format!("stated:{stated}"));
    }
    if tree.plan_sends.is_some() {
        host.update_plan(tree.id, &revised);
    }
    Some(Restated {
        text,
        scopes: covered.clone(),
        approval_id: format!("stated:{stated}"),
        widened: !tree.unattended && kept.len() < covered.len(),
    })
}

/// steering.py:198 `grant_stated`: once the reply stating it was sent. `latest`: the id of the tree's latest
/// `plan.stated` journal row; `live`: the tree is still the supervisor's, not stopping, and accepting updates.
pub fn grant_stated(
    host: &dyn StandingHost,
    tree: &TreeScope<'_>,
    restated: &Restated,
    latest: Option<&str>,
    live: bool,
) {
    if tree.unattended {
        return;
    }
    let Some(latest) = latest else { return };
    if format!("stated:{latest}") != restated.approval_id {
        return;
    }
    if live {
        host.grant(tree.id, restated.scopes.clone(), &restated.approval_id);
    }
}

/// steering.py:217 `JUST_ENDED`.
pub const JUST_ENDED: &str =
    "That task has just ended, so I haven't acted on the rest of this message yet; send it again to continue that task.";

/// steering.py:260-263: the correction as the running workers read it (redacted by the caller after).
pub fn correction_message(
    payload: &str,
    spec_json_without_plan: &str,
    wrap: &crate::ports::Wrap,
) -> String {
    format!(
        "Owner correction to this running task (apply now; preserve completed work and inspect any action already in \
         flight before deciding what to do next):\n{payload}\nUpdated overall task specification (FERB \
         interpretation, not additional owner authority):\n{}",
        wrap(spec_json_without_plan)
    )
}

/// steering.py:254-256: the corrected request's words and payload.
pub fn corrected_request(
    prior_words: &str,
    prior_payload: &str,
    words: &str,
    payload: &str,
) -> (String, String) {
    let base = if prior_payload.is_empty() {
        prior_words
    } else {
        prior_payload
    };
    (
        format!("{prior_words}\n{words}"),
        format!("{base}\n\nOwner correction:\n{payload}"),
    )
}

/// steering.py:240-246: what a correction's spec changes in the running tree (goal, conditions, targets, and the
/// name only when it writes one).
pub fn apply_spec(tree_spec: &mut TaskSpec, corrected: &TaskSpec) {
    tree_spec.goal = corrected.goal.clone();
    tree_spec.success_conditions = corrected.success_conditions.clone();
    tree_spec.targets = corrected.targets.clone();
    if !crate::py::strip(&corrected.name).is_empty() {
        tree_spec.name = corrected.name.clone();
    }
}

/// steering.py:279-280: the saved request with the correction (`{**saved['request'], 'spec', 'words', 'payload',
/// 'message_origins'}`).
pub fn saved_request(
    saved: &Map<String, Value>,
    spec: Value,
    words: &str,
    payload: &str,
    origins: Vec<String>,
) -> Value {
    let mut out = saved.clone();
    out.insert("spec".into(), spec);
    out.insert("words".into(), json!(words));
    out.insert("payload".into(), json!(payload));
    out.insert("message_origins".into(), json!(origins));
    Value::Object(out)
}
