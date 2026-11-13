//! ferb/orchestrator/test_runs.py: which chats, requests and tasks belong to a test run (GATE-550): the `test_run`
//! tag on their chat message. Records so tagged stay out of learning and other runs' context. Structure only.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use ferb_store::Db;

use crate::sql::{self, text, Result};

#[derive(Default)]
struct Known {
    sessions: Option<HashSet<String>>,
    requests: HashSet<String>,
    tasks: HashMap<String, bool>,
}

/// test_runs.py:15 `TestRuns`: one per database (the daemon keeps the one instance; Python's `of`).
pub struct TestRuns<'a> {
    db: &'a Db,
    known: Mutex<Known>,
}

impl<'a> TestRuns<'a> {
    pub fn new(db: &'a Db) -> TestRuns<'a> {
        TestRuns {
            db,
            known: Mutex::new(Known::default()),
        }
    }

    fn known(&self) -> std::sync::MutexGuard<'_, Known> {
        self.known.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn load(&self, known: &mut Known) -> Result<()> {
        if known.sessions.is_some() {
            return Ok(());
        }
        let mut sessions = HashSet::new();
        for row in sql::rows(
            self.db.conn(),
            "SELECT data_json FROM journal WHERE kind = 'chat.message' AND json_extract(data_json, '$.test_run') IS NOT NULL",
            &[],
        )? {
            let data = sql::loads(&sql::col(&row, "data_json"))?;
            let field = |k: &str| match data.get(k) {
                Some(v) if ferb_py::value::truthy(v) => crate::py::str_of(v),
                _ => String::new(),
            };
            sessions.insert(field("session_id"));
            known.requests.insert(field("request_id"));
        }
        known.sessions = Some(sessions);
        Ok(())
    }

    /// test_runs.py:41 `add`.
    pub fn add(&self, session_id: &str, request_id: &str) -> Result<()> {
        let mut known = self.known();
        self.load(&mut known)?;
        known
            .sessions
            .get_or_insert_with(HashSet::new)
            .insert(session_id.into());
        known.requests.insert(request_id.into());
        known.tasks.retain(|_, v| *v);
        Ok(())
    }

    /// test_runs.py:48 `session`.
    pub fn session(&self, session_id: &str) -> Result<bool> {
        let mut known = self.known();
        self.load(&mut known)?;
        Ok(!session_id.is_empty()
            && known
                .sessions
                .as_ref()
                .is_some_and(|s| s.contains(session_id)))
    }

    /// test_runs.py:52 `request`.
    pub fn request(&self, request_id: Option<&str>) -> Result<bool> {
        let mut known = self.known();
        self.load(&mut known)?;
        Ok(request_id.is_some_and(|r| !r.is_empty() && known.requests.contains(r)))
    }

    /// test_runs.py:56 `task`.
    pub fn task(&self, task_id: Option<&str>) -> Result<bool> {
        let Some(task_id) = task_id.filter(|t| !t.is_empty()) else {
            return Ok(false);
        };
        let mut known = self.known();
        self.load(&mut known)?;
        if known.requests.is_empty() {
            return Ok(false);
        }
        if let Some(v) = known.tasks.get(task_id) {
            return Ok(*v);
        }
        let mut row = sql::row(
            self.db.conn(),
            "SELECT request_id FROM tasks WHERE id = ?",
            &[text(task_id)],
        )?;
        if row.is_none() && task_id.contains('.') {
            let root = task_id.split('.').next().unwrap_or("");
            row = sql::row(
                self.db.conn(),
                "SELECT request_id FROM tasks WHERE id = ?",
                &[text(root)],
            )?;
        }
        let is = row.is_some_and(|r| {
            known
                .requests
                .contains(&crate::py::str_of(&r["request_id"]))
        });
        known.tasks.insert(task_id.into(), is);
        Ok(is)
    }
}
