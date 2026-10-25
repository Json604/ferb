"""Send to developer (L35, the simple cut the owner approved 2026-10-25): `reports.build` writes one readable text file
for a failed or interrupted task, or a turn a planning failure ended, and returns the address and subject the notch
puts in a new Mail email. FERB sends nothing; the owner presses Send. The file is redacted whole again before it is
written (rule 14), lives in an owner-only folder, and reports older than 30 days go when the next one is built."""

import json
import os
import stat
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from ferb.daemon import auth
from ferb.daemon.reports import FEEDBACK_ADDRESS, KEEP_DAYS, SECTIONS
from tests.conftest import RunningDaemon, chat, install, sup

SEALED = "Kestrovan-Plume-4471"                       # a sealed value FERB learned after the record was written
API_KEY = "sk-ant-api03-" + "Zq7" * 12                 # an Anthropic key shape (a secret class)
REQUEST = "Rename the invoices in Downloads by month"
ANSWER = "I couldn't finish that: the Downloads folder was protected."


def _on_loop(daemon: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


def _failed_task(daemon: RunningDaemon, task_id: str = "t-fail", status: str = "failed") -> None:
    """A root task as the supervisor leaves it: its row with the final outcome, the owner's chat message, a tool call
    with an error in its trace. The secrets are planted raw (as a value learned after capture would be)."""
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
                                       "args": {"path": "~/Downloads", "note": SEALED + " " + API_KEY}}), call.id))
        outcome = {"status": status, "summary": ANSWER, "answer": ANSWER}
        d.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES (?, ?, NULL, ?, 'rename invoices', 'ferb', 'worker', '[]', "
            "?, ?, ?, ?)", (task_id, task_id, "r-" + task_id, status, json.dumps(outcome), start.isoformat(),
                            d.clock.now().isoformat()))
        d.redact.remember_hidden_secret(SEALED)                   # type: ignore[attr-defined]
    _on_loop(daemon, seed)


def _error_log(daemon: RunningDaemon, lines: list[str]) -> None:
    logs = daemon.daemon.home / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    (logs / "daemon.err.log").write_text("".join(line + "\n" for line in lines))


def _build(daemon: RunningDaemon, task_id: str, **params: Any) -> dict[str, Any]:
    c = Client(daemon.sock)
    try:
        return c.call("reports.build", {"task_id": task_id, **params})
    finally:
        c.close()


def test_the_file_has_the_four_sections_in_order(daemon: RunningDaemon) -> None:
    _failed_task(daemon)
    _error_log(daemon, [f"old line {i}" for i in range(300)] + ["Traceback: the last error"])
    r = _build(daemon, "t-fail", app_version="0.1.0 (7)")
    text = Path(r["path"]).read_text()
    heads = [text.index(f"\n## {name}\n") for name in SECTIONS]
    assert SECTIONS == ("Summary", "Request", "Steps", "Recent FERB errors") and heads == sorted(heads)
    summary, request, steps, errors = (text[a:b] for a, b in zip(heads, heads[1:] + [len(text)]))
    import ferb
    for fact in (f"FERB version: {ferb.__version__}", "macOS version: ", "Notch version: 0.1.0 (7)",
                 "AI runtime and model: ", "Status: failed", f"Failure reason: {ANSWER}", "Duration: 1m05s"):
        assert fact in summary, fact
    assert REQUEST in request and ANSWER in request
    # the renderer behind `ferb trace TASK --full`: every span field, the tool's arguments and its error
    assert "tool.call  fs.list" in steps and "denied: ~/Downloads is protected" in steps
    assert "find the invoices" in steps
    # the error log's last 200 lines
    assert "Traceback: the last error" in errors and "old line 101" in errors and "old line 100\n" not in errors
    assert r["subject"] == f"FERB report: {ANSWER}"
    assert ANSWER in r["summary"] and r["address"] == FEEDBACK_ADDRESS
    assert Path(r["path"]).name == "FERB-report-2026-01-01-t-fail.txt"


def test_a_planted_sealed_value_and_api_key_never_reach_the_file(daemon: RunningDaemon) -> None:
    _failed_task(daemon)
    _error_log(daemon, [f"ValueError: bad key {API_KEY}", f"login with {SEALED} refused"])
    stored = _on_loop(daemon, lambda: daemon.daemon.db.conn.execute(          # type: ignore[union-attr]
        "SELECT input_json FROM spans WHERE kind = 'tool.call'").fetchone()[0])
    assert SEALED in stored and API_KEY in stored                 # the trace really holds them
    r = _build(daemon, "t-fail")
    text = Path(r["path"]).read_text()
    assert SEALED not in text and API_KEY not in text and "sk-ant" not in text
    assert "[redacted]" in text and "login with [redacted] refused" in text
    assert all(SEALED not in v and API_KEY not in v for v in r.values())


@pytest.mark.parametrize("status", ["done", "cancelled", "done_unverified", "needs_user", "running"])
def test_a_task_that_did_not_fail_is_refused(daemon: RunningDaemon, status: str) -> None:
    _failed_task(daemon, status=status)
    with pytest.raises(RpcError, match="Only a failed or interrupted task"):
        _build(daemon, "t-fail")
    assert not (daemon.daemon.home / "reports").exists()


def test_an_interrupted_task_is_reportable(daemon: RunningDaemon) -> None:
    _failed_task(daemon, status="interrupted")
    assert "Status: interrupted" in Path(_build(daemon, "t-fail")["path"]).read_text()


