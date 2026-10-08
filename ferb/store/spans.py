"""Own redacted span persistence, evidence reads and task exports. Must not invent missing observations."""

import json
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Callable

from ferb.core import origin
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
               cost_usd: float | None = None, origin: str | None = None) -> Span:
        span.ended_at = self._clock.now()
        span.output = None if output is None else self._redact(output)
        span.error = None if error is None else self._redact({"error": error})["error"]
        span.cost_usd = cost_usd
        if origin is not None:                       # no origin given: keep one recorded earlier (set_origin)
            span.origin = self._redact({"origin": origin})["origin"]
        self._db.conn.execute(
            "UPDATE spans SET ended_at = ?, output_json = ?, error = ?, cost_usd = ?, origin = COALESCE(?, origin) "
            "WHERE id = ?",
            (span.ended_at.isoformat(), None if span.output is None else json.dumps(span.output), span.error,
             cost_usd, None if origin is None else span.origin, span.id))
        return span

    def set_origin(self, span_id: str, origin: str) -> None:
        """Record where a span's content came from before it finishes (e.g. a task's request); redacted like finish."""
        self._db.conn.execute("UPDATE spans SET origin = ? WHERE id = ?",
                              (self._redact({"origin": origin})["origin"], span_id))

    def tree(self, task_id: str) -> list[SpanNode]:
        """All spans of a task and its steps (<id>.<step>…) as a forest (roots = spans whose parent isn't among
        them), in start order."""
        rows = self._task_rows(task_id)
        nodes = {r["id"]: SpanNode(_row_to_span(r)) for r in rows}
        roots = []
        for node in nodes.values():
            parent = nodes.get(node.span.parent_id or "")
            (parent.children if parent else roots).append(node)
        return roots

    def _tree_where(self, root_id: str) -> tuple[str, tuple[Any, ...]]:
        # A range, not substr(): it can use the task_id index. "/" is the character right after ".".
        return "(task_id = ? OR (task_id >= ? AND task_id < ?))", (root_id, root_id + ".", root_id + "/")

    def tool_origins(self, root_id: str) -> set[str]:
        """Every origin recorded on this task tree's tool results."""
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(f"SELECT origin FROM spans WHERE kind = 'tool.call' AND {where}", params)
        return {o for r in rows for o in origin.split(r["origin"])}

    def task_origins(self, root_id: str) -> set[str]:
        """Every origin recorded on this task tree's request (its root task span), tool results and context
        sources: what its models were shown. Labels derived from the task's work (worker reports, answers) inherit
        these (MP2)."""
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(
            f"SELECT origin FROM spans WHERE kind IN ('task', 'tool.call', 'context.source') AND {where}", params)
        return {o for r in rows for o in origin.split(r["origin"])}

    def tool_outputs(self, root_id: str) -> list[tuple[set[str], str]]:
        """(origins, text) of this task tree's finished tool calls, in call order. The text is every string value of
        the decoded output (numbers as strings) joined by newlines, capped at 20000 characters; past the cap stays
        unsearched."""
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(f"SELECT origin, output_json FROM spans WHERE kind = 'tool.call' AND {where} "
                                     "AND output_json IS NOT NULL ORDER BY rowid", params)
        return [(origin.split(r["origin"]), "\n".join(_strings(json.loads(r["output_json"])))[:20000])
                for r in rows]

    def owner_answers(self, root_id: str) -> list[str]:
        """The owner's answers to owner.ask in this task tree: only spans whose input names that tool and whose
        origin is exactly the owner. Nothing else the owner's name is on (memory, history) counts."""
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(
            f"SELECT output_json FROM spans WHERE kind = 'tool.call' AND origin = ? AND output_json IS NOT NULL "
            f"AND json_extract(input_json, '$.tool') = 'owner.ask' AND {where} ORDER BY rowid", (origin.OWNER, *params))
        answers = [json.loads(r["output_json"]) for r in rows]
        return [a["answer"] for a in answers if isinstance(a, dict) and isinstance(a.get("answer"), str)]

    def _task_rows(self, task_id: str) -> list[Any]:
        """Include the intake planner by exact request ID; it ran before the task ID existed."""
        root = self._db.conn.execute("SELECT request_id FROM tasks WHERE id = ?", (task_id,)).fetchone()
        request_id = root[0] if root else None
        return self._db.conn.execute(
            "SELECT * FROM spans WHERE task_id = ? OR substr(task_id, 1, ?) = ? "
            "OR (task_id IS NULL AND json_extract(input_json, '$.request_id') = ?) ORDER BY started_at, rowid",
            (task_id, len(task_id) + 1, task_id + '.', request_id)).fetchall()

    def outcome(self, task_id: str) -> dict[str, Any]:
        """Export one redacted evidence object. Missing historical facts stay explicit, never reconstructed guesses."""
        events = [_span_record(_row_to_span(row)) for row in self._task_rows(task_id)]
        roots = [e for e in events if e['kind'] == 'task' and e['task_id'] == task_id]
        root = roots[0] if len(roots) == 1 else {}
        source = root.get('input', {})
        tasks = []
        for row in self._db.conn.execute("SELECT * FROM tasks WHERE tree_id = ? ORDER BY created_at, id", (task_id,)):
            task = dict(row)
            for name in ('scopes', 'depends', 'outcome'):
                task[name] = json.loads(task.pop(name + '_json'))
            tasks.append(task)
        request_id = source.get('request_id') or (tasks[0]['request_id'] if tasks else None)
        messages = []
        if request_id:
            messages = [{'id': r['id'], 'at': r['at'], 'data': json.loads(r['data_json'])}
                        for r in self._db.conn.execute(
                            "SELECT * FROM journal WHERE kind = 'chat.message' "
                            "AND json_extract(data_json, '$.request_id') = ? ORDER BY seq", (request_id,))]
        outcomes = [json.loads(r[0]) for r in self._db.conn.execute(
            "SELECT data_json FROM journal WHERE kind IN ('task.outcome', 'task.answer') "
            "AND json_extract(data_json, '$.task_id') = ? ORDER BY seq", (task_id,))]
        verdict_row = self._db.conn.execute(
            "SELECT verdict, note, at FROM owner_verdicts WHERE task_id = ? ORDER BY seq DESC LIMIT 1",
            (task_id,)).fetchone()
        owner_verdict = ({'verdict': verdict_row['verdict'], 'note': verdict_row['note'], 'at': verdict_row['at']}
                         if verdict_row else None)
        return {'schema_version': 1, 'task_id': task_id,
                'request': {'id': request_id, 'text': source.get('request'),
                            'authority_words': source.get('authority_words'), 'messages': messages},
                'task_spec': source.get('task_spec'), 'tasks': tasks,
                'planner_decisions': [e for e in events if e['kind'] == 'model.turn'
                                      and e['input'].get('role') == 'supervisor'],
                'approach_decisions': [e for e in events if e['kind'].startswith('model.')],
                'context_sources': [e for e in events if e['kind'] == 'context.source'],
                'worker_runs': [e for e in events if e['kind'] == 'worker'],
                'runtime_tools': [e for e in events if e['kind'] == 'runtime.tools'],
                'intake': [e for e in events if e['kind'] == 'task.intake'],
                'actions': [e for e in events if e['kind'] == 'tool.call'],
                'verification': [e for e in events if e['kind'] in ('verification', 'verify')
                                 or (e['task_id'] or '').endswith('.verify')],
                'disturbances': [e for e in events if e['kind'].startswith('disturbance.')],
                'cleanup': [e for e in events if e['kind'] == 'task.cleanup'],
                'result': root.get('output'), 'outcome_records': outcomes, 'owner_verdict': owner_verdict,
                'coverage': {'full_request_recorded': 'request' in source,
                             'sentinel_recorded': any(e['kind'] == 'disturbance.summary' for e in events),
                             'owner_verdict': 'recorded' if owner_verdict else 'no owner verdict collected; null is not acceptance'}}

    def update_input(self, span_id: str, values: dict[str, Any]) -> None:
        """Add measured runtime input after tokens/tools/time are assembled; redact before persistence."""
        row = self._db.conn.execute("SELECT input_json FROM spans WHERE id = ?", (span_id,)).fetchone()
        if row is None:
            raise ValueError("unknown span")
        data = json.loads(row[0]) | self._redact(values)
        self._db.conn.execute("UPDATE spans SET input_json = ? WHERE id = ?", (json.dumps(data), span_id))

    @staticmethod
    def _evidence_where(task_id: str, root_id: str | None) -> tuple[str, list[Any]]:
        # Root identity is resolved from the registered live task, never inferred from a name/prefix.
        where = "(task_id = ? AND kind = 'tool.call' AND json_extract(input_json, '$.tool') NOT IN " \
                "('task.evidence','chat.history','memory.read'))"
        params: list[Any] = [task_id]
        if root_id:
            where += " OR (task_id = ? AND kind IN ('task','task.intake','context.source','disturbance.baseline'," \
                     "'disturbance.change','disturbance.error','disturbance.summary'))"
            params.append(root_id)
            where += " OR (task_id IN (?, ?) AND kind IN ('worker','task.cleanup'))"
            params.extend([task_id, task_id + ".verify"])
        return "(" + where + ")", params

    def tool_call(self, task_id: str, span_id: str, root_id: str | None = None) -> dict[str, Any] | None:
        """An authorized recorded fact/report; never provider reasoning or recursive evidence copies."""
        where, params = self._evidence_where(task_id, root_id)
        row = self._db.conn.execute("SELECT * FROM spans WHERE " + where + " AND id = ?",
                                    (*params, span_id)).fetchone()
        if row is None:
            return None
        return {"id": row["id"], "kind": row["kind"], "task_id": row["task_id"],
                "started_at": row["started_at"], "ended_at": row["ended_at"],
                "input": json.loads(row["input_json"]),
                "output": json.loads(row["output_json"]) if row["output_json"] else None, "error": row["error"],
                "origin": row["origin"]}

    def tool_calls(self, task_id: str, query: str = '', after: int = 0, limit: int = 20,
                   root_id: str | None = None, kind: str = '') -> dict[str, Any]:
        """Search full redacted observations using literal text; page references rather than dropping records."""
        if limit < 1 or after < 0:
            raise ValueError('limit must be positive and after nonnegative')
        where, params = self._evidence_where(task_id, root_id)
        if query:
            where += " AND instr(lower(kind || input_json || coalesce(output_json, '') || coalesce(error, '')), lower(?)) > 0"
            params.append(query)
        if kind:
            where += " AND kind = ?"
            params.append(kind)
        rows = self._db.conn.execute("SELECT rowid AS cursor, * FROM spans WHERE " + where +
                                     " AND rowid > ? ORDER BY rowid LIMIT ?", (*params, after, limit + 1)).fetchall()
        records = [{"id": row["id"], "kind": row["kind"], "task_id": row["task_id"],
                    "tool": json.loads(row["input_json"]).get('tool'),
                    "started_at": row["started_at"], "ended_at": row["ended_at"], "error": row["error"],
                    "input_preview": row["input_json"][:500], "input_truncated": len(row["input_json"]) > 500,
                    "read_full": {"tool": "task.evidence", "span_id": row["id"]},
                    "has_output": row["output_json"] is not None, "origin": row["origin"]} for row in rows[:limit]]
        return {"records": records, "next_after": rows[limit - 1]["cursor"] if len(rows) > limit else None}


