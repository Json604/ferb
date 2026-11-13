//! What FERB told (or will tell) the owner about each task (`task_results`) and which of those
//! a chat has shown. A port of `ferb/store/task_results.py`. A result is marked shown only when
//! a client says it showed it (`mark_shown`). Task results stay forever (decision: STORE-401).

use rusqlite::params;
use serde_json::{Map, Value};

use crate::{
    pyjson,
    pyval::{is_py_space, query_maps, squeeze},
    Db, Result,
};

/// A task's short name (`task_results.py:10-12`; T-1055, decision: DMN-544): the one its
/// result was told with, else its spec's; '' for a task saved before names.
const NAME: &str = "COALESCE(NULLIF(n.name, ''), json_extract(x.spec_json, '$.name'), '')";

/// A row as `sqlite3.Row` gives it: every column by name, in the statement's order.
pub type Row = Map<String, Value>;

/// Python's `TaskResults` (`task_results.py:15`).
pub struct TaskResults<'a> {
    db: &'a Db,
}

impl<'a> TaskResults<'a> {
    pub fn new(db: &'a Db) -> Self {
        TaskResults { db }
    }

    /// `record` (`task_results.py:19-33`): a task's latest result (a later one replaces it, its
    /// cards too), with its typed cards and the short name it was told with. `cards`: a JSON
    /// list; `None` or an empty one removes the cards kept.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        task_id: &str,
        at: &str,
        status: &str,
        goal: &str,
        answer: &str,
        shown: bool,
        cards: Option<&Value>,
        name: &str,
    ) -> Result<()> {
        let conn = self.db.conn();
        conn.execute(
            "INSERT OR REPLACE INTO task_results (task_id, at, status, goal, answer, shown) VALUES \
             (?, ?, ?, ?, ?, ?)",
            params![task_id, at, status, goal, answer, i64::from(shown)],
        )?;
        if !name.trim_matches(is_py_space).is_empty() {
            conn.execute(
                "INSERT OR REPLACE INTO task_result_names (task_id, name) VALUES (?, ?)",
                params![task_id, squeeze(name)],
            )?;
        }
        match cards.filter(|c| crate::pyval::truthy(c)) {
            Some(cards) => conn.execute(
                "INSERT OR REPLACE INTO task_result_cards (task_id, cards_json) VALUES (?, ?)",
                params![task_id, pyjson::dumps(cards)],
            )?,
            None => conn.execute("DELETE FROM task_result_cards WHERE task_id = ?", [task_id])?,
        };
        Ok(())
    }

    /// `interrupted` (`task_results.py:35-38`): a task a restart cut off, told once (a result
    /// already there stays).
    pub fn interrupted(&self, task_id: &str, at: &str, goal: &str, answer: &str) -> Result<()> {
        self.db.conn().execute(
            "INSERT OR IGNORE INTO task_results (task_id, at, status, goal, answer, shown) \
             VALUES (?, ?, 'interrupted', ?, ?, 0)",
            params![task_id, at, goal, answer],
        )?;
        Ok(())
    }

    /// `unshown` (`task_results.py:40-48`): results no chat has shown yet, oldest first, with
    /// their task's recorded outcome and request, and their cards (`cards_json`, NULL without
    /// any).
    pub fn unshown(&self) -> Result<Vec<Row>> {
        query_maps(
            self.db.conn(),
            &format!(
                "SELECT r.*, t.outcome_json, t.request_id, c.cards_json, \
                 {NAME} AS name FROM task_results r \
                 LEFT JOIN tasks t ON t.id = r.task_id LEFT JOIN task_result_cards c \
                 ON c.task_id = r.task_id LEFT JOIN task_context x ON x.task_id = r.task_id \
                 LEFT JOIN task_result_names n ON n.task_id = r.task_id \
                 WHERE r.shown = 0 ORDER BY r.at"
            ),
            [],
        )
    }

    /// `page` (`task_results.py:50-59`): root tasks created before `before`, newest first, at
    /// most `limit`, with their result's answer and the owner's latest verdict.
    pub fn page(&self, before: &str, limit: i64) -> Result<Vec<Row>> {
        query_maps(
            self.db.conn(),
            &format!(
                "SELECT t.id, t.goal, t.status, t.created_at, t.updated_at, t.outcome_json, r.answer, {NAME} AS name, \
                 (SELECT v.verdict FROM owner_verdicts v WHERE v.task_id = t.id ORDER BY v.seq DESC LIMIT 1) AS verdict \
                 FROM tasks t LEFT JOIN task_results r ON r.task_id = t.id LEFT JOIN task_context x ON x.task_id = t.id \
                 LEFT JOIN task_result_names n ON n.task_id = t.id \
                 WHERE t.parent_id IS NULL AND t.created_at < ? ORDER BY t.created_at DESC, t.rowid DESC LIMIT ?"
            ),
            params![before, limit],
        )
    }

    /// `latest_of_schedule` (`task_results.py:61-67`): the latest result of a root task a
    /// `schedule.run` of this schedule started, or `None`.
    pub fn latest_of_schedule(&self, schedule_id: &str) -> Result<Option<Row>> {
        Ok(query_maps(
            self.db.conn(),
            "SELECT r.at, r.status, r.answer, r.task_id FROM task_results r JOIN tasks t ON t.id = r.task_id \
             WHERE t.parent_id IS NULL AND t.request_id IN (SELECT json_extract(data_json, '$.request_id') FROM journal \
             WHERE kind = 'schedule.run' AND json_extract(data_json, '$.schedule_id') = ?) ORDER BY r.at DESC LIMIT 1",
            [schedule_id],
        )?
        .into_iter()
        .next())
    }

    /// `record_verdict` (`task_results.py:69-73`): the owner's pass/fail verdict on a root task.
    /// As Python's `conn.commit()` does, it also commits a transaction left open.
    pub fn record_verdict(
        &self,
        verdict_id: &str,
        task_id: &str,
        verdict: &str,
        note: &str,
        at: &str,
    ) -> Result<()> {
        let conn = self.db.conn();
        conn.execute(
            "INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES (?,?,?,?,?)",
            params![verdict_id, task_id, verdict, note, at],
        )?;
        if !conn.is_autocommit() {
            conn.execute_batch("COMMIT")?;
        }
        Ok(())
    }

    /// `mark_shown` (`task_results.py:75-76`).
    pub fn mark_shown(&self, task_ids: &[&str]) -> Result<()> {
        let mut statement = self
            .db
            .conn()
            .prepare("UPDATE task_results SET shown = 1 WHERE task_id = ?")?;
        for task_id in task_ids {
            statement.execute([task_id])?;
        }
        Ok(())
    }
}
