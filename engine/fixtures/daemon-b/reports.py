"""Fixtures for the Rust port of ferb/daemon/reports.py (ENG-3): failure reports and diagnostic files built from
invented stores, the Send's preparation and its requests to a relay faked on 127.0.0.1, and the module's pure rules.

    cd <repo> && uv run python rust/fixtures/daemon-b/reports.py

Each build case runs in its own temporary FERB home (<tmp>/fxb-reports/<NN>/home, written /FX/<NN>/home), with a
FakeClock at 2026-01-01T09:00:00Z, sequential ids, TZ=UTC, macOS "26.1" (platform.mac_ver stood in for) and the
real Redactor (`sealed`: the values added to it after the records were written, as tests/test_failure_report.py
plants them). Writes reports.json beside this file:

- `builds`: `seed` (every row of tasks, journal, spans and task_results after seeding, in rowid order: the Rust
  test inserts the same rows), `files` (report files already in the folder, with their age in days), the call
  (`report` or `diagnostic` and its arguments), and what came out: `failure`, the returned dict, the file's text
  byte for byte and its mode, the folder after, or the FerbError's text.
- `prepare`: a saved file's text, the call, and the Outgoing (`file` as text, `issue`), the file after.
- `send`: an Outgoing, the relay's answers in order, every request FERB made (path with query, Content-Type,
  User-Agent, Content-Length, body) and the report id or the Unsent text. `pause` is 0.
- `relay_url`, `saved`, `shorten`, `summarize`, `redacted_text` (with PIECE_CHARS lowered), `problems`.

The cases are tests/test_failure_report.py's (every test but the 30 MB timing one, which the Rust test makes with a
lowered limit) plus edges found reading the code. Invented data only (no key-shaped values); the same bytes on every
run.
"""

import asyncio
import base64
import json
import os
import shutil
import socket
import stat
import sys
import tempfile
import threading
import time
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
ROOT = Path(tempfile.gettempdir()) / "fxb-reports"
shutil.rmtree(ROOT, ignore_errors=True)
ROOT.mkdir()
os.environ["FERB_HOME"] = str(ROOT / "home")
os.environ["HOME"] = str(ROOT / "user")
os.environ["TZ"] = "UTC"
time.tzset()
for _name in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy", "NO_PROXY",
              "no_proxy"):
    os.environ.pop(_name, None)
sys.path.insert(0, str(REPO))

import ferb  # noqa: E402
import ferb.daemon.reports as reports  # noqa: E402
from ferb.clock.clock import FakeClock  # noqa: E402
from ferb.core.errors import FerbError  # noqa: E402
from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.daemon.server import INTERNAL_ERROR, SLOW_CALL  # noqa: E402
from ferb.memory.journal import Journal  # noqa: E402
from ferb.sanitize.redact import Redactor  # noqa: E402
from ferb.store.db import Database  # noqa: E402
from ferb.store.spans import SpanStore  # noqa: E402

reports.platform = SimpleNamespace(mac_ver=lambda: ("26.1", ("", "", ""), ""))     # type: ignore[attr-defined]
MAX = reports.MAX_REPORT_BYTES
SEALED = "Kestrovan-Plume-4471"
QUOTED = 'Tr0ub"dor\\x9!\tQ'
LABELLED = "password: Plover-Quarry-9921"
REQUEST = "Rename the invoices in Downloads by month"
ANSWER = "I couldn't finish that: the Downloads folder was protected."
TABLES = ("tasks", "journal", "spans", "task_results")


def fx(value: Any) -> Any:
    """Every temporary path written /FX."""
    return json.loads(json.dumps(value).replace(json.dumps(str(ROOT))[1:-1], "/FX"))


