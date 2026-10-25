"""L34 Phase E: the FERB window's Tasks, Noticed and Schedules pages read the daemon: root tasks newest first with
their answers and the owner's verdicts, every notice FERB posted (reading acknowledges nothing), and each schedule's
latest finished run."""

import json
from datetime import timedelta
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from ferb.daemon import auth
from tests.conftest import RunningDaemon


def _call(daemon: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    c = Client(daemon.sock)
    try:
        return c.call(method, params or {})
    finally:
        c.close()


def _on_loop(daemon: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


def _root(daemon: RunningDaemon, task_id: str, minutes: int, status: str = "done", answer: str | None = None,
          outcome: dict[str, Any] | None = None, request_id: str = "") -> None:
    d = daemon.daemon
    at = (d.clock.now() + timedelta(minutes=minutes)).isoformat()

    def seed() -> None:
        assert d.db is not None
        d.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES (?, ?, NULL, ?, ?, 'ferb', 'worker', '[]', ?, ?, ?, ?)",
            (task_id, task_id, request_id or "r-" + task_id, f"goal of {task_id}", status,
             json.dumps(outcome or {}), at, at))
        if answer is not None:
            d.db.conn.execute("INSERT INTO task_results (task_id, at, status, goal, answer, shown) "
                              "VALUES (?, ?, ?, ?, ?, 1)", (task_id, at, status, f"goal of {task_id}", answer))
    _on_loop(daemon, seed)


def _verdict(daemon: RunningDaemon, task_id: str, verdict: str) -> None:
    """A verdict row as `verdict.set` writes it (its learning side effects aren't this test's)."""
    d = daemon.daemon
    _on_loop(daemon, lambda: d.db.conn.execute(                                  # type: ignore[union-attr]
        "INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES (?, ?, ?, '', ?)",
        (f"v-{task_id}-{verdict}", task_id, verdict, d.clock.now().isoformat())))


def test_tasks_come_newest_first_with_their_answers_and_verdicts(daemon: RunningDaemon) -> None:
    _root(daemon, "t-old", 1, answer="Renamed 4 files.")
    _root(daemon, "t-new", 2, status="failed", outcome={"summary": "The folder was protected."})
    _verdict(daemon, "t-old", "fail")
    _verdict(daemon, "t-old", "pass")                                            # the latest verdict counts
    rows = _call(daemon, "tasks.list")["tasks"]
    mine = [r for r in rows if r["task_id"] in ("t-old", "t-new")]
    assert [r["task_id"] for r in mine] == ["t-new", "t-old"]
    assert mine[0]["answer"] == "The folder was protected." and mine[0]["verdict"] is None
    assert mine[1]["answer"] == "Renamed 4 files." and mine[1]["verdict"] == "pass"
    assert mine[1]["goal"] == "goal of t-old" and mine[1]["status"] == "done"


def test_tasks_page_by_created_at(daemon: RunningDaemon) -> None:
    for n in range(5):
        _root(daemon, f"t-{n}", 10 + n)
    first = _call(daemon, "tasks.list", {"limit": 2})
    assert [r["task_id"] for r in first["tasks"]] == ["t-4", "t-3"] and first["more"] is True
    second = _call(daemon, "tasks.list", {"limit": 2, "before": first["tasks"][-1]["created_at"]})
    assert [r["task_id"] for r in second["tasks"]] == ["t-2", "t-1"]


@pytest.mark.parametrize("limit", [0, 201, "50", True])
def test_a_page_size_outside_1_to_200_is_refused(daemon: RunningDaemon, limit: Any) -> None:
    with pytest.raises(RpcError, match="limit must be a whole number from 1 to 200"):
        _call(daemon, "tasks.list", {"limit": limit})


def test_a_sealed_value_in_an_answer_never_reaches_the_window(daemon: RunningDaemon) -> None:
    sealed = "Vantorel-Quill-8812"
    _root(daemon, "t-sealed", 3, answer=f"done with {sealed}")
    _on_loop(daemon, lambda: daemon.daemon.redact.remember_hidden_secret(sealed))   # type: ignore[attr-defined]
    row = next(r for r in _call(daemon, "tasks.list")["tasks"] if r["task_id"] == "t-sealed")
    assert sealed not in json.dumps(row)


def test_noticed_lists_every_notice_and_reading_acknowledges_nothing(daemon: RunningDaemon) -> None:
    notices = daemon.daemon.attention_notices
    assert notices is not None
    _on_loop(daemon, lambda: notices.add("n-1", [], "Your build finished.", [{"text": "BUILD OK"}]))
    _on_loop(daemon, lambda: daemon.daemon.clock.advance(5))                    # type: ignore[attr-defined]
    _on_loop(daemon, lambda: notices.add("n-2", [], "Mail from the bank arrived."))
    _on_loop(daemon, lambda: notices.acknowledge(["n-1"]))
    rows = _call(daemon, "notices.list")["notices"]
    assert [(r["id"], r["shown"]) for r in rows] == [("n-2", False), ("n-1", True)]   # newest first
    assert rows[1]["quotes"][0]["text"] == "BUILD OK"
    assert [n["id"] for n in _call(daemon, "chat.status", {"peek": True})["notices"]] == ["n-2"]   # still unseen


def test_a_schedule_shows_its_latest_finished_run(daemon: RunningDaemon) -> None:
    from ferb.orchestrator.schedules import Rule
    d = daemon.daemon
    first = d.clock.now() + timedelta(days=1)
    sid = _on_loop(daemon, lambda: d.schedules.add("Post the weekly update", "every day post the weekly update", {},
                                                   Rule.build("daily", first), first).id)
    assert _call(daemon, "schedules.list")["schedules"][0]["last_result"] is None
    _on_loop(daemon, lambda: d.journal.append("schedule.run", {"schedule_id": sid, "request_id": "req-run-1",   # type: ignore[union-attr]
                                                               "late": False}))
    _root(daemon, "t-run-1", 5, answer="Posted it.", request_id="req-run-1")
    row = _call(daemon, "schedules.list")["schedules"][0]
    assert row["last_result"]["status"] == "done" and row["last_result"]["answer"] == "Posted it."


def test_only_the_owners_connection_may_read_the_lists() -> None:
    for method in ("tasks.list", "notices.list"):
        assert auth.allowed(auth.Role.CLI, method)
        for role in (auth.Role.MCP_SHIM, auth.Role.VOICE, auth.Role.NATIVE_HOST):
            assert not auth.allowed(role, method), (role, method)
