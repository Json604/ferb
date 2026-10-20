"""Owns registered tasks and request execution state. Must not infer intent, choose a target or claim semantic success from transport facts.

Task tree: what's running, for the gateway (request id, deadline, span, role, tree) and,
persisted in SQLite, for `ferb trace` and restarts: a daemon that restarts marks subtasks that were running as
`interrupted` (their consequential actions are already `unknown` in the ledger and get reconciled)."""

import json
from dataclasses import dataclass
from datetime import datetime
from typing import Any

from ferb.core.types import Task
from ferb.store.db import Database
from ferb.daemon.questions import Approvals
from ferb.policy.targets import Requests


class ExecutionState:
    """Live request context, waiting owner input and explicit stop state."""

    def __init__(self) -> None:
        self.requests = Requests()
        self.approvals = Approvals()
        self.stopped = False

    def forget_request(self, request_id: str) -> None:
        self.requests.forget(request_id)
        self.approvals.forget_request(request_id)


@dataclass
class TaskRecord:
    task: Task
    started_at: datetime
    span_id: str                   # the task's root span: tool spans hang under it
    tree_id: str | None = None     # the request's root task (None: the task is its own tree)


# A conversation's request trees recorded since a rowid floor (a chat session's start): the tasks above the floor by
# the rowid key, each matched to its conversation by its key; CROSS JOIN keeps that order, so a turn's work is the
# tasks recorded since the session started, never the task table or the conversation's whole history (R4).
CONVERSATION_ROOTS = ("SELECT t.* FROM tasks t CROSS JOIN task_context c ON c.task_id = t.id "
                      "WHERE t.rowid > ? AND c.conversation_id = ? AND t.parent_id IS NULL ORDER BY t.created_at")


class TaskTree:
    def __init__(self) -> None:
        self._tasks: dict[str, TaskRecord] = {}
        self.db: Database | None = None

    def attach(self, db: Database) -> None:
        self.db = db

    def add(self, rec: TaskRecord, depends_on: list[str] | None = None, status: str = "running") -> None:
        self._tasks[rec.task.id] = rec
        if self.db is None:
            return
        t, now = rec.task, rec.started_at.isoformat()
        self.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, "
            "depends_json, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) "
            "ON CONFLICT(id) DO UPDATE SET request_id=excluded.request_id,goal=excluded.goal,"
            "identity=excluded.identity,role=excluded.role,scopes_json=excluded.scopes_json,"
            "depends_json=excluded.depends_json,status=excluded.status,updated_at=excluded.updated_at,"
            "outcome_json='{}'",
            (t.id, rec.tree_id or t.id, t.parent_id, t.request_id, t.goal, t.identity.value, t.role,
             json.dumps(sorted(t.scopes)), json.dumps(depends_on or []), status, now, now))

    def get(self, task_id: str) -> TaskRecord | None:
        return self._tasks.get(task_id)

    def tree_of(self, task_id: str) -> str | None:
        """The root of a task's tree (S4: a per-tree sink decision). From the live record if present, else the
        persisted `tasks.tree_id`, so a background process judged after its tree ended still gets the full tree; None
        when the task is unknown."""
        rec = self._tasks.get(task_id)
        if rec is not None:
            return rec.tree_id or task_id
        if self.db is None:
            return None
        row = self.db.conn.execute("SELECT tree_id FROM tasks WHERE id = ?", (task_id,)).fetchone()
        return row["tree_id"] if row is not None else None

    def set_span(self, task_id: str, span_id: str) -> None:
        self._tasks[task_id].span_id = span_id

    def set_status(self, task_id: str, status: str, outcome: dict[str, Any] | None = None, at: str = "") -> None:
        if self.db is None:
            return
        self.db.conn.execute("UPDATE tasks SET status = ?, outcome_json = COALESCE(?, outcome_json), updated_at = ? "
                             "WHERE id = ?", (status, json.dumps(outcome) if outcome is not None else None,
                                              at or datetime.now().astimezone().isoformat(), task_id))

    def remove(self, task_id: str) -> None:
        self._tasks.pop(task_id, None)

    def rows(self, tree_id: str) -> list[dict[str, Any]]:
        if self.db is None:
            return []
        return [dict(r) for r in self.db.conn.execute("SELECT * FROM tasks WHERE tree_id = ? ORDER BY created_at",
                                                      (tree_id,)).fetchall()]

    def conversation_roots(self, conversation: str, floor: int) -> list[dict[str, Any]]:
        """A conversation's request trees recorded above the rowid floor (the planner's task status facts)."""
        if self.db is None:
            return []
        return [dict(r) for r in self.db.conn.execute(CONVERSATION_ROOTS, (floor, conversation))]

    def max_rowid(self) -> int:
        """The last task row recorded so far: a chat session's floor (the rowid key's end, no scan)."""
        if self.db is None:
            return 0
        return int(self.db.conn.execute("SELECT COALESCE(MAX(rowid), 0) FROM tasks").fetchone()[0])

    def recent_roots(self, since_iso: str) -> list[dict[str, Any]]:
        """Requests' task trees updated since then (continuity, 14a)."""
        if self.db is None:
            return []
        return [dict(r) for r in self.db.conn.execute(
            "SELECT * FROM tasks WHERE parent_id IS NULL AND updated_at >= ? ORDER BY created_at", (since_iso,))]

    def interrupt_running(self, now: datetime | None = None) -> list[str]:
        """After a restart: whatever was running or planned didn't finish."""
        if self.db is None:
            return []
        ids = [r["id"] for r in self.db.conn.execute(
            "SELECT id FROM tasks WHERE status IN ('running', 'planned')").fetchall()]
        self.db.conn.execute("UPDATE tasks SET status = 'interrupted', updated_at = ? "
                             "WHERE status IN ('running', 'planned')", ((now or datetime.now().astimezone()).isoformat(),))
        return ids
