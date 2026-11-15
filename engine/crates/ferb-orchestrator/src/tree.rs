//! ferb/orchestrator/tree.py: what's running (for the gateway) and, in SQLite, every task row (for traces and
//! restarts): a restart marks what was running as `interrupted`.
//!
//! `ExecutionState` (tree.py:19) holds the gateway's `Requests` and the daemon's `Approvals`: it belongs with them
//! (the gateway part), not here.
//!
//! decision: ORCH-323, ORCH-510

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use ferb_core::types::Task;
use ferb_store::Db;
use serde_json::{Map, Value};

use crate::sql::{self, opt, text, Result};

/// tree.py:32 `TaskRecord`.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRecord {
    pub task: Task,
    /// `started_at.isoformat()`.
    pub started_at: String,
    pub span_id: String,
    pub tree_id: Option<String>,
}

/// tree.py:40 `TASK_COLUMNS`.
pub const TASK_COLUMNS: &[&str] = &[
    "request_id",
    "created_at",
    "updated_at",
    "goal",
    "status",
    "tree_id",
    "outcome_json",
    "parent_id",
];

/// tree.py:46 `STATUSES` (migration 0049).
pub const STATUSES: &[&str] = &[
    "planned",
    "running",
    "paused",
    "done",
    "done_unverified",
    "failed",
    "cancelled",
    "needs_user",
    "interrupted",
];

/// tree.py:50 `TaskTree`.
pub struct TaskTree<'a> {
    tasks: Mutex<HashMap<String, TaskRecord>>,
    pub db: Option<&'a Db>,
    /// The daemon's clock, as `isoformat()`.
    pub now: Box<dyn Fn() -> String + Send + Sync + 'a>,
}

