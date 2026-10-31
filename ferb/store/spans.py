"""Own redacted span persistence, evidence reads and task exports. Must not invent missing observations."""

import json
from dataclasses import asdict, dataclass, field
from datetime import datetime
from typing import Any, Callable, Iterator

from ferb.core import origin
from ferb.core.interfaces import Clock, IdSource
from ferb.core.types import Span
from ferb.policy.rules import REVERSIBLE
from ferb.store.db import Database, swap

Redact = Callable[[dict[str, Any]], dict[str, Any]]
OUTPUT_TEXT_CAP = 20000          # characters of each tool result matched for argument sources (S3)
# origin.recorded in SQL: a non-root span's output with no recorded origin is unlabeled (the one parameter).
_RECORDED = "CASE WHEN kind != 'task' AND origin IS NULL AND output_json IS NOT NULL THEN ? ELSE origin END"


def _dumps(value: Any) -> str:
    """A span's stored JSON: text as written (UTF-8), not as \\uXXXX escapes, so a search for non-ASCII words finds it
    (step 12a; rows from before hold escapes, and the search matches both forms)."""
    return json.dumps(value, ensure_ascii=False)


def tree_where(root_id: str) -> tuple[str, tuple[Any, ...]]:
    """SQL and parameters selecting a task tree's spans. A range, not substr(): it can use the task_id index.
    "/" is the character right after "."."""
    return "(task_id = ? OR (task_id >= ? AND task_id < ?))", (root_id, root_id + ".", root_id + "/")


@dataclass
class SpanNode:
    span: Span
    children: list["SpanNode"] = field(default_factory=list)


@dataclass
class PayloadOutput:
    rowid: int
    origins: set[str]
    text: str
    path: str = ""
    offset: int = 0
    next_offset: int | None = None