class Case:
    def __init__(self, n: int) -> None:
        self.n = n
        self.home = ROOT / f"{n:02d}" / "home"
        self.home.mkdir(parents=True)
        self.db = Database(self.home / "ferb.db")
        self.db.migrate()
        self.clock = FakeClock()
        self.ids = SequentialIds("id")
        self.redact = Redactor()
        self.journal = Journal(self.db, self.clock, self.ids, self.redact)
        self.spans = SpanStore(self.db, self.clock, self.ids, self.redact)
        self.sealed: list[str] = []
        self.files: list[list[Any]] = []

    def seal(self, *values: str) -> None:
        for v in values:
            self.redact.add_secret_value(v)
            self.sealed.append(v)

    def raw(self, at: datetime, kind: str, data: dict[str, Any]) -> None:
        """A journal row written raw (a value sealed later was never matched in it)."""
        self.db.conn.execute("INSERT INTO journal (id, at, kind, data_json) VALUES (?, ?, ?, ?)",
                             (self.ids.new_id(), at.isoformat(), kind, json.dumps(data)))

    def task(self, task_id: str, status: str, outcome: dict[str, Any] | None, *, parent: str | None = None,
             tree: str | None = None, request: str | None = None, created: str | None = None,
             updated: str | None = None, goal: str = "rename invoices") -> None:
        self.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'ferb', 'worker', '[]', ?, ?, ?, ?)",
            (task_id, tree or task_id, parent, request if request is not None else "r-" + task_id, goal, status,
             json.dumps(outcome) if outcome is not None else "{}",
             created or self.clock.now().isoformat(), updated or self.clock.now().isoformat()))

    def file(self, name: str, days: float) -> None:
        folder = self.home / "reports"
        folder.mkdir(mode=0o700, exist_ok=True)
        (folder / name).write_text("x")
        when = (self.clock.now() - timedelta(days=days)).timestamp()
        os.utime(folder / name, (when, when))
        self.files.append([name, days])

    def dump(self) -> dict[str, Any]:
        out = {}
        for t in TABLES:
            cur = self.db.conn.execute(f"SELECT * FROM {t} ORDER BY rowid")
            rows = [list(r) for r in cur.fetchall()]
            if rows:
                out[t] = {"columns": [d[0] for d in cur.description], "rows": rows}
        return out

    def listing(self) -> list[list[Any]]:
        folder = self.home / "reports"
        if not folder.is_dir():
            return []
        return [[p.name, oct(stat.S_IMODE(p.lstat().st_mode))] for p in sorted(folder.iterdir())]


def failed_task(c: Case, task_id: str = "t-fail", status: str = "failed") -> datetime:
    """tests/test_failure_report.py `_failed_task`: the row, the owner's message, a failed tool call in the trace,
    secrets planted raw after capture."""
    start = c.clock.now()
    c.journal.append("chat.message", {"session_id": "s1", "request_id": "r-" + task_id, "text": REQUEST})
    root = c.spans.start("task", task_id=task_id, input={"goal": "rename invoices", "request_id": "r-" + task_id})
    call = c.spans.start("tool.call", task_id=task_id, parent_id=root.id,
                         input={"tool": "fs.list", "args": {"path": "~/Downloads"}, "why": "find the invoices"})
    c.clock.advance(65)
    c.spans.finish(call, error="denied: ~/Downloads is protected")
    c.spans.finish(root, output={"status": status, "summary": ANSWER, "answer": ANSWER})
    c.db.conn.execute("UPDATE spans SET input_json = ? WHERE id = ?",
                      (json.dumps({"tool": "fs.list", "why": "find the invoices",
                                   "args": {"path": "~/Downloads", "note": f"{SEALED} {LABELLED}", "text": QUOTED}}),
                       call.id))
    outcome = {"status": status, "summary": ANSWER, "answer": ANSWER}
    c.task(task_id, status, outcome, created=start.isoformat())
    c.seal(SEALED, QUOTED)
    return start


def steps_seed(c: Case, count: int, filler: int = 16) -> None:
    root = c.db.conn.execute("SELECT id FROM spans WHERE kind = 'task'").fetchone()[0]
    rows = []
    for i in range(count):
        at = (c.clock.now() + timedelta(seconds=i + 1)).isoformat()
        if i == count // 2:
            inp = {"tool": "owner.ask", "args": {"question": "Which folder holds the invoices?"}}
            out = {"answer": "Documents/Invoices"}
        else:
            inp = {"tool": "browser.read", "args": {"tab": i}}
            out = {"output": ("the last step " if i == count - 1 else "") + "filler text " * filler}
        rows.append((f"bulk-{i:04d}", root, at, at, json.dumps(inp), json.dumps(out)))
    c.db.conn.executemany("INSERT INTO spans (id, parent_id, task_id, kind, started_at, ended_at, input_json, "
                          "output_json) VALUES (?, ?, 't-fail', 'tool.call', ?, ?, ?, ?)", rows)