impl<'a> TaskTree<'a> {
    pub fn new(
        db: Option<&'a Db>,
        now: Box<dyn Fn() -> String + Send + Sync + 'a>,
    ) -> TaskTree<'a> {
        TaskTree {
            tasks: Mutex::new(HashMap::new()),
            db,
            now,
        }
    }

    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<String, TaskRecord>> {
        self.tasks.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// tree.py:62 `add`.
    pub fn add(&self, rec: TaskRecord, status: &str) -> Result<()> {
        self.live().insert(rec.task.id.clone(), rec.clone());
        let Some(db) = self.db else { return Ok(()) };
        let t = &rec.task;
        let scopes: BTreeSet<&String> = t.scopes.iter().collect();
        let scopes: Vec<&String> = scopes.into_iter().collect();
        sql::exec(
            db.conn(),
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, \
             status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET request_id=excluded.request_id,goal=excluded.goal,\
             identity=excluded.identity,role=excluded.role,scopes_json=excluded.scopes_json,\
             status=excluded.status,updated_at=excluded.updated_at,outcome_json='{}'",
            &[
                text(&t.id),
                text(rec.tree_id.as_deref().unwrap_or(&t.id)),
                opt(t.parent_id.as_deref()),
                text(&t.request_id),
                text(&t.goal),
                text(t.identity.as_str()),
                text(t.role.as_str()),
                text(&ferb_py::json::dumps(&scopes)),
                text(status),
                text(&rec.started_at),
                text(&rec.started_at),
            ],
        )?;
        Ok(())
    }

    /// tree.py:76 `hold`: a live record with no task row (the planner's lookups).
    pub fn hold(&self, rec: TaskRecord) {
        self.live().insert(rec.task.id.clone(), rec);
    }

    pub fn get(&self, task_id: &str) -> Option<TaskRecord> {
        self.live().get(task_id).cloned()
    }

    /// tree.py:84 `tree_of`.
    pub fn tree_of(&self, task_id: &str) -> Result<Option<String>> {
        if let Some(rec) = self.live().get(task_id) {
            return Ok(Some(
                rec.tree_id.clone().unwrap_or_else(|| task_id.to_string()),
            ));
        }
        let Some(db) = self.db else { return Ok(None) };
        Ok(sql::row(
            db.conn(),
            "SELECT tree_id FROM tasks WHERE id = ?",
            &[text(task_id)],
        )?
        .map(|r| sql::col(&r, "tree_id")))
    }

    pub fn set_span(&self, task_id: &str, span_id: &str) {
        if let Some(rec) = self.live().get_mut(task_id) {
            rec.span_id = span_id.to_string();
        }
    }

    /// tree.py:99 `set_status`: `outcome` is written as `json.dumps(outcome)`.
    pub fn set_status(
        &self,
        task_id: &str,
        status: &str,
        outcome: Option<&Value>,
        at: &str,
    ) -> Result<()> {
        let Some(db) = self.db else { return Ok(()) };
        let when = if at.is_empty() {
            (self.now)()
        } else {
            at.to_string()
        };
        sql::exec(
            db.conn(),
            "UPDATE tasks SET status = ?, outcome_json = COALESCE(?, outcome_json), updated_at = ? WHERE id = ?",
            &[text(status), opt(outcome.map(ferb_py::json::dumps).as_deref()), text(&when), text(task_id)],
        )?;
        Ok(())
    }

    /// tree.py:106 `set_identity`.
    pub fn set_identity(&self, task_id: &str, identity: &str) -> Result<()> {
        if let Some(db) = self.db {
            sql::exec(
                db.conn(),
                "UPDATE tasks SET identity = ? WHERE id = ?",
                &[text(identity), text(task_id)],
            )?;
        }
        Ok(())
    }

    pub fn remove(&self, task_id: &str) {
        self.live().remove(task_id);
    }

    /// tree.py:114 `rows`.
    pub fn rows(&self, tree_id: &str) -> Result<Vec<Map<String, Value>>> {
        let Some(db) = self.db else {
            return Ok(Vec::new());
        };
        sql::rows(
            db.conn(),
            "SELECT * FROM tasks WHERE tree_id = ? ORDER BY created_at",
            &[text(tree_id)],
        )
    }

    /// tree.py:120 `column`: one recorded column (of TASK_COLUMNS) of a task.
    pub fn column(&self, task_id: &str, name: &str) -> Result<Option<Value>> {
        let Some(db) = self.db else { return Ok(None) };
        if !TASK_COLUMNS.contains(&name) {
            return Ok(None);
        }
        Ok(sql::row(
            db.conn(),
            &format!("SELECT {name} FROM tasks WHERE id = ?"),
            &[text(task_id)],
        )?
        .and_then(|r| r.get(name).cloned()))
    }

    /// tree.py:127 `latest_root`.
    pub fn latest_root(&self) -> Result<Option<String>> {
        let Some(db) = self.db else { return Ok(None) };
        Ok(sql::row(
            db.conn(),
            "SELECT id FROM tasks WHERE parent_id IS NULL ORDER BY created_at DESC, rowid DESC LIMIT 1",
            &[],
        )?
        .map(|r| sql::col(&r, "id")))
    }

    pub fn recorded(&self, task_id: &str) -> Result<bool> {
        let Some(db) = self.db else { return Ok(false) };
        Ok(sql::row(
            db.conn(),
            "SELECT 1 FROM tasks WHERE id = ?",
            &[text(task_id)],
        )?
        .is_some())
    }

    /// tree.py:139 `roots_with_prefix`.
    pub fn roots_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let Some(db) = self.db else {
            return Ok(Vec::new());
        };
        Ok(sql::rows(
            db.conn(),
            "SELECT id FROM tasks WHERE substr(id, 1, ?) = ? AND parent_id IS NULL ORDER BY id",
            &[
                rusqlite::types::Value::Integer(prefix.chars().count() as i64),
                text(prefix),
            ],
        )?
        .iter()
        .map(|r| sql::col(r, "id"))
        .collect())
    }

    /// tree.py:147 `trees_of_request`.
    pub fn trees_of_request(&self, request_id: &str) -> Result<BTreeSet<String>> {
        let Some(db) = self.db else {
            return Ok(BTreeSet::new());
        };
        Ok(sql::rows(
            db.conn(),
            "SELECT DISTINCT tree_id FROM tasks WHERE request_id=?",
            &[text(request_id)],
        )?
        .iter()
        .map(|r| sql::col(r, "tree_id"))
        .collect())
    }

    /// tree.py:154 `interrupt_running`: after a restart.
    pub fn interrupt_running(&self) -> Result<Vec<String>> {
        let Some(db) = self.db else {
            return Ok(Vec::new());
        };
        let ids: Vec<String> = sql::rows(
            db.conn(),
            "SELECT id FROM tasks WHERE status IN ('running', 'planned', 'paused')",
            &[],
        )?
        .iter()
        .map(|r| sql::col(r, "id"))
        .collect();
        sql::exec(
            db.conn(),
            "UPDATE tasks SET status = 'interrupted', updated_at = ? WHERE status IN ('running', 'planned', 'paused')",
            &[text(&(self.now)())],
        )?;
        Ok(ids)
    }
}