class SpanStore:
    def __init__(self, db: Database, clock: Clock, ids: IdSource, redact: Redact) -> None:
        self._db, self._clock, self._ids, self._redact = db, clock, ids, redact

    @property
    def db(self) -> Database:
        """The database the spans are kept in; records kept beside them (the journal) share it."""
        return self._db

    def start(self, kind: str, *, input: dict[str, Any], task_id: str | None = None,
              parent_id: str | None = None, device_id: str | None = None) -> Span:
        span = Span(id=self._ids.new_id(), parent_id=parent_id, kind=kind, task_id=task_id, device_id=device_id,
                    started_at=self._clock.now(), ended_at=None, input=self._redact_input(input), output=None,
                    cost_usd=None, error=None)
        self._db.conn.execute(
            "INSERT INTO spans (id, parent_id, task_id, kind, device_id, started_at, input_json)"
            " VALUES (?, ?, ?, ?, ?, ?, ?)",
            (span.id, parent_id, task_id, kind, device_id, span.started_at.isoformat(), _dumps(span.input)))
        return span

    def _redact_input(self, input: dict[str, Any]) -> dict[str, Any]:
        """Redacted; FERB's own ids in it (source_refs, task ids) are kept whole by key (`Redactor.ident`, L16/L28)."""
        return self._redact(input)

    def finish(self, span: Span, *, output: dict[str, Any] | None = None, error: str | None = None,
               cost_usd: float | None = None, origin: str | None = None,
               summary: str | None = None) -> Span:
        span.ended_at = self._clock.now()
        span.output = None if output is None else self._redact(output)
        span.error = None if error is None else self._redact({"error": error})["error"]
        span.cost_usd = cost_usd
        if origin is not None:                       # no origin given: keep one recorded earlier (set_origin)
            span.origin = self._redact({"origin": origin})["origin"]
        if summary is not None:
            span.summary = self._redact({"summary": summary})["summary"]
        self._db.conn.execute(
            "UPDATE spans SET ended_at = ?, output_json = ?, error = ?, cost_usd = ?, origin = COALESCE(?, origin), "
            "summary = COALESCE(?, summary) WHERE id = ?",
            (span.ended_at.isoformat(), None if span.output is None else _dumps(span.output), span.error,
             cost_usd, None if origin is None else span.origin, span.summary, span.id))
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
        roots: list[SpanNode] = []
        for node in nodes.values():
            parent = nodes.get(node.span.parent_id or "")
            (parent.children if parent else roots).append(node)
        return roots

    def _tree_where(self, root_id: str) -> tuple[str, tuple[Any, ...]]:
        return tree_where(root_id)

    def tool_origins(self, root_id: str) -> set[str]:
        """Every origin recorded on this task tree's tool results (`origin.recorded`: a result with no recorded origin
        is unlabeled)."""
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(f"SELECT {_RECORDED} AS origin FROM spans WHERE kind = 'tool.call' AND {where}",
                                     (origin.external("unlabeled"), *params))
        return {o for r in rows for o in origin.split(r["origin"])}

    def request_origins(self, root_id: str) -> set[str]:
        """The origins of the request itself (S3): the owner's message and what it carried (pasted text, the earlier
        messages a clarify continues, a scheduled request's turn, the request a retry continues), as the supervisor
        recorded them on the tree's root task span (`input.request_origins`). Planner blocks and memory are not in it.
        A root recorded without them (before F5a) gives the root's own origin, everything its planning turn saw:
        fail-closed."""
        rows = self._db.conn.execute("SELECT input_json, origin FROM spans WHERE kind = 'task' AND task_id = ? "
                                     "ORDER BY rowid", (root_id,))
        found: set[str] = set()
        for row in rows:
            recorded = json.loads(row["input_json"]).get("request_origins")
            found |= {str(o) for o in recorded if o} if isinstance(recorded, list) else origin.split(row['origin'])
        return found

    def request_began(self, root_id: str, request_id: str, worker_id: str) -> tuple[str, int] | None:
        """When this tree's run for `request_id` began (its first root task span) and how many runs of its worker
        (`worker_id`) came before it, but only when an earlier message's run of the same tree did (a continue or retry
        carried the task on in place): None for a tree with one message, however many times its run resumed (a
        restart keeps the request). Counted in record order, not by time."""
        rows = self._db.conn.execute("SELECT rowid, started_at, input_json FROM spans WHERE kind = 'task' "
                                     "AND task_id = ? AND parent_id IS NULL ORDER BY rowid", (root_id,)).fetchall()
        mine = [r for r in rows if json.loads(r["input_json"]).get("request_id") == request_id]
        if not mine or len(mine) == len(rows):
            return None
        earlier = self._db.conn.execute("SELECT COUNT(*) FROM spans WHERE kind = 'worker' AND task_id = ? AND rowid < ?",
                                        (worker_id, mine[0]["rowid"])).fetchone()[0]
        return str(mine[0]["started_at"]), int(earlier)

    def asked_origins(self, root_id: str) -> set[str]:
        """The outside origins of FERB's earlier questions this tree's requests answered (S4 b), as the supervisor
        recorded them on each attempt's root task span (`input.asked_origins`). A root recorded without them adds
        none."""
        rows = self._db.conn.execute("SELECT input_json FROM spans WHERE kind = 'task' AND task_id = ?", (root_id,))
        found: set[str] = set()
        for row in rows:
            recorded = json.loads(row["input_json"]).get("asked_origins")
            found |= {str(o) for o in recorded if o} if isinstance(recorded, list) else set()
        return found

    def reopened_answers(self, root_id: str) -> set[str]:
        """The ids of the owner's No answers in this tree that the owner's own later message carrying the task on
        reopened (`confirmations.reopened` spans, Gateway.reopen_noes)."""
        rows = self._db.conn.execute("SELECT input_json FROM spans WHERE kind = 'confirmations.reopened' "
                                     "AND task_id = ?", (root_id,))
        return {str(cid) for row in rows for cid in json.loads(row["input_json"]).get("reopened", [])}

    def exposure_origins(self, root_id: str) -> set[str]:
        """Actual retrieved content given to models, distinct from stored-but-omitted context sources."""
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(f"SELECT {_RECORDED} AS origin FROM spans WHERE kind='context.exposure' AND {where}",
                                     (origin.external('unlabeled'), *params))
        return {o for row in rows for o in origin.split(row['origin'])}

    def outside_exposure_origins(self, root_id: str) -> set[str]:
        """The origins of the context given to this tree's models that counts as outside content read (L4): a context
        exposure whose stored origin is outside content (`origin.stored_outside`; one recorded without an origin is
        unlabeled, so it counts). FERB's own memory, history and device facts hold a label of their own beside the
        ones they inherited, so they never make a tree count as having read outside content."""
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(f"SELECT {_RECORDED} AS origin FROM spans WHERE kind='context.exposure' AND {where}",
                                     (origin.external('unlabeled'), *params))
        stored = [origin.split(row['origin']) for row in rows]
        return {o for labels in stored if origin.stored_outside(labels) for o in labels}

    def task_origins(self, root_id: str) -> set[str]:
        """Every origin recorded on this task tree's request (its root task span), tool results and context
        sources: what its models were shown. Labels derived from the task's work (worker reports, answers) inherit
        these (MP2). A tool result or context source with output but no recorded origin (from before F3) is
        unlabeled (`origin.recorded`); the root's origin is set explicitly, so without one it adds nothing."""
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(
            f"SELECT {_RECORDED} AS origin FROM spans WHERE kind IN ('task', 'tool.call', 'context.source', 'context.exposure') "
            f"AND {where}", (origin.external("unlabeled"), *params))
        found = {o for r in rows for o in origin.split(r["origin"])}
        # Retention deletes the spans, never the episode: its origins keep the labels fail-closed (MP2).
        ep = self._db.conn.execute("SELECT origins FROM episodes WHERE task_id = ?", (root_id,)).fetchone()
        return found | (set(origin.split(ep["origins"])) if ep and ep["origins"] else set())

    def tool_outputs(self, root_id: str, after_rowid: int = 0, *, include_context: bool = False) -> list[tuple[int, set[str], str]]:
        """(rowid, origins, text) of this task tree's finished tool calls after `after_rowid`, in call order. The text
        is every string value of the decoded output (numbers as strings) joined by newlines, capped at 20000
        characters while it is joined; past the cap stays unsearched. `include_context` adds the context given to
        its models whose stored origin is outside content (L4, `origin.stored_outside`): FERB's own memory, history
        and device facts are not outside content, whatever they inherited."""
        where, params = self._tree_where(root_id)
        kinds = "('tool.call','context.exposure')" if include_context else "('tool.call')"
        rows = self._db.conn.execute(f"SELECT rowid, kind, origin, output_json FROM spans WHERE kind IN {kinds} AND "
                                     f"{where} AND rowid > ? AND output_json IS NOT NULL ORDER BY rowid",
                                     (*params, after_rowid))
        out: list[tuple[int, set[str], str]] = []
        for r in rows:
            labels = origin.split(r["origin"])
            if r["kind"] == "context.exposure" and not origin.stored_outside(labels):
                continue
            out.append((r["rowid"], labels, _joined(json.loads(r["output_json"]), OUTPUT_TEXT_CAP)))
        return out

    def payload_outputs(self, root_id: str, after_rowid: int = 0, *, seen: set[int] | None = None) -> list["PayloadOutput"]:
        """New actual source data, using capability schemas rather than globally dropping field names.

        Context producers supply wrapper-free payloads. Legacy context stays conservative: its complete text is
        evidence. File chunk boundaries accompany the payload so the gateway can assemble only changed streams.
        """
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(
            f"SELECT rowid,kind,origin,input_json,output_json FROM spans WHERE kind IN ('tool.call','context.exposure') AND {where} "
            "AND rowid > ? AND output_json IS NOT NULL ORDER BY rowid", (*params, after_rowid))
        out: list[PayloadOutput] = []
        for row in rows:
            if seen is not None and row['rowid'] in seen:
                continue
            inp, body = json.loads(row['input_json']), json.loads(row['output_json'])
            result = PayloadOutput(row['rowid'], origin.split(row['origin']), '')
            tool = inp.get('tool', '')
            payload: Any = body
            if isinstance(body, dict):
                if row['kind'] == 'context.exposure':
                    payload = body.get('payload', body.get('text', ''))
                elif tool == 'fs.read':
                    payload = body.get('text', '')
                    result.path = str(body.get('path') or inp.get('args', {}).get('path') or '')
                    result.offset, result.next_offset = body.get('offset', 0), body.get('next_offset')
                elif tool in {'shell.run', 'shell.read', 'shell.wait'}:
                    payload = body.get('output', '')
                elif tool in {'fs.list', 'fs.write', 'fs.patch', 'fs.mkdir', 'fs.trash', 'fs.delete',
                              'browser.open', 'browser.navigate', 'browser.close', 'ferb_browser.open',
                              'ferb_browser.navigate', 'ferb_browser.close'}:
                    payload = ''
            result.text = '\n'.join(_strings(payload))
            out.append(result)  # even empty results advance the incremental cursor
        return out

    def latest_acting_call(self, root_id: str, tool: str | None = None) -> dict[str, Any] | None:
        """The latest tool.call in this task tree: of `tool` when named, else any consequential (non-reading) one.
        Failed calls count. Never a model's claim: the recorded span."""
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(f"SELECT input_json, output_json, error FROM spans WHERE kind = 'tool.call' AND "
                                     f"{where} ORDER BY rowid DESC", params)
        for r in rows:
            inp = json.loads(r["input_json"])
            if (inp.get("tool") == tool) if tool else (inp.get("kind") is not None and inp.get("kind") not in REVERSIBLE):
                return {"tool": inp.get("tool"), "output": json.loads(r["output_json"]) if r["output_json"] else None,
                        "error": r["error"]}
        return None

    def first_open_tool_call(self, root_id: str, after_rowid: int = 0, *, include_context: bool = False) -> int | None:
        """The rowid of this task tree's earliest tool call after `after_rowid` that hasn't finished (still running),
        if any: its result arrives later than calls started after it."""
        where, params = self._tree_where(root_id)
        kinds = "('tool.call','context.exposure')" if include_context else "('tool.call')"
        row = self._db.conn.execute(f"SELECT min(rowid) FROM spans WHERE kind IN {kinds} AND {where} "
                                    "AND rowid > ? AND ended_at IS NULL", (*params, after_rowid)).fetchone()
        return row[0] if row else None

    def owner_answers(self, root_id: str) -> list[str]:
        """The owner's answers to owner.ask in this task tree: only spans whose input names that tool and whose
        origin is exactly the owner. Nothing else the owner's name is on (memory, history) counts."""
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(
            f"SELECT output_json FROM spans WHERE kind = 'tool.call' AND origin = ? AND output_json IS NOT NULL "
            f"AND json_extract(input_json, '$.tool') = 'owner.ask' AND {where} ORDER BY rowid", (origin.OWNER, *params))
        answers = [json.loads(r["output_json"]) for r in rows]
        return [a["answer"] for a in answers if isinstance(a, dict) and isinstance(a.get("answer"), str)]

    def owner_questions(self, root_id: str) -> list[dict[str, str]]:
        """The task's questions to the owner (owner.ask, not FERB's own checks or P19 retry questions) that the owner
        answered, each with the answer, in order: what carries across the task's retries (rule 9). Only spans whose
        origin is exactly the owner, as `owner_answers`; an orchestrator's answer is not the owner's."""
        where, params = self._tree_where(root_id)
        rows = self._db.conn.execute(
            f"SELECT input_json, output_json FROM spans WHERE kind = 'tool.call' AND origin = ? "
            f"AND output_json IS NOT NULL AND json_extract(input_json, '$.tool') = 'owner.ask' "
            f"AND json_extract(input_json, '$.ferb_question') IS NULL "
            f"AND json_extract(input_json, '$.args.retry_action') IS NULL "
            f"AND json_extract(input_json, '$.args.question') IS NOT NULL AND {where} ORDER BY rowid",
            (origin.OWNER, *params))
        found: list[dict[str, str]] = []
        for r in rows:
            asked, given = json.loads(r["input_json"]), json.loads(r["output_json"])
            if isinstance(given, dict) and isinstance(given.get("answer"), str):
                item = {"question": str(asked["args"]["question"]), "answer": given["answer"]}
                if item not in found:
                    found.append(item)
        return found

    def last_typed(self, task_id: str, tool: str, tab: str) -> str:
        """The text this task's worker last typed with `tool` into the page tab `tab` (a type action that didn't
        fail), as recorded (redacted at write); "" when none. Display only (L3: the text a send carries)."""
        row = self._db.conn.execute(
            "SELECT json_extract(input_json, '$.args.text') AS text FROM spans WHERE kind = 'tool.call' AND "
            "task_id = ? AND json_extract(input_json, '$.tool') = ? AND json_extract(input_json, '$.args.tab') = ? "
            "AND json_extract(input_json, '$.args.action') = 'type' AND error IS NULL ORDER BY rowid DESC LIMIT 1",
            (task_id, tool, tab)).fetchone()
        return str(row["text"]) if row is not None and row["text"] else ""

    def typed_in_tab(self, task_id: str, tab: str) -> list[str]:
        """Every text this task typed or chose into the page tab `tab` (a fill that didn't fail), as recorded
        (redacted at write), oldest first: what a form submitted there can carry (the sink rule, step 6b)."""
        rows = self._db.conn.execute(
            "SELECT json_extract(input_json, '$.args.text') AS text FROM spans WHERE kind = 'tool.call' AND "
            "task_id = ? AND json_extract(input_json, '$.args.tab') = ? AND json_extract(input_json, '$.kind') = "
            "'fill_form' AND error IS NULL ORDER BY rowid", (task_id, tab)).fetchall()
        return [str(r["text"]) for r in rows if r["text"]]

    def _task_rows(self, task_id: str) -> list[Any]:
        """Include the intake planner by exact request ID; it ran before the task ID existed. Then every descendant
        of a selected span (e.g. the planner's model.usage, which has no task ID), in start order. An id that is no
        task's may be a learning model call's (`memory/calls.py`: no task ID, the call's id as `call`), or a request's
        whose planning failed before any task existed (its planner turns; L35's report)."""
        root = self._db.conn.execute("SELECT request_id FROM tasks WHERE id = ?", (task_id,)).fetchone()
        request_id = root[0] if root else task_id
        where, params = self._tree_where(task_id)
        rows = self._db.conn.execute(
            f"SELECT rowid AS row_order, * FROM spans WHERE {where} "
            "OR (task_id IS NULL AND json_extract(input_json, '$.request_id') = ?) "
            "OR (task_id IS NULL AND kind = 'model.learning' AND json_extract(input_json, '$.call') = ?)",
            (*params, request_id, None if root else task_id)).fetchall()
        seen = {r["id"] for r in rows}
        frontier = list(seen)
        while frontier:
            found = []
            for i in range(0, len(frontier), 500):
                chunk = frontier[i:i + 500]
                found += [r for r in self._db.conn.execute(
                    f"SELECT rowid AS row_order, * FROM spans WHERE parent_id IN ({','.join('?' * len(chunk))})",
                    chunk) if r["id"] not in seen]
            seen |= {r["id"] for r in found}
            rows += found
            frontier = [r["id"] for r in found]
        return sorted(rows, key=lambda r: (r["started_at"], r["row_order"]))

    def tree_json(self, task_id: str) -> list[dict[str, Any]]:
        """`tree` as plain dicts (times as ISO strings, children nested): what trace.get and episodes read."""
        return [node_json(n) for n in self.tree(task_id)]

    def outcome(self, task_id: str) -> dict[str, Any]:
        """Export one redacted evidence object. Missing historical facts stay explicit, never reconstructed guesses."""
        events = [_span_record(_row_to_span(row)) for row in self._task_rows(task_id)]
        # A continued tree has one root span per run (a resume, a continuation): the request is the first run's, the
        # result the last run's (step 12a: an episode built from a continued tree had no result and a wrong end)
        roots = [e for e in events if e['kind'] == 'task' and e['task_id'] == task_id]
        root = roots[-1] if roots else {}
        source = roots[0].get('input', {}) if roots else {}
        tasks = []
        for row in self._db.conn.execute("SELECT * FROM tasks WHERE tree_id = ? ORDER BY created_at, id", (task_id,)):
            task = dict(row)
            for name in ('scopes', 'outcome'):
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
        """Add measured runtime input after tokens/tools/time are assembled; redact before persistence. Written only
        while the span still holds the input read (`swap`): a forget's scrub rewriting it meanwhile, on its own
        connection, is never undone (fix wave 6a)."""
        clean = self._redact(values)
        row = self._db.conn.execute("SELECT input_json FROM spans WHERE id = ?", (span_id,)).fetchone()
        if row is None:
            raise ValueError("unknown span")
        swap(self._db, "spans", "id", span_id, ("input_json",),
             lambda held: (_dumps(json.loads(held[0]) | clean),), (row[0],))

    @staticmethod
    def _evidence_where(task_id: str, root_id: str | None) -> tuple[str, list[Any]]:
        # Root identity is resolved from the registered live task, never inferred from a name/prefix.
        where = "(task_id = ? AND kind = 'tool.call' AND json_extract(input_json, '$.tool') NOT IN " \
                "('task.evidence','chat.history','memory.read'))"
        params: list[Any] = [task_id]
        if root_id:
            where += " OR (task_id = ? AND kind IN ('task','task.intake','context.source','context.exposure','disturbance.baseline'," \
                     "'disturbance.change','disturbance.error','disturbance.summary','disturbance.paused'," \
                     "'disturbance.resumed'))"
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
            # as written now (UTF-8), and as rows written before step 12a hold it (JSON's \\uXXXX escapes)
            forms = list(dict.fromkeys((query, json.dumps(query)[1:-1])))
            where += " AND (" + " OR ".join(
                "instr(lower(kind || input_json || coalesce(output_json, '') || coalesce(error, '')), lower(?)) > 0"
                for _ in forms) + ")"
            params.extend(forms)
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