FACTS = {"runtimes": ["supervisor: codex model-a", "worker: claude model-b"],
         "extensions": ["chrome Default: 0.19.4"]}
BUILDS: list[dict[str, Any]] = []
CASES: dict[str, Case] = {}
BUILT: dict[str, dict[str, Any]] = {}


def build(name: str, what: str, c: Case, call: dict[str, Any]) -> None:
    out: dict[str, Any] = {"name": name, "what": what, "seed": c.dump(), "sealed": list(c.sealed),
                           "files": list(c.files), "now": c.clock.now().isoformat(), "call": call}
    try:
        if call["fn"] == "report":
            found, built = reports.report(c.db.path, call["task_id"], call["request_id"], call["facts"],
                                          home=c.home, now=c.clock.now(), clock=c.clock, ids=c.ids, walk=c.redact,
                                          text=c.redact.text, app_version=call["app_version"])
            out["failure"] = dict(vars(found))
        else:
            built = reports.diagnostic(c.db.path, call["health"], call["recording"], call["facts"], home=c.home,
                                       now=c.clock.now(), walk=c.redact, text=c.redact.text,
                                       app_version=call["app_version"])
        out["built"] = built
        out["text"] = Path(built["path"]).read_text(encoding="utf-8")
        out["mode"] = oct(stat.S_IMODE(Path(built["path"]).stat().st_mode))
        BUILT[name] = built
    except FerbError as e:
        out["error"] = str(e)
    out["folder"] = c.listing()
    CASES[name] = c
    BUILDS.append(fx(out))


def report_call(task_id: str = "t-fail", request_id: str = "", facts: Any = None, app_version: str = "") -> dict:
    return {"fn": "report", "task_id": task_id, "request_id": request_id, "facts": facts if facts is not None else {},
            "app_version": app_version}


