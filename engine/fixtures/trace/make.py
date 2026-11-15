"""Fixtures for the Rust port of the trace composer and renderers (rust/crates/ferb-trace), made by the Python code
(ENG-3).

    cd <repo> && uv run python rust/fixtures/trace/make.py

Builds invented span trees through the real SpanStore in a temporary home (never ~/.ferb) with a settable clock and
sequential ids, plus the span tables of the recorded chat scenarios (rust/fixtures/daemon/chat_scenarios.json) loaded
into a temporary database. For every tree it records what the Python code produced: `compose.compose` (the episode),
`timing.breakdown` and `timing.lines`, and the exact bytes `ferb trace` prints in each mode (`cli.admin.trace` run on
a fake client: the story, --full, --json, and the raw tree when nothing was composed). Separate files record the
`lines.py` helpers and `render.story` on hand-made episodes. Writes the same bytes on every run. All data invented.
"""

import hashlib
import io
import json
import random
import sqlite3
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.dont_write_bytecode = True
sys.path.insert(0, str(REPO))

TMP = Path(tempfile.mkdtemp(prefix="ferbtrace-"))
import os  # noqa: E402

os.environ["FERB_HOME"] = str(TMP / "home")
os.environ["HOME"] = str(TMP / "user")
os.environ["TZ"] = "UTC"
(TMP / "user").mkdir()

from ferb.cli import admin  # noqa: E402
from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.store import spans as spans_mod  # noqa: E402
from ferb.store.db import Database  # noqa: E402
from ferb.trace import compose, lines, render, timing  # noqa: E402

BASE = datetime(2026, 3, 4, 9, 0, 0, tzinfo=timezone.utc)


def t(sec: float, zone: timezone = timezone.utc) -> datetime:
    return (BASE + timedelta(microseconds=round(sec * 1_000_000))).astimezone(zone)


class Clock:
    def __init__(self) -> None:
        self.value = BASE

    def now(self) -> datetime:
        return self.value


class World:
    """One temporary database with a settable clock; spans, journal rows, task rows and verdicts as the daemon keeps them."""

    count = 0

    def __init__(self) -> None:
        World.count += 1
        self.db = Database(TMP / f"w{World.count}.db")
        self.db.migrate()
        self.clock = Clock()
        self.spans = spans_mod.SpanStore(self.db, self.clock, SequentialIds("s"), lambda v: v)
        self.n = 0

    def span(self, kind: str, a: float | datetime, b: float | datetime | None = None, *, inp: dict | None = None,
             out: Any = None, err: str | None = None, parent: Any = None, task: str | None = None,
             origin: str | None = None, early_origin: str | None = None, cost: float | None = None,
             summary: str | None = None) -> Any:
        self.clock.value = a if isinstance(a, datetime) else t(a)
        s = self.spans.start(kind, input=inp if inp is not None else {}, task_id=task,
                             parent_id=parent.id if parent is not None else None)
        if early_origin is not None:
            self.spans.set_origin(s.id, early_origin)
        if b is not None:
            self.clock.value = b if isinstance(b, datetime) else t(b)
            self.spans.finish(s, output=out, error=err, cost_usd=cost, origin=origin, summary=summary)
        return s

    def journal(self, kind: str, data: dict, at: str = "2026-03-04T09:00:00+00:00") -> None:
        self.n += 1
        self.db.conn.execute("INSERT INTO journal (id, at, kind, data_json) VALUES (?, ?, ?, ?)",
                             (f"j-{self.n:04d}", at, kind, json.dumps(data)))

    def task_row(self, task: str, request_id: str, goal: str, status: str, outcome: dict | None = None,
                 tree: str | None = None) -> None:
        self.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES (?, ?, NULL, ?, ?, 'i', 'worker', '[]', ?, ?, ?, ?)",
            (task, tree or task, request_id, goal, status, json.dumps(outcome or {}),
             "2026-03-04T09:00:00+00:00", "2026-03-04T09:00:00+00:00"))

    def verdict(self, task: str, verdict: str, note: str, at: str) -> None:
        self.n += 1
        self.db.conn.execute("INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES (?, ?, ?, ?, ?)",
                             (f"v-{self.n}", task, verdict, note, at))


def result_for(w: World, task_id: str) -> dict[str, Any]:
    """The `trace.get` answer as the daemon builds it (daemon/areas/rpc.py `_trace`), the episode composed live."""
    spans = w.spans.tree_json(task_id)
    outcome = w.spans.outcome(task_id)
    if not outcome.get("result"):
        row = w.db.conn.execute("SELECT outcome_json FROM tasks WHERE id = ?", (task_id,)).fetchone()
        if row is not None and row[0] != "{}":
            outcome["result"] = json.loads(row[0])
    episode = None
    if spans and not all(s["kind"] == "model.learning" for s in spans):
        episode = compose.compose({"task_id": task_id, "spans": spans, "outcome": outcome})
    return {"task_id": task_id, "spans": spans, "outcome": outcome, "episode": episode}


