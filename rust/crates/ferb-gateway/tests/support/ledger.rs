//! A test double of the action ledger (`ferb/orchestrator/ledger.py`), on the same store, so the replay compares the
//! `action_ledger` rows Python wrote. The real port belongs to the orchestration part (rust/crates/ferb-orchestrator);
//! this follows ledger.py line for line only as far as the gateway's seam needs.

use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{Clock, IdSource};
use ferb_gateway::seams::{BadTransition, DuplicateUnresolved, Ledger};
use ferb_gateway::words::LedgerRow;
use ferb_store::spans::tree_where;
use ferb_store::Db;
use rusqlite::params;
use serde_json::{Map, Value};

pub struct SqlLedger {
    pub db: Arc<Mutex<Db>>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
}

fn sorted(v: &Value) -> String {
    ferb_py::json::dumps_with(
        v,
        ferb_py::json::Style {
            ascii: true,
            sort_keys: true,
            indent: None,
        },
    )
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<LedgerRow> {
    let target: String = r.get("target_json")?;
    let evidence: String = r.get("evidence_json")?;
    Ok(LedgerRow {
        id: r.get("id")?,
        task_id: r.get("task_id")?,
        capability: r.get("capability")?,
        target: serde_json::from_str::<Map<String, Value>>(&target).unwrap_or_default(),
        payload_hash: r.get("payload_hash")?,
        state: r.get("state")?,
        reconciled: r.get::<_, i64>("reconciled")? != 0,
        evidence: serde_json::from_str(&evidence).unwrap_or_default(),
        retry_approved: r.get("retry_approved")?,
        retried_by: r.get("retried_by")?,
    })
}

impl SqlLedger {
    fn now(&self) -> String {
        ferb_core::when::isoformat(&self.clock.now())
    }

    fn check_repeat(
        &self,
        db: &Db,
        task_id: &str,
        capability: &str,
        hash: &str,
    ) -> Result<Option<String>, DuplicateUnresolved> {
        let tree = task_id.split('.').next().unwrap_or(task_id);
        let (w, p) = tree_where(tree);
        let mut st = db
            .conn()
            .prepare(&format!(
                "SELECT id, state, retry_approved, retried_by FROM action_ledger WHERE {w} AND capability = ? AND \
                 payload_hash = ? AND state IN ('unknown', 'intended', 'executing') ORDER BY rowid"
            ))
            .unwrap();
        let rows: Vec<(String, String, Option<String>, Option<String>)> = st
            .query_map(params![p[0], p[1], p[2], capability, hash], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        if let Some(r) = rows.iter().find(|r| r.1 != "unknown") {
            return Err(DuplicateUnresolved {
                message: format!("an identical {capability} is {}", r.1),
                intent_id: r.0.clone(),
                state: r.1.clone(),
            });
        }
        let Some(latest) = rows.iter().rfind(|r| r.1 == "unknown") else {
            return Ok(None);
        };
        if latest.2.is_none() || latest.3.is_some() {
            return Err(DuplicateUnresolved {
                message: format!("an identical {capability} is unknown; reconcile before retrying"),
                intent_id: latest.0.clone(),
                state: "unknown".into(),
            });
        }
        Ok(Some(latest.0.clone()))
    }

    fn get_in(&self, db: &Db, id: &str) -> Option<LedgerRow> {
        db.conn()
            .query_row("SELECT * FROM action_ledger WHERE id = ?", [id], row)
            .ok()
    }

    fn mv(
        &self,
        db: &Db,
        id: &str,
        state: &str,
        evidence: &[String],
        reconciled: bool,
    ) -> Result<(), BadTransition> {
        let current = self
            .get_in(db, id)
            .ok_or_else(|| BadTransition(id.into()))?;
        let allowed: &[&str] = match current.state.as_str() {
            "intended" => &["executing", "failed", "unknown"],
            "executing" => &["done", "failed", "unknown"],
            "unknown" => &["done", "failed"],
            _ => &[],
        };
        if !allowed.contains(&state) {
            return Err(BadTransition(format!(
                "{} -> {state} not allowed",
                current.state
            )));
        }
        let mut all = current.evidence.clone();
        all.extend(evidence.iter().cloned());
        db.conn()
            .execute(
                "UPDATE action_ledger SET state = ?, updated_at = ?, reconciled = ?, evidence_json = ? WHERE id = ?",
                params![state, self.now(), i64::from(reconciled || current.reconciled), ferb_py::json::dumps(&all), id],
            )
            .unwrap();
        Ok(())
    }

    pub fn mark_unknown_after_restart(&self) -> Vec<String> {
        let db = self.db.lock().unwrap();
        let mut st = db
            .conn()
            .prepare("SELECT id FROM action_ledger WHERE state IN ('intended', 'executing')")
            .unwrap();
        let ids: Vec<String> = st
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for id in &ids {
            self.mv(
                &db,
                id,
                "unknown",
                &["daemon restarted mid-action".into()],
                false,
            )
            .unwrap();
        }
        ids
    }
}

impl Ledger for SqlLedger {
    fn intend(
        &self,
        task_id: &str,
        capability: &str,
        target: &Map<String, Value>,
        payload_hash: &str,
    ) -> Result<String, DuplicateUnresolved> {
        let db = self.db.lock().unwrap();
        db.conn().execute_batch("BEGIN IMMEDIATE").unwrap();
        let approval = match self.check_repeat(&db, task_id, capability, payload_hash) {
            Ok(a) => a,
            Err(e) => {
                db.conn().execute_batch("ROLLBACK").unwrap();
                return Err(e);
            }
        };
        let now = self.now();
        let id = self.ids.new_id();
        db.conn()
            .execute(
                "INSERT INTO action_ledger (id, task_id, capability, target_json, payload_hash, state, created_at, \
                 updated_at) VALUES (?, ?, ?, ?, ?, 'intended', ?, ?)",
                params![id, task_id, capability, sorted(&Value::Object(target.clone())), payload_hash, now, now],
            )
            .unwrap();
        if let Some(a) = approval {
            db.conn()
                .execute(
                    "UPDATE action_ledger SET retried_by = ? WHERE id = ?",
                    params![id, a],
                )
                .unwrap();
        }
        db.conn().execute_batch("COMMIT").unwrap();
        Ok(id)
    }

    fn mark(&self, intent_id: &str, state: &str, evidence: &[String]) -> Result<(), BadTransition> {
        let db = self.db.lock().unwrap();
        if (state == "done" || state == "failed")
            && self
                .get_in(&db, intent_id)
                .is_some_and(|r| r.state == "unknown")
        {
            return Err(BadTransition(
                "an unknown action is settled only by reconcile()".into(),
            ));
        }
        self.mv(&db, intent_id, state, evidence, false)
    }

    fn refusal(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Option<DuplicateUnresolved> {
        let db = self.db.lock().unwrap();
        self.check_repeat(&db, task_id, capability, payload_hash)
            .err()
    }

    fn approved_retry(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Option<String> {
        let db = self.db.lock().unwrap();
        self.check_repeat(&db, task_id, capability, payload_hash)
            .ok()
            .flatten()
    }

    fn attempts(&self, intent_id: &str) -> Vec<LedgerRow> {
        let db = self.db.lock().unwrap();
        let Some(r) = self.get_in(&db, intent_id) else {
            return Vec::new();
        };
        let tree = r
            .task_id
            .split('.')
            .next()
            .unwrap_or(&r.task_id)
            .to_string();
        let (w, p) = tree_where(&tree);
        let mut st = db
            .conn()
            .prepare(&format!("SELECT * FROM action_ledger WHERE {w} AND capability = ? AND payload_hash = ? ORDER BY rowid"))
            .unwrap();
        st.query_map(params![p[0], p[1], p[2], r.capability, r.payload_hash], row)
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn approve_retry(&self, intent_id: &str, ask_span: &str) -> Result<(), BadTransition> {
        let r = self
            .get(intent_id)
            .ok_or_else(|| BadTransition("no such row".into()))?;
        if r.state != "unknown" {
            return Err(BadTransition(
                "only an unknown action's retry is approved".into(),
            ));
        }
        if self
            .attempts(intent_id)
            .iter()
            .any(|a| a.state == "intended" || a.state == "executing")
        {
            return Err(BadTransition(
                "an attempt at this action is still running".into(),
            ));
        }
        let mut evidence = r.evidence.clone();
        evidence.push(format!(
            "owner approved retry after inspection (owner.ask span {ask_span})"
        ));
        let db = self.db.lock().unwrap();
        db.conn()
            .execute(
                "UPDATE action_ledger SET retry_approved = ?, retried_by = NULL, updated_at = ?, evidence_json = ? WHERE id = ?",
                params![ask_span, self.now(), ferb_py::json::dumps(&evidence), intent_id],
            )
            .unwrap();
        Ok(())
    }

    fn get(&self, intent_id: &str) -> Option<LedgerRow> {
        let db = self.db.lock().unwrap();
        self.get_in(&db, intent_id)
    }
}