def builds() -> None:
    n = iter(range(1, 100))
    # 1: the failed task, its window's problems and the latest 20, old reports purged, versions given
    c = Case(next(n))
    start = failed_task(c)
    end = c.clock.now()
    for i in range(30):
        c.raw(start - timedelta(hours=2, minutes=i), INTERNAL_ERROR,
              {"method": "chat.status", "error": "RuntimeError", "message": f"old-{i}", "frames": []})
    c.raw(start + timedelta(seconds=30), INTERNAL_ERROR,
          {"method": "tools.call", "error": "KeyError", "message": "in-window " + repr(QUOTED) + f" {SEALED}",
           "frames": ["ferb/x.py:3:f"]})
    c.raw(start + timedelta(seconds=31), "chat.failed", {"request_id": "r", "why": "not-a-problem-row"})
    for i in range(1, 26):
        c.raw(end + timedelta(minutes=i), SLOW_CALL, {"method": "chat.status", "seconds": 10 + i})
    c.file("FERB-report-2025-11-30-old.txt", reports.KEEP_DAYS + 1)
    c.file("FERB-report-2025-12-04-recent.txt", reports.KEEP_DAYS - 2)
    c.file("FERB-diagnostic-2025-11-01-090000.txt", reports.KEEP_DAYS + 5)
    c.file("notes.txt", reports.KEEP_DAYS + 5)
    build("failed_task", "the failed task: problems, secrets planted, purge, versions", c,
          report_call(facts=FACTS, app_version="0.28.0 (7)"))
    # 2: an interrupted task whose answer is what FERB told (task_results), no problems, no facts
    c = Case(next(n))
    c.journal.append("chat.message", {"session_id": "s1", "request_id": "r-t-int", "text": "  Tidy the desktop  \n"})
    c.task("t-int", "interrupted", {"status": "interrupted", "summary": ""}, created=c.clock.now().isoformat())
    c.clock.advance(3725.5)
    c.db.conn.execute("UPDATE tasks SET updated_at = ? WHERE id = 't-int'", (c.clock.now().isoformat(),))
    c.db.conn.execute("INSERT INTO task_results (task_id, status, answer, goal, at) VALUES "
                      "('t-int', 'interrupted', ?, 'tidy', ?)",
                      ("FERB restarted while this was running.\n\nIt stopped at step 2.", c.clock.now().isoformat()))
    build("interrupted", "an interrupted task: told answer, no spans, no problems", c, report_call("t-int"))
    # 3: a turn that failed before any task (chat.failed), by request id, with a reply and an ack
    c = Case(next(n))
    c.journal.append("chat.ack", {"session_id": "s1", "request_id": "r-9", "received_at": c.clock.now().isoformat()})
    c.journal.append("chat.message", {"session_id": "s1", "request_id": "r-9", "text": "Book a table for two"})
    c.clock.advance(12)
    c.journal.append("chat.reply", {"session_id": "s1", "request_id": "r-9", "text": "Something went wrong."})
    c.journal.append("chat.failed", {"request_id": "r-9", "why": "planner error"})
    build("turn_failed_no_task", "a failed turn with no task: named by its request", c,
          report_call("", "r-9", app_version="0.28.0 (12)\n"))
    # 4: a step of a tree: its request's chat.failed makes it a failed turn of the tree, with the task's status
    c = Case(next(n))
    c.task("tree-1", "running", None, request="r-tree")
    c.task("tree-1.step.a", "done", {"status": "done"}, parent="tree-1", tree="tree-1", request="r-tree")
    c.clock.advance(5)
    c.journal.append("chat.failed", {"request_id": "r-tree", "why": "the plan failed after its first line\nmore"})
    build("turn_failed_step", "a step whose request failed: the tree's report, the task's status", c,
          report_call("tree-1.step.a"))
    # 5: a scheduled run: no chat message or reply, its result names the request as the task
    c = Case(next(n))
    c.db.conn.execute("INSERT INTO task_results (task_id, status, answer, goal, at) VALUES "
                      "('r-sched', 'failed', '', 'check the weather', ?)", (c.clock.now().isoformat(),))
    c.journal.append("chat.failed", {"request_id": "r-sched", "why": "\n\n  the runtime was signed out  \n"})
    build("scheduled_run", "a scheduled run: the goal from its result, the reason from chat.failed", c,
          report_call("r-sched"))
    # 6: times FERB couldn't subtract (one without an offset): duration unknown; outcome summary only
    c = Case(next(n))
    c.task("t-naive", "failed", {"summary": "line one\nline two"}, created="2026-01-01T08:00:00",
           updated="2026-01-01T09:00:00+00:00")
    build("naive_time", "a start time without its offset: duration unknown", c, report_call("t-naive"))
    c = Case(next(n))
    c.task("t-none", "failed", None, created="", updated="")
    build("no_times", "no times, no outcome", c, report_call("t-none"))
    # refusals
    c = Case(next(n))
    c.task("t-done", "done", {"status": "done"})
    c.task("t-done.step.x", "failed", {"status": "failed"}, parent="t-done", tree="t-done", request="r-t-done")
    c.task("t-wait", "needs_user", None)
    for name, call in [("unknown", report_call("nope-nope-nope")), ("dots", report_call("../../etc")),
                       ("dots_request", report_call("", "../../etc")), ("empty", report_call("")),
                       ("child", report_call("t-done.step.x")), ("done", report_call("t-done")),
                       ("waiting", report_call("t-wait")), ("space", report_call("t done"))]:
        build("refused_" + name, "refused: " + name, c, call)
    # the bulky trace (for the shortening at Send)
    c = Case(next(n))
    failed_task(c)
    steps_seed(c, 200)
    build("bulky", "200 bulky steps with a question among them", c, report_call())
    # diagnostics
    c = Case(next(n))
    for i in range(25):
        c.task(f"d-{i:02d}", ["done", "failed", "running"][i % 3], None,
               created=(c.clock.now() + timedelta(minutes=i % 7)).isoformat())
    c.task("d-00.step.a", "done", None, parent="d-00", tree="d-00")
    for kind in ("setup.permission_asked", "Setup.mixed_case", "browsers.paired", "browser.handoff_shown",
                 "browser.other", "sense.watcher_failed", "schedule.retry_queued", "sense.other"):
        c.raw(c.clock.now(), kind, {"note": f"{kind} {SEALED}"})
    for i in range(45):
        c.raw(c.clock.now() + timedelta(seconds=i), "setup.step", {"i": i})
    c.raw(c.clock.now(), SLOW_CALL, {"method": "chat.status", "seconds": 11, "é": "ü"})
    c.file("FERB-diagnostic-2025-11-01-090000.txt", reports.KEEP_DAYS + 5)
    c.file("FERB-report-2025-11-01-x.txt", reports.KEEP_DAYS + 5)
    c.seal(SEALED)
    build("diagnostic", "a diagnostic file: health, recording, tasks, problems, setup events", c,
          {"fn": "diagnostic", "health": {"z": [1, 2.5, None, True], "note": f"fine {SEALED}", "a": {"b": "ü"}},
           "recording": {"paused": False}, "facts": FACTS, "app_version": "0.28.0 (7)"})
    c = Case(next(n))
    build("diagnostic_empty", "an empty store's diagnostic file", c,
          {"fn": "diagnostic", "health": {}, "recording": {}, "facts": {}, "app_version": ""})