Held = tuple[str, str | None]                   # a span's recorded input and output, as stored


def rewrite_recorded(db: Any, ident: str, change: Callable[[Any, Any], tuple[Any, Any]], *,
                     needles: Any = None, held: Held | None = None) -> bool:
    """One span's recorded input and output (decoded) rewritten through `change(input, output)`, written only while
    the span still holds what was read (`swap`): a live write landing in between (the span finishing, measured input)
    makes it read and change the span again, so its new output is never overwritten with an old one.
    `held`: the span as the caller already read it. `needles`: a span whose stored text (lowercased) holds none of them
    can't change, and isn't decoded. A side `change` leaves alone keeps its stored text. Returns whether it wrote."""
    def step(stored: tuple[Any, ...]) -> tuple[Any, ...]:
        inp, out = stored
        if needles is not None and not any(n in (inp + (out or "")).lower() for n in needles):
            return stored
        before_in, before_out = json.loads(inp), json.loads(out or "null")
        new_in, new_out = change(before_in, before_out)
        if new_in == before_in and new_out == before_out:
            return stored
        return (inp if new_in == before_in else _dumps(new_in),
                out if new_out == before_out else None if new_out is None else _dumps(new_out))
    written = swap(db, "spans", "id", ident, ("input_json", "output_json"), step, held)
    return written is not None