class Client:
    def __init__(self, result: dict[str, Any]) -> None:
        self.result = result

    def call(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        assert method == "trace.get"
        return self.result


def cli(result: dict[str, Any], full: bool, raw: bool) -> str:
    out = io.StringIO()
    admin.trace(Client(result), result["task_id"], out, full, raw)  # type: ignore[arg-type]
    return out.getvalue()


CASES: list[dict[str, Any]] = []


def record(name: str, result: dict[str, Any], note: str = "", composed: bool = True) -> None:
    """One case: the input (task_id, spans, outcome), the episode Python composed, the breakdown, and every output."""
    breakdown = timing.breakdown(result["spans"])
    case: dict[str, Any] = {
        "name": name, "note": note, "task_id": result["task_id"], "spans": result["spans"],
        "outcome": result["outcome"], "episode": result["episode"], "has_episode": result["episode"] is not None,
        "composed": composed,
        "breakdown": breakdown, "timing_lines": timing.lines(breakdown),
        "story": cli(result, False, False),
    }
    # the long outputs are kept as text for the invented trees; for the recorded scenarios (big) as a hash and length
    for key, text in (("full", cli(result, True, False)), ("json", cli(result, False, True))):
        if name.startswith("scenario_") or len(text) > 30000:
            case[key + "_sha256"] = hashlib.sha256(text.encode()).hexdigest()
            case[key + "_len"] = len(text.encode())
        else:
            case[key] = text
    if result["episode"] is not None:
        again = compose.compose({"task_id": result["task_id"], "spans": result["spans"], "outcome": result["outcome"]})
        assert not composed or again == result["episode"]
        # the same tree with the episode withheld: the raw tree in short form
        bare = dict(result, episode=None)
        case["plain"] = cli(bare, False, False)
    else:
        case["plain"] = case["story"]
    CASES.append(case)


# ---------------------------------------------------------------------------------------------------- shapes

def root(w: World, task: str, a: float, b: float | None, *, request: str = "Find the cheapest flight to Lisbon",
         words: str | None = None, goal: str = "Find the cheapest flight to Lisbon",
         conditions: tuple[str, ...] = ("The price is quoted", "The date is checked"), out: Any = "default",
         origin: str | None = "owner", rid: str = "r1") -> Any:
    spec = {"goal": goal, "success_conditions": [{"text": c} for c in conditions]}
    inp = {"request_id": rid, "request": request, "authority_words": request if words is None else words,
           "task_spec": spec}
    if out == "default":
        out = {"status": "done", "verified": True, "summary": "Booked nothing; found the fare.", "how": "searched"}
    return w.span("task", a, b, inp=inp, out=out, task=task, early_origin=origin, origin=origin)


def planner(w: World, task: str, a: float, b: float | None, payload: Any, parent: Any = None, err: str | None = None,
            kind: str = "model.turn", rid: str = "r1", timing_: dict | None = None, role: str = "supervisor") -> Any:
    out = {"text": payload if isinstance(payload, str) else json.dumps(payload)}
    if timing_:
        out["timing"] = timing_
    return w.span(kind, a, b, inp={"role": role, "request_id": rid}, out=out if b is not None else None, err=err,
                  parent=parent, task=task)


def worker(w: World, task: str, a: float, b: float | None, report: Any, parent: Any = None, role: str = "worker",
           status: str | None = None, err: str | None = None, timing_: dict | None = None) -> Any:
    out: dict[str, Any] = {"result": report if isinstance(report, str) else json.dumps(report)}
    if status:
        out["status"] = status
    if timing_:
        out["timing"] = timing_
    return w.span("worker", a, b, inp={"role": role, "goal": "Look up the fare"}, out=out if b is not None else None,
                  err=err, parent=parent, task=task)


def tool(w: World, parent: Any, task: str, a: float, b: float | None, name: str, args: dict, out: Any = None, *,
         why: str = "", kind: str = "read", err: str | None = None, origin: str | None = "external:web:example.com",
         point_checks: list | None = None, early: str | None = None) -> Any:
    inp: dict[str, Any] = {"tool": name, "args": args, "kind": kind}
    if why:
        inp["why"] = why
    if point_checks:
        inp["point_checks"] = point_checks
    return w.span("tool.call", a, b, inp=inp, out=out if out is not None else {"ok": True}, err=err,
                  parent=parent, task=task, origin=origin)


def usage(w: World, parent: Any, task: str, at: float, **fields: Any) -> Any:
    out = {"input_tokens": 1200, "cached_input_tokens": 300, "output_tokens": 150, "cost_usd": 0.0123,
           "model": "model-a", "billing": "api"} | fields
    return w.span("model.usage", at, at, inp={}, out=out, parent=parent, task=task)


def checker(w: World, task: str, a: float, b: float | None, results: list | None, parent: Any = None,
            nxt: str | None = None, err: str | None = None) -> Any:
    body: dict[str, Any] = {"results": results or []}
    if nxt:
        body["next"] = nxt
    return worker(w, task, a, b, body, parent=parent, role="verifier", err=err)


def finish_outcome(w: World, task: str, rid: str, text: str, status: str = "done", verification: list | None = None,
                   answer: str | None = None) -> None:
    w.task_row(task, rid, "goal", status, {})
    rec: dict[str, Any] = {"task_id": task, "status": status}
    if verification is not None:
        rec["verification"] = verification
    w.journal("task.outcome", rec)
    if answer is not None:
        w.journal("task.answer", {"task_id": task, "text": answer})


def typical(name: str = "typical", verified: bool = True) -> None:
    w, T = World(), "t-typical"
    r = root(w, T, 0, 42.5)
    w.journal("chat.message", {"request_id": "r1", "text": "Find the cheapest flight to Lisbon"})
    planner(w, T, 1, 3.25, {"kind": "task", "text": "ok", "task": {"goal": "Find the cheapest flight to Lisbon"}})
    wk = worker(w, T + ".1", 4, 30, {"status": "done", "summary": "The fare is 89 euro on the 12th."}, parent=r)
    tool(w, wk, T + ".1", 5, 7.5, "browser.open", {"url": "https://example.com/flights", "tab": "t1"},
         {"title": "Flights", "url": "https://example.com/flights"}, why="open the airline page")
    tool(w, wk, T + ".1", 8, 12.75, "browser.read", {"action": "text", "element": "e4"},
         {"output": "89 euro\nSat 12\n"}, kind="read")
    usage(w, wk, T + ".1", 20, input_tokens=1234, cached_input_tokens=1000, output_tokens=987, cost_usd=0.5)
    checker(w, T + ".1.verify", 31, 40, [{"met": True, "evidence": "The page showed 89 euro"},
                                          {"met": True, "evidence": "Date was read from the page"}], parent=r)
    ver = [{"condition": "The price is quoted", "met": True, "evidence": "the page said 89 euro"},
           {"condition": "The date is checked", "met": True, "evidence": "Sat 12"}]
    finish_outcome(w, T, "r1", "ok", verification=ver, answer="The cheapest fare is 89 euro on the 12th.")
    record(name, result_for(w, T))


typical()


def nested_tools() -> None:
    w, T = World(), "t-nested"
    r = root(w, T, 0, 61.0, request="Update the notes file", goal="Update the notes file", conditions=("File saved",))
    planner(w, T, 0.5, 2.0, {"kind": "task", "task": {"goal": "Update the notes file"}})
    wk = worker(w, T + ".1", 3, 58.5, {"status": "done", "summary": "Saved."}, parent=r)
    outer = tool(w, wk, T + ".1", 4, 20, "shell.run", {"command": "ls -la ~/Documents && cat notes.txt", "why": "x"},
                 {"exit_code": 0, "output": "\x1b[1mtotal 8\x1b[0m\n-rw-r--r--  1 u  s  12 notes.txt\r\n"
                                           "\x1b]0;title\x07\n\n──────\n.\nok\nok\nlast line one\nlast line two\n"
                                           "l3\nl4\nl5\nl6\nl7\nl8", "running": False},
                 why="look at the folder first", kind="shell")
    tool(w, outer, T + ".1", 5, 6.2, "fs.list", {"path": "~/Documents"},
         {"entries": ["a", "b", "c"], "total": 40}, kind="read")
    tool(w, outer, T + ".1", 6.3, 8, "fs.read", {"path": "~/Documents/notes.txt"},
         {"size": 12, "text": "hello world!", "offset": 0, "next_offset": None})
    tool(w, wk, T + ".1", 21, 25, "fs.edit", {"path": "~/Documents/notes.txt", "text": "x" * 300},
         {"path": "~/Documents/notes.txt"}, kind="write")
    tool(w, wk, T + ".1", 26, 26.0001, "mac.scroll", {"action": "scroll", "direction": "up", "x": 10, "y": 20},
         {"ok": True}, kind="click")
    tool(w, wk, T + ".1", 27, 27.5, "mac.scroll", {"action": "scroll", "direction": "diagonal"}, {"ok": True})
    tool(w, wk, T + ".1", 28, 29, "mac.scroll", {"action": "scroll"}, {"ok": True})
    tool(w, wk, T + ".1", 30, 31, "browser.click", {"action": "click", "element": "e1"}, None, err="Element e1 is gone",
         point_checks=["hit_test", "window_list"], kind="click")
    tool(w, wk, T + ".1", 32, 33, "browser.click", {"action": "click", "element": "e2"},
         {"title": "Done", "url": "https://example.com/done", "detail": "ok", "opened": True, "matches": 3,
          "met": None, "path": "/p", "error": ""}, point_checks=["hit_test", "refused: cant_tell"], kind="click")
    tool(w, wk, T + ".1", 34, None, "shell.run", {"command": "sleep 100"}, kind="shell")          # never finished
    tool(w, wk, T + ".1", 40, 41, "mac.clipboard", {"text": "abc"}, {"characters": 3})
    tool(w, wk, T + ".1", 41.5, 42, "shell.send", {"id": "p1", "text": "y" * 200}, {"sent": True})
    tool(w, wk, T + ".1", 43, 44, "owner.ask", {"question": "Which file?"}, {"answer": "notes", "answered_by": "owner"})
    tool(w, wk, T + ".1", 44, 45, "owner.ask", {"question": "Which file?", "confirm": True},
         {"answer": "yes " * 60, "answered_by": "orchestrator"})
    tool(w, wk, T + ".1", 45, 46, "owner.ask", {"question": "Which?"}, {"declined": True})
    tool(w, wk, T + ".1", 46, 47, "unknown.tool", {"id": 7, "tab": 3, "x": 0}, {"b": 1, "a": 2})
    tool(w, wk, T + ".1", 47, 48, "unknown.empty", {}, {})
    tool(w, wk, T + ".1", 48, 49, "fs.write", {"path": "/tmp/x"}, {"bytes": 12})
    tool(w, wk, T + ".1", 49, 50, "fs.trash", {"path": "/tmp/y"}, {})
    tool(w, wk, T + ".1", 50, 51, "fs.list", {"path": "/tmp"}, {"entries": 7})                    # len(int) raises
    tool(w, wk, T + ".1", 51, 52, "fs.list", {"path": "/tmp"}, {"entries": "abc", "total": True})
    tool(w, wk, T + ".1", 52, 53, "shell.wait", {"id": "p1"}, {"running": True, "id": "p1"})
    tool(w, wk, T + ".1", 53, 54, "shell.wait", {"id": "p1"}, {"exit_code": 1, "output": "a\n\n b\n", "capture_error": "x"})
    tool(w, wk, T + ".1", 54, 55, "fs.read", {"path": "/a"}, {"size": 100, "text": "héllo", "offset": 5, "next_offset": 9})
    finish_outcome(w, T, "r1", "ok", answer="Saved the notes.")
    record("nested_tools", result_for(w, T))


nested_tools()


def failures() -> None:
    w, T = World(), "t-fail"
    r = root(w, T, 0, 30, out={"status": "failed", "verified": False, "summary": "It broke."},
             conditions=("A", "B", "C"))
    planner(w, T, 1, 2, {"kind": "task", "task": {"goal": "g"}}, err="Model refused the request")
    wk = worker(w, T + ".1", 3, 10, {"status": "failed", "summary": "Could not log in. " * 20}, parent=r)
    worker(w, T + ".2", 11, 12, "not json at all", parent=r)
    worker(w, T + ".3", 12, 13, {"status": "blocked", "summary": "Needs a code"}, parent=r)
    worker(w, T + ".4", 13, 14, {"summary": "No status here"}, parent=r)
    worker(w, T + ".5", 14, 15, {"status": "cancelled", "summary": "Stopped by the owner"}, parent=r)
    worker(w, T + ".6", 15, 16, "", parent=r, status="done")
    worker(w, T + ".7", 16, 17, {"status": "done", "summary": "boom"}, parent=r, err="runtime crashed")
    worker(w, T + ".8", 17, 18, {}, parent=r, err="runtime crashed")
    checker(w, T + ".v1", 19, 20, [{"met": False, "evidence": "no"}, {"met": None, "evidence": "?"},
                                    {"met": True, "evidence": "yes"}], parent=r)
    checker(w, T + ".v2", 20, 21, [{"met": None, "evidence": "could not see the screen " * 5}, {"met": None}], parent=r)
    checker(w, T + ".v3", 21, 22, [], parent=r)
    checker(w, T + ".v4", 22, 23, [{"met": 1, "evidence": "one"}, {"met": 0}, {"met": True}], parent=r)
    checker(w, T + ".v5", 23, 24, None, parent=r, err="checker crashed " * 20)
    checker(w, T + ".v6", 24, 25, [{"met": None}, {"met": True}], parent=r)
    w.span("tool.refused", 26, 26.5, inp={"tool": "shell.run"}, out={"reason": "out of scope"}, parent=wk, task=T)
    w.span("tool.refused", 27, 27.5, inp={"tool": "fs.write"}, out={"reason": "x"}, parent=wk, task=T)
    finish_outcome(w, T, "r1", "bad", status="failed")
    record("failures", result_for(w, T))


failures()


def open_spans() -> None:
    w, T = World(), "t-open"
    r = root(w, T, 0, None, out=None)
    planner(w, T, 1, 2, {"kind": "reply", "text": "Working on it"})
    planner(w, T, 3, None, "{}")
    wk = worker(w, T + ".1", 4, None, {}, parent=r)
    tool(w, wk, T + ".1", 5, None, "browser.open", {"url": "https://example.com"}, parent_dummy := None) if False else None
    tool(w, wk, T + ".1", 5, None, "browser.open", {"url": "https://example.com"})
    checker(w, T + ".v", 8, None, None, parent=r)
    worker(w, T + ".2", 9, 11, {"status": "done", "summary": "after the open one"}, parent=r)
    record("open_spans", result_for(w, T))
    # a finished root with an open step, and a task whose episode is only spans of a restart
    w2, T2 = World(), "t-open2"
    r2 = root(w2, T2, 0, 9, out={"status": "interrupted"})
    worker(w2, T2 + ".1", 1, None, {}, parent=r2)
    record("open_step_closed_root", result_for(w2, T2))


open_spans()


def long_and_unicode() -> None:
    w, T = World(), "t-unicode"
    words = "Réserve un vol à Zürich 🙂 pour 2 personnes demain – مرحبا 你好  done"
    r = root(w, T, 0, 12, request=words, goal=words * 3,
             out={"status": "done", "verified": True, "summary": words, "how": "ünï"})
    planner(w, T, 1, 2, {"kind": "reply", "text": "Voilà " + "é" * 400})
    planner(w, T, 2, 3, {"kind": "task", "task": {"goal": "γ" * 300}})
    planner(w, T, 3, 4, {"kind": None, "text": None})
    planner(w, T, 4, 5, {"text": "no kind"})
    planner(w, T, 5, 6, {"kind": 5, "text": ["a", {"b": None}, True, 1.5, "it's"]})
    planner(w, T, 6, 7, {"kind": "task", "task": "not a dict", "text": "t"})
    planner(w, T, 7, 8, {"kind": "reply", "text": {"k": "v", "n": 1e22, "s": "q'\"\n\x07é​"}})
    planner(w, T, 8, 9, ["a list"])
    planner(w, T, 9, 10, "plain words, not json")
    wk = worker(w, T + ".1", 3, 11, {"status": "done", "summary": "ligne\none\ttab  " + "é" * 300}, parent=r)
    tool(w, wk, T + ".1", 4, 5, "shell.run", {"command": "echo 'héllo wörld' 🙂 " + "z" * 200},
         {"output": "héllo second third\x85fourth\x0bfifth\x0csixth\x1csev\nline nbsp   \n", "exit_code": 0})
    tool(w, wk, T + ".1", 6, 7, "browser.read", {"url": "https://exämple.com/日本語", "text": "t" * 500},
         {"output": "\x1b[2K\x1b[1Gprogress 10%\rprogress 50%\rprogress 100%\n││\n  ok  \nok\n"
                    "╭────╮\n│ hi │\n╰────╯\nreal text\nreal text\nab\nabc\n"})
    tool(w, wk, T + ".1", 7.5, 8, "browser.read", {"url": "u"}, {"output": 5})
    tool(w, wk, T + ".1", 8, 9, "browser.read", {"url": "u"}, {"output": ""})
    tool(w, wk, T + ".1", 9, 9.5, "fs.read", {"path": "/étude/résumé.txt"}, {"size": 3, "text": "日本語", "offset": 1,
                                                                            "next_offset": 2}, err="échec " * 80)
    record("long_and_unicode", result_for(w, T))


long_and_unicode()


def origins_and_questions() -> None:
    w, T = World(), "t-origins"
    wrapped = ('Please summarize <untrusted-data id="a1b2c3">\nsome pasted text\n</untrusted-data id="a1b2c3"> '
               'and then email it')
    earlier = ('\nEarlier FERB question: <untrusted-data id="0f0f0f">\nShall I send it?\n</untrusted-data id="0f0f0f">')
    r = root(w, T, 0, 50, request=wrapped, words=wrapped + earlier, origin="owner,external:paste")
    w.span("context.source", 0.1, 0.2, inp={"ref": "m1"}, out={"text": "a"}, origin="external:email:bob@example.com",
           parent=r, task=T)
    w.span("context.source", 0.2, 0.3, inp={"ref": "m2"}, out={"text": "b"}, parent=r, task=T)           # unlabeled
    w.span("context.source", 0.3, 0.4, inp={"ref": "m3"}, parent=r, task=T)                                 # no output
    w.span("context.exposure", 0.4, 0.5, inp={}, out={"x": 1}, origin="private,inferred", parent=r, task=T)
    w.span("other.kind", 0.5, 0.6, inp={}, out={"x": 1}, origin="external:ignored", parent=r, task=T)
    planner(w, T, 1, 2, {"kind": "task", "task": {"goal": "Summarize"}})
    pq = w.span("model.question", 3, 4, inp={"role": "supervisor"},
                out={"decision": json.dumps({"ask_owner": True, "question": "Which account? " * 20})}, parent=r, task=T)
    w.span("model.question", 4, 5, inp={}, out={"decision": json.dumps({"answer": "Use the work one"})}, parent=r, task=T)
    w.span("model.question", 5, 6, inp={}, out={"decision": {"answer": "decision as an object"}}, parent=r, task=T)
    w.span("model.question", 6, 7, inp={}, out={"decision": "oops"}, parent=r, task=T)
    w.span("model.dispatch", 7, 8, inp={}, out={"text": json.dumps({"action": "retry", "instructions": "Try again"})},
           parent=r, task=T)
    w.span("model.dispatch", 8, 9, inp={}, out={"text": json.dumps({"action": "stop", "stop_reason": "enough"})},
           parent=r, task=T)
    w.span("model.dispatch", 9, 10, inp={}, out={"text": json.dumps({})}, parent=r, task=T)
    w.span("model.dispatch", 10, 11, inp={}, out={"text": json.dumps({"question": "Which?"})}, parent=r, task=T)
    w.span("model.resources", 11, 12, inp={}, out={"text": json.dumps({"status": "done", "summary": "cleaned up"})},
           parent=r, task=T)
    w.span("model.resources", 12, 13, inp={}, out={"text": json.dumps({"status": "failed"})}, parent=r, task=T)
    w.span("model.resources", 13, 14, inp={}, out={"text": "free text"}, parent=r, task=T)
    w.span("model.resources", 14, 15, inp={}, out={"text": ""}, parent=r, task=T)
    w.span("model.resources", 15, 16, inp={}, out={"text": json.dumps({"summary": "no status"})}, parent=r, task=T)
    wk = worker(w, T + ".1", 16, 40, {"status": "done", "summary": "drafted the email"}, parent=r)
    tool(w, wk, T + ".1", 17, 18, "browser.read", {"url": "https://mail.example.com"}, {"title": "Inbox"},
         origin="external:web:mail.example.com")
    tool(w, wk, T + ".1", 18, 19, "fs.read", {"path": "/x"}, {"size": 1}, origin="private", kind="read")
    tool(w, wk, T + ".1", 19, 20, "fs.read", {"path": "/y"}, {"size": 1}, origin=None, kind="read")        # unlabeled
    tool(w, wk, T + ".1", 20, None, "fs.read", {"path": "/z"}, origin=None)                                  # open: none
    tool(w, wk, T + ".1", 21, 22, "mail.send", {"text": "hi"}, {"sent": True}, origin="ferb,external:email:a@b.c,"
         "external:web:site.example", kind="send")
    checker(w, T + ".v", 41, 49, [{"met": True, "evidence": "sent"}], parent=r)
    ver = [{"condition": "FERB's report condition", "met": True, "evidence": "ok"},
           {"condition": "Email sent", "met": False, "evidence": "not seen"}]
    w.journal("chat.message", {"request_id": "r1", "text": wrapped})
    finish_outcome(w, T, "r1", "t", verification=ver, answer="Done, mostly.")
    w.verdict(T, "pass", "looks right", "2026-03-04T10:30:00+00:00")
    w.verdict(T, "fail", "", "2026-03-04T11:05:00+00:00")
    record("origins_and_questions", result_for(w, T))

    # request text: every branch of the choice
    for i, (req, words_, msgs) in enumerate([
        ("plain request", "plain request", []),
        ("plain request", None, []),
        (wrapped, wrapped, []),
        ("<untrusted-data id=\"ab12\">\nonly pasted\n</untrusted-data id=\"ab12\">", "", []),
        ("<untrusted-data id=\"ab12\">\nonly pasted\n</untrusted-data id=\"ab12\">", None, []),
        ("<untrusted-data id=\"ab12\">\nonly pasted\n</untrusted-data id=\"ab12\">", "[pasted text] ok", []),
        ("from request", None, [{"data": {"text": "  last   message\ntext "}}]),
        ("", None, [{"data": {"text": ""}}]),
        ("", None, [{"data": {}}]),
        ("", None, []),
        ("<untrusted-data id=\"zz\">no</untrusted-data id=\"zz\"> <untrusted-data id=\"c1\">\nx\n</untrusted-data id=\"c1\">"
         " <untrusted-data id=\"c1\">\nunterminated", "typed words", []),
        ("x", "a b c\x1cd  e", []),
    ]):
        w3, T3 = World(), f"t-req{i}"
        root(w3, T3, 0, 2, request=req, words=words_ if words_ is not None else req, goal="the goal")
        res = result_for(w3, T3)
        if words_ is None:                   # no authority words recorded at all
            for s in w3.spans.tree_json(T3):
                pass
            w3.db.conn.execute("UPDATE spans SET input_json = json_remove(input_json, '$.authority_words')")
            res = result_for(w3, T3)
        if msgs:
            w3.journal("chat.message", {"request_id": "r1", **msgs[0]["data"]})
            res = result_for(w3, T3)
            res["outcome"]["request"]["messages"] = [{"id": "m", "at": "a", "data": m["data"]} for m in msgs]
            res["episode"] = compose.compose({"task_id": T3, "spans": res["spans"], "outcome": res["outcome"]})
        record(f"request_text_{i}", res)


origins_and_questions()


def retries_and_reruns() -> None:
    w, T = World(), "t-retry"
    r = root(w, T, 0, 120, conditions=("one", "two", "three"))
    planner(w, T, 1, 2, {"kind": "task", "task": {"goal": "g"}})
    w1 = worker(w, T + ".1", 3, 20, {"status": "done", "summary": "first try"}, parent=r)
    c1 = checker(w, T + ".v1", 21, 30, [{"met": False, "evidence": "no"}, {"met": None}, {"met": True}], parent=r,
                 nxt="retry")
    w.span("verify.rerun", 30.5, 30.6, inp={"reason": "the window overflowed " * 10}, parent=r, task=T)
    checker(w, T + ".v1", 31, 35, [{"met": None, "evidence": "short"}], parent=r)
    w.span("verify.rerun", 35.1, 35.2, inp={}, parent=r, task=T)
    w.span("verify.rerun", 35.2, 35.3, inp={"reason": "again"}, parent=r, task=T)
    checker(w, T + ".v2", 36, 40, [{"met": True, "evidence": "ok " * 40}] * 3, parent=r)
    w.span("verify.rerun", 0.5, 0.6, inp={"reason": "before any checker"}, parent=r, task=T)
    worker(w, T + ".2", 41, 60, {"status": "done", "summary": "second try"}, parent=r)
    w.span("model.dispatch", 61, 62, inp={}, out={"text": json.dumps({"action": "retry", "instructions": "again"})},
           parent=r, task=T)
    worker(w, T + ".3", 63, 70, {"status": "done", "summary": "third try"}, parent=r)
    checker(w, T + ".v3", 71, 80, [{"met": True, "evidence": "a"}, {"met": True, "evidence": "b"},
                                    {"met": True, "evidence": "c"}], parent=r)
    worker(w, T + ".4", 81, 90, {"status": "done", "summary": "fourth"}, parent=r)
    worker(w, T + ".5", 91, 95, {"status": "done", "summary": "fifth"}, parent=r)
    checker(w, T + ".v4", 96, 100, [{"met": True, "evidence": "ok"}], parent=r)
    w.span("model.usage", 100, 100, inp={}, out={"input_tokens": 8, "cached_input_tokens": 1, "output_tokens": 12500,
                                                  "cost_usd": 0.004, "model": "m1", "billing": "subscription"},
           parent=r, task=T)
    w.span("model.usage", 101, 101, inp={}, out={"input_tokens": 10000, "cached_input_tokens": 5000,
                                                  "output_tokens": 999, "cost_usd": None, "model": "m2"}, parent=r, task=T)
    w.span("model.usage", 102, 102, inp={}, out={"input_tokens": 1250, "output_tokens": 1000, "model": None,
                                                  "cache_write_tokens": 5, "cache_write_5m_tokens": 3,
                                                  "cache_write_1h_tokens": 2, "billing": "api"}, parent=r, task=T)
    w.span("model.usage", 103, 103, inp={}, out={"input_tokens": "7", "output_tokens": 2.9, "cost_usd": 1}, parent=r,
           task=T)
    finish_outcome(w, T, "r1", "x", verification=[{"condition": "c", "met": True, "evidence": "e"}])
    record("retries_and_reruns", result_for(w, T))


retries_and_reruns()


def usage_variants() -> None:
    cases = {
        "none": [],
        "all_priced_ints": [dict(cost_usd=1), dict(cost_usd=2)],
        "mixed_cost": [dict(cost_usd=1), dict(cost_usd=0.1), dict(cost_usd=0.2)],
        "subscription": [dict(billing="subscription", cost_usd=2.5)],
        "unpriced_only": [dict(cost_usd=None, model="zeta"), dict(cost_usd=None, model="alpha")],
        "percent_ties": [dict(input_tokens=8, cached_input_tokens=1, cost_usd=None),
                         dict(input_tokens=40, cached_input_tokens=1, cost_usd=None)],
        "zero_input": [dict(input_tokens=0, cached_input_tokens=0, output_tokens=0, cost_usd=0.0)],
        "big": [dict(input_tokens=123456, cached_input_tokens=99999, output_tokens=10500, cost_usd=12.345)],
        "half_cent": [dict(cost_usd=0.125), dict(cost_usd=0.0)],
        "tiny": [dict(cost_usd=0.0000004)],
    }
    for name, recs in cases.items():
        w, T = World(), "t-usage-" + name.replace("_", "-")
        r = root(w, T, 0, 10)
        planner(w, T, 1, 2, {"kind": "reply", "text": "hi"})
        if name == "none":
            worker(w, T + ".1", 3, 4, {"status": "done", "summary": "s"}, parent=r)
        for i, u in enumerate(recs):
            usage(w, r, T, 5 + i, **u)
        record("usage_" + name, result_for(w, T))


usage_variants()


def desktop() -> None:
    w, T = World(), "t-desk"
    r = root(w, T, 0, 100)
    wk = worker(w, T + ".1", 1, 90, {"status": "done", "summary": "s"}, parent=r)
    tc = tool(w, wk, T + ".1", 10, 12, "browser.click", {"action": "click"}, kind="click")
    tc2 = tool(w, wk, T + ".1", 13, 14, "fs.read", {"path": "/a"}, kind="read")
    tc3 = tool(w, wk, T + ".1", 15, 16, "shell.run", {"command": "ls"}, kind="shell")
    snap = {"frontmost_app": {"pid": 100, "name": "Notes", "bundle_id": "com.example.notes"}, "active_space_epoch": 4}
    w.span("disturbance.baseline", 2, 2, inp={}, out={"snapshot": snap}, parent=r, task=T)

    def change(at: float, changes: dict, actions: list, **extra: Any) -> None:
        w.span("disturbance.change", at, at, inp={}, out={"changes": changes, "actions": actions, **extra}, parent=r,
               task=T)

    def fm(before: Any, after: Any) -> dict:
        return {"frontmost_app": {"before": before, "after": after}}

    app = lambda pid, name=None, bid=None: {"pid": pid, **({"name": name} if name else {}), **({"bundle_id": bid} if bid else {})}  # noqa: E731
    change(10.5, fm(app(100), app(200, "Chrome")), [{"span_id": tc.id, "tool": "browser.click", "on_screen": True,
                                                    "task_id": T + ".1"}])
    change(13.5, fm(app(200), app(300, None, "com.example.mail")),
           [{"span_id": tc2.id, "tool": "fs.read", "task_id": "other-task.1"}])                            # old record
    change(15.5, fm(app(300), app(400)), [{"span_id": tc3.id, "tool": "shell.run"}])                       # old: shell
    change(20, fm(app(400), app(500, "Mail")), [], processes=[{"command": "sleep 100 " * 20, "task": T + ".1"},
                                                              {"id": "p9", "task": "zzz.1"}, {"id": "p8", "command": "x"}])
    change(21, fm(app(500), app(600, "Preview")), [], processes=[], on_screen_in_task=True, last_on_screen_ended_s=3.456)
    change(22, fm(app(600), app(600)), [])                                                                 # same app: nothing
    change(23, fm({}, app(700, "Maps")), [], processes=None, on_screen_in_task=False, last_on_screen_ended_s=12.9)
    change(24, {"active_space_epoch": {"before": 4, "after": 5}}, [], on_screen_in_task=True, last_on_screen_ended_s=None,
           processes=[])
    change(25, {"active_space_epoch": {"before": 5, "after": 5}}, [])
    change(26, {"active_space_epoch": {"before": None, "after": 6}}, [{"span_id": "x", "tool": "mac.key", "on_screen": False}],
           on_screen_in_task=False, last_on_screen_ended_s=None, processes=[])
    change(27, {"active_space_epoch": {"before": 6, "after": 7}, "frontmost_app": {"before": app(700), "after": None}},
           [{"span_id": "y", "tool": "mac.key", "on_screen": False}], on_screen_in_task=True, last_on_screen_ended_s=0.04)
    change(28, {"tab_sound": {"before": False, "after": True}},
           [{"span_id": "z", "tool": "browser.read", "on_screen": False, "task_id": T + ".2"},
            {"span_id": "z2", "tool": "browser.read", "on_screen": False, "task_id": "other-task.3"}])
    change(29, {"tab_sound": True}, [])
    change(30, {"title": "x"}, [{"span_id": "q", "tool": "mac.key", "on_screen": True}])
    change(31, {"title": "x", "url": "y"}, [{"span_id": "q", "tool": "mac.key", "on_screen": True, "task_id": None}])
    change(32, {}, [])
    change(33, fm(app(1), app(2, "Terminal")), [{"span_id": "w", "tool": "mac.key", "on_screen": False}], processes=[],
           on_screen_in_task=True, last_on_screen_ended_s=10)
    change(34, fm(app(2), app(3, "Notes")), [{"span_id": "w", "tool": "mac.key", "on_screen": False}], processes=[],
           on_screen_in_task=False, last_on_screen_ended_s=99.99)
    w.span("disturbance.paused", 40, 40, inp={}, out={}, parent=r, task=T)
    w.span("disturbance.resumed", 45, 45, inp={}, out={}, parent=r, task=T)
    w.span("disturbance.resumed", 46, 46, inp={}, out={}, parent=r, task=T)
    w.span("disturbance.paused", 50, 50, inp={}, out={}, parent=r, task=T)
    w.span("disturbance.paused", 55, 55, inp={}, out={}, parent=r, task=T)
    w.span("disturbance.summary", 60, 60, inp={}, out={"changes": [1, 2, 3]}, parent=r, task=T)
    record("desktop_changes", result_for(w, T))

    # a summary with a count, a dict, none; paused to the end
    for i, summary in enumerate([5, {"a": 1, "b": 2}, None, "bad"]):
        w2, T2 = World(), f"t-desk{i}"
        r2 = root(w2, T2, 0, 30)
        w2.span("disturbance.paused", 5, 5, inp={}, out={}, parent=r2, task=T2)
        if i != 2:
            w2.span("disturbance.summary", 20, 20, inp={}, out={"changes": summary} if summary != "bad" else {"changes": "3"},
                    parent=r2, task=T2)
        record(f"desktop_summary_{i}", result_for(w2, T2))
    w3, T3 = World(), "t-desk-open"
    r3 = root(w3, T3, 0, None, out=None)
    w3.span("disturbance.paused", 5, 5, inp={}, out={}, parent=r3, task=T3)
    record("desktop_paused_open", result_for(w3, T3))
    # the plain tree form of disturbance spans (no episode)
    res = result_for(w, T)
    res["episode"] = None
    record("desktop_plain", res)


desktop()


def timing_cases() -> None:
    w, T = World(), "t-timing"
    r = root(w, T, 0, 100)
    # planner turn with a prepare record, a reply, model calls, lookups
    w.span("planner.prepare", 0.2, 0.4, inp={"request_id": "q1"}, out={"steps_ms": {"history": 12.5, "memory": 40,
                                                                                    "skipped": None}, "total_ms": 120},
           parent=r, task=T)
    pt = planner(w, T, 1, 9, {"kind": "reply", "text": "hello"}, rid="q1", parent=r,
                 timing_={"total_ms": 7800, "steps_ms": {"ferb_before": 30, "ferb_after": 55}, "call": {
                     "steps_ms": {"prepare": 15, "start": 640, "stream": 6100, "close": 20},
                     "start_steps_ms": {"spawn": 400, "handshake": 240}}})
    usage(w, pt, T, 4, timing={"total_ms": 2500, "runtime_ms": 100, "ttft_ms": 900, "generate_ms": 1400,
                               "tail_ms": 100, "by": "runtime"})
    usage(w, pt, T, 8, timing={"total_ms": 3000, "runtime_ms": 50.5, "ttft_ms": 1000.25, "generate_ms": 1900,
                               "by": "arrival"})
    usage(w, pt, T, 8.5)                                                         # no timing recorded
    usage(w, pt, T, 8.6, timing={"runtime_ms": 5})                                 # no total
    tool(w, pt, T + ".lookups", 5, 5.8, "memory.search", {"query": "x"}) if False else None
    w.span("tool.call", 5, 5.8, inp={"tool": "memory.search"}, out={"ok": 1}, task="q1.lookups", parent=pt)
    w.span("tool.call", 6, 6.001, inp={"tool": "fs.read"}, out={"ok": 1}, task="q1.lookups", parent=pt)
    w.span("tool.call", 6.5, None, inp={"tool": "fs.list"}, task="q1.lookups", parent=pt)           # unfinished: ms None
    w.span("tool.call", 50, 51, inp={"tool": "later.tool"}, out={"ok": 1}, task="q1.lookups", parent=pt)     # outside
    w.span("planner.reply", 9.1, 9.2, inp={"request_id": "q1"}, out={"after_decision_ms": 205}, parent=r, task=T)
    w.span("planner.reply", 0.1, 0.2, inp={"request_id": "q1"}, out={"after_decision_ms": 1}, parent=r, task=T)
    # a turn with an error and no prepare
    pe = planner(w, T, 10, 12, {"kind": "reply", "text": "x"}, rid="q2", parent=r, err="boom",
                 timing_={"total_ms": 1999.5, "steps_ms": {"ferb_before": 1}, "call": {"steps_ms": {"stream": "x"}}})
    # a turn without timing
    planner(w, T, 13, 14, {"kind": "reply", "text": "x"}, rid="q3", parent=r)
    # two turns sharing a request id: the prepare is used once
    w.span("planner.prepare", 14.5, 14.6, inp={"request_id": "q4"}, out={"total_ms": 5000, "steps_ms": {}}, parent=r, task=T)
    for k, a in enumerate((15, 20)):
        planner(w, T, a, a + 1, {"kind": "reply", "text": "x"}, rid="q4", parent=r,
                timing_={"total_ms": 1000 + k, "steps_ms": {"ferb_before": 2, "ferb_after": 3}, "call": 5 if k else {}})
    # workers
    wk = worker(w, T + ".1", 30, 70, {"status": "done", "summary": "s"}, parent=r,
                timing_={"steps_ms": {"prepare": 12, "start": 800, "stream": 38000, "close": 33},
                         "start_steps_ms": {"spawn_process": 700}})
    usage(w, wk, T + ".1", 40, timing={"total_ms": 20000, "runtime_ms": 10, "ttft_ms": 3000, "generate_ms": 16990,
                                       "tail_ms": 0, "by": "runtime"})
    usage(w, wk, T + ".1", 60, timing={"total_ms": 15000, "ttft_ms": 1, "generate_ms": 14000})
    w.span("tool.call", 41, 44.5, inp={"tool": "browser.read"}, out={"ok": 1}, task=T + ".1", parent=wk)
    w.span("tool.call", 45, 55.25, inp={"tool": "shell.run"}, out={"ok": 1}, task=T + ".1", parent=wk)
    w.span("tool.call", 3, 4, inp={"tool": "before.run"}, out={"ok": 1}, task=T + ".1", parent=r)         # outside
    w.span("tool.call", 41.5, 43, inp={}, out={"ok": 1}, task=T + ".1", parent=wk)                          # no tool name
    worker(w, T + ".2", 71, 75, {"status": "failed", "summary": "s"}, parent=r, err="x")
    checker(w, T + ".v", 76, 90, [{"met": True, "evidence": "e"}], parent=r)
    ck = worker(w, T + ".v2", 91, 99.123456, {"results": []}, parent=r, role="verifier",
                timing_={"steps_ms": {"prepare": 1, "start": 2, "stream": 3, "close": 4}})
    record("timing_everything", result_for(w, T))

    # timing numbers: every kind of gap (rounding to ms and to a tenth of a second)
    w2, T2 = World(), "t-gaps"
    r2 = root(w2, T2, 0, 500.123456)
    rnd = random.Random(20261113)
    at = 1.0
    for i in range(80):
        gap = rnd.choice([0.05, 0.25, 0.35, 0.45, 0.0005, 0.0015, 0.0025, 1.0, 9.95, 59.96, 0.123456, 3.000001,
                          rnd.random() * 100])
        tool(w2, r2, T2, at, at + gap, f"tool{i}", {"id": i}, {"k": i})
        at += gap + 0.1
    record("timing_gaps", result_for(w2, T2))

    # started times in other zones and with different precision: spans are ordered by their text
    w3, T3 = World(), "t-zones"
    zone = timezone(timedelta(hours=2))
    r3 = root(w3, T3, t(0, zone), t(30, zone))
    pa = planner(w3, T3, t(1, zone), t(2.5, zone), {"kind": "reply", "text": "a"}, rid="z1", parent=r3,
                 timing_={"total_ms": 1500, "steps_ms": {"ferb_before": 1, "ferb_after": 2}, "call": {"steps_ms": {}}})
    worker(w3, T3 + ".1", t(3, zone), t(9.5, zone), {"status": "done", "summary": "s"}, parent=r3)
    w3.span("tool.call", t(4, zone), t(5, zone), inp={"tool": "z.tool", "args": {"id": 1}}, out={"ok": 1},
            task=T3 + ".1", parent=r3)
    record("timing_zones", result_for(w3, T3))


timing_cases()


def empty_and_learning() -> None:
    w, T = World(), "t-empty"
    record("no_spans", result_for(w, T))
    w2, T2 = World(), "t-learn"
    w2.span("model.learning", 0, 1, inp={"goal": "learn"}, out={"result": json.dumps({"status": "done", "summary": "s"})},
            task=T2)
    record("only_learning", result_for(w2, T2))
    w3, T3 = World(), "t-learn2"
    w3.span("model.learning", 0, 1, inp={"goal": "learn"}, out={"result": "not json"}, task=T3)
    w3.span("model.learning", 2, 3, inp={"tool": "learn.tool"}, out={"result": "{\"x\": 1}"}, task=T3, err="oops " * 100)
    w3.span("model.learning", 3, 4, inp={}, out={"result": json.dumps({"status": "done"})}, task=T3)
    w3.span("model.learning", 5, 6, inp={"goal": "g" * 200}, out={"result": json.dumps({"summary": "x" * 500})}, task=T3)
    w3.span("tool.call", 6, 7, inp={"tool": "t", "args": {"q": "z"}}, out={"output": "a\nb"}, task=T3)
    record("learning_mixed", result_for(w3, T3))
    # a trace.get answer with an episode of an older build (no desktop, no problems) and a bare outcome
    w4, T4 = World(), "t-old"
    r4 = root(w4, T4, 0, 5)
    res = result_for(w4, T4)
    res["episode"] = {"task_id": T4, "request_text": "old", "status": "done", "seconds": 5.0, "steps": [], "usage": {}}
    res["outcome"] = {}
    record("old_episode", res, composed=False)


empty_and_learning()


# --------------------------------------------------------------------------------- recorded chat scenarios

def scenarios() -> None:
    data = json.loads((REPO / "rust/fixtures/daemon/chat_scenarios.json").read_text())["scenarios"]
    for sc in data:
        tables = sc["tables"]
        if "tasks" not in tables or not tables["spans"]["rows"]:
            continue
        w = World()
        conn = w.db.conn
        conn.execute("PRAGMA foreign_keys=OFF")
        for table in ("spans", "journal", "tasks"):
            spec = tables[table]
            cols = spec["columns"]
            for row in spec["rows"]:
                conn.execute(f"INSERT INTO {table} ({', '.join(cols)}) VALUES ({', '.join('?' * len(cols))})",
                             [json.dumps(v) if isinstance(v, (dict, list)) else v for v in row])
        sp = tables["spans"]
        ti = sp["columns"].index("task_id")
        roots = sorted({r[ti].split(".")[0] for r in sp["rows"] if r[ti]})
        episodes = {}
        if "episodes" in tables:
            ec = tables["episodes"]["columns"]
            for row in tables["episodes"]["rows"]:
                episodes[row[0]] = dict(zip(ec, row))
        for root_id in roots:
            res = result_for(w, root_id)
            record(f"scenario_{sc['name']}_{root_id}", res)
            # the stored episode (as the daemon returns it once the task ended), if the scenario kept one
            stored = episodes.get(root_id)
            if stored:
                for key, value in stored.items():
                    if isinstance(value, str) and value.startswith("{") and "task_id" in value:
                        ep = json.loads(value)
                        res2 = dict(res, episode=ep)
                        record(f"scenario_{sc['name']}_{root_id}_stored_episode", res2, composed=False)
                        break


scenarios()

# ----------------------------------------------------------------------------------- hand-made episodes

STORIES: list[dict[str, Any]] = []


def story_case(name: str, ep: dict[str, Any], verdict: dict[str, Any] | None = None) -> None:
    STORIES.append({"name": name, "episode": ep, "verdict": verdict, "lines": render.story(ep, verdict)})


def usage_ep(**kw: Any) -> dict[str, Any]:
    base = {"model_calls": 2, "usage_records": 2, "input_tokens": 1500, "cached_input_tokens": 750,
            "output_tokens": 480, "cost_usd": 0.0349, "unpriced_models": [], "billing": []}
    return base | kw


base_ep = {"task_id": "abcdef123456", "request_text": "x " * 100, "status": "done", "verified": True, "seconds": 125.4,
           "steps": [{"at": "2026-03-04T09:00:01+00:00", "role": "Planner", "seconds": 1.2, "text": "task — go", "mark": ""},
                     {"at": "2026-03-04T09:00:05.5+00:00", "role": "Worker", "seconds": None, "text": "done", "mark": "✗"}],
           "answer": "  the   answer\nhere ", "problems": ["⚠ 1 retry", "✗ 2 steps failed"], "point_checks": {"hit_test": 3, "x": 1},
           "usage": usage_ep(), "desktop": {"changes": 3, "near_ferb": [], "no_on_screen_action": [], "paused": []}}
story_case("base", base_ep)
for status, verified in [("done", False), ("done", None), ("done_unverified", False), ("failed", False), ("needs_user", None),
                         ("cancelled", None), ("interrupted", None), ("running", None), ("paused", None),
                         ("unknown", None), ("weird", None), (None, None)]:
    story_case(f"status_{status}_{verified}", base_ep | {"status": status, "verified": verified})
for tokens in [(8, 1), (0, 0), (40, 1), (1000, 333), (9999, 9999), (10000, 5000), (12500, 3), (13500, 13499),
               (123456, 100000), (999, 1), (1250, 0), (1350, 0)]:
    story_case(f"tokens_{tokens[0]}_{tokens[1]}", base_ep | {"usage": usage_ep(input_tokens=tokens[0],
                                                                                cached_input_tokens=tokens[1],
                                                                                output_tokens=tokens[0] + 5)})
for i, cost in enumerate([0.0349, 0.005, 0.015, 0.025, 0.125, 1, 12.345, 0.0, 1e-9, 1234567.891]):
    story_case(f"cost_{i}", base_ep | {"usage": usage_ep(cost_usd=cost, billing=["subscription"] if i % 2 else ["api"])})
story_case("unpriced", base_ep | {"usage": usage_ep(cost_usd=None, unpriced_models=["a", "b"])})
story_case("unpriced_partial", base_ep | {"usage": usage_ep(unpriced_models=["a"], model_calls=5, usage_records=2)})
story_case("no_usage_records", base_ep | {"usage": {"usage_records": 0, "model_calls": 4}})
story_case("no_usage_at_all", base_ep | {"usage": {}})
story_case("no_usage_zero", base_ep | {"usage": {"usage_records": 0}})
story_case("no_usage_key", {k: v for k, v in base_ep.items() if k != "usage"})
story_case("minimal", {"task_id": "t"})
story_case("minimal_status", {"task_id": "tt", "status": "done"})
story_case("seconds_none", base_ep | {"seconds": None})
story_case("seconds_long", base_ep | {"seconds": 3599.5})
story_case("seconds_short", base_ep | {"seconds": 0.4})
story_case("seconds_tie", base_ep | {"seconds": 59.5})
story_case("no_answer", base_ep | {"answer": "", "problems": [], "point_checks": {}})
story_case("model_calls_one", base_ep | {"usage": usage_ep(model_calls=1, usage_records=1)})
story_case("verdict_pass", base_ep, {"verdict": "pass", "note": "good", "at": "2026-03-04T10:00:00+00:00"})
story_case("verdict_fail_nonote", base_ep, {"verdict": "fail", "note": "", "at": "2026-12-25T23:59:00+00:00"})
story_case("verdict_months", base_ep, {"verdict": "pass", "note": "n", "at": "2026-09-01T00:00:00+02:00"})
dk = {"changes": 4, "near_ferb": [
    {"at": "2026-03-04T09:00:10+00:00", "changed": ["frontmost_app", "title"], "tools": ["browser.click", "fs.read"],
     "other_task_tools": ["mac.key"]},
    {"at": "2026-03-04T09:00:30+00:00", "changed": ["a"], "tools": [], "other_task_tools": ["mac.key", "x"]},
    {"at": "2026-03-04T09:00:50+00:00", "changed": ["b"], "tools": ["t"]}],
    "no_on_screen_action": [{"at": "2026-03-04T09:00:20+00:00", "text": "Chrome came to the front — none"}],
    "paused": [{"at": "2026-03-04T09:00:40+00:00", "until": "2026-03-04T09:00:45+00:00"},
               {"at": "2026-03-04T09:00:25+00:00", "until": None}]}
story_case("desktop_full", base_ep | {"desktop": dk})
story_case("desktop_others_only", base_ep | {"desktop": {"changes": 1, "no_on_screen_action": [dk["no_on_screen_action"][0]] * 2}})
story_case("desktop_old", base_ep | {"desktop": {"changes": 0, "near_ferb": [dk["near_ferb"][0]]}})
story_case("desktop_empty", {k: v for k, v in base_ep.items() if k != "desktop"})
story_case("desktop_equal_times", base_ep | {"desktop": {"changes": 2, "near_ferb": [dk["near_ferb"][1]],
                                                        "no_on_screen_action": [{"at": dk["near_ferb"][1]["at"], "text": "t"}],
                                                        "paused": [{"at": dk["near_ferb"][1]["at"], "until": None}]}})
story_case("step_no_time", base_ep | {"steps": [{"at": "2026-03-04T09:00:01+00:00", "role": "Checker", "seconds": 2.0,
                                                  "text": "✓ ok", "mark": ""}]})
story_case("unicode_request", base_ep | {"request_text": "é" * 100 + " 🙂", "answer": "ünï code x"})

# ------------------------------------------------------------------------------------------ lines helpers

HELPERS: dict[str, list[Any]] = {k: [] for k in
    ("clip", "shorten", "plural", "choices", "point_checks", "duration", "seconds", "no_on_screen_action", "summarize",
     "step_line", "front", "cameforward")}

TEXTS = ["", " ", "hello", "  hello \n world\t!  ", "a" * 200, "word " * 60, "Sentence one. Sentence two! Third? Fourth.",
         "A short one. " + "longword" * 30, "x" * 40 + ". " + "y" * 50 + ", and then more words follow along here",
         "naïve café — 日本語 🙂 " * 12, "line1\nline2\n\nline4", "ends with. " * 10, "tab\tseparated nbsp em\x1cfs",
         ". . . . . . . .", "Hi. " + "z" * 300, "— leading dash, trailing ,;:(- " + "q" * 100, "a b" * 3,
         "x" * 159, "x" * 160, "x" * 161, "áé" * 80, "a. b. c. d. e. f. g. h. i. j. k. l. m. n. o. p." * 3,
         "hello,;:( —-\n\t world " * 15, " sep ar", "\x85nel"]
for s in TEXTS:
    for n in (1, 2, 5, 10, 25, 60, 100, 160):
        HELPERS["clip"].append({"text": s, "n": n, "out": lines.clip(s, n)})
        HELPERS["shorten"].append({"text": s, "n": n, "fold": True, "out": lines.shorten(s, n)})
        HELPERS["shorten"].append({"text": s, "n": n, "fold": False, "out": lines.shorten(s, n, False)})
for v in [None, 0, 1.5, True, False, [1, "a", None], {"a": [1, {"b": "c'd"}], "e": 2.0}, ["it's", 'say "x"', "both ' and \""],
          "str", 10 ** 18, -3, 1e22, 1e-5, float("1e16"), {"k": None}, [], {}, ["\x07​\n\t\\é🙂\U000e0001"], 123456789.123456789,
          0.1 + 0.2, [0.1, 5e-324], "mixed ' \"", 1e15, 123456789012345678.0]:
    HELPERS["clip"].append({"value": v, "n": 160, "out": lines.clip(v, 160)})
for n in (0, 1, 2, 3, 99):
    HELPERS["plural"].append({"n": n, "one": "step", "many": "steps", "out": lines.plural(n, "step", "steps")})
for opts in [[], ["Yes"], ["Yes", "No"], ["A", "B", "C"], [1, 2, 3, 4], ["é", "ü"]]:
    HELPERS["choices"].append({"options": opts, "out": lines.choices(opts)})
for counts in [{}, {"hit_test": 3}, {"hit_test": 1, "window_list": 2, "refused: cant_tell": 3, "other": 9}]:
    HELPERS["point_checks"].append({"counts": counts, "out": lines.point_checks(counts)})
for sec in [None, 0, 0.4, 0.5, 1.5, 2.5, 59.49, 59.5, 60, 61, 119.5, 125.4, 3599.5, 3600, 36000.2, -1.4, -0.5, 1e6]:
    HELPERS["duration"].append({"seconds": sec, "out": lines.duration(sec)})
for sec in [0, 0.04, 0.05, 0.15, 0.25, 0.35, 9.94, 9.95, 9.99, 10, 10.9, 99.99, 100.5, -0.04, -3.25, 12345.678, 1e9, 0.0]:
    HELPERS["seconds"].append({"value": sec, "out": lines.seconds(sec)})
procs = [None, [], [{"command": "sleep 5", "task": "t1.2"}], [{"id": "p1", "task": "zz"}, {"command": "x" * 100, "task": "t1"}],
         [{"id": "p3"}], [{"command": "é" * 70, "task": "t1.1"}, {"id": "p4", "task": "other.1"}]]
for what in ["Chrome came to the front", ""]:
    for ps in procs:
        for in_task in (None, True, False):
            for last in (None, 0.0, 0.95, 3.25, 9.99, 10.5, 77):
                HELPERS["no_on_screen_action"].append({"what": what, "processes": ps, "task_id": "t1.7", "in_task": in_task,
                                                       "last_ended_s": last,
                                                       "out": lines.no_on_screen_action(what, ps, "t1.7", in_task=in_task,
                                                                                         last_ended_s=last)})
SUMMARIZE = [
    ("fs.list", {}, {"entries": [1, 2, 3]}, False), ("fs.list", {}, {"entries": [1], "total": 5}, False),
    ("fs.list", {}, {"entries": [1, 2], "total": 2}, False), ("fs.list", {}, {"entries": "abc", "total": True}, False),
    ("fs.list", {}, {"entries": {"a": 1, "b": 2}, "total": 9}, False), ("fs.list", {}, {}, False),
    ("fs.list", {}, {"entries": None}, False), ("fs.list", {}, {"entries": 5}, False), ("fs.list", {}, {"entries": [1], "total": "9"}, False),
    ("fs.list", {}, {"entries": [1], "total": 2.0}, False),
    ("fs.read", {}, {"size": 10, "text": "abc"}, False), ("fs.read", {}, {"size": 10, "text": "héé", "offset": 3, "next_offset": None}, False),
    ("fs.read", {}, {"size": 10, "text": "abc", "offset": 0, "next_offset": 3}, False), ("fs.read", {}, {}, False),
    ("fs.read", {}, {"size": None, "text": None, "offset": None}, False),
    ("fs.read", {}, {"size": 4, "text": "日本", "offset": 1.5, "next_offset": 2}, False),
    ("fs.write", {"path": "/a"}, {"bytes": 3}, False), ("fs.write", {"path": "/a"}, {"size": 3, "path": "/b"}, False),
    ("fs.write", {}, {}, False), ("fs.write", {"path": None}, {"path": None}, False),
    ("fs.edit", {"path": "/a"}, {}, False), ("fs.edit", {"path": "/a"}, {"path": "/é"}, False),
    ("fs.trash", {"path": "/a"}, {}, False), ("fs.trash", {}, {"path": "/t"}, False),
    ("shell.run", {}, {"running": True, "id": "p1"}, False), ("shell.run", {}, {"exit_code": 0, "output": "a\n\nb\n  \nc"}, False),
    ("shell.run", {}, {"exit_code": 2, "output": "", "capture_error": "x"}, False), ("shell.run", {}, {}, False),
    ("shell.run", {}, {"exit_code": None, "output": None}, False), ("shell.run", {}, {"exit_code": 1, "output": ["a", "b"]}, False),
    ("shell.run", {}, {"exit_code": 1, "output": "a b\x85c\x0bd\r\ne"}, False),
    ("shell.wait", {}, {"exit_code": 0, "output": "x"}, False),
    ("shell.send", {"text": "yes"}, {}, False), ("shell.send", {}, {}, False), ("shell.send", {"text": "q" * 300}, {}, False),
    ("mac.clipboard", {}, {"characters": 12}, False), ("mac.clipboard", {}, {}, False),
    ("owner.ask", {}, {"answer": "yes"}, False), ("owner.ask", {"confirm": True}, {"answer": "yes", "answered_by": "orchestrator"}, False),
    ("owner.ask", {}, {"declined": True}, False), ("owner.ask", {"confirm": True}, {}, False), ("owner.ask", {}, {"answer": "é" * 150}, False),
    ("owner.ask", {}, {"answer": None}, False), ("owner.ask", {}, {"answer": ["a"], "declined": 0}, False),
    ("browser.open", {}, {"title": "T", "url": "u"}, False), ("browser.open", {}, {}, False),
    ("browser.open", {}, {"error": "oops", "x": 1}, True), ("fs.read", {}, {"error": "denied " * 40}, True), ("fs.read", {}, {"size": 1}, True),
    ("fs.read", {}, {"error": ""}, True), ("fs.read", {}, {"error": 5}, True),
]
for tool_, args, content, is_err in SUMMARIZE:
    HELPERS["summarize"].append({"tool": tool_, "args": args, "content": content, "is_error": is_err,
                                 "out": lines.summarize(tool_, args, content, is_err)})
for at, tool_, args, why, summary, secs, err in [
    ("2026-03-04T09:01:02+00:00", "browser.open", {"url": "https://x.example/a", "tab": "t1", "id": 3}, "", "opened", 1.25, None),
    ("2026-03-04T09:01:02.5+00:00", "shell.run", {"command": "ls " + "a" * 200, "text": "", "query": None, "action": 0},
     "to look " * 30, "exit 0", None, None),
    ("2026-03-04T23:59:59+02:00", "x", {}, "", "s", 0.05, "failed " * 60),
    ("2026-03-04T09:01:02+00:00", "x", {"element": [1, 2], "profile": {"a": 1}, "app": "é" * 120, "bundle_id": False}, "w", "s",
     0.25, ""),
    ("2026-03-04T09:01:02+00:00", "x", {"path": "/p", "extra": "ignored"}, "", "→ already", 10.04, None),
    ("2026-03-04T09:01:02+00:00", "x", {"url": "u"}, "", "s", 0.0, None),
    ("2026-03-04T09:01:02+00:00", "x", {"url": "u"}, "", "s", 59.96, None),
]:
    HELPERS["step_line"].append({"at": at, "tool": tool_, "args": args, "why": why, "summary": summary, "seconds": secs,
                                 "error": err, "out": lines.step_line(at, tool_, args, why, summary, secs, err)})
for app in [None, {}, {"pid": 5}, {"pid": "5"}, {"pid": True}, {"pid": 1.0}, {"pid": None}, "x", {"pid": -3}]:
    HELPERS["front"].append({"app": app, "out": lines.front_pid(app)})


def cf_script(baseline: Any, steps: list[Any]) -> None:
    cf = lines.CameForward()
    if baseline is not None:
        cf.baseline(baseline)
    outs = []
    for st in steps:
        outs.append({"out": cf.change(st), "pid": cf.pid, "epoch": cf.epoch})
    HELPERS["cameforward"].append({"baseline": baseline, "steps": steps, "outs": outs})


a = lambda pid, name=None, bid=None: {"pid": pid, **({"name": name} if name else {}), **({"bundle_id": bid} if bid else {})}  # noqa: E731
cf_script({"frontmost_app": a(1), "active_space_epoch": 3}, [
    {"frontmost_app": {"before": a(1), "after": a(2, "Chrome")}}, {"frontmost_app": {"before": a(2), "after": a(2, "Chrome")}},
    {"frontmost_app": {"before": None, "after": a(3, None, "com.x")}}, {"frontmost_app": {"before": a(3), "after": a(4)}},
    {"frontmost_app": {"before": a(4), "after": None}}, {"frontmost_app": {"before": a(9), "after": a(10, "Z")}},
    {"active_space_epoch": {"before": 3, "after": 4}}, {"active_space_epoch": {"before": 4, "after": 4}},
    {"active_space_epoch": {"before": None, "after": 9}}, {"active_space_epoch": {"after": 10}},
    {"frontmost_app": {"before": a(10), "after": a(11, "Both")}, "active_space_epoch": {"before": 10, "after": 11}},
    {"frontmost_app": None}, {"frontmost_app": {}}, {"active_space_epoch": None}, {}, {"active_space_epoch": {"before": "x", "after": 2}},
    {"frontmost_app": {"before": a(11), "after": {"pid": 12, "name": "", "bundle_id": ""}}},
])
cf_script(None, [{"frontmost_app": {"before": None, "after": a(1, "First")}}, {"frontmost_app": {"before": a(1), "after": a(2, "Next")}},
                 {"active_space_epoch": {"before": 1, "after": 2}}])
cf_script({"frontmost_app": {"pid": None}, "active_space_epoch": "bad"}, [
    {"frontmost_app": {"after": a(5, "Q")}}, {"active_space_epoch": {"after": 1}}, {"active_space_epoch": {"after": 2}}])
cf_script({}, [{"frontmost_app": {"before": a(True), "after": a(0, "Zero")}}])

# ----------------------------------------------------------------------------- Python crashes the port fixes

CRASHES: list[dict[str, Any]] = []
for name, result_text in [("number", "5"), ("list", "[1, 2]"), ("null", "null"), ("string", '"text"')]:
    w, T = World(), f"t-crash-{name}"
    r = root(w, T, 0, 5)
    w.span("model.learning", 1, 2, inp={"goal": "g"}, out={"result": result_text}, parent=r, task=T)
    res = result_for(w, T)
    res["episode"] = None
    try:
        admin.render_trace(res, False)
        raised = None
    except Exception as e:  # noqa: BLE001 - the crash is the fixture
        raised = type(e).__name__
    CRASHES.append({"name": name, "result": res, "python_raises": raised})
    assert raised == "AttributeError", (name, raised)

# ------------------------------------------------------------------------------------------------- write


def dump(name: str, value: Any) -> int:
    """One top-level key per line; a list's items one per line (a changed case is a small diff)."""
    out = ["{"]
    for n, (key, item) in enumerate(value.items()):
        end = "," if n < len(value) - 1 else ""
        if isinstance(item, list) and item:
            out.append(f" {json.dumps(key)}: [")
            out += [f"  {json.dumps(x, ensure_ascii=False)}" + ("," if i < len(item) - 1 else "") for i, x in enumerate(item)]
            out.append(" ]" + end)
        else:
            out.append(f" {json.dumps(key)}: {json.dumps(item, ensure_ascii=False)}{end}")
    text = "\n".join(out) + "\n}\n"
    (HERE / name).write_text(text, encoding="utf-8")
    return len(text.encode())


total = dump("traces.json", {"cases": CASES})
total += dump("render.json", {"stories": STORIES})
total += dump("lines.json", HELPERS)
total += dump("crashes.json", {"cases": CRASHES})
print(f"{len(CASES)} trace cases, {len(STORIES)} story cases, {sum(len(v) for v in HELPERS.values())} helper cases, "
      f"{len(CRASHES)} crash cases, {total} bytes")
