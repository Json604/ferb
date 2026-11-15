"""Fixtures for the Rust port of the typed stores (rust/crates/ferb-store), made by the Python stores (ENG-3).

    cd <repo> && uv run python rust/fixtures/store/make_tables.py

Drives every module of ferb/store but db.py (spans, task_results, chat_history, retention, cleanup, salvage, and db.swap)
through scripted, invented operations in temporary folders (never ~/.ferb), with a fixed clock and sequential ids, and
writes tables.json next to this file: per scenario the steps in order: raw seed SQL (`sql`), a store call with its
arguments and what it returned or raised (`call`), and the affected tables' rows (`dump`). The Rust test
(tests/tables.rs) replays the same steps on a store the Rust code made and must get the same returns and rows.
It also records what Python's datetime.fromisoformat, json.loads, json.dumps and UTF-8 decoding say about a list of
texts, and regenerates rust/crates/ferb-py/src/unicode_nd.rs (Python's decimal digits). All data is invented.
"""

import base64
import dataclasses
import json
import sqlite3
import sys
import tempfile
import unicodedata
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO))

from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.policy.rules import REVERSIBLE  # noqa: E402
from ferb.store import chat_history, cleanup, retention, salvage  # noqa: E402
from ferb.store import db as store_db  # noqa: E402
from ferb.store import spans as spans_mod  # noqa: E402
from ferb.store.db import Database  # noqa: E402
from ferb.store.task_results import TaskResults  # noqa: E402

HERE = Path(__file__).parent
BASE = datetime(2026, 1, 2, 3, 4, 5, tzinfo=timezone.utc)


def replace_text(value: Any, find: str, new: str) -> Any:
    """The fake redactor and the fake rewrite: `find` replaced in every string value (never in a key)."""
    if isinstance(value, str):
        return value.replace(find, new)
    if isinstance(value, list):
        return [replace_text(v, find, new) for v in value]
    if isinstance(value, dict):
        return {k: replace_text(v, find, new) for k, v in value.items()}
    return value


def redact(value: Any) -> Any:
    return replace_text(value, "SECRET", "[redacted]")


class Clock:
    """Each reading is 1.25 s after the one before, so some times carry microseconds and some don't."""

    def __init__(self) -> None:
        self.n = 0

    def now(self) -> datetime:
        self.n += 1
        return BASE + timedelta(seconds=self.n, microseconds=250000 * self.n)


def norm(value: Any) -> Any:
    if isinstance(value, datetime):
        return value.isoformat()
    if isinstance(value, spans_mod.SpanNode):
        return spans_mod.node_json(value)
    if isinstance(value, retention.Plan):
        return {"roots": value.roots, "trees": [[k, v] for k, v in value.trees.items()], "loose": value.loose,
                "decisions": value.decisions, "operational": [[k, v] for k, v in value.operational.items()],
                "left": value.left}
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        return {f.name: norm(getattr(value, f.name)) for f in dataclasses.fields(value)}
    if isinstance(value, sqlite3.Row):
        return {k: norm(value[k]) for k in value.keys()}
    if isinstance(value, (set, frozenset)):
        return sorted(norm(v) for v in value)
    if isinstance(value, (list, tuple)):
        return [norm(v) for v in value]
    if isinstance(value, dict):
        return {str(k): norm(v) for k, v in value.items()}
    if isinstance(value, bytes):
        return {"blob": value.hex()}
    if isinstance(value, Path):
        return str(value)
    return value


class Run:
    def __init__(self, folder: Path) -> None:
        self.steps: list[dict[str, Any]] = []
        self.home = folder
        self.db = Database(folder / "ferb.db")
        self.db.migrate()
        self.clock, self.n = Clock(), 0
        self.spans = spans_mod.SpanStore(self.db, self.clock, SequentialIds("s"), redact)
        self.held: dict[str, Any] = {}
        self.results = TaskResults(self.db)
        self.chat = chat_history.ChatHistory(self.db, redact, self.chat_now)
        self.journal: list[Any] = []

    def chat_now(self) -> str:
        self.n += 1
        return f"2026-11-09T12:00:{self.n:02d}+00:00"

    def sql(self, query: str, *params: Any) -> None:
        self.db.conn.execute(query, params)
        self.steps.append({"sql": query, "params": list(params)})

    def call(self, name: str, /, **args: Any) -> Any:
        step: dict[str, Any] = {"call": name, "args": args}
        ret = None
        try:
            ret = CALLS[name](self, **json.loads(json.dumps(args)))
            step["ret"] = norm(ret)
        except Exception as e:  # noqa: BLE001 - what it raised is the fixture
            if self.db.conn.in_transaction:
                self.db.conn.execute("ROLLBACK")
            step["err"] = [type(e).__name__, str(e)]
        self.steps.append(step)
        return ret

    def dump(self, *tables: str) -> None:
        self.steps.append({"dump": {t: [norm(list(r)) for r in self.db.conn.execute(
            f"SELECT * FROM {t} ORDER BY rowid")] for t in tables}})


# ---------- the calls, by the name the Rust test dispatches on ----------

def _start(r: Run, kind: str, input: Any, task_id: Any = None, parent_id: Any = None, device_id: Any = None) -> Any:
    span = r.spans.start(kind, input=input, task_id=task_id, parent_id=parent_id, device_id=device_id)
    r.held[span.id] = span
    return span


def _finish(r: Run, id: str, output: Any = None, error: Any = None, cost_usd: Any = None, origin: Any = None,
            summary: Any = None) -> Any:
    return r.spans.finish(r.held[id], output=output, error=error, cost_usd=cost_usd, origin=origin, summary=summary)


def _stopper(after: Any) -> Any:
    seen = {"n": 0}

    def stop() -> bool:
        seen["n"] += 1
        return after is not None and seen["n"] > after
    return stop


class FakeCards:
    """The learning seam (cleanup._learning): one card version loses two refs; pruning deletes exactly those rows."""

    def __init__(self, db: Any) -> None:
        self.db = db

    def card_source_plan(self, removing: Any = ()) -> dict[str, Any]:
        have = {r[0] for r in self.db.conn.execute("SELECT source_ref FROM learning_sources WHERE node_id = 'card-1'")}
        gone = sorted(have & {"trail:a", "trail:b"})
        return {"tasks": 1, "nodes": 3, "untraced": 0, "stopped": False, "drop": {"card-1": gone} if gone else {},
                "removing": sorted(removing)}

    def prune_card_sources(self, stop: Any, plan: Any) -> dict[str, Any]:
        rows = 0
        for node, refs in plan["drop"].items():
            for ref in refs:
                rows += self.db.conn.execute("DELETE FROM learning_sources WHERE node_id = ? AND source_ref = ?",
                                             (node, ref)).rowcount
        return {"rows": rows, "stopped": False}


cleanup._learning = FakeCards   # the seam the Rust port names CardSources


class FakeJournal:
    def __init__(self, r: Run) -> None:
        self.r = r

    def append(self, kind: str, data: Any) -> None:
        self.r.journal.append([kind, norm(data)])


def _sizes_out(value: dict[str, Any]) -> dict[str, Any]:
    """Byte counts depend on the SQLite build's page use: the Rust test checks them by their own rule."""
    return {k: ("<bytes>" if k in ("bytes", "needed", "bytes_before", "bytes_after") else v) for k, v in value.items()}


def _apply(r: Run, ok: str, stop_after: Any = None, free: int = 1 << 60) -> Any:
    r.journal = []
    out = cleanup.apply(r.db, FakeJournal(r), ok, stop=_stopper(stop_after), free=lambda folder: free)
    journal = [[k, _sizes_out(d)] for k, d in r.journal]
    return {"out": _sizes_out(out), "journal": journal}


def _config(cfg: dict[str, Any]) -> Any:
    return lambda section, key, default: cfg.get(f"{section}.{key}", default)


