"""Span writer (every input/output passes the redaction seam) and the span tree query for `ferb trace`."""

import json
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Callable

from ferb.core.interfaces import Clock, IdSource
from ferb.core.types import Span
from ferb.store.db import Database

Redact = Callable[[dict[str, Any]], dict[str, Any]]


@dataclass
class SpanNode:
    span: Span
    children: list["SpanNode"] = field(default_factory=list)


class SpanStore:
    def __init__(self, db: Database, clock: Clock, ids: IdSource, redact: Redact) -> None:
        self._db, self._clock, self._ids, self._redact = db, clock, ids, redact

    def start(self, kind: str, *, input: dict[str, Any], task_id: str | None = None,
              parent_id: str | None = None, device_id: str | None = None) -> Span:
        span = Span(id=self._ids.new_id(), parent_id=parent_id, kind=kind, task_id=task_id, device_id=device_id,
                    started_at=self._clock.now(), ended_at=None, input=self._redact(input), output=None,
                    cost_usd=None, error=None)
        self._db.conn.execute(
            "INSERT INTO spans (id, parent_id, task_id, kind, device_id, started_at, input_json)"
            " VALUES (?, ?, ?, ?, ?, ?, ?)",
            (span.id, parent_id, task_id, kind, device_id, span.started_at.isoformat(), json.dumps(span.input)))
        return span

    def finish(self, span: Span, *, output: dict[str, Any] | None = None, error: str | None = None,
               cost_usd: float | None = None) -> Span:
        span.ended_at = self._clock.now()
        span.output = None if output is None else self._redact(output)
        span.error = None if error is None else self._redact({"error": error})["error"]
        span.cost_usd = cost_usd
        self._db.conn.execute(
            "UPDATE spans SET ended_at = ?, output_json = ?, error = ?, cost_usd = ? WHERE id = ?",
            (span.ended_at.isoformat(), None if span.output is None else json.dumps(span.output), span.error,
             cost_usd, span.id))
        return span

    def tree(self, task_id: str) -> list[SpanNode]:
        """All spans of a task and its steps (<id>.<step>…) as a forest (roots = spans whose parent isn't among
        them), in start order."""
        rows = self._db.conn.execute("SELECT * FROM spans WHERE task_id = ? OR task_id LIKE ? "
                                     "ORDER BY started_at, rowid", (task_id, f"{task_id}.%")).fetchall()
        nodes = {r["id"]: SpanNode(_row_to_span(r)) for r in rows}
        roots = []
        for node in nodes.values():
            parent = nodes.get(node.span.parent_id or "")
            (parent.children if parent else roots).append(node)
        return roots


def _row_to_span(r: Any) -> Span:
    return Span(id=r["id"], parent_id=r["parent_id"], kind=r["kind"], task_id=r["task_id"], device_id=r["device_id"],
                started_at=datetime.fromisoformat(r["started_at"]),
                ended_at=datetime.fromisoformat(r["ended_at"]) if r["ended_at"] else None,
                input=json.loads(r["input_json"]),
                output=json.loads(r["output_json"]) if r["output_json"] else None,
                cost_usd=r["cost_usd"], error=r["error"])