def problems() -> list[Any]:
    c = CASES["failed_task"]
    out = []
    for started, ended in [(None, None), ("2026-01-01T09:00:00+00:00", "2026-01-01T09:01:05+00:00"),
                           ("2026-01-01T09:00:30+00:00", None)]:
        out.append([started, ended, [list(r) for r in reports.problems(c.db, started, ended)]])
    return out


# ---------- the Send ----------

def prepares() -> list[Any]:
    out = []

    def one(name: str, case: str, kind: str, title: str, app_version: str, *, late: str = "",
            append: str = "", limit: int = MAX) -> None:
        c = CASES[case]
        path = Path(BUILT[case]["path"])
        if append:
            path.write_text(path.read_text(encoding="utf-8") + append, encoding="utf-8")
        written = path.read_text(encoding="utf-8")
        if late:
            c.seal(late)
        reports.MAX_REPORT_BYTES = limit
        try:
            o = reports.prepare(path, kind, title, app_version, text=c.redact.text)
            result: dict[str, Any] = {"kind": o.kind, "report_id": o.report_id, "file": o.file.decode(),
                                      "issue": o.issue, "shortened": o.shortened}
        except reports.Unsent as e:
            result = {"unsent": str(e)}
        finally:
            reports.MAX_REPORT_BYTES = MAX
        out.append(fx({"name": name, "case": case, "path": str(path), "sealed": list(c.sealed),
                       "written": written, "call": {"kind": kind, "title": title, "app_version": app_version,
                                                    "limit": limit},
                       "result": result, "after": (lambda a: "(unchanged)" if a == written else a)(
                           path.read_text(encoding="utf-8"))}))
    one("plain", "failed_task", "failure", BUILT["failed_task"]["subject"], "0.28.0 (7)")
    one("sealed_later", "failed_task", "failure", "FERB report: x " + "y" * 300, "",
        append="\nlearned later: Quillon-Brask-8812\n", late="Quillon-Brask-8812")
    one("odd_version", "interrupted", "failure", "", "0.28\n.0\t(7)\x00  beta " + "9" * 60)
    one("shortened", "bulky", "failure", BUILT["bulky"]["subject"], "0.28.0", limit=40_000)
    one("diagnostic", "diagnostic", "diagnostic", BUILT["diagnostic"]["subject"], "0.28.0")
    return out


class Relay:
    """tests/test_failure_report.py's Relay: each request kept; `/file` and `/report` answered from queues."""

    def __init__(self, answers: list[Any], delay: float = 0.0) -> None:
        self.received: list[dict[str, Any]] = []
        self.answers = list(answers)
        relay = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
                relay.received.append({"path": self.path, "content_type": self.headers.get("Content-Type"),
                                       "user_agent": self.headers.get("User-Agent"),
                                       "content_length": self.headers.get("Content-Length"),
                                       "body": body.decode()})
                status, answer = relay.answers.pop(0) if relay.answers else (200, {"ok": True, "ticket": "t-x"})
                if delay:
                    time.sleep(delay)
                out = answer.encode() if isinstance(answer, str) else json.dumps(answer).encode()
                try:
                    self.send_response(status)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(out)))
                    self.end_headers()
                    self.wfile.write(out)
                except OSError:
                    pass

            def log_message(self, *args: Any) -> None:
                pass
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()


