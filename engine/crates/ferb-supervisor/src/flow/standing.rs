//! A big job's plan statement and its standing scope (D24/D30, L3, ORCH-503, ORCH-306): supervisor.py:5069
//! `_safe_scopes`, :5072 `owner_typed_scopes`, :5085 `_allowed_scopes`, :5092 `_plan_statement`, the statement and
//! grant at the plan's start (`_execute_plan`), and a correction's restated scope (steering.py `restate_scope`,
//! `grant_stated`, on ferb-orchestrator's [`restate_scope`] through [`StandingHost`]).
//!
//! The scopes are kept in the standing permissions table (`standing_permissions`) on the supervisor's own connection,
//! as Python's supervisor and gateway share one `StandingPermissions` on one database. ferb-policy's store is Mac-side
//! (`#[cfg(target_os = "macos")]`); the few statements the supervisor needs are policy/standing.py's own, here.
//!
//! decision: ORCH-503, ORCH-306

use std::sync::Arc;

use ferb_orchestrator::models::{Plan, SendScope};
use ferb_orchestrator::steering::{
    grant_stated, not_covered, not_typed, restate_scope, scope_line, Restated, Standing,
    StandingHost, TreeScope,
};
use ferb_policy::standing::SendScope as Stored;
use rusqlite::params;
use serde_json::{json, Value};

use super::{Inner, Tree};
use crate::words::PLAN_STATED;

/// The orchestrator's scope as ferb-policy's store keeps it (the same fields: plans.py `SendScope`).
pub(crate) fn to_policy(s: &SendScope) -> ferb_policy::standing::SendScope {
    ferb_policy::standing::SendScope {
        capability: s.capability.clone(),
        identity: s.identity.clone(),
        account: s.account.clone(),
        site: s.site.clone(),
        recipients: s.recipients.clone(),
        content: s.content.clone(),
    }
}

pub(crate) fn from_policy(s: &ferb_policy::standing::SendScope) -> SendScope {
    SendScope {
        capability: s.capability.clone(),
        identity: s.identity.clone(),
        account: s.account.clone(),
        site: s.site.clone(),
        recipients: s.recipients.clone(),
        content: s.content.clone(),
    }
}

/// A scope from its `model_dump()` (a tree's recorded `unattended_scopes`).
fn from_value(v: &Value) -> Option<SendScope> {
    ferb_orchestrator::validate::send_scope_validate(&ferb_context::Py::from_value(v)).ok()
}

/// `" ".join(printable(text).split())`.
fn one(text: &str) -> String {
    ferb_orchestrator::py::squeeze(&ferb_sanitize::terminal::printable(text))
}

