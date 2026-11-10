"""Own what FERB told (or will tell) the owner about each task (`task_results`) and which of those a chat has shown.
Must not mark a result shown unless a client says it showed it (`mark_shown`)."""

import json
from typing import Any

from ferb.store.db import Database


# A task's short name (T-1055): the one its result was told with (`task_result_names` n), else its spec's (task_context
# `x`); '' for a task saved before names
_NAME = "COALESCE(NULLIF(n.name, ''), json_extract(x.spec_json, '$.name'), '')"


class TaskResults:
    def __init__(self, db: Database) -> None:
        self.db = db

    def record(self, task_id: str, at: str, status: str, goal: str, answer: str, shown: bool,
               cards: list[dict[str, Any]] | None = None, name: str = "") -> None:
        """A task's latest result (a later one replaces it, its cards too), with its typed cards (step 8e) and the
        short name it was told with (T-1055)."""
        self.db.conn.execute("INSERT OR REPLACE INTO task_results (task_id, at, status, goal, answer, shown) VALUES "
                             "(?, ?, ?, ?, ?, ?)", (task_id, at, status, goal, answer, int(shown)))
        if name.strip():
            self.db.conn.execute("INSERT OR REPLACE INTO task_result_names (task_id, name) VALUES (?, ?)",
                                 (task_id, " ".join(name.split())))
        if cards:
            self.db.conn.execute("INSERT OR REPLACE INTO task_result_cards (task_id, cards_json) VALUES (?, ?)",
                                 (task_id, json.dumps(cards)))
        else:
            self.db.conn.execute("DELETE FROM task_result_cards WHERE task_id = ?", (task_id,))

    def interrupted(self, task_id: str, at: str, goal: str, answer: str) -> None:
        """A task a restart cut off, told once (a result already there stays)."""
        self.db.conn.execute("INSERT OR IGNORE INTO task_results (task_id, at, status, goal, answer, shown) "
                             "VALUES (?, ?, 'interrupted', ?, ?, 0)", (task_id, at, goal, answer))

    def unshown(self) -> list[Any]:
        """Results no chat has shown yet, oldest first, with their task's recorded outcome and request, and their cards
        (`cards_json`, NULL without any)."""
        return self.db.conn.execute("SELECT r.*, t.outcome_json, t.request_id, c.cards_json, "
                                    f"{_NAME} AS name FROM task_results r "
                                    "LEFT JOIN tasks t ON t.id = r.task_id LEFT JOIN task_result_cards c "
                                    "ON c.task_id = r.task_id LEFT JOIN task_context x ON x.task_id = r.task_id "
                                    "LEFT JOIN task_result_names n ON n.task_id = r.task_id "
                                    "WHERE r.shown = 0 ORDER BY r.at").fetchall()

    def page(self, before: str, limit: int) -> list[Any]:
        """Root tasks created before `before`, newest first, at most `limit`, with their result's answer and the
        owner's latest verdict."""
        return self.db.conn.execute(
            f"SELECT t.id, t.goal, t.status, t.created_at, t.updated_at, t.outcome_json, r.answer, {_NAME} AS name, "
            "(SELECT v.verdict FROM owner_verdicts v WHERE v.task_id = t.id ORDER BY v.seq DESC LIMIT 1) AS verdict "
            "FROM tasks t LEFT JOIN task_results r ON r.task_id = t.id LEFT JOIN task_context x ON x.task_id = t.id "
            "LEFT JOIN task_result_names n ON n.task_id = t.id "
            "WHERE t.parent_id IS NULL AND t.created_at < ? ORDER BY t.created_at DESC, t.rowid DESC LIMIT ?",
            (before, limit)).fetchall()

    def latest_of_schedule(self, schedule_id: str) -> Any:
        """The latest result of a root task a `schedule.run` of this schedule started, or None."""
        return self.db.conn.execute(
            "SELECT r.at, r.status, r.answer, r.task_id FROM task_results r JOIN tasks t ON t.id = r.task_id "
            "WHERE t.parent_id IS NULL AND t.request_id IN (SELECT json_extract(data_json, '$.request_id') FROM journal "
            "WHERE kind = 'schedule.run' AND json_extract(data_json, '$.schedule_id') = ?) ORDER BY r.at DESC LIMIT 1",
            (schedule_id,)).fetchone()

    def record_verdict(self, verdict_id: str, task_id: str, verdict: str, note: str, at: str) -> None:
        """The owner's pass/fail verdict on a root task (acceptance is the owner's word)."""
        self.db.conn.execute("INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES (?,?,?,?,?)",
                             (verdict_id, task_id, verdict, note, at))
        self.db.conn.commit()

    def mark_shown(self, task_ids: list[str]) -> None:
        self.db.conn.executemany("UPDATE task_results SET shown = 1 WHERE task_id = ?", [(t,) for t in task_ids])