def sends() -> list[Any]:
    issue = {"version": 2, "title": "FERB report: it failed", "summary": "Status: failed\nü “quoted”",
             "app_version": "0.28.0 (7)"}
    file = json.dumps({"message": "FERB failure report FERB-report-2026-01-01-t-fail",
                       "content": base64.b64encode("the report é\n".encode()).decode()}).encode()
    out = reports.Outgoing("failure", "FERB-report-2026-01-01-t-fail", file, issue, False)
    ok_file = (200, {"ok": True, "ticket": "ticket-1"})
    conflict = (409, {"ok": False, "retry": True, "error": "another report was being saved at the same time"})
    cases: list[tuple[str, list[Any], dict[str, Any]]] = [
        ("ok", [ok_file, (200, {"ok": True, "report_id": "from-relay"})], {}),
        ("ok_no_report_id", [ok_file, (200, {"ok": True})], {}),
        ("conflicts_then_ok", [conflict, conflict, (200, {"ok": True, "ticket": "ticket-3"}),
                               (200, {"ok": True, "report_id": "r3"})], {}),
        ("conflicts_three", [conflict, conflict, conflict], {}),
        ("conflict_no_retry", [(409, {"ok": False, "error": "conflict"})], {}),
        ("file_502", [(502, {"ok": False, "error": "GitHub refused the file (HTTP 401)"})], {}),
        ("file_200_not_ok", [(200, {"ok": False, "error": "odd"})], {}),
        ("file_no_ticket", [(200, {"ok": True, "ticket": 7})], {}),
        ("file_429", [(429, {"ok": False, "error": "too many"})], {}),
        ("report_502", [ok_file, (502, {"ok": False, "error": "GitHub refused the report (HTTP 403)"})], {}),
        ("report_403", [ok_file, (403, {"ok": False, "error": "the ticket isn't valid " + "x" * 300})], {}),
        ("report_429", [ok_file, (429, {"ok": False})], {}),
        ("not_json", [(500, "<html>oops</html>")], {}),
        ("json_list", [(200, [1, 2])], {}),
        ("error_number", [(400, {"ok": False, "error": 12})], {}),
        ("timeout", [ok_file], {"delay": 1.0, "timeout": 0.2}),
    ]
    results = []
    for name, answers, opts in cases:
        relay = Relay(answers, opts.get("delay", 0.0))
        try:
            got: dict[str, Any] = {"report_id": asyncio.run(reports.send(relay.url, out,
                                                                         timeout=opts.get("timeout", 60.0),
                                                                         pause=0.0))}
        except reports.Unsent as e:
            got = {"unsent": str(e)}
        finally:
            relay.close()
        reqs = [dict(r, path=r["path"]) for r in relay.received]
        results.append({"name": name, "answers": [[s, a] for s, a in answers], "timeout": "timeout" in opts,
                        "requests": reqs, "result": got})
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    try:
        got = {"report_id": asyncio.run(reports.send(f"http://127.0.0.1:{port}", out, pause=0.0))}
    except reports.Unsent as e:
        got = {"unsent": str(e)}
    results.append({"name": "connect_refused", "answers": [], "timeout": False, "connect_error": True,
                    "requests": [], "result": got})
    return [{"outgoing": {"kind": out.kind, "report_id": out.report_id, "file": out.file.decode(),
                          "issue": out.issue, "shortened": out.shortened},
             "version": ferb.__version__, "cases": results}]


# ---------- the pure rules ----------

