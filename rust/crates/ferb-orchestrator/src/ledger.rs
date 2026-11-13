//! ferb/orchestrator/ledger.py: the action ledger, intended -> executing -> done | failed | unknown (P19). The row is
//! written before a consequential action runs; a restart makes what was in flight unknown; an unknown action is
//! reconciled from a fresh read, never blindly retried; the owner may approve exactly one more attempt.
//!
//! decision: ORCH-503

use ferb_store::spans::tree_where;
use ferb_store::Db;
use serde_json::{Map, Value};

use crate::sql::{self, text, Error, Result};

pub const RETRY_APPROVED: &str = "owner approved retry after inspection";

/// ledger.py:26 `TRANSITIONS`.
pub fn allowed(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("intended", "executing" | "failed" | "unknown")
            | ("executing", "done" | "failed" | "unknown")
            | ("unknown", "done" | "failed")
    )
}

/// ferb/core/types.py `ActionIntent`, as the ledger keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub id: String,
    pub task_id: String,
    pub capability: String,
    pub target: Value,
    pub payload_hash: String,
    pub state: String,
}

/// ledger.py:46 `LedgerRow`.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerRow {
    pub intent: Intent,
    pub reconciled: bool,
    pub evidence: Vec<Value>,
    pub retry_approved: Option<String>,
    pub retried_by: Option<String>,
}

/// ledger.py:55 `Ledger`.
pub struct Ledger<'a> {
    db: &'a Db,
    now: Box<dyn Fn() -> String + Send + Sync + 'a>,
    ids: Box<dyn Fn() -> String + Send + Sync + 'a>,
}

fn duplicate(message: String, intent_id: String, state: String) -> Error {
    Error::Duplicate {
        message,
        intent_id,
        state,
    }
}

