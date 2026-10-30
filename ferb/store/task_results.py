"""Own what FERB told (or will tell) the owner about each task (`task_results`) and which of those a chat has shown.
Must not mark a result shown unless a client says it showed it (`mark_shown`)."""

from typing import Any

from ferb.store.db import Database


class TaskResults:
    def __init__(self, db: Database) -> None:
        self.db = db

    def record(self, task_id: str, at: str, status: str, goal: str, answer: str, shown: bool) -> None:
        """A task's latest result (a later one replaces it)."""
        self.db.conn.execute("INSERT OR REPLACE INTO task_results (task_id, at, status, goal, answer, shown) VALUES "
                             "(?, ?, ?, ?, ?, ?)", (task_id, at, status, goal, answer, int(shown)))

    def interrupted(self, task_id: str, at: str, goal: str, answer: str) -> None:
        """A task a restart cut off, told once (a result already there stays)."""
        self.db.conn.execute("INSERT OR IGNORE INTO task_results (task_id, at, status, goal, answer, shown) "
                             "VALUES (?, ?, 'interrupted', ?, ?, 0)", (task_id, at, goal, answer))

    def unshown(self) -> list[Any]:
        """Results no chat has shown yet, oldest first, with their task's recorded outcome and request."""
        return self.db.conn.execute("SELECT r.*, t.outcome_json, t.request_id FROM task_results r LEFT JOIN tasks t "
                                    "ON t.id = r.task_id WHERE r.shown = 0 ORDER BY r.at").fetchall()

    def mark_shown(self, task_ids: list[str]) -> None:
        self.db.conn.executemany("UPDATE task_results SET shown = 1 WHERE task_id = ?", [(t,) for t in task_ids])