def relay_urls() -> list[Any]:
    urls = ["", "   ", "https://ferb-reports.example.workers.dev", "https://ferb-reports.example.workers.dev/report",
            "https://ferb-reports.example.workers.dev/", "https://Relay.Example.ORG:8443/base/report/",
            "HTTPS://relay.example.org/a//", "http://127.0.0.1:8787", "http://localhost/report", "http://[::1]:9/x",
            "http://relay.example.org/report", "ftp://127.0.0.1/report", "not a url",
            "https://user:pw@relay.example.org/report", "https://user@relay.example.org", "https://:pw@host.example",
            "https://", "https:///path", "https://[::1", "https://[zz]/", "https://host.example/report?x=1#f",
            "  https://host.example/reportreport  ", "https://host.example/a/report/report", "//host.example/x",
            "https:host.example", "http://LOCALHOST", "http://127.0.0.1.example.org", "https://host.example:99999/"]
    out = []
    for u in urls:
        try:
            out.append([u, {"url": reports.relay_url(u)}])
        except reports.Unsent as e:
            out.append([u, {"unsent": str(e)}])
    out.append([None, {"url": reports.relay_url(None)}])
    return out


def saved_cases() -> list[Any]:
    home = ROOT / "saved" / "home"
    folder = home / "reports"
    folder.mkdir(parents=True)
    other = ROOT / "saved" / "elsewhere"
    other.mkdir()
    (other / "FERB-report-2026-01-01-t-x.txt").write_text("not FERB's")
    (folder / "FERB-report-2026-01-01-t-fail.txt").write_text("r")
    (folder / "FERB-diagnostic-2026-01-01-090000.txt").write_text("d")
    (folder / "notes.txt").write_text("the owner's")
    (folder / "FERB-report-2026-01-01-link.txt").symlink_to(other / "FERB-report-2026-01-01-t-x.txt")
    (folder / "FERB-report-2026-01-01-dir.txt").mkdir()
    f = str(folder)
    paths = [f + "/FERB-report-2026-01-01-t-fail.txt", f + "/FERB-diagnostic-2026-01-01-090000.txt",
             str(other / "FERB-report-2026-01-01-t-x.txt"), f + "/notes.txt", f + "/FERB-report-2026-01-01-link.txt",
             f + "/../reports/FERB-report-2026-01-01-t-fail.txt", "", "FERB-report-2026-01-01-t-fail.txt",
             f + "/FERB-report-2026-01-01-gone.txt", f + "/FERB-report-2026-01-01-dir.txt",
             f + "//FERB-report-2026-01-01-t-fail.txt", f + "/./FERB-report-2026-01-01-t-fail.txt",
             f + "/FERB-report-2026-01-01-t-fail.txt/", f + "/FERB-report-01-01-x.txt",
             f + "/FERB-diagnostic-2026-01-01-0900.txt", f + "/FERB-report-2026-01-01-a b.txt"]
    out = []
    for p in paths:
        try:
            got, kind = reports.saved(home, p)
            out.append([p, {"path": str(got), "kind": kind}])
        except FerbError as e:
            out.append([p, {"error": str(e)}])
    return fx({"home": str(home), "setup": {
        "files": {"reports/FERB-report-2026-01-01-t-fail.txt": "r",
                  "reports/FERB-diagnostic-2026-01-01-090000.txt": "d", "reports/notes.txt": "the owner's"},
        "outside": "../elsewhere/FERB-report-2026-01-01-t-x.txt",
        "links": {"reports/FERB-report-2026-01-01-link.txt": "../elsewhere/FERB-report-2026-01-01-t-x.txt"},
        "dirs": ["reports/FERB-report-2026-01-01-dir.txt"]}, "cases": out})


def step_block(i: int, extra: str = "", width: int = 300) -> str:
    return (f"12:00:{i % 60:02d}  tool.call  browser.read  [0.1s]\n    {{\n      \"error\": null,{extra}\n"
            f"      \"output\": \"{'é' * width} step {i}\"\n    }}")