def test_an_unknown_task_is_refused(daemon: RunningDaemon) -> None:
    with pytest.raises(RpcError, match="no failed task or request"):
        _build(daemon, "nope-nope-nope")
    with pytest.raises(RpcError, match="no failed task or request"):
        _build(daemon, "../../etc")


def test_an_empty_address_is_refused(daemon: RunningDaemon) -> None:
    _failed_task(daemon)
    daemon.daemon.config.set("feedback", "address", "")
    with pytest.raises(RpcError, match="No feedback address set"):
        _build(daemon, "t-fail")
    assert not (daemon.daemon.home / "reports").exists()


def test_the_address_comes_from_config(daemon: RunningDaemon) -> None:
    _failed_task(daemon)
    assert _build(daemon, "t-fail")["address"] == "ktky.attri@gmail.com"     # the owner's default, 2026-10-25
    daemon.daemon.config.set("feedback", "address", "reports@example.org")
    assert _build(daemon, "t-fail")["address"] == "reports@example.org"


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


def test_a_turn_a_planning_failure_ended_is_reportable_and_its_reply_says_failed(daemon: RunningDaemon) -> None:
    """The planner's answer stayed invalid after its one repair: no task exists, the turn's reply says it failed (so
    the notch offers the button), and the report holds the request, FERB's reply and the planner's turns."""
    stop = json.loads(sup("reply", "Stopping it."))
    stop["control"] = {"task_id": "no-such-task", "action": "stop", "standing_sends": "unchanged", "supersedes": []}
    install(daemon, [json.dumps(stop), json.dumps(stop)])
    replies = [e for e in chat(daemon, "stop the invoice renaming") if e["type"] == "reply"]
    assert len(replies) == 1 and replies[0]["status"] == "failed"
    request_id = _on_loop(daemon, lambda: daemon.daemon.journal.of_kind("chat.failed")[-1].data["request_id"])
    r = _build(daemon, request_id)
    text = Path(r["path"]).read_text()
    assert "Status: planning failed" in text and "stop the invoice renaming" in text
    assert replies[0]["text"] in text and r["subject"].startswith("FERB report: I couldn't start that")
    steps = text[text.index("\n## Steps\n"):text.index("\n## Recent FERB errors\n")]
    assert "model.turn" in steps


def test_a_turn_that_failed_after_its_task_started_says_failed(daemon: RunningDaemon) -> None:
    async def send(session_id: str, text: str, **kwargs: Any) -> Any:
        yield {"type": "plan", "text": "On it.", "task_id": "t-started", "goal": "x", "identity": "ferb",
               "subtasks": []}
        raise KeyError("worker_type")
    daemon.daemon.supervisor.send = send                                # type: ignore[union-attr]
    assert [e.get("status") for e in chat(daemon, "do it") if e["type"] == "reply"] == ["failed"]
    # no reply was journaled on this path: the reason is FERB's own record of the failure
    request_id = _on_loop(daemon, lambda: daemon.daemon.journal.of_kind("chat.failed")[-1].data["request_id"])
    text = Path(_build(daemon, request_id)["path"]).read_text()
    assert "Status: planning failed" in text and "Failure reason: KeyError: 'worker_type'" in text


def test_a_scheduled_run_that_failed_before_its_task_is_reportable(daemon: RunningDaemon) -> None:
    """A scheduled run has no chat message: its request is the schedule's, as `task_results` keeps it."""
    def seed() -> None:
        d = daemon.daemon
        d.journal.append("chat.failed", {"request_id": "r-sched", "why": "ServiceError", "detail": "usage limit"})
        d.db.conn.execute("INSERT INTO task_results (task_id, at, status, goal, answer, shown) VALUES (?, ?, 'failed', "
                          "?, ?, 0)", ("r-sched", d.clock.now().isoformat(), "Check the visa slots",
                                       "You've hit your usage limit."))
    _on_loop(daemon, seed)
    text = Path(_build(daemon, "r-sched")["path"]).read_text()
    assert "Check the visa slots" in text and "Failure reason: You've hit your usage limit." in text


def test_the_notch_waits_longer_for_a_report_so_a_slow_one_is_no_slow_call(daemon: RunningDaemon,
                                                                           monkeypatch: pytest.MonkeyPatch) -> None:
    """A long task's whole record is rendered and redacted before the file is written (a 12.6 MB trace took 5 s;
    the owner's largest failed task holds 41.6 MB), so the notch waits up to LONG_WAITS for it (NotchModel.reportWait),
    and `ferb stats` doesn't count it as answered after the app stopped waiting."""
    import asyncio
    import ferb.daemon.server as server
    from ferb.daemon.server import LONG_WAITS, SLOW_CALL
    assert LONG_WAITS["reports.build"] == 120.0
    monkeypatch.setattr(server, "CLIENT_WAIT_S", 0.05)

    async def slow(session: Any, params: dict[str, Any]) -> dict[str, Any]:
        await asyncio.sleep(0.1)
        return {"ok": True}
    daemon.daemon.server.unary["reports.build"] = slow
    assert _build(daemon, "t-fail") == {"ok": True}
    assert not _on_loop(daemon, lambda: daemon.daemon.journal.of_kind(SLOW_CALL))


def test_only_the_owners_clients_may_build_a_report() -> None:
    assert auth.allowed(auth.Role.CLI, "reports.build")
    assert not auth.allowed(auth.Role.MCP_SHIM, "reports.build")
    assert not auth.allowed(auth.Role.VOICE, "reports.build")