CALLS: dict[str, Any] = {
    "spans.start": _start,
    "spans.finish": _finish,
    "spans.set_origin": lambda r, id, origin: r.spans.set_origin(id, origin),
    "spans.update_input": lambda r, id, values: r.spans.update_input(id, values),
    "spans.tree_json": lambda r, task_id: r.spans.tree_json(task_id),
    "spans.tree": lambda r, task_id: r.spans.tree(task_id),
    "spans.outcome": lambda r, task_id: r.spans.outcome(task_id),
    "spans.tool_origins": lambda r, root_id: r.spans.tool_origins(root_id),
    "spans.request_origins": lambda r, root_id: r.spans.request_origins(root_id),
    "spans.asked_origins": lambda r, root_id: r.spans.asked_origins(root_id),
    "spans.reopened_answers": lambda r, root_id: r.spans.reopened_answers(root_id),
    "spans.exposure_origins": lambda r, root_id: r.spans.exposure_origins(root_id),
    "spans.outside_exposure_origins": lambda r, root_id: r.spans.outside_exposure_origins(root_id),
    "spans.task_origins": lambda r, root_id: r.spans.task_origins(root_id),
    "spans.request_began": lambda r, root_id, request_id, worker_id: r.spans.request_began(root_id, request_id,
                                                                                         worker_id),
    "spans.tool_outputs": lambda r, root_id, after, include_context: r.spans.tool_outputs(
        root_id, after, include_context=include_context),
    "spans.payload_outputs": lambda r, root_id, after, seen: r.spans.payload_outputs(
        root_id, after, seen=None if seen is None else set(seen)),
    "spans.latest_acting_call": lambda r, root_id, tool: r.spans.latest_acting_call(root_id, tool),
    "spans.first_open_tool_call": lambda r, root_id, after, include_context: r.spans.first_open_tool_call(
        root_id, after, include_context=include_context),
    "spans.owner_answers": lambda r, root_id: r.spans.owner_answers(root_id),
    "spans.owner_questions": lambda r, root_id: r.spans.owner_questions(root_id),
    "spans.last_typed": lambda r, task_id, tool, tab: r.spans.last_typed(task_id, tool, tab),
    "spans.typed_in_tab": lambda r, task_id, tab: r.spans.typed_in_tab(task_id, tab),
    "spans.tool_call": lambda r, task_id, span_id, root_id, whole: r.spans.tool_call(task_id, span_id, root_id, whole),
    "spans.tool_calls": lambda r, task_id, query, after, limit, root_id, kind, whole: r.spans.tool_calls(
        task_id, query, after, limit, root_id, kind, whole),
    "spans.evidence_kinds": lambda r, task_id, root_id, whole: list(
        r.spans.evidence_kinds(task_id, root_id, whole).items()),
    "spans.rewrite": lambda r, ident, find, new, needles, held: spans_mod.rewrite_recorded(
        r.db, ident, lambda i, o: (replace_text(i, find, new), replace_text(o, find, new)), needles=needles,
        held=None if held is None else tuple(held)),
    "swap": lambda r, table, key, ident, columns, new, held: store_db.swap(
        r.db, table, key, ident, tuple(columns), lambda row: tuple(new), None if held is None else tuple(held)),
    "results.record": lambda r, task_id, at, status, goal, answer, shown, cards, name: r.results.record(
        task_id, at, status, goal, answer, shown, cards, name),
    "results.interrupted": lambda r, task_id, at, goal, answer: r.results.interrupted(task_id, at, goal, answer),
    "results.unshown": lambda r: r.results.unshown(),
    "results.page": lambda r, before, limit: r.results.page(before, limit),
    "results.latest_of_schedule": lambda r, schedule_id: r.results.latest_of_schedule(schedule_id),
    "results.record_verdict": lambda r, verdict_id, task_id, verdict, note, at: r.results.record_verdict(
        verdict_id, task_id, verdict, note, at),
    "results.mark_shown": lambda r, task_ids: r.results.mark_shown(task_ids),
    "chat.write": lambda r, entries, replace: r.chat.write(entries, replace=replace),
    "chat.page": lambda r, day, before, limit: r.chat.page(day, before, limit),
    "chat.open": lambda r: r.chat.open(),
    "chat.days": lambda r: r.chat.days(),
    "chat.clear_notice": lambda r, ref, text: chat_history.clear_notice(r.db, ref, text),
    "retention.windows": lambda r, cfg: list(retention.windows(_config(cfg)).items()),
    "retention.describe": lambda r, cfg: retention.describe(_config(cfg)),
    "retention.finished_roots_before": lambda r, cutoff: retention.finished_roots_before(r.db.conn, cutoff),
    "retention.operational": lambda r, cutoff: list(retention.operational(r.db.conn, cutoff).items()),
    "retention.plan": lambda r, trace, decision, operational, limit: retention.plan(
        r.db.conn, trace, decision, operational, limit),
    "retention.delete_rows": lambda r, table, key, ids, batch: list(retention.delete_rows(r.db, table, key, ids,
                                                                                         batch)),
    "cleanup.plan": lambda r: cleanup.plan(r.db),
    "cleanup.preview": lambda r, free: _sizes_out(cleanup.preview(r.db.path, lambda folder: free)),
    "cleanup.pending": lambda r: cleanup.pending(r.db.path),
    "cleanup.apply": _apply,
}


# ---------- scenarios ----------

TASK = ("INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
        "outcome_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'ferb', ?, ?, ?, ?, ?, ?)")
JOURNAL = "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)"
SPAN = ("INSERT INTO spans (id, parent_id, task_id, kind, started_at, ended_at, input_json, output_json, error, "
        "origin) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")


