"""Own which chats, requests and tasks belong to a test run (GATE-550): the tag `test_run` {run, connection} on their
chat message, which the developer run tool (scripts/question_run.py) sets; it changes no question or send. Code keeps
those records out of learning (memory captures, episode cards, summary folds, the daily distill) and out of other
runs' context (the activity trail). Must not judge content: the tag is structure."""

import json
import weakref
from typing import Any

_BY_DB: "weakref.WeakKeyDictionary[Any, TestRuns]" = weakref.WeakKeyDictionary()


class TestRuns:
    """One per database (`of`), so every reader sees a request as soon as its message is tagged."""

    def __init__(self, db: Any) -> None:
        self.db = db
        self._sessions: set[str] | None = None
        self._requests: set[str] = set()
        self._tasks: dict[str, bool] = {}

    @classmethod
    def of(cls, db: Any) -> "TestRuns":
        found = _BY_DB.get(db)
        if found is None:
            found = _BY_DB[db] = cls(db)
        return found

    def _load(self) -> None:
        if self._sessions is not None:
            return
        self._sessions = set()
        for row in self.db.conn.execute("SELECT data_json FROM journal WHERE kind = 'chat.message' AND "
                                        "json_extract(data_json, '$.test_run') IS NOT NULL"):
            data = json.loads(row[0])
            self._sessions.add(str(data.get("session_id") or ""))
            self._requests.add(str(data.get("request_id") or ""))

    def add(self, session_id: str, request_id: str) -> None:
        self._load()
        assert self._sessions is not None
        self._sessions.add(session_id)
        self._requests.add(request_id)
        self._tasks = {k: v for k, v in self._tasks.items() if v}      # a cached "not a test task" may be stale now

    def session(self, session_id: str) -> bool:
        self._load()
        return bool(session_id) and session_id in (self._sessions or set())

    def request(self, request_id: str | None) -> bool:
        self._load()
        return bool(request_id) and request_id in self._requests

    def task(self, task_id: str | None) -> bool:
        """Whether a task (a tree's root, step or worker id) belongs to a test run's request."""
        if not task_id:
            return False
        self._load()
        if not self._requests:
            return False
        if task_id not in self._tasks:
            row = self.db.conn.execute("SELECT request_id FROM tasks WHERE id = ?", (task_id,)).fetchone()
            if row is None and "." in task_id:
                row = self.db.conn.execute("SELECT request_id FROM tasks WHERE id = ?",
                                           (task_id.split(".", 1)[0],)).fetchone()
            self._tasks[task_id] = row is not None and str(row[0]) in self._requests
        return self._tasks[task_id]
