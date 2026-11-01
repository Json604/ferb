"""Owns registered tasks and request execution state. Must not infer intent, choose a target or claim semantic success from transport facts.

Task tree: what's running, for the gateway (request id, deadline, span, role, tree) and,
persisted in SQLite, for `ferb trace` and restarts: a daemon that restarts marks subtasks that were running as
`interrupted` (their consequential actions are already `unknown` in the ledger and get reconciled)."""

import json
from collections.abc import Callable
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


TASK_COLUMNS = frozenset({"request_id", "created_at", "updated_at", "goal", "status", "tree_id", "outcome_json",
                          "parent_id"})


# What a task row's status can be (migration 0049). `paused`: the task waits for the owner in place (ORCH-308);
# `interrupted`: FERB restarted while it ran and it didn't resume.
STATUSES = ("planned", "running", "paused", "done", "done_unverified", "failed", "cancelled", "needs_user",
            "interrupted")


class TaskTree:
    def __init__(self) -> None:
        self._tasks: dict[str, TaskRecord] = {}
        self.db: Database | None = None
        self.now: Callable[[], datetime] = lambda: datetime.now().astimezone()

    def attach(self, db: Database, now: Callable[[], datetime] | None = None) -> None:
        """The database, and the daemon's clock (its `now`): every status time comes from it."""
        self.db = db
        if now is not None:
            self.now = now

    def add(self, rec: TaskRecord, status: str = "running") -> None:
        self._tasks[rec.task.id] = rec
        if self.db is None:
            return
        t, now = rec.task, rec.started_at.isoformat()
        self.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, "
            "status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) "
            "ON CONFLICT(id) DO UPDATE SET request_id=excluded.request_id,goal=excluded.goal,"
            "identity=excluded.identity,role=excluded.role,scopes_json=excluded.scopes_json,"
            "status=excluded.status,updated_at=excluded.updated_at,outcome_json='{}'",
            (t.id, rec.tree_id or t.id, t.parent_id, t.request_id, t.goal, t.identity.value, t.role,
             json.dumps(sorted(t.scopes)), status, now, now))

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
                                              at or self.now().isoformat(), task_id))

    def remove(self, task_id: str) -> None:
        self._tasks.pop(task_id, None)

    def rows(self, tree_id: str) -> list[dict[str, Any]]:
        if self.db is None:
            return []
        return [dict(r) for r in self.db.conn.execute("SELECT * FROM tasks WHERE tree_id = ? ORDER BY created_at",
                                                      (tree_id,)).fetchall()]

    def column(self, task_id: str, name: str) -> Any:
        """One recorded column of a task (request_id, created_at, goal, ...), or None when the task isn't recorded."""
        if self.db is None or name not in TASK_COLUMNS:
            return None
        row = self.db.conn.execute(f"SELECT {name} FROM tasks WHERE id = ?", (task_id,)).fetchone()
        return row[0] if row is not None else None

    def latest_root(self) -> str | None:
        """The newest root task's id."""
        if self.db is None:
            return None
        row = self.db.conn.execute("SELECT id FROM tasks WHERE parent_id IS NULL "
                                   "ORDER BY created_at DESC, rowid DESC LIMIT 1").fetchone()
        return str(row["id"]) if row else None

    def recorded(self, task_id: str) -> bool:
        return self.db is not None and self.db.conn.execute(
            "SELECT 1 FROM tasks WHERE id = ?", (task_id,)).fetchone() is not None

    def roots_with_prefix(self, prefix: str) -> list[str]:
        """Root task ids that start with `prefix`, in id order."""
        if self.db is None:
            return []
        return [str(r["id"]) for r in self.db.conn.execute(
            "SELECT id FROM tasks WHERE substr(id, 1, ?) = ? AND parent_id IS NULL ORDER BY id",
            (len(prefix), prefix))]

    def trees_of_request(self, request_id: str) -> set[str]:
        """The task trees a request started."""
        if self.db is None:
            return set()
        return {row[0] for row in self.db.conn.execute("SELECT DISTINCT tree_id FROM tasks WHERE request_id=?",
                                                       (request_id,))}

    def interrupt_running(self) -> list[str]:
        """After a restart: whatever was running, planned or paused didn't finish (a tree that resumes records its
        rows again as it runs)."""
        if self.db is None:
            return []
        ids = [r["id"] for r in self.db.conn.execute(
            "SELECT id FROM tasks WHERE status IN ('running', 'planned', 'paused')").fetchall()]
        self.db.conn.execute("UPDATE tasks SET status = 'interrupted', updated_at = ? "
                             "WHERE status IN ('running', 'planned', 'paused')", (self.now().isoformat(),))
        return ids