def spans_scenario(r: Run) -> None:
    r.sql(TASK, "T1", "T1", None, "req-1", "tidy the notes", "supervisor", '["fs"]', "done", '{"verified": true}',
          "2026-01-02T03:00:00+00:00", "2026-01-02T03:30:00+00:00")
    r.sql(TASK, "T1.1", "T1", "T1", "req-1", "write the file", "worker", "[]", "done", "{}",
          "2026-01-02T03:01:00+00:00", "2026-01-02T03:20:00+00:00")
    r.sql(TASK, "T10", "T10", None, "req-9", "another tree", "supervisor", "[]", "running", "{}",
          "2026-01-02T04:00:00+00:00", "2026-01-02T04:00:00+00:00")
    r.sql(JOURNAL, "j-1", "2026-01-02T03:00:00+00:00", "chat.message",
          '{"request_id": "req-1", "text": "tidy the notes, caf\\u00e9"}', "owner")
    r.sql(JOURNAL, "j-2", "2026-01-02T03:30:00+00:00", "task.outcome", '{"task_id": "T1", "status": "done"}', "ferb")
    r.sql(JOURNAL, "j-3", "2026-01-02T03:31:00+00:00", "task.answer", '{"task_id": "T1", "answer": "Done."}', "ferb")
    r.sql("INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES ('v1', 'T1', 'fail', 'not yet', "
          "'2026-01-02T05:00:00+00:00')")
    r.sql("INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES ('v2', 'T1', 'pass', '', "
          "'2026-01-02T06:00:00+00:00')")
    r.sql("INSERT INTO episodes (task_id, request_text, goal, status, answer, conditions_json, steps_json, "
          "problems_json, tools_json, origins, external_read, usage_json, desktop_json, made_by, built_at) VALUES "
          "('T1', 'x', 'x', 'done', 'x', '[]', '[]', '[]', '{}', 'owner,external:email:someone', 1, '{}', '{}', "
          "'code:test', '2026-01-02T07:00:00+00:00')")
    # the intake planner ran before the task existed: its spans carry the request id only
    intake = r.call("spans.start", kind="task.intake", input={"request_id": "req-1", "text": "tidy SECRET notes"})
    usage = r.call("spans.start", kind="model.usage", input={"tokens": 12, "cost": 0.5}, parent_id=intake.id)
    r.call("spans.finish", id=usage.id, output={"ok": True}, cost_usd=0.015)
    r.call("spans.finish", id=intake.id, output={"plan": "one step"}, origin="ferb")
    root = r.call("spans.start", kind="task", task_id="T1", device_id="mac",
                  input={"request_id": "req-1", "request": "tidy the notes, café SECRET", "authority_words": ["tidy"],
                         "task_spec": {"name": "Tidy notes", "n": 1.0}, "request_origins": ["owner", "", None,
                                                                                            "external:web:example.com"],
                         "asked_origins": ["external:email:someone", 0]})
    r.call("spans.set_origin", id=root.id, origin="owner,external:web:example.com")
    turn = r.call("spans.start", kind="model.turn", task_id="T1", parent_id=root.id, input={"role": "supervisor"})
    r.call("spans.finish", id=turn.id, output={"decision": "go"}, cost_usd=1e-05)
    source = r.call("spans.start", kind="context.source", task_id="T1", parent_id=root.id, input={"block": "memory"})
    r.call("spans.finish", id=source.id, output={"text": "a remembered thing"})          # no origin: unlabeled
    shown = r.call("spans.start", kind="context.exposure", task_id="T1", parent_id=root.id, input={"block": "page"})
    r.call("spans.finish", id=shown.id, origin="external:web:example.com,external:email:someone",
           output={"payload": {"title": "Página", "n": 3, "f": 2.5}, "text": "ignored", "segments": [{"ref": "a"}]})
    own = r.call("spans.start", kind="context.exposure", task_id="T1", parent_id=root.id, input={"block": "history"})
    r.call("spans.finish", id=own.id, origin="ferb,external:web:example.com", output={"text": "FERB's own history"})
    bare = r.call("spans.start", kind="context.exposure", task_id="T1.1", parent_id=root.id, input={"block": "old"})
    r.call("spans.finish", id=bare.id, output="just text")
    worker = r.call("spans.start", kind="worker", task_id="T1.1", parent_id=root.id, input={"step": 1})
    read = r.call("spans.start", kind="tool.call", task_id="T1.1", parent_id=worker.id,
                  input={"tool": "fs.read", "kind": "read", "args": {"path": "/tmp/notes.txt"}})
    r.call("spans.finish", id=read.id, origin="private", summary="read SECRET notes",
           output={"text": "héllo wörld 日本語 😀", "offset": 0, "next_offset": 42, "more": True, "size": 1e16})
    shell = r.call("spans.start", kind="tool.call", task_id="T1.1", parent_id=worker.id,
                   input={"tool": "shell.run", "kind": "run_shell", "args": {"command": "ls"}})
    r.call("spans.finish", id=shell.id, output={"output": "a.txt\nb.txt", "exit": 0, "url": 5}, origin="private")
    page = r.call("spans.start", kind="tool.call", task_id="T1.1", parent_id=worker.id,
                  input={"tool": "browser.read", "kind": "read", "args": {"tab": "t1"}})
    r.call("spans.finish", id=page.id, origin="external:web:example.com",
           output={"url": "https://example.com/a", "text": "page text", "links": ["x", ["y", 7]], "none": None})
    opened = r.call("spans.start", kind="tool.call", task_id="T1.1", parent_id=worker.id,
                    input={"tool": "browser.open", "kind": "open_tab", "args": {"url": "https://example.com"}})
    r.call("spans.finish", id=opened.id, output={"url": "https://example.com/", "tab": "t1"})
    for text, action, kind, error in (("", "type", "fill_form", None),
        ("first draft", "type", "fill_form", None), ("failed text", "type", "fill_form",
                                                                                "the field was gone"),
                                      ("second SECRET draft", "type", "fill_form", None),
                                      ("chosen", "select", "fill_form", None)):
        typed = r.call("spans.start", kind="tool.call", task_id="T1.1", parent_id=worker.id,
                       input={"tool": "browser.act", "kind": kind, "args": {"tab": "t1", "text": text,
                                                                           "action": action}})
        r.call("spans.finish", id=typed.id, output={"ok": error is None}, error=error)
    for question, answer, extra, origin in (("Which folder?", "Documents", {}, "owner"),
                                            ("Which folder?", "Documents", {}, "owner"),
                                            ("Really?", "yes", {"ferb_question": True}, "owner"),
                                            ("Retry?", "no", {"retry": 1}, "owner"),
                                            ("Whose?", "theirs", {}, "inferred"),
                                            ("Colour?", "blue SECRET", {}, "owner")):
        args = {"question": question} | ({"retry_action": "send"} if extra.get("retry") else {})
        ask = r.call("spans.start", kind="tool.call", task_id="T1.1", parent_id=worker.id,
                     input={"tool": "owner.ask", "args": args} | ({"ferb_question": True} if extra.get(
                         "ferb_question") else {}))
        r.call("spans.finish", id=ask.id, output={"answer": answer}, origin=origin)
    big = r.call("spans.start", kind="tool.call", task_id="T1.1", parent_id=worker.id,
                 input={"tool": "fs.list", "kind": "read", "args": {"path": "/tmp"}})
    r.call("spans.finish", id=big.id, origin="private",
           output={"entries": ["é" * 9000, "b" * 9000, "c" * 9000, "never reached"], "count": 4})
    send = r.call("spans.start", kind="tool.call", task_id="T1.1", parent_id=worker.id,
                  input={"tool": "mail.send", "kind": "send", "args": {"to": "someone@example.com"}})
    r.call("spans.finish", id=send.id, error="SECRET refused by the server", output=None)
    still = r.call("spans.start", kind="tool.call", task_id="T1.1", parent_id=worker.id,
                   input={"tool": "shell.wait", "kind": "read", "args": {}})
    r.call("spans.finish", id=worker.id, output={"report": "done"}, origin="inferred")
    reopened = r.call("spans.start", kind="confirmations.reopened", task_id="T1", input={"reopened": ["c-1", 22]})
    r.call("spans.finish", id=reopened.id)
    summary = r.call("spans.start", kind="disturbance.summary", task_id="T1", parent_id=root.id, input={})
    r.call("spans.finish", id=summary.id, output={"changes": 0})
    check = r.call("spans.start", kind="worker", task_id="T1.verify", parent_id=root.id, input={"step": "verify"})
    r.call("spans.finish", id=check.id, output={"verified": True})
    r.call("spans.finish", id=root.id, output={"answer": "first run"})
    # the tree carried on by a later message: a second root span, and a worker run after it
    again = r.call("spans.start", kind="task", task_id="T1", input={"request_id": "req-2", "request": "and the rest"})
    worker2 = r.call("spans.start", kind="worker", task_id="T1.1", parent_id=again.id, input={"step": 2})
    r.call("spans.finish", id=worker2.id, output={"report": "more"})
    r.call("spans.finish", id=again.id, output={"answer": "second run"}, origin="owner")
    # a row as a build before step 12a wrote it (\\uXXXX escapes), and a learning call with no task
    r.sql(SPAN, "old-1", None, "T1.1", "tool.call", "2026-01-01T00:00:00+00:00", "2026-01-01T00:00:01+00:00",
          '{"tool": "fs.read", "args": {"path": "/tmp/caf\\u00e9.txt"}}', '{"text": "na\\u00efve"}', None, None)
    r.sql(SPAN, "learn-1", None, None, "model.learning", "2026-01-01T00:00:02", None, '{"call": "call-7"}', None, None,
          None)
    r.call("spans.update_input", id=turn.id, values={"tokens": 99, "role": "supervisor", "note": "SECRET café"})
    r.call("spans.update_input", id="no-such-span", values={"a": 1})
    r.dump("spans")
    for name in ("tool_origins", "request_origins", "asked_origins", "reopened_answers", "exposure_origins",
                 "outside_exposure_origins", "task_origins"):
        for task in ("T1", "T1.1", "T10", "nothing"):
            r.call(f"spans.{name}", root_id=task)
    for request in ("req-1", "req-2", "req-3"):
        r.call("spans.request_began", root_id="T1", request_id=request, worker_id="T1.1")
    r.call("spans.request_began", root_id="T10", request_id="req-9", worker_id="T10.1")
    for context in (False, True):
        r.call("spans.tool_outputs", root_id="T1", after=0, include_context=context)
        r.call("spans.first_open_tool_call", root_id="T1", after=0, include_context=context)
    r.call("spans.tool_outputs", root_id="T1", after=12, include_context=True)
    r.call("spans.first_open_tool_call", root_id="T1", after=10_000, include_context=False)
    r.call("spans.first_open_tool_call", root_id="T10", after=0, include_context=False)
    r.call("spans.payload_outputs", root_id="T1", after=0, seen=None)
    r.call("spans.payload_outputs", root_id="T1", after=5, seen=[9, 10, 11])
    for tool in (None, "fs.read", "mail.send", "no.such"):
        r.call("spans.latest_acting_call", root_id="T1", tool=tool)
    r.call("spans.latest_acting_call", root_id="T10", tool=None)
    r.call("spans.owner_answers", root_id="T1")
    r.call("spans.owner_questions", root_id="T1")
    r.call("spans.last_typed", task_id="T1.1", tool="browser.act", tab="t1")
    r.call("spans.last_typed", task_id="T1.1", tool="browser.act", tab="t2")
    r.call("spans.typed_in_tab", task_id="T1.1", tab="t1")
    r.call("spans.typed_in_tab", task_id="T1", tab="t1")
    for task in ("T1", "req-1", "call-7", "nothing"):
        r.call("spans.tree_json", task_id=task)
    r.call("spans.tree", task_id="T10")
    for task in ("T1", "T10", "nothing"):
        r.call("spans.outcome", task_id=task)
    for scope in ({"root_id": None, "whole": False}, {"root_id": "T1", "whole": False}, {"root_id": None, "whole": True}):
        task = "T1" if scope["whole"] else "T1.1"
        r.call("spans.evidence_kinds", task_id=task, **scope)
        for span_id in (read.id, root.id, worker.id, check.id, intake.id, "nope"):
            r.call("spans.tool_call", task_id=task, span_id=span_id, **scope)
        for query, kind in (("", ""), ("café", ""), ("naïve", ""), ("WÖRLD", ""), ("owner.ask", ""), ("", "worker"),
                            ("draft", "tool.call"), ("no such text", "")):
            r.call("spans.tool_calls", task_id=task, query=query, after=0, limit=20, kind=kind, **scope)
    first = r.call("spans.tool_calls", task_id="T1.1", query="", after=0, limit=3, root_id="T1", kind="", whole=False)
    r.call("spans.tool_calls", task_id="T1.1", query="", after=first["next_after"], limit=3, root_id="T1", kind="",
           whole=False)
    r.call("spans.tool_calls", task_id="T1.1", query="", after=0, limit=0, root_id=None, kind="", whole=False)
    r.call("spans.tool_calls", task_id="T1.1", query="", after=-1, limit=5, root_id=None, kind="", whole=False)
    # a forget's scrub: needles that miss, a change that changes nothing, one side, both, and a stale read
    r.call("spans.rewrite", ident=read.id, find="wörld", new="[forgotten]", needles=["absent"], held=None)
    r.call("spans.rewrite", ident=read.id, find="absent", new="x", needles=["héllo"], held=None)
    r.call("spans.rewrite", ident=read.id, find="wörld", new="[forgotten]", needles=["wörld"], held=None)
    r.call("spans.rewrite", ident=read.id, find="notes", new="[forgotten]", needles=None, held=None)
    r.call("spans.rewrite", ident=shell.id, find="a.txt", new="[forgotten]", needles=None,
           held=['{"stale": "a.txt"}', None])
    r.call("spans.rewrite", ident=still.id, find="shell", new="[forgotten]", needles=["shell"], held=None)
    r.call("spans.rewrite", ident="no-such-span", find="a", new="b", needles=None, held=None)
    r.call("swap", table="spans", key="id", ident=send.id, columns=["error", "summary"], new=["cleared", None],
           held=None)
    r.call("swap", table="spans", key="id", ident=send.id, columns=["error", "summary"], new=["cleared", None],
           held=None)
    r.call("swap", table="spans", key="id", ident=send.id, columns=["error", "summary"], new=["again", "s"],
           held=["stale", "stale"])
    r.call("swap", table="spans", key="id", ident="no-such-span", columns=["error"], new=["x"], held=None)
    r.dump("spans")