def _row_to_span(r: Any) -> Span:
    return Span(id=r["id"], parent_id=r["parent_id"], kind=r["kind"], task_id=r["task_id"], device_id=r["device_id"],
                started_at=datetime.fromisoformat(r["started_at"]),
                ended_at=datetime.fromisoformat(r["ended_at"]) if r["ended_at"] else None,
                input=json.loads(r["input_json"]),
                output=json.loads(r["output_json"]) if r["output_json"] else None,
                cost_usd=r["cost_usd"], error=r["error"], origin=r["origin"], summary=r["summary"])


def node_json(node: SpanNode) -> dict[str, Any]:
    s = asdict(node.span)
    for k in ("started_at", "ended_at"):
        s[k] = s[k].isoformat() if s[k] else None
    s["children"] = [node_json(c) for c in node.children]
    return s


def _span_record(span: Span) -> dict[str, Any]:
    return {'id': span.id, 'parent_id': span.parent_id, 'task_id': span.task_id, 'kind': span.kind,
            'started_at': span.started_at.isoformat(),
            'ended_at': span.ended_at.isoformat() if span.ended_at else None,
            'input': span.input, 'output': span.output, 'error': span.error, 'cost_usd': span.cost_usd,
            'origin': span.origin, 'summary': span.summary}


def _strings(value: Any) -> Iterator[str]:
    """Every string (and number as a string) inside decoded JSON, in order, as they are reached."""
    if isinstance(value, str):
        yield value
    elif isinstance(value, (int, float)) and not isinstance(value, bool):
        yield str(value)
    elif isinstance(value, (dict, list)):
        for v in (value.values() if isinstance(value, dict) else value):
            yield from _strings(v)


def _joined(value: Any, cap: int) -> str:
    """The strings of decoded JSON joined by newlines, cut at `cap` characters: stops walking once the cap is reached,
    so a huge output is never joined whole."""
    parts: list[str] = []
    size = 0
    for text in _strings(value):
        if parts:
            size += 1                                    # the newline before this string
        part = text[:max(cap - size, 0)]
        parts.append(part)
        size += len(part)
        if size >= cap:
            break
    return "\n".join(parts)[:cap]