impl Inner {
    fn standing<T>(&self, f: impl FnOnce(&Permissions<'_>) -> T) -> T {
        let db = self.db();
        f(&Permissions { db: &db })
    }

    /// supervisor.py:5069 `_safe_scopes`: the plan's stated scopes, redacted.
    pub(crate) fn safe_scopes(&self, plan: &Plan) -> Vec<SendScope> {
        plan.standing_sends
            .iter()
            .map(|s| self.redacted_scope(s))
            .collect()
    }

    /// `SendScope.model_validate(gateway.redact(s.model_dump()))`.
    fn redacted_scope(&self, s: &SendScope) -> SendScope {
        let dumped = ferb_orchestrator::validate::dump_send_scope(s).to_value();
        let redacted = self.redact(&dumped);
        if redacted == dumped {
            return s.clone();
        }
        from_value(&redacted).unwrap_or_else(|| s.clone())
    }

    /// supervisor.py:5072 `owner_typed_scopes` (final review I1, D30, S3, MP2): of the scopes stated for a run the
    /// owner started from chat, those whose every destination is in what the owner typed: this request's words, their
    /// answers in the task, the words they typed behind a remembered item.
    pub(crate) fn owner_typed_scopes(&self, tree: &Tree, scopes: &[SendScope]) -> Vec<SendScope> {
        let request_id = &tree.worker.request_id;
        let mut text = vec![self.deps.daemon.request(request_id).words];
        text.extend(
            self.with_spans(|s| s.owner_answers(&tree.id))
                .unwrap_or_default(),
        );
        text.extend(self.deps.daemon.typed_behind_memory(request_id));
        let owner_text = text.join("\n");
        scopes
            .iter()
            .filter(|s| ferb_policy::standing::owner_named(&to_policy(s), &owner_text))
            .cloned()
            .collect()
    }

    /// supervisor.py:5085 `_allowed_scopes`: with nobody present, those an owner turn recorded; else those whose
    /// destinations the owner typed.
    pub(crate) fn allowed_scopes(&self, tree: &Tree, plan: &Plan) -> Vec<SendScope> {
        let recorded = tree.lock().unattended_scopes.clone();
        match recorded {
            Some(scopes) => scopes.iter().filter_map(from_value).collect(),
            None => self.owner_typed_scopes(tree, &self.safe_scopes(plan)),
        }
    }

    /// supervisor.py:5092 `_plan_statement` (D24/D30, L3): a big job's plan, its estimates and any standing scope as
    /// the plan line states them; a statement, never a question.
    pub(crate) fn plan_statement(
        &self,
        plan: &Plan,
        allowed: Option<&[SendScope]>,
        unattended: bool,
        attended: bool,
    ) -> String {
        let mut lines = vec![PLAN_STATED.to_string()];
        for s in &plan.steps {
            let what = if !s.goal.is_empty() {
                &s.goal
            } else if !s.question.is_empty() {
                &s.question
            } else {
                &s.kind
            };
            lines.push(format!("- {}", one(what)));
        }
        if plan.big_job {
            lines.push(format!(
                "Estimated time: {}. Estimated cost: {}.",
                one(&plan.time_estimate),
                one(&plan.cost_estimate)
            ));
        }
        let scopes = self.safe_scopes(plan);
        let granted: Vec<&SendScope> = scopes
            .iter()
            .filter(|s| allowed.is_none_or(|a| a.contains(s)))
            .collect();
        if !granted.is_empty() {
            lines.push("For this task I'll send these without asking each time:".into());
            lines.extend(granted.iter().map(|s| format!("- {}", one(&scope_line(s)))));
        }
        if granted.len() < scopes.len() {
            lines.push(if attended {
                not_typed()
            } else {
                not_covered(unattended)
            });
            lines.extend(
                scopes
                    .iter()
                    .filter(|s| !granted.contains(s))
                    .map(|s| format!("- {}", one(&scope_line(s)))),
            );
        }
        lines.join("\n")
    }

    /// run_tree's plan line (supervisor.py:4576): the statement of a big job or a stated scope, redacted.
    pub(crate) fn stated_line(&self, tree: &Tree, plan: &Plan) -> String {
        let unattended = self.deps.daemon.unattended(&tree.worker.request_id);
        let attended = tree.lock().unattended_scopes.is_none();
        let allowed = self.allowed_scopes(tree, plan);
        format!(
            "\n{}",
            self.redact_text(&self.plan_statement(plan, Some(&allowed), unattended, attended))
        )
    }

    /// `_execute_plan`'s start (supervisor.py:5121): the plan line stated the plan and its standing scope; work
    /// starts at once and the owner corrects by replying. The statement is journaled (`plan.stated`), the plan marked
    /// approved, and with the owner present the scopes whose destinations they typed are granted under it.
    pub(crate) fn state_plan(&self, tree: &Tree, plan: &Plan) -> super::Flow<()> {
        let safe = self.safe_scopes(plan);
        let attended = tree.lock().unattended_scopes.is_none();
        let allowed = self.allowed_scopes(tree, plan);
        let unattended = self.deps.daemon.unattended(&tree.worker.request_id);
        let text =
            self.redact_text(&self.plan_statement(plan, Some(&allowed), unattended, attended));
        let not_granted: Vec<String> = safe
            .iter()
            .filter(|s| !allowed.contains(s))
            .map(scope_line)
            .collect();
        let stated = self.journal(
            "plan.stated",
            json!({"task_id": tree.id, "text": text, "not_granted": not_granted}),
            None,
        );
        let failed = |e: rusqlite::Error| super::Failure::raised("OperationalError", e.to_string());
        self.db()
            .conn()
            .execute(
                "UPDATE task_plans SET approved=1 WHERE task_id=?",
                rusqlite::params![tree.id],
            )
            .map_err(failed)?;
        if attended && !allowed.is_empty() {
            let scopes: Vec<_> = allowed.iter().map(to_policy).collect();
            let approval = format!("stated:{}", stated.id);
            self.standing(|s| s.grant(&tree.id, &scopes, &approval))
                .map_err(failed)?;
        }
        Ok(())
    }

    /// steering.py:145 `restate_scope`: the planner's decision about the tree's stated standing scope.
    pub(crate) fn restate(&self, tree: &Arc<Tree>, revised: &[SendScope]) -> Option<Restated> {
        let spec = tree.spec();
        let unattended = tree.lock().unattended_scopes.is_some();
        let sends = spec.plan.as_ref().map(|p| p.standing_sends.clone());
        let scope = TreeScope {
            id: &tree.id,
            plan_sends: sends.as_deref(),
            unattended,
            request_id: &tree.worker.request_id,
        };
        let host = Host { inner: self, tree };
        let restated = restate_scope(&host, &scope, &Standing::Revised(revised.to_vec()));
        if restated.is_some() {
            // `plan.standing_sends = revised` on the tree's own plan, as Python's restate changes it in place
            if let Some(plan) = tree.lock().spec.as_mut().and_then(|s| s.plan.as_mut()) {
                plan.standing_sends = revised.iter().map(|s| self.redacted_scope(s)).collect();
            }
        }
        restated
    }

    /// steering.py:198 `grant_stated`: once the reply stating it was handed to the chat, the revised scope, new or
    /// widened parts included, is granted under its statement while the task still runs and that statement is still
    /// its latest.
    pub(crate) fn grant_restated(&self, tree: &Arc<Tree>, restated: &Restated) {
        let unattended = tree.lock().unattended_scopes.is_some();
        let latest: Option<String> = self
            .db()
            .conn()
            .query_row(
                "SELECT id FROM journal WHERE kind='plan.stated' AND json_extract(data_json, '$.task_id')=? \
                 ORDER BY seq DESC LIMIT 1",
                rusqlite::params![tree.id],
                |r| r.get(0),
            )
            .ok();
        let live = self
            .live()
            .tree_of
            .get(&tree.id)
            .is_some_and(|t| Arc::ptr_eq(t, tree))
            && tree.lock().stop.is_empty()
            && tree.lock().accepting_updates;
        let scope = TreeScope {
            id: &tree.id,
            plan_sends: None,
            unattended,
            request_id: &tree.worker.request_id,
        };
        let host = Host { inner: self, tree };
        grant_stated(&host, &scope, restated, latest.as_deref(), live);
    }
}

/// What restating reads and changes, on the supervisor.
struct Host<'a> {
    inner: &'a Inner,
    tree: &'a Arc<Tree>,
}