def results_scenario(r: Run) -> None:
    for i, (task, parent, request, status) in enumerate((("A", None, "req-a", "done"), ("A.1", "A", "req-a", "done"),
                                                         ("B", None, "req-b", "failed"), ("C", None, "req-c", "running"),
                                                         ("D", None, "req-d", "interrupted"))):
        r.sql(TASK, task, task.split(".")[0], parent, request, f"goal of {task}", "supervisor", "[]", status,
              '{"n": %d}' % i, f"2026-02-0{i + 1}T10:00:00+00:00", f"2026-02-0{i + 1}T11:00:00+00:00")
    r.sql("INSERT INTO conversations (id) VALUES ('conv')")
    r.sql("INSERT INTO task_context (task_id, conversation_id, spec_json, owner_words) VALUES "
          "('A', 'conv', '{\"name\": \"Spec name\"}', 'words')")
    r.sql("INSERT INTO task_context (task_id, conversation_id, spec_json, owner_words) VALUES "
          "('B', 'conv', '{}', 'words')")
    r.sql(JOURNAL, "j-s1", "2026-02-01T09:00:00+00:00", "schedule.run", '{"schedule_id": "sch", "request_id": "req-a"}',
          "ferb")
    r.sql(JOURNAL, "j-s2", "2026-02-03T09:00:00+00:00", "schedule.run", '{"schedule_id": "sch", "request_id": "req-b"}',
          "ferb")
    r.call("results.record", task_id="A", at="2026-02-01T11:00:00+00:00", status="done", goal="goal of A",
           answer="All done, café.", shown=False, cards=[{"type": "file", "path": "/tmp/é.txt", "size": 1.5}],
           name="  Tidy \t the\n notes ")
    r.call("results.record", task_id="B", at="2026-02-03T11:00:00+00:00", status="failed", goal="goal of B",
           answer="It failed.", shown=True, cards=None, name="   ")
    r.call("results.record", task_id="A.1", at="2026-02-02T11:00:00+00:00", status="done", goal="step", answer="ok",
           shown=False, cards=[], name="Step")
    r.call("results.interrupted", task_id="D", at="2026-02-05T12:00:00+00:00", goal="goal of D", answer="A restart cut it")
    r.call("results.interrupted", task_id="A", at="2026-02-09T00:00:00+00:00", goal="x", answer="never replaces")
    r.dump("task_results", "task_result_names", "task_result_cards")
    r.call("results.unshown")
    r.call("results.page", before="2026-03-01", limit=10)
    r.call("results.page", before="2026-02-04T10:00:00+00:00", limit=2)
    r.call("results.latest_of_schedule", schedule_id="sch")
    r.call("results.latest_of_schedule", schedule_id="none")
    r.call("results.record_verdict", verdict_id="v-1", task_id="A", verdict="fail", note="half done", at="2026-02-02")
    r.call("results.record_verdict", verdict_id="v-2", task_id="A", verdict="pass", note="", at="2026-02-03")
    r.call("results.record_verdict", verdict_id="v-3", task_id="A", verdict="maybe", note="", at="2026-02-04")
    r.call("results.record", task_id="A", at="2026-02-06T11:00:00+00:00", status="done", goal="goal of A",
           answer="A later answer.", shown=False, cards=None, name="")
    r.call("results.mark_shown", task_ids=["A", "D", "no-such"])
    r.call("results.unshown")
    r.call("results.page", before="2026-03-01", limit=10)
    r.dump("task_results", "task_result_names", "task_result_cards", "owner_verdicts")


def uid(n: int) -> str:
    return f"00000000-0000-4000-8000-{n:012x}"