impl<'a> Ledger<'a> {
    pub fn new(
        db: &'a Db,
        now: Box<dyn Fn() -> String + Send + Sync + 'a>,
        ids: Box<dyn Fn() -> String + Send + Sync + 'a>,
    ) -> Ledger<'a> {
        Ledger { db, now, ids }
    }

    /// ledger.py:59 `intend`.
    pub fn intend(
        &self,
        task_id: &str,
        capability: &str,
        target: &Value,
        payload_hash: &str,
        reconcile_first: bool,
    ) -> Result<Intent> {
        let c = self.db.conn();
        c.execute_batch("BEGIN IMMEDIATE")?;
        let run = || -> Result<Intent> {
            let approval = if reconcile_first {
                self.check_repeat(task_id, capability, payload_hash)?
            } else {
                None
            };
            let now = (self.now)();
            let intent = Intent {
                id: (self.ids)(),
                task_id: task_id.into(),
                capability: capability.into(),
                target: target.clone(),
                payload_hash: payload_hash.into(),
                state: "intended".into(),
            };
            sql::exec(
                c,
                "INSERT INTO action_ledger (id, task_id, capability, target_json, payload_hash, state, created_at, \
                 updated_at) VALUES (?, ?, ?, ?, ?, 'intended', ?, ?)",
                &[
                    text(&intent.id),
                    text(task_id),
                    text(capability),
                    text(&sorted_dumps(target)),
                    text(payload_hash),
                    text(&now),
                    text(&now),
                ],
            )?;
            if let Some(approval) = approval {
                sql::exec(
                    c,
                    "UPDATE action_ledger SET retried_by = ? WHERE id = ?",
                    &[text(&intent.id), text(&approval)],
                )?;
            }
            Ok(intent)
        };
        match run() {
            Ok(intent) => {
                c.execute_batch("COMMIT")?;
                Ok(intent)
            }
            Err(e) => {
                let _ = c.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// ledger.py:79 `refusal`: why `intend` would refuse this action now (read only).
    pub fn refusal(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Result<Option<Error>> {
        match self.check_repeat(task_id, capability, payload_hash) {
            Err(e @ Error::Duplicate { .. }) => Ok(Some(e)),
            Err(e) => Err(e),
            Ok(_) => Ok(None),
        }
    }

    /// ledger.py:88 `approved_retry`.
    pub fn approved_retry(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Result<Option<String>> {
        match self.check_repeat(task_id, capability, payload_hash) {
            Err(Error::Duplicate { .. }) => Ok(None),
            other => other,
        }
    }

    fn check_repeat(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Result<Option<String>> {
        let tree = task_id.split('.').next().unwrap_or("");
        let (where_, params) = tree_where(tree);
        let mut args: Vec<_> = params.iter().map(|p| text(p)).collect();
        args.push(text(capability));
        args.push(text(payload_hash));
        let rows = sql::rows(
            self.db.conn(),
            &format!(
                "SELECT id, state, retry_approved, retried_by FROM action_ledger WHERE {where_} AND capability = ? AND \
                 payload_hash = ? AND state IN ('unknown', 'intended', 'executing') ORDER BY rowid"
            ),
            &args,
        )?;
        if let Some(running) = rows.iter().find(|r| sql::col(r, "state") != "unknown") {
            return Err(duplicate(
                format!(
                    "an identical {capability} is {}",
                    sql::col(running, "state")
                ),
                sql::col(running, "id"),
                sql::col(running, "state"),
            ));
        }
        let Some(latest) = rows.iter().rfind(|r| sql::col(r, "state") == "unknown") else {
            return Ok(None);
        };
        if latest.get("retry_approved").is_none_or(Value::is_null)
            || latest.get("retried_by").is_some_and(|v| !v.is_null())
        {
            return Err(duplicate(
                format!("an identical {capability} is unknown; reconcile before retrying"),
                sql::col(latest, "id"),
                "unknown".into(),
            ));
        }
        Ok(Some(sql::col(latest, "id")))
    }

    /// ledger.py:119 `attempts`.
    pub fn attempts(&self, intent_id: &str) -> Result<Vec<LedgerRow>> {
        let row = self.get(intent_id)?;
        let tree = row
            .intent
            .task_id
            .split('.')
            .next()
            .unwrap_or("")
            .to_string();
        let (where_, params) = tree_where(&tree);
        let mut args: Vec<_> = params.iter().map(|p| text(p)).collect();
        args.push(text(&row.intent.capability));
        args.push(text(&row.intent.payload_hash));
        sql::rows(
            self.db.conn(),
            &format!("SELECT * FROM action_ledger WHERE {where_} AND capability = ? AND payload_hash = ? ORDER BY rowid"),
            &args,
        )?
        .iter()
        .map(row_of)
        .collect()
    }

    /// ledger.py:128 `approve_retry`.
    pub fn approve_retry(&self, intent_id: &str, ask_span: &str) -> Result<()> {
        let row = self.get(intent_id)?;
        if row.intent.state != "unknown" {
            return Err(Error::BadTransition(
                "only an unknown action's retry is approved".into(),
            ));
        }
        if self
            .attempts(intent_id)?
            .iter()
            .any(|a| a.intent.state == "intended" || a.intent.state == "executing")
        {
            return Err(Error::BadTransition(
                "an attempt at this action is still running".into(),
            ));
        }
        let mut evidence = row.evidence.clone();
        evidence.push(Value::String(format!(
            "{RETRY_APPROVED} (owner.ask span {ask_span})"
        )));
        sql::exec(
            self.db.conn(),
            "UPDATE action_ledger SET retry_approved = ?, retried_by = NULL, updated_at = ?, evidence_json = ? WHERE id = ?",
            &[text(ask_span), text(&(self.now)()), text(&ferb_py::json::dumps(&evidence)), text(intent_id)],
        )?;
        Ok(())
    }

    /// ledger.py:143 `mark`.
    pub fn mark(&self, intent_id: &str, state: &str, evidence: &[String]) -> Result<()> {
        if (state == "done" || state == "failed") && self.get(intent_id)?.intent.state == "unknown"
        {
            return Err(Error::BadTransition(
                "an unknown action is settled only by reconcile()".into(),
            ));
        }
        self.move_to(intent_id, state, evidence, false)
    }

    /// ledger.py:148 `reconcile`.
    pub fn reconcile(&self, intent_id: &str, observed: &str, evidence: &[String]) -> Result<()> {
        if evidence.is_empty() {
            return Err(Error::Value(
                "reconciliation needs evidence (span ids / snapshot refs)".into(),
            ));
        }
        if self.get(intent_id)?.intent.state != "unknown" {
            return Err(Error::BadTransition(
                "only unknown actions are reconciled".into(),
            ));
        }
        self.move_to(intent_id, observed, evidence, true)
    }

    /// ledger.py:156 `mark_unknown_after_restart`.
    pub fn mark_unknown_after_restart(&self) -> Result<Vec<String>> {
        let ids: Vec<String> = sql::rows(
            self.db.conn(),
            "SELECT id FROM action_ledger WHERE state IN ('intended', 'executing')",
            &[],
        )?
        .iter()
        .map(|r| sql::col(r, "id"))
        .collect();
        for id in &ids {
            self.move_to(
                id,
                "unknown",
                &["daemon restarted mid-action".to_string()],
                false,
            )?;
        }
        Ok(ids)
    }

    /// ledger.py:163 `unresolved`.
    pub fn unresolved(&self, tree_id: &str) -> Result<Vec<Value>> {
        let (where_, params) = tree_where(tree_id);
        let args: Vec<_> = params.iter().map(|p| text(p)).collect();
        let rows = sql::rows(
            self.db.conn(),
            &format!("SELECT * FROM action_ledger WHERE state IN ('unknown', 'executing') AND {where_} ORDER BY created_at, rowid"),
            &args,
        )?;
        rows.iter()
            .map(|r| {
                let mut out = Map::new();
                out.insert("id".into(), r["id"].clone());
                out.insert("task_id".into(), r["task_id"].clone());
                out.insert("capability".into(), r["capability"].clone());
                out.insert("target".into(), sql::loads(&sql::col(r, "target_json"))?);
                out.insert("payload_hash".into(), r["payload_hash"].clone());
                out.insert("state".into(), r["state"].clone());
                out.insert(
                    "evidence".into(),
                    sql::loads(&sql::col(r, "evidence_json"))?,
                );
                out.insert("retry_approved".into(), r["retry_approved"].clone());
                out.insert("retried_by".into(), r["retried_by"].clone());
                Ok(Value::Object(out))
            })
            .collect()
    }

    /// ledger.py:176 `get`.
    pub fn get(&self, intent_id: &str) -> Result<LedgerRow> {
        match sql::row(
            self.db.conn(),
            "SELECT * FROM action_ledger WHERE id = ?",
            &[text(intent_id)],
        )? {
            Some(r) => row_of(&r),
            None => Err(Error::Key(intent_id.into())),
        }
    }

    fn move_to(
        &self,
        intent_id: &str,
        state: &str,
        evidence: &[String],
        reconciled: bool,
    ) -> Result<()> {
        let current = self.get(intent_id)?;
        if !allowed(&current.intent.state, state) {
            return Err(Error::BadTransition(format!(
                "{} -> {state} not allowed",
                current.intent.state
            )));
        }
        let mut all = current.evidence.clone();
        all.extend(evidence.iter().map(|e| Value::String(e.clone())));
        sql::exec(
            self.db.conn(),
            "UPDATE action_ledger SET state = ?, updated_at = ?, reconciled = ?, evidence_json = ? WHERE id = ?",
            &[
                text(state),
                text(&(self.now)()),
                rusqlite::types::Value::Integer(i64::from(reconciled || current.reconciled)),
                text(&ferb_py::json::dumps(&all)),
                text(intent_id),
            ],
        )?;
        Ok(())
    }
}

/// `json.dumps(target, sort_keys=True)`.
fn sorted_dumps(v: &Value) -> String {
    ferb_py::json::dumps_with(
        v,
        ferb_py::json::Style {
            ascii: true,
            sort_keys: true,
            indent: None,
        },
    )
}

fn row_of(r: &Map<String, Value>) -> Result<LedgerRow> {
    let opt = |k: &str| r.get(k).and_then(|v| v.as_str()).map(String::from);
    Ok(LedgerRow {
        intent: Intent {
            id: sql::col(r, "id"),
            task_id: sql::col(r, "task_id"),
            capability: sql::col(r, "capability"),
            target: sql::loads(&sql::col(r, "target_json"))?,
            payload_hash: sql::col(r, "payload_hash"),
            state: sql::col(r, "state"),
        },
        reconciled: r.get("reconciled").and_then(|v| v.as_i64()).unwrap_or(0) != 0,
        evidence: match sql::loads(&sql::col(r, "evidence_json"))? {
            Value::Array(a) => a,
            _ => Vec::new(),
        },
        retry_approved: opt("retry_approved"),
        retried_by: opt("retried_by"),
    })
}
