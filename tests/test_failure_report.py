"""Send to developer (L35; relay owner decision 2026-11-01): `reports.build` writes one readable text file for a turn
FERB recorded as ending badly (its root task failed or was interrupted, or its request has a `chat.failed` row) and
returns its subject; the owner previews it in FERB.app and presses Send, and `reports.send` sends that same text to
the developer's report relay as one file plus one issue with a short summary linking it (a report over 25 MB goes
shortened; the saved file stays whole). Every part of the file is redacted (rule 14): each span structurally before it is
rendered, then each block as text, with sealed values matched in their escaped forms too, and once more at Send. It
lives in an owner-only folder, and reports older than 30 days go when the next one is built. A failed send keeps the
file and says why; nothing retries by itself. FERB's raw stderr never goes in: its last section is FERB's recorded
problems."""

import asyncio
import base64
import json
import os
import stat
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from ferb.daemon import auth
from ferb.daemon import reports as reports_module
from ferb.daemon.reports import RELAY_URL
from ferb.daemon.reports import (KEEP_DAYS, NOT_SET_UP, PROBLEMS_LAST, SECTIONS, relay_url,
                                  shorten, summarize)
from ferb.daemon.server import INTERNAL_ERROR, SLOW_CALL
from ferb.sanitize.redact import Redactor, SealedWord, escaped_forms, sealed_spans
from tests.conftest import RunningDaemon, chat

SEALED = "Kestrovan-Plume-4471"                       # a sealed value FERB learned after the record was written
QUOTED = 'Tr0ub"dor\\x9!\tQ'                          # one holding a quote, a backslash and a tab (review 1, I1)
API_KEY = "sk-ant-api03-" + "Zq7" * 12                 # an Anthropic key shape (a secret class)
REQUEST = "Rename the invoices in Downloads by month"
ANSWER = "I couldn't finish that: the Downloads folder was protected."


class Relay:
    """The developer's report relay, faked on this Mac: what each request carried, and the answers it gives. `/file`
    takes GitHub's "create a file" JSON and answers a ticket; `/report` takes the issue's fields with that ticket."""

    def __init__(self) -> None:
        self.received: list[tuple[str, dict[str, str], bytes]] = []
        self.file_answers: list[tuple[int, dict[str, Any]]] = []     # queued answers to /file, else a ticket
        self.status, self.answer = 200, {"ok": True, "report_id": ""}     # the answer to /report
        relay = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
                relay.received.append((self.path, dict(self.headers), body))
                url = urlsplit(self.path)
                if url.path == "/file":
                    status, answer = (relay.file_answers.pop(0) if relay.file_answers else
                                      (200, {"ok": True, "ticket": f"ticket-{len(relay.received)}"}))
                else:
                    status, answer = relay.status, dict(relay.answer)
                    if answer.get("report_id") == "":
                        answer["report_id"] = "from-relay"
                out = json.dumps(answer).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(out)))
                self.end_headers()
                self.wfile.write(out)

            def log_message(self, *args: Any) -> None:
                pass
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def files(self) -> list[tuple[dict[str, list[str]], dict[str, Any], str]]:
        """Each /file request: its query, its JSON and the file's text."""
        out = []
        for path, _, body in self.received:
            if urlsplit(path).path == "/file":
                data = json.loads(body)
                out.append((parse_qs(urlsplit(path).query), data, base64.b64decode(data["content"]).decode()))
        return out

    def issues(self) -> list[dict[str, Any]]:
        return [json.loads(body) for path, _, body in self.received if urlsplit(path).path == "/report"]