def chat_scenario(r: Run) -> None:
    r.sql("INSERT INTO attention_notices (id, source_refs_json, text, origin, created_at, quotes_json) VALUES "
          "('n-1', '[]', 'what FERB kept', 'ferb', '2026-11-09', '[\"a quote\", \"caf\\u00e9\"]')")
    day = "2026-11-09"
    entries = []
    for i in range(40):                                # one day, 40 entries: more than a page
        kind = "reply" if i % 4 == 3 else "turn"
        body: dict[str, Any] = {"text": f"entry {i} é SECRET", "n": i, "ratio": i / 8}
        if kind == "reply":
            body["turnID"] = uid(i - 3).lower() if i % 8 == 3 else uid(i - 3).upper()
        entries.append({"id": uid(i), "kind": kind, "at": f"2026-11-09T10:{i:02d}:00+00:00", "day": day,
                        "open": i == 38, "body": body})
    r.call("chat.write", entries=entries, replace=True)
    odd = [
        {"id": "{" + uid(100).upper() + "}", "kind": "turn", "at": "2026-11-08T23:30:00.5+05:30", "day": "2026-11-08",
         "body": {"text": "braces, a half-hour offset"}, "open": True, "ref": ""},
        {"id": "urn:uuid:" + uid(101), "kind": "notice", "at": "2026-11-08T18:00:00Z", "day": "2026-11-08",
         "body": {"text": "the app's old copy", "quotes": ["old"], "kept": 1}, "ref": "n-1"},
        {"id": uid(102).replace("-", ""), "kind": "notice", "at": "20261107T101112,123456789-0800", "day": "2026-11-07",
         "body": {"text": "no record of this one"}, "ref": "n-missing"},
        {"id": uid(103), "kind": "notice", "at": "2026-W45-6 08:00+00:00", "day": "2026-11-07", "body": {}},
        {"id": uid(104), "kind": "reply", "at": "2026-11-07T08:00:00+00:00:30", "day": "2026-11-07",
         "body": {"turnID": 12, "text": "a reply whose turn is a number"}},
        {"id": uid(105), "kind": "turn", "at": "2026-11-09T10:05:00+00:00", "day": day, "body": {"text": "same time"}},
    ]
    r.call("chat.write", entries=odd, replace=True)
    r.call("chat.write", entries=[{**entries[0], "body": {"text": "changed"}}, {**entries[38], "open": False}],
           replace=True)
    r.call("chat.write", entries=[{**entries[1], "body": {"text": "an import never takes back"}},
                                  {"id": uid(200), "kind": "turn", "at": "2026-11-06T00:00:00+00:00",
                                   "day": "2026-11-06", "body": {"text": "imported"}}], replace=False)
    good = {"id": uid(300), "kind": "turn", "at": "2026-11-05T00:00:00+00:00", "day": "2026-11-05", "body": {}}
    for bad in ("not a list", [good, "not an object"], [{**good, "id": "nope"}], [{**good, "id": None}],
                [{**good, "id": uid(1)[:-1]}], [{**good, "kind": "note"}], [{**good, "kind": None}],
                [{**good, "day": "2026-11-5"}], [{**good, "day": 20261105}], [{**good, "day": "2026-11-05 "}],
                [{**good, "body": []}], [{**good, "ref": 5}], [{**good, "ref": "r" * 201}], [{**good, "open": 1}],
                [{**good, "at": "2026-11-05T00:00:00"}], [{**good, "at": "yesterday"}], [{**good, "at": None}],
                [{**good, "at": 5}], [{**good, "at": "2026-11-05"}], [{**good, "at": "2026-13-05T00:00:00Z"}],
                [{**good, "at": "2026-11-05T00:00:00+24:00"}], [good] * 501):
        r.call("chat.write", entries=bad, replace=True)
    r.call("chat.write", entries=[{**good, "ref": "r" * 200, "day": "٢٠٢٦-١١-٠٥"}, {**good, "id": uid(301)}],
           replace=True)
    r.call("chat.write", entries=[], replace=True)
    r.dump("chat_history")
    first = r.call("chat.page", day=day, before=None, limit=30)
    second = r.call("chat.page", day=day, before=first["cursor"], limit=30)
    r.call("chat.page", day=day, before=second["cursor"], limit=30)
    r.call("chat.page", day=day, before="", limit=5)
    r.call("chat.page", day=day, before="2026-11-09T10:05:00.000000+00:00|" + uid(105).upper(), limit=3)
    r.call("chat.page", day=day, before="2026-11-09T10:20:00.000000+00:00", limit=200)
    for other in ("2026-11-08", "2026-11-07", "2026-11-01"):
        r.call("chat.page", day=other, before=None, limit=30)
    for limit in (0, 201, -1):
        r.call("chat.page", day=day, before=None, limit=limit)
    r.call("chat.page", day="2026-11-9", before=None, limit=30)
    r.call("chat.open")
    r.call("chat.days")
    r.call("chat.clear_notice", ref="n-1", text="(forgotten) é")
    r.call("chat.clear_notice", ref="n-none", text="x")
    r.call("chat.page", day="2026-11-08", before=None, limit=30)
    r.dump("chat_history")


def retention_scenario(r: Run) -> None:
    old, new = "2025-06-01T00:00:00+00:00", "2026-11-01T00:00:00+00:00"
    for task, parent, request, status, at in (
            ("OLD1", None, "rq-1", "done", "2025-05-01T00:00:00+00:00"), ("OLD1.1", "OLD1", "rq-1", "done", old),
            ("OLD2", None, "rq-2", "failed", "2025-05-02T00:00:00+00:00"), ("OLD3", None, "rq-3", "cancelled", old),
            ("RUN", None, "rq-4", "running", old), ("PAUSED", None, "rq-5", "paused", old),
            ("NEW", None, "rq-6", "done", new), ("BARE", None, "rq-7", "done", old)):
        r.sql(TASK, task, task.split(".")[0], parent, request, "g", "supervisor", "[]", status, "{}", at, at)

    def span(ident: str, parent: Any, task: Any, started: str = old, request: Any = None, kind: str = "tool.call") -> None:
        r.sql(SPAN, ident, parent, task, kind, started, None, json.dumps({"request_id": request} if request else {}),
              None, None, None)
    span("o1-root", None, "OLD1", kind="task")
    span("o1-a", "o1-root", "OLD1.1")
    span("o1-b", "o1-a", "OLD1.1")
    span("o1-c", "o1-b", None)                      # a child with no task id, three levels down
    span("o1-d", "o1-root", "OLD1")
    span("o1-intake", None, None, request="rq-1", kind="task.intake")
    span("o1-usage", "o1-intake", None, kind="model.usage")
    span("o2-root", None, "OLD2", kind="task")
    span("o2-x", None, "OLD2.verify")
    span("o3-root", None, "OLD3", kind="task")
    span("run-root", None, "RUN", kind="task")
    span("new-root", None, "NEW", started=new, kind="task")
    span("new-intake", None, None, request="rq-6", started=old, kind="task.intake")
    span("loose-old", None, None, kind="daemon")
    span("loose-kid", "loose-old", None)
    span("loose-new", None, None, started=new, kind="daemon")
    span("loose-req", None, None, request="rq-gone", kind="task.intake")
    span("OLD10-root", None, "OLD10", kind="task")   # another tree whose id starts the same
    for i, at in enumerate((old, old, new)):
        r.sql("INSERT INTO decisions (id, at, question, baseline, mode) VALUES (?, ?, 'q', 'b', 'shadow')", f"d{i}", at)
    for ident, task, at in (("a1", "OLD1.1", new), ("a2", "RUN.1", old), ("a3", "GONE.2", old), ("a4", "GONE", new),
                            ("a5", "NEW", old)):
        r.sql("INSERT INTO action_ledger (id, task_id, capability, target_json, payload_hash, state, created_at, "
              "updated_at) VALUES (?, ?, 'fs.write', '{}', 'h', 'done', ?, ?)", ident, task, at, at)
    for ident, tree, at in (("c1", "OLD2", new), ("c2", "NEW", old), ("c3", "GONE", old)):
        r.sql("INSERT INTO confirmations (id, tree_id, rule, key, granted, at) VALUES (?, ?, 'floor:pay', 'k', 1, ?)",
              ident, tree, at)
    for task, at in (("OLD1.1", new), ("PAUSED.1", old), ("GONE.1", old)):
        r.sql("INSERT INTO loop_state (task_id, step_id, updated_at) VALUES (?, 's', ?)", task, at)
    for tree, at in (("OLD3", new), ("RUN", old), ("GONE", old), ("GONE2", new)):
        r.sql("INSERT INTO shell_downloads (tree_id, bytes, at) VALUES (?, 10, ?)", tree, at)
    for task, status in (("OLD1", "done"), ("OLD2", "suspended"), ("OLD3.1", "failed"), ("RUN", "done"),
                         ("GONE", "done")):
        r.sql("INSERT INTO task_plans (task_id, plan, request, status) VALUES (?, '{}', 'r', ?)", task, status)
        r.sql("INSERT INTO plan_steps (task_id, step_id) VALUES (?, 's1')", task)
        r.sql("INSERT INTO plan_steps (task_id, step_id) VALUES (?, 's2')", task)
    for ident, state, fire, delivered in (("t1", "delivered", old, old), ("t2", "cancelled", old, None),
                                          ("t3", "pending", old, None), ("t4", "delivered", old, new),
                                          ("t5", "claimed", old, None)):
        r.sql("INSERT INTO timers (id, kind, fire_at, state, delivered_at, created_at) VALUES (?, 'reminder', ?, ?, ?, ?)",
              ident, fire, state, delivered, old)
    for timer, at in (("t1", old), ("t3", old), ("t-gone", old), ("t-gone2", new)):
        r.sql("INSERT INTO timer_deliveries (timer_id, at) VALUES (?, ?)", timer, at)
    for ident, at, kind, data in (
            ("jr1", old, "daemon.started", "{}"), ("jr2", new, "daemon.started", "{}"),
            ("jr3", old, "plan.step", '{"task_id": "OLD1.1"}'), ("jr4", old, "plan.step", '{"task_id": "RUN.2"}'),
            ("jr5", old, "plan.step", '{"task_id": "NEW"}'), ("jr6", old, "plan.step", '{"task_id": "GONE.1"}'),
            ("jr7", old, "chat.message", "{}"), ("jr8", old, "security.alignment", "{}"),
            ("jr9", old, "loop.round", '{"task_id": "OLD2"}'), ("jr10", old, "retention.purged", "{}"),
            ("jr11", "2025-06-01T05:30:00+05:30", "daemon.stopped", "{}"),
            ("jr12", "2025-06-01T00:00:00.5+00:00", "timer.failed", '{"task_id": null}')):
        r.sql(JOURNAL, ident, at, kind, data, "ferb")
    r.sql("INSERT INTO memory_sources (item_id, kind, source_id) VALUES ('m1', 'journal', 'jr10')")
    r.sql("INSERT INTO memory_sources (item_id, kind, source_id) VALUES ('m1', 'trail', 'jr1')")
    for cfg in ({}, {"retention.trace_days": 0, "retention.operational_days": 1, "sense.trail_retention_days": 45,
                     "retention.evidence_days": -3}):
        r.call("retention.windows", cfg=cfg)
        r.call("retention.describe", cfg=cfg)
    cutoff = "2026-01-01T00:00:00+00:00"
    r.call("retention.finished_roots_before", cutoff=cutoff)
    r.call("retention.finished_roots_before", cutoff="2025-05-01T12:00:00+00:00")
    r.call("retention.operational", cutoff=cutoff)
    r.call("retention.operational", cutoff="2025-06-01T00:00:00+00:00")
    r.call("retention.operational", cutoff="2027-01-01T00:00:00+00:00")
    r.call("retention.plan", trace=cutoff, decision=cutoff, operational=cutoff, limit=1)
    r.call("retention.plan", trace="2020-01-01", decision="2020-01-01", operational="2020-01-01", limit=5)
    plan = r.call("retention.plan", trace=cutoff, decision=cutoff, operational=cutoff, limit=50)
    r.call("retention.delete_rows", table="spans", key="id", ids=["o1-root"], batch=200)      # a parent first: refused
    r.dump("spans")
    for ids in plan.trees.values():
        r.call("retention.delete_rows", table="spans", key="id", ids=ids, batch=2)
    r.call("retention.delete_rows", table="spans", key="id", ids=plan.loose, batch=200)
    r.call("retention.delete_rows", table="decisions", key="rowid", ids=plan.decisions, batch=1)
    for table, ids in plan.operational.items():
        r.call("retention.delete_rows", table=table, key="rowid", ids=ids, batch=2)
    r.call("retention.delete_rows", table="spans", key="id", ids=[], batch=200)
    r.call("retention.plan", trace=cutoff, decision=cutoff, operational=cutoff, limit=50)
    r.dump("spans", "decisions", "action_ledger", "confirmations", "loop_state", "shell_downloads", "task_plans",
           "plan_steps", "timers", "timer_deliveries", "journal", "tasks", "memory_sources")