def _row_to_span(r: Any) -> Span:
    return Span(id=r["id"], parent_id=r["parent_id"], kind=r["kind"], task_id=r["task_id"], device_id=r["device_id"],
                started_at=datetime.fromisoformat(r["started_at"]),
                ended_at=datetime.fromisoformat(r["ended_at"]) if r["ended_at"] else None,
                input=json.loads(r["input_json"]),
                output=json.loads(r["output_json"]) if r["output_json"] else None,
                cost_usd=r["cost_usd"], error=r["error"], origin=r["origin"])


def _span_record(span: Span) -> dict[str, Any]:
    return {'id': span.id, 'parent_id': span.parent_id, 'task_id': span.task_id, 'kind': span.kind,
            'started_at': span.started_at.isoformat(),
            'ended_at': span.ended_at.isoformat() if span.ended_at else None,
            'input': span.input, 'output': span.output, 'error': span.error, 'cost_usd': span.cost_usd,
            'origin': span.origin}


def _strings(value: Any) -> list[str]:
    """Every string (and number as a string) inside decoded JSON, in order."""
    if isinstance(value, str):
        return [value]
    if isinstance(value, bool) or value is None:
        return []
    if isinstance(value, (int, float)):
        return [str(value)]
    items = value.values() if isinstance(value, dict) else value if isinstance(value, list) else []
    return [s for v in items for s in _strings(v)]