impl StandingHost for Host<'_> {
    fn redacted_scope(&self, scope: &SendScope) -> SendScope {
        self.inner.redacted_scope(scope)
    }
    fn redact_text(&self, text: &str) -> String {
        self.inner.redact_text(text)
    }
    fn in_force(&self, tree_id: &str) -> Vec<(SendScope, String)> {
        self.inner
            .standing(|s| s.in_force(tree_id))
            .unwrap_or_default()
            .iter()
            .map(|(s, a)| (from_policy(s), a.clone()))
            .collect()
    }
    fn scopes(&self, tree_id: &str) -> Vec<SendScope> {
        self.inner
            .standing(|s| s.scopes(tree_id))
            .unwrap_or_default()
            .iter()
            .map(from_policy)
            .collect()
    }
    fn revoke(&self, tree_id: &str) {
        let _ = self.inner.standing(|s| s.revoke(tree_id));
    }
    fn grant_each(&self, tree_id: &str, scopes: Vec<(SendScope, String)>) {
        let each: Vec<_> = scopes
            .iter()
            .map(|(s, a)| (to_policy(s), a.clone()))
            .collect();
        let _ = self.inner.standing(|s| s.grant_each(tree_id, &each));
    }
    fn grant(&self, tree_id: &str, scopes: Vec<SendScope>, approval_id: &str) {
        let scopes: Vec<_> = scopes.iter().map(to_policy).collect();
        let _ = self
            .inner
            .standing(|s| s.grant(tree_id, &scopes, approval_id));
    }
    fn owner_typed_scopes(&self, _tree_id: &str, revised: &[SendScope]) -> Vec<SendScope> {
        self.inner.owner_typed_scopes(self.tree, revised)
    }
    fn unattended_request(&self, request_id: &str) -> bool {
        self.inner.deps.daemon.unattended(request_id)
    }
    fn stated(&self, tree_id: &str, text: &str) -> String {
        self.inner
            .journal(
                "plan.stated",
                json!({"task_id": tree_id, "text": text}),
                None,
            )
            .id
    }
    fn update_plan(&self, tree_id: &str, standing_sends: &[SendScope]) {
        let Some(mut plan) = self.tree.spec().plan else {
            return;
        };
        plan.standing_sends = standing_sends.to_vec();
        let _ = self.inner.plans(|p| p.update_plan(tree_id, &plan));
    }
}