def shorten_bodies() -> dict[str, str]:
    steps = [step_block(i) for i in range(100)]
    steps[10] = steps[10].replace('"error": null', '"error": "boom at ten"')
    steps[20] = steps[20].replace("browser.read", "owner.ask")
    steps[30] = steps[30].replace('"error": null', '"error": nullish')
    steps[40] = steps[40].replace('"error": null', '"error": "with \\"quote\\" and \\u00e9"') \
        .replace("browser.read", "owner.ask").replace('"output"', '"question": "Pick one?",\n      "answer": "B",\n'
                                                                   '      "output"')
    steps[50] = steps[50].replace("tool.call  browser.read", "model.question  ask the orchestrator")
    steps[60] = steps[60].replace('"error": null', '"error": "bad \\x escape"')
    body = ("FERB report for t\n\n## Summary\nStatus: failed\nFERB version: 0.27.3\n\n## Request\nYour request:\nx\n\n"
            "## Steps\ntask t\n" + "\n".join(steps) + "\n\n## FERB's recorded problems\nFERB's own records.\n"
            "2026-01-01T09:00:00+00:00  daemon.slow_call  {\"seconds\": 11}\n(none recorded)\n")
    many = body.replace("## Steps\ntask t\n", "## Steps\ntask t\n" + "\n".join(
        step_block(i, width=20).replace('"error": null', f'"error": "fail {i}"') for i in range(400)) + "\n")
    return {"steps": body, "plain": "é" * 30_000, "many": many,
            "no_steps_end": "## Summary\nx\n\n## Steps\ntask t\n" + "z" * 50,
            "label_only": "\n## Steps\ntask t\n 09:00:01  tool.call  [1.0s]\n 09:00:02  model.turn  plan  [2s]\n",
            "no_summary": "nothing here\n## Steps\n" + "\n".join(steps[:5])}


def shorten_cases(bodies: dict[str, str]) -> list[Any]:
    cases = [("steps", None, ""), ("steps", 20_000, "FERB-report-x.txt"), ("steps", 60_000, ""),
             ("steps", 9_000, ""), ("plain", 20_000, ""), ("plain", 60_001, "n.txt"), ("plain", 1_500, ""),
             ("label_only", 2_100, ""),
             ("many", 60_000, "m.txt"), ("no_steps_end", 30, ""), ("label_only", 40, "")]
    out = []
    for body, limit, name in cases:
        lim = len(bodies[body].encode()) if limit is None else limit
        text, note = reports.shorten(bodies[body], lim, name)
        out.append({"body": body, "limit": lim, "name": name, "text": text, "note": note})
    return out


def summarize_cases(bodies: dict[str, str]) -> list[Any]:
    out = []
    for body, note, limit in [("steps", "", 30_000), ("steps", "[Shortened to fit 0 MB: x]", 30_000),
                              ("many", "", 5_000), ("many", "", 30_000), ("plain", "", 300),
                              ("no_summary", "", 30_000), ("label_only", "n", 30_000)]:
        out.append({"body": body, "note": note, "limit": limit,
                    "summary": reports.summarize(bodies[body], note, limit)})
    return out


def redacted_text_cases() -> list[Any]:
    c = CASES["failed_task"]
    text = Path(BUILT["failed_task"]["path"]).read_text(encoding="utf-8")
    out = []
    held = reports.PIECE_CHARS
    for pieces in (held, 256, 2048):
        reports.PIECE_CHARS = pieces
        seen: list[str] = []

        def recording(s: str) -> str:
            seen.append(s)
            return c.redact.text(s)
        got = reports._redacted_text(text + f"\nlater {SEALED}\n## tail\n", recording)
        out.append({"piece_chars": pieces, "pieces": seen, "result": got})
    reports.PIECE_CHARS = held
    return fx({"input": text + f"\nlater {SEALED}\n## tail\n", "sealed": c.sealed, "cases": out})


def main() -> None:
    builds()
    bodies = shorten_bodies()
    out = {"version": ferb.__version__, "macos": "26.1", "builds": BUILDS, "problems": problems(),
           "prepare": prepares(), "send": sends(), "relay_url": relay_urls(), "saved": saved_cases(),
           "bodies": bodies, "shorten": shorten_cases(bodies), "summarize": summarize_cases(bodies),
           "redacted_text": redacted_text_cases()}
    text = json.dumps(out, ensure_ascii=False, indent=1) + "\n"
    (HERE / "reports.json").write_text(text, encoding="utf-8")
    print(f"reports.json: {len(text.encode())} bytes, {len(BUILDS)} builds")
    shutil.rmtree(ROOT, ignore_errors=True)


if __name__ == "__main__":
    main()
