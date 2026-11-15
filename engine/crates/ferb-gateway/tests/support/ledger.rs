//! The real action ledger (`ferb_orchestrator::ledger`, ported from `ferb/orchestrator/ledger.py` by the
//! orchestration part, journal 33) behind the gateway's `Ledger` seam, on the scenario's store, as the daemon wires
//! it.

use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{Clock, IdSource};
use ferb_gateway::seams::{BadTransition, DuplicateUnresolved, Ledger};
use ferb_gateway::words::LedgerRow;
use ferb_orchestrator::ledger;
use ferb_orchestrator::sql::Error;
use ferb_store::Db;
use serde_json::{Map, Value};

pub struct RealLedger {
    pub db: Arc<Mutex<Db>>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
}

fn row(r: ledger::LedgerRow) -> LedgerRow {
    LedgerRow {
        id: r.intent.id,
        task_id: r.intent.task_id,
        capability: r.intent.capability,
        target: r.intent.target.as_object().cloned().unwrap_or_default(),
        payload_hash: r.intent.payload_hash,
        state: r.intent.state,
        reconciled: r.reconciled,
        evidence: r
            .evidence
            .iter()
            .map(|e| match e {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        retry_approved: r.retry_approved,
        retried_by: r.retried_by,
    }
}

fn dup(e: Error) -> Option<DuplicateUnresolved> {
    match e {
        Error::Duplicate {
            message,
            intent_id,
            state,
        } => Some(DuplicateUnresolved {
            message,
            intent_id,
            state,
        }),
        _ => None,
    }
}

fn bad(e: Error) -> BadTransition {
    match e {
        Error::BadTransition(m) => BadTransition(m),
        other => BadTransition(format!("{other:?}")),
    }
}

impl RealLedger {
    /// The ledger on the store, with the scenario's clock and ids.
    pub fn with<T>(&self, f: impl FnOnce(&ledger::Ledger<'_>) -> T) -> T {
        let db = self.db.lock().unwrap();
        let clock = self.clock.clone();
        let ids = self.ids.clone();
        let l = ledger::Ledger::new(
            &db,
            Box::new(move || ferb_core::when::isoformat(&clock.now())),
            Box::new(move || ids.new_id()),
        );
        f(&l)
    }

    /// ledger.py:156: what a restart leaves: every row still in flight is unknown.
    pub fn mark_unknown_after_restart(&self) -> Vec<String> {
        self.with(|l| l.mark_unknown_after_restart()).unwrap()
    }
}

impl Ledger for RealLedger {
    fn intend(
        &self,
        task_id: &str,
        capability: &str,
        target: &Map<String, Value>,
        payload_hash: &str,
    ) -> Result<String, DuplicateUnresolved> {
        let target = Value::Object(target.clone());
        self.with(|l| l.intend(task_id, capability, &target, payload_hash, true))
            .map(|i| i.id)
            .map_err(|e| dup(e).expect("a refusal of the ledger"))
    }

    fn mark(&self, intent_id: &str, state: &str, evidence: &[String]) -> Result<(), BadTransition> {
        self.with(|l| l.mark(intent_id, state, evidence))
            .map_err(bad)
    }

    fn refusal(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Option<DuplicateUnresolved> {
        self.with(|l| l.refusal(task_id, capability, payload_hash))
            .ok()
            .flatten()
            .and_then(dup)
    }

    fn approved_retry(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Option<String> {
        self.with(|l| l.approved_retry(task_id, capability, payload_hash))
            .ok()
            .flatten()
    }

    fn attempts(&self, intent_id: &str) -> Vec<LedgerRow> {
        self.with(|l| l.attempts(intent_id))
            .map(|rows| rows.into_iter().map(row).collect())
            .unwrap_or_default()
    }

    fn approve_retry(&self, intent_id: &str, ask_span: &str) -> Result<(), BadTransition> {
        self.with(|l| l.approve_retry(intent_id, ask_span))
            .map_err(bad)
    }

    fn get(&self, intent_id: &str) -> Option<LedgerRow> {
        self.with(|l| l.get(intent_id)).ok().map(row)
    }
}