/// policy/standing.py `StandingPermissions`: the statements the supervisor uses (ferb-policy's `stores` is Mac-side).
struct Permissions<'a> {
    db: &'a ferb_store::Db,
}

impl Permissions<'_> {
    fn grant(&self, task_id: &str, scopes: &[Stored], approval_id: &str) -> rusqlite::Result<()> {
        let each: Vec<(Stored, String)> = scopes
            .iter()
            .map(|s| (s.clone(), approval_id.to_string()))
            .collect();
        self.grant_each(task_id, &each)
    }

    fn grant_each(&self, task_id: &str, granted: &[(Stored, String)]) -> rusqlite::Result<()> {
        for (index, (scope, approval_id)) in granted.iter().enumerate() {
            self.db.conn().execute(
                "INSERT OR REPLACE INTO standing_permissions VALUES(?,?,?,?,1)",
                params![
                    format!("{task_id}:{index}"),
                    task_id,
                    scope.dump_json(),
                    approval_id
                ],
            )?;
        }
        Ok(())
    }

    fn scopes(&self, task_id: &str) -> rusqlite::Result<Vec<Stored>> {
        Ok(self
            .in_force(task_id)?
            .into_iter()
            .map(|(s, _)| s)
            .collect())
    }

    fn in_force(&self, task_id: &str) -> rusqlite::Result<Vec<(Stored, String)>> {
        let mut stmt = self.db.conn().prepare(
            "SELECT scope, approval_id FROM standing_permissions WHERE task_id=? AND active=1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![task_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (scope, approval) = row?;
            if let Ok(s) = serde_json::from_str::<Stored>(&scope) {
                out.push((s, approval));
            }
        }
        Ok(out)
    }

    fn revoke(&self, task_id: &str) -> rusqlite::Result<()> {
        self.db.conn().execute(
            "UPDATE standing_permissions SET active=0 WHERE task_id=?",
            params![task_id],
        )?;
        Ok(())
    }
}