def cleanup_seed(r: Run, small: bool) -> None:
    r.sql("INSERT INTO decisions (id, at, question, baseline, mode) VALUES ('d1', '2026', 'q', 'b', 'shadow')")
    r.sql("INSERT INTO task_memory_refs (task_id, ref) VALUES ('T', 'memory:1')")
    r.sql("INSERT INTO owner_questions (id, at, kind, target, question, state) VALUES ('q1', '2026', 'folder_access', "
          "'/tmp', 'ok?', 'open')")
    node = ("INSERT INTO learning_nodes (id, kind, key, body_json, origin, made_by, recorded_at, valid_from) VALUES "
            "(?, ?, ?, '{}', 'ferb', ?, '2026', '2026')")
    r.sql(node, "card-1", "episode", "T", "code:card/2")
    for ref in ("trail:a", "trail:b", "entry:c"):
        r.sql("INSERT INTO learning_sources (node_id, source_ref) VALUES ('card-1', ?)", ref)
    if small:
        return
    r.sql("INSERT INTO decisions (id, at, question, baseline, mode) VALUES ('d2', '2026', 'q', 'b', 'shadow')")
    r.sql("INSERT INTO perception_batches (id, created_at, reason, state, event_ids_json) VALUES ('b1', '2026', 'r', "
          "'done', '[]')")
    r.sql("INSERT INTO perception_receipts (event_id, batch_id) VALUES ('e1', 'b1')")
    r.sql("INSERT INTO perception_labels (event_id, expected, owner_quote, at) VALUES ('e1', 'x', 'q', '2026')")
    r.sql("INSERT INTO notification_told (id, app, conversation, event_id, text, at) VALUES ('n1', 'a', 'c', 'e', 't', "
          "'2026')")
    r.sql("INSERT INTO learning_embedding_evals (id, model, dataset_hash, at, result_json) VALUES ('ev', 'm', 'h', "
          "'2026', '{}')")
    for i, relevance in enumerate(("high", None, "low")):
        r.sql("INSERT INTO trail (id, at, day, device_id, actor, source, data_json, relevance) VALUES (?, '2026', "
              "'2026-01-01', 'mac', 'owner', 'app', '{\"text\": \"x\"}', ?)", f"tr{i}", relevance)
    for i in range(3):
        r.sql(node, f"roll-{i}", "rollup", f"day-{i}", cleanup.ROLLUP_MAKER)
        r.sql("INSERT INTO learning_sources (node_id, source_ref) VALUES (?, 'entry:x')", f"roll-{i}")
        r.sql("INSERT INTO learning_changes (batch_id, node_id, action, at) VALUES ('b', ?, 'add', '2026')", f"roll-{i}")
    r.sql("INSERT INTO learning_vectors (node_id, model, vector_blob, month, text_hash) VALUES ('card-1', 'm', x'0102', "
          "'2026-01', 'h')")
    r.sql("INSERT INTO learning_jobs (source_ref, status, at) VALUES ('s', 'done', '2026')")
    r.sql("INSERT INTO notification_rules (id, app, owner_quote, created_at) VALUES ('r1', 'a', 'q', '2026')")
    r.sql("DROP TABLE learning_receipts")            # an install whose clean-up already dropped one


KEPT = ("decisions", "perception_receipts", "perception_batches", "perception_labels", "notification_told",
        "learning_vectors", "learning_embedding_evals", "task_memory_refs", "trail", "learning_nodes",
        "learning_sources", "learning_changes")