@pytest.fixture
def relay(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> Any:
    for name in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
        monkeypatch.delenv(name, raising=False)
    r = Relay()
    daemon.daemon.config.set("feedback", "relay_url", r.url)
    yield r
    r.server.shutdown()
    r.server.server_close()


def _forms(value: str, nested: bool = False) -> set[str]:
    """Every way a text can show a value: as is, JSON-escaped (with and without ASCII escapes) and as a repr; `nested`:
    also a repr inside JSON (a problem row's message holding an exception's repr), which only the report's structural
    pass sees unescaped."""
    forms = {value, json.dumps(value, ensure_ascii=False)[1:-1], json.dumps(value)[1:-1],
             value.encode("unicode_escape").decode(), repr(value)[1:-1]}
    return forms | ({json.dumps(repr(value)[1:-1], ensure_ascii=False)[1:-1]} if nested else set())


def _on_loop(daemon: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


def _problem(daemon: RunningDaemon, at: datetime, kind: str, data: dict[str, Any]) -> None:
    """A problem row as the daemon journals it; written raw (a value sealed later was never matched in it)."""
    _on_loop(daemon, lambda: daemon.daemon.db.conn.execute(               # type: ignore[union-attr]
        "INSERT INTO journal (id, at, kind, data_json) VALUES (?, ?, ?, ?)",
        (uuid.uuid4().hex, at.isoformat(), kind, json.dumps(data))))


def _failed_task(daemon: RunningDaemon, task_id: str = "t-fail", status: str = "failed") -> None:
    """A root task as the supervisor leaves it: its row with the final outcome, the owner's chat message, a tool call
    with an error in its trace. The secrets are planted raw (as values learned after capture would be)."""
    d = daemon.daemon

    def seed() -> None:
        assert d.db is not None and d.spans is not None and d.journal is not None
        start = d.clock.now()
        d.journal.append("chat.message", {"session_id": "s1", "request_id": "r-" + task_id, "text": REQUEST})
        root = d.spans.start("task", task_id=task_id, input={"goal": "rename invoices", "request_id": "r-" + task_id})
        call = d.spans.start("tool.call", task_id=task_id, parent_id=root.id,
                             input={"tool": "fs.list", "args": {"path": "~/Downloads"}, "why": "find the invoices"})
        d.clock.advance(65)                                                  # type: ignore[attr-defined]
        d.spans.finish(call, error="denied: ~/Downloads is protected")
        d.spans.finish(root, output={"status": status, "summary": ANSWER, "answer": ANSWER})
        # planted after capture, raw: the store's own redaction never saw them
        d.db.conn.execute("UPDATE spans SET input_json = ? WHERE id = ?",
                          (json.dumps({"tool": "fs.list", "why": "find the invoices",
                                       "args": {"path": "~/Downloads", "note": f"{SEALED} {API_KEY}",
                                                "text": QUOTED}}), call.id))
        outcome = {"status": status, "summary": ANSWER, "answer": ANSWER}
        d.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES (?, ?, NULL, ?, 'rename invoices', 'ferb', 'worker', '[]', "
            "?, ?, ?, ?)", (task_id, task_id, "r-" + task_id, status, json.dumps(outcome), start.isoformat(),
                            d.clock.now().isoformat()))
        d.redact.remember_hidden_secret(SEALED)                   # type: ignore[attr-defined]
        d.redact.remember_hidden_secret(QUOTED)                   # type: ignore[attr-defined]
    _on_loop(daemon, seed)


def _start(daemon: RunningDaemon) -> datetime:
    """When `_failed_task`'s task started (FakeClock's start)."""
    return daemon.daemon.clock.now() - timedelta(seconds=65)


def _build(daemon: RunningDaemon, task_id: str = "", timeout: float = 10, **params: Any) -> dict[str, Any]:
    c = Client(daemon.sock, timeout=timeout)
    try:
        return c.call("reports.build", ({"task_id": task_id} if task_id else {}) | params)
    finally:
        c.close()


def _send(daemon: RunningDaemon, path: str, **params: Any) -> dict[str, Any]:
    c = Client(daemon.sock, timeout=120)
    try:
        return c.call("reports.send", {"path": path} | params)
    finally:
        c.close()


def _turn(daemon: RunningDaemon, text: str) -> list[dict[str, Any]]:
    """One chat turn's raw events, request ids kept (the ids the notch binds a turn to)."""
    c = Client(daemon.sock, timeout=30)
    try:
        return list(c.stream("chat.send", {"session_id": "s1", "text": text}))
    finally:
        c.close()


def _section(text: str, name: str) -> str:
    start = text.index(f"\n## {name}\n")
    later = [text.index(f"\n## {n}\n") for n in SECTIONS if text.index(f"\n## {n}\n") > start]
    return text[start:min(later, default=len(text))]


# decision: DMN-505
def test_recorded_problems_are_those_of_the_tasks_window_and_the_last_20(daemon: RunningDaemon) -> None:
    """Review 1, I3 (ruling): section 4 is FERB's own problem rows (`daemon.internal_error`, `daemon.slow_call`,
    redacted at capture, with frames), never the raw stderr in logs/daemon.err.log."""
    _failed_task(daemon)
    start, end = _start(daemon), daemon.daemon.clock.now()
    for i in range(30):
        _problem(daemon, start - timedelta(hours=2, minutes=i), INTERNAL_ERROR,
                 {"method": "chat.status", "error": "RuntimeError", "message": f"old-{i}", "frames": []})
    _problem(daemon, start + timedelta(seconds=30), INTERNAL_ERROR,
             {"method": "tools.call", "error": "KeyError", "message": "in-window", "frames": ["ferb/x.py:3:f"]})
    _problem(daemon, start + timedelta(seconds=31), "chat.failed", {"request_id": "r", "why": "not-a-problem-row"})
    for i in range(1, 26):
        _problem(daemon, end + timedelta(minutes=i), SLOW_CALL, {"method": "chat.status", "seconds": 10 + i})
    logs = daemon.daemon.home / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    (logs / "daemon.err.log").write_text("Traceback: raw-stderr-line\nCookie: sessionid=9f8e7d6c5b4a\n")
    text = Path(_build(daemon, "t-fail")["path"]).read_text()
    problems = _section(text, "FERB's recorded problems")
    assert PROBLEMS_LAST == 20
    assert "in-window" in problems and "ferb/x.py:3:f" in problems
    assert all(f'"seconds": {10 + i}' in problems for i in range(6, 26))
    assert '"seconds": 15' not in problems and "old-" not in problems and "not-a-problem-row" not in problems
    assert "raw-stderr-line" not in text and "sessionid" not in text and "daemon.err.log" not in text


def test_planted_sealed_values_in_any_escaped_form_and_an_api_key_never_reach_the_file(daemon: RunningDaemon) -> None:
    """Review 1, I1: a sealed value holding `"`, `\\` or a control character was matched only as typed, so the JSON
    rendering of the trace (`\\"`, `\\\\`, `\\t`) and a problem row's repr carried it out escaped."""
    _failed_task(daemon)

    def plant() -> None:            # in the reason and the answer too
        outcome = {"status": "failed", "summary": f"{ANSWER} ({QUOTED})", "answer": f"{ANSWER}\nseen: {QUOTED}"}
        daemon.daemon.db.conn.execute("DELETE FROM tasks WHERE id = 't-fail'")      # type: ignore[union-attr]
        daemon.daemon.db.conn.execute(                                              # type: ignore[union-attr]
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES ('t-fail', 't-fail', NULL, 'r-t-fail', 'rename invoices', "
            "'ferb', 'worker', '[]', 'failed', ?, ?, ?)",
            (json.dumps(outcome), _start(daemon).isoformat(), daemon.daemon.clock.now().isoformat()))
    _on_loop(daemon, plant)
    _problem(daemon, _start(daemon) + timedelta(seconds=5), INTERNAL_ERROR,
             {"method": "tools.call", "error": "KeyError", "message": repr(QUOTED) + f" {SEALED} {API_KEY}",
              "frames": []})
    stored = _on_loop(daemon, lambda: daemon.daemon.db.conn.execute(          # type: ignore[union-attr]
        "SELECT input_json FROM spans WHERE kind = 'tool.call'").fetchone()[0])
    assert SEALED in stored and API_KEY in stored and json.dumps(QUOTED)[1:-1] in stored   # the trace holds them
    r = _build(daemon, "t-fail")
    text = Path(r["path"]).read_text()
    for value in (SEALED, QUOTED):
        for form in _forms(value, nested=True):
            assert form not in text, form
            assert all(form not in v for v in r.values()), form
    assert API_KEY not in text and "sk-ant" not in text and "[redacted]" in text


def test_the_redactor_hides_a_sealed_values_escaped_forms_everywhere() -> None:
    """Review 1, I1 part 2: every consumer of the redactor matches a sealed value's escaped forms; a dictionary word's
    forms keep its whole-word rule (L36 §7)."""
    r = Redactor()
    r.remember_hidden_secret(QUOTED)
    for form in _forms(QUOTED):
        assert form not in r.text(f"before {form} after"), form
        assert form not in json.dumps(r({"value": f"x {form} y"})), form
    word = SealedWord("naïve")
    forms = escaped_forms(word)
    assert {str(f) for f in forms} == {"na\\u00efve", "na\\xefve", "na%C3%AFve"}           # JSON, repr, URL
    assert all(isinstance(f, SealedWord) for f in forms)
    ascii_form = next(f for f in forms if "u00ef" in f)
    assert sealed_spans('"na\\u00efve"', ascii_form) and not sealed_spans("na\\u00efvety", ascii_form)
    assert escaped_forms("plain-value-1") == ()


def test_an_unknown_task_is_refused(daemon: RunningDaemon) -> None:
    with pytest.raises(RpcError, match="no failed task or request"):
        _build(daemon, "nope-nope-nope")
    with pytest.raises(RpcError, match="no failed task or request"):
        _build(daemon, "../../etc")
    with pytest.raises(RpcError, match="no failed task or request"):
        _build(daemon, request_id="../../etc")
    with pytest.raises(RpcError, match="no failed task or request"):
        _build(daemon)


# decision: DMN-8, DMN-9, DMN-28, DMN-31
def test_send_posts_the_previewed_text_and_no_secret_reaches_the_relay(daemon: RunningDaemon, relay: Relay) -> None:
    """The owner's Send: the relay gets exactly the file they previewed, as GitHub's "create a file" JSON, then the
    issue's short summary built from the same text, with the relay's ticket; the secrets planted in a span (a sealed
    value in its escaped forms, an API key) reach neither, nor the title."""
    _failed_task(daemon)
    built = _build(daemon, "t-fail", app_version="0.28.0 (7)")
    previewed = Path(built["path"]).read_text()
    r = _send(daemon, built["path"], title=built["subject"], app_version="0.28.0 (7)")
    assert r == {"sent": True, "path": built["path"], "report_id": "from-relay", "shortened": False}
    assert [urlsplit(path).path for path, _, _ in relay.received] == ["/file", "/report"]     # file first, one issue
    for _, headers, _ in relay.received:
        assert headers["Content-Type"] == "application/json" and headers["User-Agent"].startswith("FERB/")
    [(query, data, sent)] = relay.files()
    assert query == {"kind": ["failure"], "report_id": ["FERB-report-2026-01-01-t-fail"]}
    assert set(data) == {"message", "content"} and sent == previewed
    assert Path(built["path"]).read_text() == previewed                     # unchanged, still saved
    [issue] = relay.issues()
    assert set(issue) == {"version", "ticket", "title", "summary", "app_version"}
    assert (issue["version"], issue["ticket"], issue["app_version"]) == (2, "ticket-1", "0.28.0 (7)")
    assert issue["title"] == built["subject"]
    for fact in ("Status: failed", f"Failure reason: {ANSWER}", "Errors (1)", "denied: ~/Downloads is protected",
                 "Questions (0)"):
        assert fact in issue["summary"], fact
    for text in (sent, relay.received[1][2].decode()):
        for value in (SEALED, QUOTED):
            for form in _forms(value, nested=True):
                assert form not in text, form
        assert API_KEY not in text and "sk-ant" not in text
    assert "[redacted]" in sent
    sent_rows = _on_loop(daemon, lambda: [e.data for e in daemon.daemon.journal.of_kind("report.sent")])
    assert sent_rows == [{"file": built["path"], "kind": "failure", "report_id": "from-relay", "shortened": False}]


# decision: DMN-28
def test_a_value_sealed_after_the_build_is_hidden_at_send_and_in_the_kept_file(daemon: RunningDaemon,
                                                                              relay: Relay) -> None:
    _failed_task(daemon)
    built = _build(daemon, "t-fail")
    late = "Quillon-Brask-8812"
    Path(built["path"]).write_text(Path(built["path"]).read_text() + f"\nlearned later: {late}\n")
    _on_loop(daemon, lambda: daemon.daemon.redact.remember_hidden_secret(late))     # type: ignore[attr-defined]
    assert _send(daemon, built["path"], title=built["subject"])["sent"] is True
    [(_, _, sent)] = relay.files()
    assert late not in sent and late not in Path(built["path"]).read_text()
    assert late not in relay.issues()[0]["summary"]
    assert sent == Path(built["path"]).read_text()                             # what is kept is what was sent


# decision: DMN-8
def test_a_diagnostic_file_is_sent_the_same_way(daemon: RunningDaemon, relay: Relay) -> None:
    async def health(session: Any, params: dict[str, Any]) -> dict[str, Any]:
        return {"note": f"fine {SEALED}"}

    async def recording(session: Any, params: dict[str, Any]) -> dict[str, Any]:
        return {"paused": False}
    daemon.daemon._doctor_check = health                 # type: ignore[method-assign]
    daemon.daemon._sense_status = recording              # type: ignore[method-assign]
    _on_loop(daemon, lambda: daemon.daemon.redact.remember_hidden_secret(SEALED))     # type: ignore[attr-defined]
    c = Client(daemon.sock, timeout=30)
    try:
        built = c.call("reports.diagnostic", {"app_version": "0.28.0", "send": True})
    finally:
        c.close()
    assert built["subject"].startswith("FERB diagnostics (FERB ")
    assert _send(daemon, built["path"], title=built["subject"], app_version="0.28.0")["sent"] is True
    [(query, _, sent)] = relay.files()
    assert query["kind"] == ["diagnostic"] and sent == Path(built["path"]).read_text()
    [issue] = relay.issues()
    assert "FERB version: " in issue["summary"] and "FERB's recorded problems (0)" in issue["summary"]
    assert all(SEALED not in body.decode() for _, _, body in relay.received)


# decision: DMN-28, DMN-31
@pytest.mark.parametrize("step,status,answer,why", [
    ("file", 502, {"ok": False, "error": "GitHub refused the file (HTTP 401)"}, "HTTP 502: GitHub refused the file"),
    ("file", 200, {"ok": False, "error": "odd"}, "HTTP 200: odd"),
    ("file", 429, {"ok": False, "error": "too many"}, "try again in a few minutes"),
    ("report", 502, {"ok": False, "error": "GitHub refused the report (HTTP 403)"}, "HTTP 502: GitHub refused the rep"),
    ("report", 403, {"ok": False, "error": "the ticket isn't valid"}, "HTTP 403: the ticket isn't valid"),
])
def test_a_refused_send_keeps_the_file_and_says_why(daemon: RunningDaemon, relay: Relay, step: str, status: int,
                                                    answer: dict[str, Any], why: str) -> None:
    _failed_task(daemon)
    built = _build(daemon, "t-fail")
    if step == "file":
        relay.file_answers = [(status, answer)]
    else:
        relay.status, relay.answer = status, answer
    r = _send(daemon, built["path"], title=built["subject"])
    assert r["sent"] is False and why in r["error"] and Path(built["path"]).exists()
    assert len(relay.received) == (1 if step == "file" else 2)              # tried once; never by itself again
    relay.status, relay.answer = 200, {"ok": True, "report_id": ""}
    assert _send(daemon, built["path"], title=built["subject"])["sent"] is True       # Send again
    failed = _on_loop(daemon, lambda: [e.data for e in daemon.daemon.journal.of_kind("report.send_failed")])
    assert len(failed) == 1 and failed[0]["file"] == built["path"] and why in failed[0]["why"]


# decision: DMN-28
def test_a_commit_conflict_sends_the_file_again_and_files_one_issue(daemon: RunningDaemon, relay: Relay,
                                                                    monkeypatch: pytest.MonkeyPatch) -> None:
    """Two reports committed at the same moment: GitHub's Contents API answers 409 to one, the relay passes that on
    with `retry`, and FERB sends the file again (the relay can't: it streamed the body), up to three times."""
    monkeypatch.setattr(reports_module, "CONFLICT_TRIES", 3)
    _failed_task(daemon)
    built = _build(daemon, "t-fail")
    conflict = (409, {"ok": False, "retry": True, "error": "another report was being saved at the same time"})
    relay.file_answers = [conflict, conflict]
    assert _send(daemon, built["path"], title=built["subject"])["sent"] is True
    assert [urlsplit(p).path for p, _, _ in relay.received] == ["/file", "/file", "/file", "/report"]
    assert relay.issues()[0]["ticket"] == "ticket-3"
    relay.received.clear()
    relay.file_answers = [conflict, conflict, conflict]
    r = _send(daemon, built["path"], title=built["subject"])
    assert r["sent"] is False and "another report was being saved" in r["error"] and len(relay.received) == 3


# decision: DMN-31
def test_without_a_relay_address_the_report_is_written_and_send_says_it_isnt_set_up(daemon: RunningDaemon) -> None:
    _failed_task(daemon)
    built = _build(daemon, "t-fail")
    assert Path(built["path"]).exists()
    r = _send(daemon, built["path"], title=built["subject"])
    assert r == {"sent": False, "path": built["path"], "error": NOT_SET_UP}
    assert Path(built["path"]).exists()


# decision: DMN-31
@pytest.mark.parametrize("url", ["http://relay.example.org/report", "ftp://127.0.0.1/report", "not a url",
                                 "https://user:pw@relay.example.org/report"])
def test_a_relay_address_that_isnt_https_is_refused(daemon: RunningDaemon, url: str) -> None:
    _failed_task(daemon)
    built = _build(daemon, "t-fail")
    daemon.daemon.config.set("feedback", "relay_url", url)
    r = _send(daemon, built["path"])
    assert r["sent"] is False and "isn't a valid https address" in r["error"]


# decision: DMN-31
def test_the_relay_address_is_its_base_and_the_first_relays_report_address_still_works() -> None:
    assert RELAY_URL.startswith("https://")              # the deployed relay, built in (tests never reach it)
    assert relay_url("https://ferb-reports.example.workers.dev") == "https://ferb-reports.example.workers.dev"
    assert relay_url("https://ferb-reports.example.workers.dev/report") == "https://ferb-reports.example.workers.dev"
    assert relay_url("https://ferb-reports.example.workers.dev/") == "https://ferb-reports.example.workers.dev"


# decision: DMN-28
def test_send_takes_only_a_file_ferb_wrote_in_its_reports_folder(daemon: RunningDaemon, relay: Relay,
                                                                tmp_path: Path) -> None:
    _failed_task(daemon)
    built = _build(daemon, "t-fail")
    folder = Path(built["path"]).parent
    other = tmp_path / "FERB-report-2026-01-01-t-x.txt"
    other.write_text("not FERB's")
    (folder / "notes.txt").write_text("the owner's")
    (folder / "FERB-report-2026-01-01-link.txt").symlink_to(other)
    for path in (str(other), str(folder / "notes.txt"), str(folder / "FERB-report-2026-01-01-link.txt"),
                 str(folder / ".." / "reports" / Path(built["path"]).name), "", "FERB-report-2026-01-01-t-fail.txt"):
        with pytest.raises(RpcError, match="only a report or diagnostic file"):
            _send(daemon, path)
    with pytest.raises(RpcError, match="no longer saved"):
        _send(daemon, str(folder / "FERB-report-2026-01-01-gone.txt"))
    assert relay.received == []


def _steps_seed(daemon: RunningDaemon, count: int) -> None:
    """`count` bulky steps after `_failed_task`'s failed call, a question to the owner among them, the last marked."""
    d = daemon.daemon

    def seed() -> None:
        root = d.db.conn.execute("SELECT id FROM spans WHERE kind = 'task'").fetchone()[0]   # type: ignore[union-attr]
        rows = []
        for i in range(count):
            at = (d.clock.now() + timedelta(seconds=i + 1)).isoformat()
            if i == count // 2:
                inp = {"tool": "owner.ask", "args": {"question": "Which folder holds the invoices?"}}
                out = {"answer": "Documents/Invoices"}
            else:
                inp = {"tool": "browser.read", "args": {"tab": i}}
                out = {"output": ("the last step " if i == count - 1 else "") + "filler text " * 80}
            rows.append((f"bulk-{i:04d}", root, at, at, json.dumps(inp), json.dumps(out)))
        d.db.conn.executemany(                                               # type: ignore[union-attr]
            "INSERT INTO spans (id, parent_id, task_id, kind, started_at, ended_at, input_json, output_json) VALUES "
            "(?, ?, 't-fail', 'tool.call', ?, ?, ?, ?)", rows)
    _on_loop(daemon, seed)


# decision: DMN-9, DMN-29
def test_a_report_over_the_cap_is_sent_shortened_and_kept_whole(daemon: RunningDaemon, relay: Relay,
                                                                monkeypatch: pytest.MonkeyPatch) -> None:
    """Owner, 2026-11-01: a report over 25 MB goes shortened to fit (here the cap is lowered to 40 KB): the note at the
    top says what was cut; the summary, every error and question and the last steps are kept; the file on the Mac
    stays whole."""
    monkeypatch.setattr(reports_module, "MAX_REPORT_BYTES", 40_000)
    _failed_task(daemon)
    _steps_seed(daemon, 200)
    built = _build(daemon, "t-fail")
    whole = Path(built["path"]).read_text()
    assert len(whole.encode()) > 200_000
    r = _send(daemon, built["path"], title=built["subject"])
    assert r["sent"] is True and r["shortened"] is True
    [(_, _, sent)] = relay.files()
    assert len(sent.encode()) <= 40_000
    assert sent.startswith("[Shortened to fit ")
    assert "steps" in sent.split("\n", 1)[0] and Path(built["path"]).name in sent.split("\n", 1)[0]
    for kept in ("## Summary", f"Failure reason: {ANSWER}", "## Request", REQUEST, "denied: ~/Downloads is protected",
                 "Which folder holds the invoices?", "the last step", "## FERB's recorded problems"):
        assert kept in sent, kept
    assert "[FERB cut " in sent
    assert Path(built["path"]).read_text() == whole                           # the full report stays on the Mac
    summary = relay.issues()[0]["summary"]
    assert summary.startswith("[Shortened to fit ") and "Which folder holds the invoices? → Documents/Invoices" in summary


# decision: DMN-29
def test_shortening_keeps_errors_questions_and_the_last_steps_and_respects_the_byte_cap() -> None:
    def block(i: int, extra: str = "") -> str:
        return f"12:00:{i % 60:02d}  tool.call  browser.read  [0.1s]\n    {{\n      \"error\": null,{extra}\n" \
               f"      \"output\": \"{'é' * 300} step {i}\"\n    }}"
    steps = [block(i) for i in range(100)]
    steps[10] = steps[10].replace('"error": null', '"error": "boom at ten"')
    steps[20] = steps[20].replace("browser.read", "owner.ask")
    body = ("FERB report for t\n\n## Summary\nStatus: failed\n\n## Request\nYour request:\nx\n\n## Steps\ntask t\n"
            + "\n".join(steps) + "\n\n## FERB's recorded problems\nFERB's own records.\n(none recorded)\n")
    assert shorten(body, len(body.encode())) == (body, "")
    text, note = shorten(body, 20_000, "FERB-report-x.txt")
    assert len(text.encode()) <= 20_000 and text.startswith(note) and "FERB-report-x.txt" in note
    assert "boom at ten" in text and "owner.ask" in text and "step 99" in text and "## Summary" in text
    assert "step 50" not in text and "[FERB cut " in text and "## FERB's recorded problems" in text
    plain = "é" * 30_000                                                      # no Steps: the middle goes
    cut, note = shorten(plain, 20_000)
    assert len(cut.encode()) <= 20_000 and cut.startswith(note) and cut.endswith("é")
    long = summarize(body.replace("## Steps\ntask t\n", "## Steps\ntask t\n" + "\n".join(
        block(i).replace('"error": null', f'"error": "fail {i}"') for i in range(400)) + "\n"), limit=5_000)
    assert len(long) <= 5_000 and "more in the full report" in long and "Status: failed" in long


def test_a_redactor_without_a_text_path_sends_nothing(daemon: RunningDaemon, relay: Relay) -> None:
    _failed_task(daemon)
    built = _build(daemon, "t-fail")
    held = daemon.daemon.redact
    daemon.daemon.redact = lambda obj: obj                                   # type: ignore[assignment]
    try:
        with pytest.raises(RpcError, match="couldn't redact"):
            _send(daemon, built["path"])
    finally:
        daemon.daemon.redact = held
    assert relay.received == []


def test_a_redactor_without_a_text_path_writes_nothing(daemon: RunningDaemon) -> None:
    """Review 1, M4: never write unredacted."""
    _failed_task(daemon)
    held = daemon.daemon.redact
    daemon.daemon.redact = lambda obj: obj                                   # type: ignore[assignment]
    try:
        with pytest.raises(RpcError, match="couldn't redact"):
            _build(daemon, "t-fail")
    finally:
        daemon.daemon.redact = held
    assert not (daemon.daemon.home / "reports").exists()


# decision: DMN-503
def test_reports_older_than_30_days_are_removed(daemon: RunningDaemon) -> None:
    _failed_task(daemon)
    folder = daemon.daemon.home / "reports"
    folder.mkdir(mode=0o700)
    now = daemon.daemon.clock.now()
    ages = {"FERB-report-2025-11-30-old.txt": KEEP_DAYS + 1, "FERB-report-2025-12-04-recent.txt": KEEP_DAYS - 2,
            "notes.txt": KEEP_DAYS + 5}
    for name, days in ages.items():
        (folder / name).write_text("x")
        when = (now - timedelta(days=days)).timestamp()
        os.utime(folder / name, (when, when))
    r = _build(daemon, "t-fail")
    assert sorted(p.name for p in folder.iterdir()) == sorted(
        ["FERB-report-2025-12-04-recent.txt", "notes.txt", Path(r["path"]).name])


def test_the_file_is_owner_only(daemon: RunningDaemon) -> None:
    _failed_task(daemon)
    path = Path(_build(daemon, "t-fail")["path"])
    assert path.parent == daemon.daemon.home / "reports"
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert stat.S_IMODE(path.parent.stat().st_mode) == 0o700


def _loop_gaps(daemon: RunningDaemon, fn: Any) -> tuple[Any, float]:
    """`fn()`'s result and the longest the daemon's event loop went without running meanwhile (seconds)."""
    gaps: list[float] = []
    stop = threading.Event()

    async def tick() -> None:
        loop = asyncio.get_running_loop()
        last = loop.time()
        while not stop.is_set():
            await asyncio.sleep(0.01)
            now = loop.time()
            gaps.append(now - last - 0.01)
            last = now
    ticker = asyncio.run_coroutine_threadsafe(tick(), daemon.loop)
    time.sleep(0.2)
    try:
        result = fn()
    finally:
        stop.set()
        ticker.result(5)
    return result, max(gaps)


# decision: DMN-29, DMN-31
def test_a_30_mb_trace_leaves_the_event_loop_responsive(daemon: RunningDaemon, relay: Relay) -> None:
    """Review 1, M1: redaction ran as one pass over the whole multi-MB text, holding the GIL in long regex calls (a
    35.9 MB build stalled the loop up to 2.77 s). It now runs per span and per section, off the loop; Send's second
    pass, the shortening to 25 MB and the base64 run off the loop in pieces too."""
    d = daemon.daemon
    _failed_task(daemon)
    word = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor "
    body = json.dumps({"output": (word * 130)[:10_000]})

    def seed() -> None:
        root = d.db.conn.execute("SELECT id FROM spans WHERE kind = 'task'").fetchone()[0]   # type: ignore[union-attr]
        at = d.clock.now().isoformat()
        d.db.conn.executemany(                                               # type: ignore[union-attr]
            "INSERT INTO spans (id, parent_id, task_id, kind, started_at, ended_at, input_json, output_json) VALUES "
            "(?, ?, 't-fail', 'tool.call', ?, ?, ?, ?)",
            [(f"bulk-{i}", root, at, at, json.dumps({"tool": "browser.read", "args": {"tab": i}}), body)
             for i in range(3000)])
    _on_loop(daemon, seed)
    r, stalled = _loop_gaps(daemon, lambda: _build(daemon, "t-fail", timeout=120))
    assert Path(r["path"]).stat().st_size > 30_000_000
    assert stalled < 0.2, f"the loop stalled {stalled:.2f} s"
    sent, stalled = _loop_gaps(daemon, lambda: _send(daemon, r["path"], title=r["subject"]))
    assert sent["sent"] is True and sent["shortened"] is True
    assert stalled < 0.2, f"the loop stalled {stalled:.2f} s while sending"
    [(_, _, text)] = relay.files()
    assert 24_000_000 < len(text.encode()) <= 25_000_000 and "denied: ~/Downloads is protected" in text


def test_only_the_owners_clients_may_build_or_send_a_report() -> None:
    for method in ("reports.build", "reports.send"):
        assert auth.allowed(auth.Role.CLI, method)
        assert not auth.allowed(auth.Role.MCP_SHIM, method) and not auth.allowed(auth.Role.NATIVE_HOST, method)
