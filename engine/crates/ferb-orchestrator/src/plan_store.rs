//! ferb/orchestrator/plan_store.py: durable plan state. Completed work is never silently put back in the pending
//! queue.
//!
//! decision: ORCH-322, ORCH-532, ORCH-546, ORCH-548

use std::collections::HashMap;
use std::sync::Mutex;

use ferb_store::Db;
use serde_json::{Map, Value};

use crate::models::{EventMatch, Plan};
use crate::plans::{single, SINGLE};
use crate::ports::Redact;
use crate::sql::{self, text, Error, Result};

/// plan_store.py:11 `EARLIER`: where a replaced plan's finished results stay (L11).
pub const EARLIER: &str = "earlier plan: ";

/// One step's stored state: `{"status", "output"}`.
#[derive(Debug, Clone, PartialEq)]
pub struct StepState {
    pub status: String,
    pub output: Value,
}

/// plan_store.py `load`: the row with its plan and request read.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub row: Map<String, Value>,
    pub plan: Plan,
    pub request: Value,
}

/// plan_store.py:14 `PlanStore`.
pub struct PlanStore<'a> {
    pub db: &'a Db,
    pub redact: Redact,
    /// task id -> how many times this store wrote its plan (since start).
    pub revisions: Mutex<HashMap<String, u64>>,
}