def cleanup_scenario(r: Run) -> None:
    cleanup_seed(r, small=False)
    plan = r.call("cleanup.plan")
    r.call("cleanup.preview", free=12345)
    r.call("cleanup.pending")
    r.call("cleanup.apply", ok="0000000000000000")
    r.dump(*KEPT)
    r.call("cleanup.apply", ok=plan["ok"], stop_after=4)
    r.dump(*KEPT)
    again = r.call("cleanup.plan")
    r.call("cleanup.apply", ok=plan["ok"])                       # the first OK no longer matches what is left
    r.call("cleanup.apply", ok=again["ok"], free=0)              # everything goes; no room to compact
    r.dump(*KEPT)
    r.steps.append({"tables": sorted(x[0] for x in r.db.conn.execute(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('owner_questions', 'notification_rules', "
        "'learning_receipts', 'learning_jobs')"))})
    last = r.call("cleanup.plan")
    r.call("cleanup.pending")
    r.call("cleanup.apply", ok=last["ok"])                       # nothing left, the file already incremental


def cleanup_compact_scenario(r: Run) -> None:
    cleanup_seed(r, small=True)
    plan = r.call("cleanup.plan")
    r.call("cleanup.apply", ok=plan["ok"])                       # removes, then compacts
    r.call("cleanup.pending")
    r.dump(*KEPT)


def scenario(build: Any) -> list[dict[str, Any]]:
    with tempfile.TemporaryDirectory() as folder:
        r = Run(Path(folder))
        build(r)
        r.db.close()
        return r.steps


# ---------- salvage: files in a temporary home ----------

def salvage_scenario() -> list[dict[str, Any]]:
    steps: list[dict[str, Any]] = []
    with tempfile.TemporaryDirectory() as folder:
        home = Path(folder).resolve() / "home"
        home.mkdir()
        minute = {"n": 0}

        def now() -> datetime:
            minute["n"] += 1
            return datetime(2026, 11, 1, 2, 15, minute["n"] % 60, 123456)

        def clean(value: Any) -> Any:
            return json.loads(json.dumps(norm(value)).replace(json.dumps(str(home))[1:-1], "<HOME>"))

        def listing() -> dict[str, Any]:
            out: dict[str, Any] = {}
            for p in sorted(home.rglob("*")):
                if p.is_file():
                    name = str(p.relative_to(home))
                    text = name in (salvage.PENDING, str(salvage.CHECKED))
                    out[name] = clean(p.read_text()) if text else len(p.read_bytes())
            return out

        def put(name: str, data: bytes) -> None:
            (home / name).parent.mkdir(parents=True, exist_ok=True)
            (home / name).write_bytes(data)
            steps.append({"put": name, "b64": base64.b64encode(data).decode()})

        def call(name: str, fn: Any, /, **args: Any) -> Any:
            step: dict[str, Any] = {"call": name, "args": args}
            ret = None
            try:
                ret = fn()
                step["ret"] = clean(ret)
            except Exception as e:  # noqa: BLE001
                step["err"] = [type(e).__name__, clean(str(e))]
            step["files"] = listing()
            steps.append(step)
            return ret

        def sqlite_file(build: Any) -> bytes:
            with tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp) / "x.db"
                conn = sqlite3.connect(path)
                build(conn)
                conn.commit()
                conn.close()
                return path.read_bytes()

        def plain(conn: Any) -> None:
            conn.execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT)")
            conn.executemany("INSERT INTO notes (text) VALUES (?)", [("note %d " % i * 40,) for i in range(200)])
            conn.execute("CREATE INDEX notes_text ON notes(text)")

        def newer(conn: Any) -> None:
            conn.execute("CREATE TABLE schema_version (version INTEGER NOT NULL, sha256 TEXT)")
            conn.execute("INSERT INTO schema_version VALUES (9999, NULL)")

        def current(conn: Any) -> None:
            conn.execute("CREATE TABLE schema_version (version INTEGER NOT NULL, sha256 TEXT)")
            conn.execute("INSERT INTO schema_version VALUES (3, NULL)")

        call("check_home", lambda: salvage.check_home(home, "b1", now), build="b1")       # an empty home
        put("ferb.db", b"")
        call("check_store", lambda: salvage.check_store(home, "b1", now), build="b1")     # an empty file: nothing yet
        put("ferb.db", sqlite_file(plain))
        call("damage", lambda: salvage.damage(home / "ferb.db", False), name="ferb.db", full=False)
        call("damage", lambda: salvage.damage(home / "ferb.db", True), name="ferb.db", full=True)
        call("check_store", lambda: salvage.check_store(home, "", now), build="")
        call("check_store", lambda: salvage.check_store(home, "b1", now), build="b1")     # the full check, marked
        call("check_store", lambda: salvage.check_store(home, "b1", now), build="b1")
        call("check_store", lambda: salvage.check_store(home, "b2", now), build="b2")
        call("refuse_newer", lambda: salvage.refuse_newer(home))
        put("ferb.db", sqlite_file(current))
        call("refuse_newer", lambda: salvage.refuse_newer(home))
        call("check_store", lambda: salvage.check_store(home, "b2", now), build="b2")
        put("ferb.db", sqlite_file(newer))
        call("refuse_newer", lambda: salvage.refuse_newer(home))
        call("damage", lambda: salvage.damage(home / "ferb.db", False), name="ferb.db", full=False)
        call("check_store", lambda: salvage.check_store(home, "b3", now), build="b3")
        call("check_home", lambda: salvage.check_home(home, "b3", now), build="b3")
        put("ferb.db", b"this is not a database at all, only some invented text " * 40)
        put("ferb.db-wal", b"wal bytes")
        put("ferb.db-shm", b"shm bytes")
        call("refuse_newer", lambda: salvage.refuse_newer(home))
        call("damage", lambda: salvage.damage(home / "ferb.db", False), name="ferb.db", full=False)
        first = call("check_store", lambda: salvage.check_store(home, "b3", now), build="b3")
        put("ferb.db", b"SQLite format 3\x00" + b"\x07" * 5000)                           # a header, then nonsense
        put("ferb.db-wal", b"second wal")
        second = call("check_store", lambda: salvage.check_store(home, "b3", now), build="b3")
        whole = sqlite_file(plain)
        put("ferb.db", whole[:4096 * 3 + 40] + b"\xff" * 3000 + whole[4096 * 3 + 3040:])   # a damaged page in a real file
        call("damage", lambda: salvage.damage(home / "ferb.db", False), name="ferb.db", full=False)
        call("damage", lambda: salvage.damage(home / "ferb.db", True), name="ferb.db", full=True)
        call("check_store", lambda: salvage.check_store(home, "b3", now), build="b3")
        call("damage", lambda: salvage.damage(home / "missing.db", False), name="missing.db", full=False)
        call("refuse_newer", lambda: salvage.refuse_newer(home))                          # no database now
        toml_bad = 'a = "unclosed\n[section\n'
        import tomlkit
        try:
            tomlkit.parse(toml_bad)
            raise SystemExit("the invented config was meant not to parse")
        except ValueError as e:
            toml_message = str(e)
        steps.append({"toml": {toml_bad: toml_message}})
        put("config.toml", b'home = "x"\n[retention]\ntrace_days = 30\n')
        put("owner-secret-handles.json", b'{"names": ["wifi"]}')
        put("persona.json", b'{"apps": []}')
        call("check_files", lambda: salvage.check_files(home, now))
        put("config.toml", toml_bad.encode())
        put("owner-secret-handles.json", b'{"names": ["wifi",]}')
        put("persona.json", b"{nope")
        third = call("check_files", lambda: salvage.check_files(home, now))
        put("config.toml", b"\xff\xfe not text")
        put("owner-secret-handles.json", b"\xe2\x82")
        call("check_files", lambda: salvage.check_files(home, now))
        put("owner-secret-handles.json", b"   \n\t ")
        call("check_home", lambda: salvage.check_home(home, "b3", now), build="b3")
        call("pending", lambda: salvage.pending(home))
        entries = salvage.pending(home)
        call("notice", lambda: [salvage.notice(e) for e in entries])
        call("describe", lambda: salvage.describe(entries))
        call("told", lambda: salvage.told(home, (first or []) + (third or [])), which="first+third")
        call("told", lambda: salvage.told(home, second or []), which="second")
        call("told", lambda: salvage.told(home, entries), which="all")
        call("told", lambda: salvage.told(home, entries), which="all")
        put("set-aside.json", b'{"not": "a list"}')
        call("pending", lambda: salvage.pending(home))
        put("set-aside.json", b'[{"moved_to": "/x"}, {"moved_to": 5}, "text", {"file": "f"}]')
        call("pending", lambda: salvage.pending(home))
        put("run/store-checked.json", b"[1]")
        put("ferb.db", sqlite_file(plain))
        call("check_store", lambda: salvage.check_store(home, "None", now), build="None")
    return steps


# ---------- what Python itself says ----------

