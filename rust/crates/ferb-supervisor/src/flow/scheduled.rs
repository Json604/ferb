//! Scheduled runs (supervisor.py:1629 `run_scheduled`, :1728 `_previous_run`, :2369 `schedule_running`), as the
//! daemon's schedules area drives them (rust/crates/ferb-daemon `areas_a::seams::Tasks`: `schedule_running`,
//! `create_scheduled`, `run_scheduled`).
//!
//! The daemon claims a due timer and starts the run in one transaction (ferb-clock `claim_run` / `start_run`): its
//! first half here, [`Inner::create_scheduled`], writes the run's record (`schedule.run`, with the run's request id)
//! on the connection the daemon hands in, so a crash between the timer and the task never loses the run (the fix
//! rust/PLAN.md part 9 names). A run of a schedule whose previous run is still under way is skipped by the daemon
//! (`schedule_running`).
//!
//! Not here: the run itself (an unattended planner turn on the schedule's own session with its previous run's
//! answer, then `run_tree(..., unattended_scopes=, start_grant=, watch=, schedule=)`) needs the forward path's planner
//! turn for a schedule (`_decide` with `schedule_previous`) and a `run_tree` that takes a schedule:
//! [`Inner::run_scheduled_rest`] answers `NotPorted` before it writes anything.

use ferb_orchestrator::sql;
use rusqlite::types::Value as Sql;
use serde_json::{json, Map, Value};

use super::Inner;
use crate::NotPorted;

/// What `create_scheduled` hands the rest of the run.
#[derive(Clone, Debug, PartialEq)]
pub struct Created {
    pub request_id: String,
    /// `_previous_run(sid)`, read before this run's own record: (ran_at, status, its answer's text, that entry's id
    /// and origin, how many later runs left no answer).
    pub previous: Option<Previous>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Previous {
    pub ran_at: String,
    pub status: String,
    pub entry_id: String,
    pub text: String,
    pub origin: String,
    pub skipped: usize,
}

impl Created {
    pub fn to_json(&self) -> Value {
        json!({"request_id": self.request_id, "previous": self.previous.as_ref().map(|p| json!({
            "ran_at": p.ran_at, "status": p.status, "entry_id": p.entry_id, "text": p.text, "origin": p.origin,
            "skipped": p.skipped}))})
    }
}

impl Inner {
    /// supervisor.py:2369 `schedule_running`: a run of this schedule (a resumed one included) is under way.
    pub fn schedule_running(&self, schedule_id: &str) -> bool {
        self.live()
            .tree_of
            .iter()
            .any(|(k, t)| *k == t.id && t.lock().schedule_id.as_deref() == Some(schedule_id))
    }

    /// The first half of `run_scheduled` (supervisor.py:1632-1635): the run's request id, its previous run (read
    /// before this run's own record), and its `schedule.run` record, written on `conn` (the daemon's transaction that
    /// marks the run's timer handled).
    pub fn create_scheduled(
        &self,
        conn: &rusqlite::Connection,
        schedule_id: &str,
        late: bool,
        retry: bool,
    ) -> Result<Created, String> {
        let request_id = self.new_id();
        let previous = previous_run(conn, schedule_id).map_err(|e| e.to_string())?;
        let mut data = json!({"schedule_id": schedule_id, "request_id": request_id, "late": late});
        if retry {
            data["retry"] = json!(true);
        }
        self.journal_on(conn, "schedule.run", &data)
            .map_err(|e| e.to_string())?;
        Ok(Created {
            request_id,
            previous,
        })
    }

    /// journal.py `append` on a connection the caller holds (a transaction of its own).
    fn journal_on(
        &self,
        conn: &rusqlite::Connection,
        kind: &str,
        data: &Value,
    ) -> rusqlite::Result<String> {
        let body = self.redact(data);
        let made = super::store::journal_origin(kind, &body);
        let id = self.new_id();
        let at = self.now_iso();
        conn.execute(
            "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![id, at, kind, ferb_py::json::dumps_utf8(&body), made],
        )?;
        Ok(id)
    }

    /// The rest of `run_scheduled` (supervisor.py:1636-1706). Not ported: answers NotPorted before it writes anything
    /// (see the module's note).
    pub async fn run_scheduled_rest(
        &self,
        _created: &Created,
        _late: bool,
        _overdue_s: f64,
        _retry: bool,
    ) -> Result<Map<String, Value>, NotPorted> {
        Err(NotPorted(
            "Supervisor.run_scheduled (the schedule's planner turn and run_tree(schedule=...))",
        ))
    }
}

/// supervisor.py:1728 `_previous_run`: this schedule's most recent earlier run that left an answer (a task's answer, or
/// the planner's reply), with how many later runs left none. Read on `conn`.
pub fn previous_run(
    conn: &rusqlite::Connection,
    schedule_id: &str,
) -> Result<Option<Previous>, sql::Error> {
    let of_kind = |kind: &str,
                   field: &str,
                   equals: &str|
     -> Result<Vec<Map<String, Value>>, sql::Error> {
        sql::rows(
            conn,
            "SELECT * FROM journal WHERE kind = ? AND COALESCE(json_extract(data_json, ?), ?) = ? ORDER BY seq",
            &[
                Sql::Text(kind.into()),
                Sql::Text(format!("$.{field}")),
                Sql::Null,
                Sql::Text(equals.into()),
            ],
        )
    };
    let runs = of_kind("schedule.run", "schedule_id", schedule_id)?;
    let mut skipped = 0;
    for run in runs.iter().rev() {
        let data: Value = serde_json::from_str(&sql::col(run, "data_json")).unwrap_or(json!({}));
        let rid = data
            .get("request_id")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let mut answers = Vec::new();
        for kind in ["task.answer", "chat.reply", "chat.clarify"] {
            answers.extend(of_kind(kind, "request_id", &rid)?);
        }
        if answers.is_empty() {
            skipped += 1;
            continue;
        }
        // `max(answers, key=lambda e: e.at)`: the first of the latest
        let mut best = &answers[0];
        for a in &answers[1..] {
            if sql::col(a, "at") > sql::col(best, "at") {
                best = a;
            }
        }
        let entry: Value = serde_json::from_str(&sql::col(best, "data_json")).unwrap_or(json!({}));
        let kind = sql::col(best, "kind");
        let status = match entry.get("status").filter(|s| ferb_py::value::truthy(s)) {
            Some(s) => ferb_py::value::py_str(s),
            None if kind == "chat.clarify" => "needs_user".into(),
            None => "done".into(),
        };
        return Ok(Some(Previous {
            ran_at: sql::col(run, "at"),
            status,
            entry_id: sql::col(best, "id"),
            text: entry
                .get("text")
                .map(ferb_py::value::py_str)
                .unwrap_or_default(),
            origin: sql::col(best, "origin"),
            skipped,
        }));
    }
    Ok(None)
}
