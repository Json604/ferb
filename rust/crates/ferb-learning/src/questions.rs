//! `questions.py`: FERB's waiting questions about memory (`memory_questions`, migration 0041): a forget's Yes / No kept
//! until it is answered, so a restart asks it again. What an answer does is the daemon's.

use std::rc::Rc;

use serde_json::Value;

use crate::sql;
use crate::store::{marks, Sql, Store};
use crate::Result;

pub struct MemoryQuestions {
    pub store: Rc<Store>,
}

impl MemoryQuestions {
    /// questions.py:14 `keep`.
    pub fn keep(
        &self,
        approval_id: &str,
        kind: &str,
        target: &str,
        reason: &str,
        request_id: &str,
        at: &str,
    ) -> Result<()> {
        self.store.exec(
            "INSERT OR IGNORE INTO memory_questions VALUES (?,?,?,?,?,?)",
            sql![approval_id, kind, target, reason, request_id, at],
        )?;
        Ok(())
    }

    /// questions.py:18 `waiting`: every question still waiting, oldest first, each row's values in column order.
    pub fn waiting(&self) -> Result<Vec<Vec<Value>>> {
        Ok(self
            .store
            .query("SELECT * FROM memory_questions ORDER BY at", sql![])?
            .iter()
            .map(|r| r.json())
            .collect())
    }

    /// questions.py:22 `answered`.
    pub fn answered(&self, approval_id: &str) -> Result<()> {
        self.store.exec(
            "DELETE FROM memory_questions WHERE approval_id = ?",
            sql![approval_id],
        )?;
        Ok(())
    }

    /// questions.py:25 `drop_about`: questions (of `kinds`; None: every kind) about these targets.
    pub fn drop_about(&self, targets: &[String], kinds: Option<&[String]>) -> Result<()> {
        let gone: Vec<String> = targets
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if gone.is_empty() {
            return Ok(());
        }
        let which = kinds.map_or_else(String::new, |k| {
            format!(" AND kind IN ({})", marks(k.len()))
        });
        let mut params: Vec<Sql> = gone.iter().map(|g| Sql::Text(g.clone())).collect();
        params.extend(kinds.unwrap_or(&[]).iter().map(|k| Sql::Text(k.clone())));
        self.store.exec(
            &format!(
                "DELETE FROM memory_questions WHERE target IN ({}){which}",
                marks(gone.len())
            ),
            params,
        )?;
        Ok(())
    }

    /// questions.py:34 `drop_kinds`.
    pub fn drop_kinds(&self, kinds: &[String]) -> Result<()> {
        if !kinds.is_empty() {
            self.store.exec(
                &format!(
                    "DELETE FROM memory_questions WHERE kind IN ({})",
                    marks(kinds.len())
                ),
                kinds.iter().map(|k| Sql::Text(k.clone())).collect(),
            )?;
        }
        Ok(())
    }
}