impl<'a> PlanStore<'a> {
    pub fn new(db: &'a Db, redact: Redact) -> PlanStore<'a> {
        PlanStore {
            db,
            redact,
            revisions: Mutex::new(HashMap::new()),
        }
    }

    fn revised(&self, task_id: &str) {
        *self
            .revisions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(task_id.to_string())
            .or_insert(0) += 1;
    }

    pub fn revision(&self, task_id: &str) -> u64 {
        self.revisions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(task_id)
            .copied()
            .unwrap_or(0)
    }

    /// `json.dumps(self.redact(value), ensure_ascii=False)`.
    fn json(&self, value: &Value) -> String {
        ferb_py::json::dumps_utf8(&(self.redact)(value))
    }

    fn plan_json(&self, plan: &Plan) -> String {
        self.json(&crate::validate::dump_plan(plan).to_value())
    }

    /// plan_store.py:25 `create`.
    pub fn create(
        &self,
        task_id: &str,
        plan: &Plan,
        request: &Value,
        conversation: Option<&str>,
    ) -> Result<()> {
        sql::transaction(self.db.conn(), |c| {
            sql::exec(
                c,
                "INSERT INTO task_plans(task_id,plan,request,conversation) VALUES(?,?,?,?) \
                 ON CONFLICT(task_id) DO UPDATE SET request=excluded.request",
                &[
                    text(task_id),
                    text(&self.plan_json(plan)),
                    text(&self.json(request)),
                    sql::opt(conversation),
                ],
            )?;
            for step in &plan.steps {
                sql::exec(
                    c,
                    "INSERT OR IGNORE INTO plan_steps(task_id,step_id) VALUES(?,?)",
                    &[text(task_id), text(&step.id)],
                )?;
            }
            Ok(())
        })?;
        self.revised(task_id);
        Ok(())
    }

    /// plan_store.py:34 `create_single`.
    pub fn create_single(
        &self,
        task_id: &str,
        goal: &str,
        request: &Value,
        conversation: Option<&str>,
        restored: bool,
    ) -> Result<()> {
        let plan = single(goal);
        sql::transaction(self.db.conn(), |c| {
            sql::exec(
                c,
                "INSERT INTO task_plans(task_id,plan,request,conversation) VALUES(?,?,?,?) \
                 ON CONFLICT(task_id) DO UPDATE SET plan=excluded.plan, request=excluded.request, status='running'",
                &[text(task_id), text(&self.plan_json(&plan)), text(&self.json(request)), sql::opt(conversation)],
            )?;
            if !restored {
                for (key, state) in self.steps(task_id)? {
                    if key.starts_with(EARLIER) || key == SINGLE {
                        continue;
                    }
                    if state.status == "done" {
                        let earlier = format!("{EARLIER}{key}");
                        sql::exec(
                            c,
                            "DELETE FROM plan_steps WHERE task_id=? AND step_id=?",
                            &[text(task_id), text(&earlier)],
                        )?;
                        sql::exec(
                            c,
                            "UPDATE plan_steps SET step_id=? WHERE task_id=? AND step_id=?",
                            &[text(&earlier), text(task_id), text(&key)],
                        )?;
                    } else {
                        sql::exec(
                            c,
                            "DELETE FROM plan_steps WHERE task_id=? AND step_id=?",
                            &[text(task_id), text(&key)],
                        )?;
                    }
                }
                sql::exec(
                    c,
                    "INSERT INTO plan_steps(task_id,step_id) VALUES(?,?) ON CONFLICT(task_id, step_id) \
                     DO UPDATE SET status='pending', output='{}'",
                    &[text(task_id), text(SINGLE)],
                )?;
            } else {
                sql::exec(
                    c,
                    "INSERT OR IGNORE INTO plan_steps(task_id,step_id) VALUES(?,?)",
                    &[text(task_id), text(SINGLE)],
                )?;
            }
            Ok(())
        })?;
        self.revised(task_id);
        Ok(())
    }

    /// plan_store.py:63 `update_plan`.
    pub fn update_plan(&self, task_id: &str, plan: &Plan) -> Result<()> {
        sql::exec(
            self.db.conn(),
            "UPDATE task_plans SET plan=? WHERE task_id=?",
            &[text(&self.plan_json(plan)), text(task_id)],
        )?;
        self.revised(task_id);
        Ok(())
    }

    /// plan_store.py:68 `load`.
    pub fn load(&self, task_id: &str) -> Result<Loaded> {
        let Some(row) = sql::row(
            self.db.conn(),
            "SELECT * FROM task_plans WHERE task_id=?",
            &[text(task_id)],
        )?
        else {
            return Err(Error::Key(task_id.into()));
        };
        let plan = crate::validate::plan_validate_json(&sql::col(&row, "plan"))
            .map_err(|e| Error::Value(e.to_string()))?;
        let request = sql::loads(&sql::col(&row, "request"))?;
        Ok(Loaded { row, plan, request })
    }

    /// plan_store.py:74 `steps`: in the store's order.
    pub fn steps(&self, task_id: &str) -> Result<Vec<(String, StepState)>> {
        sql::rows(
            self.db.conn(),
            "SELECT * FROM plan_steps WHERE task_id=?",
            &[text(task_id)],
        )?
        .iter()
        .map(|r| {
            Ok((
                sql::col(r, "step_id"),
                StepState {
                    status: sql::col(r, "status"),
                    output: sql::loads(&sql::col(r, "output"))?,
                },
            ))
        })
        .collect()
    }

    pub fn set_status(&self, task_id: &str, status: &str) -> Result<()> {
        sql::exec(
            self.db.conn(),
            "UPDATE task_plans SET status=? WHERE task_id=?",
            &[text(status), text(task_id)],
        )?;
        Ok(())
    }

    /// plan_store.py:81 `begin`.
    pub fn begin(&self, task_id: &str, step_id: &str, status: &str) -> Result<()> {
        sql::exec(
            self.db.conn(),
            "UPDATE plan_steps SET status=? WHERE task_id=? AND step_id=? AND status != ?",
            &[text(status), text(task_id), text(step_id), text("done")],
        )?;
        Ok(())
    }

    /// plan_store.py:85 `finish`.
    pub fn finish(&self, task_id: &str, step_id: &str, output: &Value) -> Result<()> {
        let status = match output.get("status") {
            Some(v) => v.clone(),
            None => Value::String("failed".into()),
        };
        sql::exec(
            self.db.conn(),
            "UPDATE plan_steps SET status=?,output=? WHERE task_id=? AND step_id=?",
            &[
                sql::to_sql(&status),
                text(&self.json(output)),
                text(task_id),
                text(step_id),
            ],
        )?;
        Ok(())
    }

    /// plan_store.py:89 `reopen`.
    pub fn reopen(&self, task_id: &str) -> Result<()> {
        sql::transaction(self.db.conn(), |c| {
            sql::exec(
                c,
                "UPDATE plan_steps SET status='pending' WHERE task_id=? AND status != 'done'",
                &[text(task_id)],
            )?;
            sql::exec(
                c,
                "UPDATE task_plans SET status='running', approved=0 WHERE task_id=?",
                &[text(task_id)],
            )?;
            Ok(())
        })
    }

    /// plan_store.py:97 `retry_unfinished`.
    pub fn retry_unfinished(&self, task_id: &str) -> Result<()> {
        sql::exec(
            self.db.conn(),
            "UPDATE plan_steps SET status='pending' WHERE task_id=? AND status IN ('failed', 'blocked', 'needs_user', 'cancelled')",
            &[text(task_id)],
        )?;
        Ok(())
    }

    /// plan_store.py:103 `replace` (L11).
    pub fn replace(&self, task_id: &str, plan: &Plan) -> Result<()> {
        let old = self.load(task_id)?.plan;
        let kept: Vec<&str> = plan
            .steps
            .iter()
            .filter(|s| old.steps.contains(s))
            .map(|s| s.id.as_str())
            .collect();
        sql::transaction(self.db.conn(), |c| {
            for (key, state) in self.steps(task_id)? {
                let head = key.split(':').next().unwrap_or("");
                if key.starts_with(EARLIER) || (kept.contains(&head) && state.status == "done") {
                    continue;
                }
                if state.status == "done" {
                    let earlier = format!("{EARLIER}{key}");
                    sql::exec(
                        c,
                        "DELETE FROM plan_steps WHERE task_id=? AND step_id=?",
                        &[text(task_id), text(&earlier)],
                    )?;
                    sql::exec(
                        c,
                        "UPDATE plan_steps SET step_id=? WHERE task_id=? AND step_id=?",
                        &[text(&earlier), text(task_id), text(&key)],
                    )?;
                } else {
                    sql::exec(
                        c,
                        "DELETE FROM plan_steps WHERE task_id=? AND step_id=?",
                        &[text(task_id), text(&key)],
                    )?;
                }
            }
            sql::exec(
                c,
                "UPDATE task_plans SET plan=?, status='running', approved=0 WHERE task_id=?",
                &[text(&self.plan_json(plan)), text(task_id)],
            )?;
            Ok(())
        })?;
        self.revised(task_id);
        Ok(())
    }

    /// plan_store.py:125 `earlier`.
    pub fn earlier(&self, task_id: &str) -> Result<Vec<(String, Value)>> {
        Ok(self
            .steps(task_id)?
            .into_iter()
            .filter_map(|(k, s)| {
                k.strip_prefix(EARLIER)
                    .map(|rest| (rest.to_string(), s.output))
            })
            .collect())
    }

    /// plan_store.py:130 `pending`.
    pub fn pending(&self) -> Result<Vec<String>> {
        Ok(sql::rows(
            self.db.conn(),
            "SELECT task_id FROM task_plans WHERE status IN ('running','suspended','steps_done')",
            &[],
        )?
        .iter()
        .map(|r| sql::col(r, "task_id"))
        .collect())
    }

    /// plan_store.py:136 `paused`.
    pub fn paused(&self, task_id: &str) -> Result<bool> {
        Ok(sql::row(
            self.db.conn(),
            "SELECT 1 FROM plan_steps s JOIN task_plans p ON p.task_id = s.task_id WHERE s.task_id=? AND \
             s.status='paused' AND p.status IN ('running','suspended') LIMIT 1",
            &[text(task_id)],
        )?
        .is_some())
    }

    /// plan_store.py:142 `event`.
    pub fn event(&self, name: &str, payload: &Value) -> Result<()> {
        sql::exec(
            self.db.conn(),
            "INSERT INTO plan_events(event,payload) VALUES(?,?)",
            &[text(name), text(&self.json(payload))],
        )?;
        Ok(())
    }

    fn matches(payload: &Value, matches: &[EventMatch]) -> bool {
        matches.iter().all(|m| {
            payload
                .get(&m.field)
                .is_some_and(|v| crate::py::str_of(v) == m.value)
        })
    }

    /// plan_store.py:145 `event_after`.
    pub fn event_after(
        &self,
        name: &str,
        after: i64,
        matches: &[EventMatch],
    ) -> Result<Option<Value>> {
        Ok(self.event_seq_after(name, after, matches)?.map(|(_, p)| p))
    }

    /// plan_store.py:153 `event_seq_after`.
    pub fn event_seq_after(
        &self,
        name: &str,
        after: i64,
        matches: &[EventMatch],
    ) -> Result<Option<(i64, Value)>> {
        for row in sql::rows(
            self.db.conn(),
            "SELECT seq,payload FROM plan_events WHERE event=? AND seq>? ORDER BY seq",
            &[text(name), rusqlite::types::Value::Integer(after)],
        )? {
            let payload = sql::loads(&sql::col(&row, "payload"))?;
            if Self::matches(&payload, matches) {
                return Ok(Some((
                    row.get("seq").and_then(|v| v.as_i64()).unwrap_or(0),
                    payload,
                )));
            }
        }
        Ok(None)
    }

    /// plan_store.py:163 `event_cursor`.
    pub fn event_cursor(&self) -> Result<i64> {
        Ok(sql::row(
            self.db.conn(),
            "SELECT COALESCE(MAX(seq),0) AS m FROM plan_events",
            &[],
        )?
        .and_then(|r| r.get("m").and_then(|v| v.as_i64()))
        .unwrap_or(0))
    }
}