TIMES = [
    "2026-11-09T10:11:12+00:00", "2026-11-09T10:11:12Z", "2026-11-09 10:11:12.5+05:30", "2026-11-09T10:11:12.123456789-08:00",
    "2026-11-09T10:11:12,25+00:00", "2026-11-09T10+01", "2026-11-09T1011+0100", "2026-11-09T101112+010203",
    "2026-11-09T10:11+01:02:03.000004", "20261109T101112+00:00", "20261109", "2026-11-09", "2026-W45", "2026W45",
    "2026-W45-1", "2026W451", "2026-W45-1T10:00+00:00", "2026W451T1000Z", "2020-W53-7T00:00:00+00:00", "2021-W53-1",
    "2026-W00-1", "2026-W45-8", "2026-W45-0", "2026-11-09T", "2026-11-09T10:", "2026-11-09T10:11:", "2026-11-09T10:11:12.",
    "2026-11-09T10:11:12.+00:00", "2026-11-09T10:11:12:99", "2026-11-09T10:11:12.12a", "2026-11-09T24:00:00+00:00",
    "2026-11-09T23:60:00+00:00", "2026-11-09T23:59:60+00:00", "2026-02-29T00:00:00+00:00", "2024-02-29T00:00:00+00:00",
    "2026-00-10T00:00:00Z", "0000-01-01T00:00:00+00:00", "0001-01-01T00:00:00+00:00", "9999-12-31T23:59:59.999999+00:00",
    "2026-11-09T10:11:12+24:00", "2026-11-09T10:11:12-23:59:59.999999", "2026-11-09T10:11:12+00:00:00.5",
    "2026-11-09T10:11:12-00:00", "2026-11-09T10:11:12 +00:00", "2026-11-09T10:11:12+0", "2026-11-09T10:11:12+00:0",
    "2026-11-09T10:11:12ZZ", "2026-11-09T10:11:12z", "2026-11-09X10:11:12+00:00", "2026-11-09é10:11:12+00:00",
    "2026-11-09日10:11:12+00:00", "2026-11-0910:11:12+00:00", "2026-1-09T10:11:12+00:00", "2026/11/09T10:11:12+00:00",
    "", "None", "5", "2026-11", "2026-11-", "2026-11-0", "2026-110", "2026-11-09T1", "2026-11-09T10:1", "2026-11-09T10:11:1",
    "2026-11-09T1011123+00:00", "2026-11-09T10:1112+00:00", "2026-11-09T10:11:12.1234567", " 2026-11-09T10:11:12+00:00",
    "2026-11-09T10:11:12+00:00 ", "２０２６-11-09T10:11:12+00:00", "2026-11-09T10:11:12.5", "2026-11-09T10:11:12−05:00",
    "1970-01-01T00:00:00+00:00", "2000-02-29T12:00:00.000001+13:45", "1900-03-01T00:00:00-12:00", "2026-12-31T23:59:59+00:00",
    "2026-W53-1", "2015-W53-7T23:59:59Z", "2026-11-09T10:11:12.000000+00:00", "2026-11-09T10:11:12.999999+00:00:01",
]

JSON_TEXTS = [
    "", " ", "{", "}", "[", "]", "[1,", "[1,]", "[1 2]", "{\"a\"", "{\"a\":", "{\"a\":1", "{\"a\":1,", "{\"a\":1,}", "{a:1}",
    "{'a':1}", "{\"a\" 1}", "{\"a\":1 \"b\":2}", "nul", "null", "true", "fals", "NaN", "Infinity", "-Infinity", "-", "-x",
    "01", "1.", "1.5", "1e", "1e5", "1E+5", "-0", ".5", "+1", "0x10", "\"abc", "\"a\\qb\"", "\"a\\u12\"", "\"a\\u12g4\"",
    "\"a\\u1234\"", "\"\\ud83d\\ude00\"", "\"\\ud83d\"", "\"\\ud83d\\u12\"", "\"\\ud83dxx\"", "\"\\ud83d\\uzzzz\"",
    "\"\\ud83d\\u0041\"", "\"a\nb\"", "\"a\tb\"", "\"a\\", "\"\\u", "\"\\u1234", "\"é\"", "[1] x", "{} {}", "[\n  1,\n  oops\n]",
    "{\"a\": [1, {\"b\": nope}]}", "\ufeff{}", "  {\"a\": [1, 2.5e-3, \"x\", null, true, false, {}]}  ", "[[]]", "[[[",
    "\"\\/\\b\\f\\n\\r\\t\\\"\\\\\"", "tru", "t", "n", "N", "I", "[NaN, -Infinity]", "{\"a\":1}\n\n\nx", "[1,\n2,\n,3]",
    "1 2", "\"a\" \"b\"", "{\"\":}", "[\"\\x\"]", "{nope", "   \n\t ", "{\"names\": [\"wifi\",]}", "٣", "[١]", "1e٣",
]

BYTES = [b"\xff\xfe not text", b"\xe2\x82", b"ok \xc3\xa9", b"a\x80", b"\xc3(", b"\xe2\x28\xa1", b"\xf0\x9f\x98", b"\xc0\xaf",
         b"\xed\xa0\x80", b"\xf5\x80\x80\x80", b"\xf4\x90\x80\x80", b"abc\xe2\x82\xacdef\xc2", b"\xe0\x80\x80", b"\xf0\x80\x80\x80"]

VALUES = [
    None, True, 0, -5, 2 ** 53, 1.0, 0.1, 1e16, 1e-7, -2.5e-5, 1e300, "", "é😀\u007f\u0001\"\\/\n\r\t\b\f\u2028", [], {},
    [1, [2, [3, []]], {}], {"b": 1, "a": {"z": [1, 2], "é": None}, "": ""}, {"k": [{"x": 1.5}, {"y": [True, False]}]},
]


def python_says() -> dict[str, Any]:
    def time(text: str) -> Any:
        try:
            when = datetime.fromisoformat(text)
        except ValueError:
            return None
        out: dict[str, Any] = {"iso": when.isoformat(), "seconds": when.isoformat(timespec="seconds"),
                               "minute": f"{when:%Y%m%d-%H%M}", "aware": when.tzinfo is not None}
        if when.tzinfo is not None:
            try:
                out["utc"] = when.astimezone(timezone.utc).isoformat(timespec="microseconds")
            except OverflowError:
                out["utc"] = None
        return out

    def loads(text: str) -> Any:
        try:
            json.loads(text)
        except ValueError as e:
            return str(e)
        return None

    def decode(data: bytes) -> Any:
        try:
            data.decode()
        except ValueError as e:
            return str(e)
        return None

    return {
        "times": [[t, time(t)] for t in TIMES],
        "json": [[t, loads(t)] for t in JSON_TEXTS],
        "bytes": [[d.hex(), decode(d)] for d in BYTES],
        "dumps": [[v, json.dumps(v), json.dumps(v, ensure_ascii=False), json.dumps(v, sort_keys=True),
                   json.dumps(v, ensure_ascii=False, indent=2)] for v in VALUES],
    }


def unicode_table() -> None:
    ranges: list[tuple[int, int]] = []
    start = None
    for c in range(0x110000):
        nd = unicodedata.category(chr(c)) == "Nd"
        if nd and start is None:
            start = c
        if not nd and start is not None:
            ranges.append((start, c - 1))
            start = None
    out = [f"//! Generated by rust/fixtures/store/make_tables.py from Python's Unicode tables ({unicodedata.unidata_version}): the",
           "//! decimal digits (category Nd), which is what `\\d` matches in Python's `re` on text.", "",
           f"const ND: [(u32, u32); {len(ranges)}] = ["]
    out += [f"    (0x{a:04x}, 0x{b:04x})," for a, b in ranges]
    out += ["];", "", "/// Whether Python's `\\d` matches `c`.", "pub fn is_decimal(c: char) -> bool {",
            "    let c = c as u32;", "    ND.binary_search_by(|&(low, high)| {", "        if c < low {",
            "            std::cmp::Ordering::Greater", "        } else if c > high {",
            "            std::cmp::Ordering::Less", "        } else {", "            std::cmp::Ordering::Equal",
            "        }", "    })", "    .is_ok()", "}", ""]
    (REPO / "rust/crates/ferb-py/src/unicode_nd.rs").write_text("\n".join(out))


def main() -> None:
    fixture = {
        "made_by": "rust/fixtures/store/make_tables.py", "python_sqlite": sqlite3.sqlite_version,
        "python": sys.version.split()[0], "reversible": sorted(REVERSIBLE),
        "operational_kinds": sorted(retention.OPERATIONAL_KINDS),
        "rules": [dataclasses.asdict(rule) for rule in retention.RULES],
        "cleanup": {"emptied": cleanup.EMPTIED, "dropped": cleanup.DROPPED, "trimmed": sorted(cleanup.TRIMMED),
                    "protected": cleanup.PROTECTED, "protected_rows": cleanup.PROTECTED_ROWS},
        "scenarios": {"spans": scenario(spans_scenario), "results": scenario(results_scenario),
                      "chat": scenario(chat_scenario), "retention": scenario(retention_scenario),
                      "cleanup": scenario(cleanup_scenario), "cleanup_compact": scenario(cleanup_compact_scenario)},
        "salvage": salvage_scenario(),
        "python_says": python_says(),
    }
    unicode_table()
    out = HERE / "tables.json"
    out.write_text(json.dumps(fixture, indent=1, ensure_ascii=False) + "\n")
    calls = sum(1 for steps in fixture["scenarios"].values() for s in steps if "call" in s)
    print(f"wrote {out}: {calls} store calls in {len(fixture['scenarios'])} scenarios, "
          f"{len(fixture['salvage'])} salvage steps")


if __name__ == "__main__":
    main()
