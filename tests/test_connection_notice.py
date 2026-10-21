"""The owner hears at once when FERB loses its AI service mid-run (fix wave 7; owner 2026-10-21: "it should be
instant"). An error the runtime retries itself (Codex `willRetry`) says so on the live line of the work the owner is
waiting on (a chat turn's planner, a task's worker, checker or next-attempt choice): once per outage, the offline
sentence when this Mac has no network route; the run's next real event says it reconnected. Background work (a
scheduled check, a learning call, an attention check) records the retry and tells no one."""
import json
from typing import Any

import pytest

from ferb.cli.chat import render
from ferb.cli.client import Client
from ferb.core.types import RunHandle, RuntimeEvent
from tests.conftest import FakeRuntime, RunningDaemon, done, install, outcome, sup, task, tool, turn_events

RETRY = "Codex is retrying after an error: Reconnecting... {}/5 (responseStreamDisconnected)"
LOST = ("Lost the connection to the AI service I use (fake); it's retrying and the task carries on. "
        "It said: Codex is retrying after an error: Reconnecting... 1/5 (responseStreamDisconnected).")
LOST_NO_TASK = ("Lost the connection to the AI service I use (fake); it's retrying, and I'll answer when it's back. "
                "It said: Codex is retrying after an error: Reconnecting... 1/5 (responseStreamDisconnected).")
OFFLINE = "Your Mac seems to be offline: I'll carry on when it's back."
BACK = "Reconnected to the AI service I use (fake)."


def _events(d: RunningDaemon, text: str, session: str = "s1") -> list[dict[str, Any]]:
    """Every event of one chat turn, the live lines included, after the stream contract's own check."""
    c = Client(d.sock, timeout=30)
    try:
        events = list(c.stream("chat.send", {"session_id": session, "text": text}))
    finally:
        c.close()
    turn_events(events)
    return events


def _notices(events: list[dict[str, Any]]) -> list[tuple[str, str]]:
    return [(e["connection"], e["text"]) for e in events if e["type"] == "progress" and e.get("connection")]


def _retrying_first(monkeypatch: pytest.MonkeyPatch, key: str, *steps: Any) -> None:
    """The run for `key` ("supervisor", "worker", "worker.verify") first yields `steps` (a str: a retry with that
    attempt number; a RuntimeEvent as it is), then runs its script as usual."""
    original = FakeRuntime.events

    async def events(self, run):
        if run.state["key"] == key:
            for step in steps:
                retry = RuntimeEvent("retrying", {"message": RETRY.format(step)})
                yield step if isinstance(step, RuntimeEvent) else retry
        async for event in original(self, run):
            yield event
    monkeypatch.setattr(FakeRuntime, "events", events)


def _retries(d: RunningDaemon) -> list[str]:
    async def rows() -> list[str]:
        return [r[0] for r in d.daemon.db.conn.execute("SELECT error FROM spans WHERE kind = 'model.retry'")]
    return d.run(rows())  # type: ignore[no-any-return]


# ---------- a task's worker ----------

def test_the_first_retry_tells_the_owner_at_once_and_the_next_real_event_says_it_reconnected(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    _retrying_first(monkeypatch, "worker", "1", "2", "3")
    install(daemon, [sup("task", "Tidying.", task("tidy the notes"))], {"worker": lambda _: done("Tidied.")})
    events = _events(daemon, "tidy the notes")
    assert _notices(events) == [("lost", LOST), ("back", BACK)]             # once per outage: not three times
    plan = next(e for e in events if e["type"] == "plan")
    lines = [e for e in events if e["type"] == "progress" and e.get("connection")]
    assert all(e["task_id"] == plan["task_id"] and e["started_at"] for e in lines)
    kinds = [e.get("connection") or e["type"] for e in events]
    assert kinds.index("plan") < kinds.index("lost") < kinds.index("back") < kinds.index("outcome")
    assert outcome(events)["status"] != "failed"                            # the run carries on to its answer
    assert len(_retries(daemon)) == 3                                       # each retry is still recorded


def test_a_mac_without_a_network_route_hears_it_is_offline(daemon: RunningDaemon,
                                                            monkeypatch: pytest.MonkeyPatch) -> None:
    import ferb.sense.network as network
    monkeypatch.setattr(network, "has_route", lambda: False)
    _retrying_first(monkeypatch, "worker", "1", "2")
    install(daemon, [sup("task", "Tidying.", task("tidy the notes"))], {"worker": lambda _: done("Tidied.")})
    assert _notices(_events(daemon, "tidy the notes")) == [("lost", OFFLINE), ("back", BACK)]


def test_a_second_outage_in_the_same_run_is_told_again(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    _retrying_first(monkeypatch, "worker", "1", RuntimeEvent("message", {"text": "reading the notes"}), "1", "2")
    install(daemon, [sup("task", "Tidying.", task("tidy the notes"))], {"worker": lambda _: done("Tidied.")})
    assert _notices(_events(daemon, "tidy the notes")) == [("lost", LOST), ("back", BACK), ("lost", LOST),
                                                          ("back", BACK)]


def test_a_run_the_service_then_fails_never_says_it_reconnected(daemon: RunningDaemon,
                                                                monkeypatch: pytest.MonkeyPatch) -> None:
    _retrying_first(monkeypatch, "worker", "1", RuntimeEvent("error", {"message": "Codex runtime reported an error"}))
    install(daemon, [sup("task", "Tidying.", task("tidy the notes"))], {"worker": lambda _: done("Tidied.")})
    events = _events(daemon, "tidy the notes")
    assert _notices(events) == [("lost", LOST)]
    assert "Codex runtime reported an error" in json.dumps(outcome(events))   # the failure is told as today


def test_a_tool_call_after_the_outage_says_reconnected_before_its_own_step_line(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    """The model asking for a tool is the connection back: the reconnect line comes before that call's live line, so it
    never replaces the step being shown."""
    _retrying_first(monkeypatch, "worker", "1")

    def worker(token: str) -> str:
        assert not tool(daemon.sock, token, "fs.write", path="notes.md", content="tidy", overwrite=True,
                        effect="edit", why="tidy")["is_error"]
        return done("Wrote notes.md.")
    install(daemon, [sup("task", "Tidying.", task("tidy the notes"))], {"worker": worker})
    lines = [(e.get("connection"), e["text"]) for e in _events(daemon, "tidy the notes") if e["type"] == "progress"]
    assert lines == [("lost", LOST), ("back", BACK), (None, "writing notes.md")]


def test_the_final_checker_tells_the_owner_too(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    _retrying_first(monkeypatch, "worker.verify", "1")
    install(daemon, [sup("task", "Tidying.", task("tidy the notes"))], {"worker": lambda _: done("Tidied.")})
    events = _events(daemon, "tidy the notes")
    assert _notices(events) == [("lost", LOST), ("back", BACK)] and outcome(events)["status"] != "failed"


# ---------- a chat turn's planner ----------

def test_the_planner_turn_tells_the_owner_before_its_reply(daemon: RunningDaemon,
                                                           monkeypatch: pytest.MonkeyPatch) -> None:
    """No task exists yet: the notice is the turn's live line, without a task id, and it promises an answer, not a
    task (review 1)."""
    _retrying_first(monkeypatch, "supervisor", "1", "2")
    install(daemon, [sup("reply", "Hi! What can I do?")])
    events = _events(daemon, "hi")
    assert _notices(events) == [("lost", LOST_NO_TASK), ("back", BACK)]
    assert not any(e.get("task_id") for e in events if e["type"] == "progress")
    kinds = [e.get("connection") or e["type"] for e in events]
    assert kinds.index("lost") < kinds.index("back") < kinds.index("reply")
    assert next(e for e in events if e["type"] == "reply")["text"] == "Hi! What can I do?"


def test_ferb_chat_shows_the_notices_and_still_hides_step_lines() -> None:
    lost = {"type": "progress", "text": LOST, "started_at": "2026-10-21T10:00:00+00:00", "connection": "lost"}
    assert render(lost) == "ferb> " + LOST and render(lost, verbose=True) == "ferb> " + LOST
    assert render({"type": "progress", "task_id": "t", "text": "writing notes.md",
                   "started_at": "2026-10-21T10:00:00+00:00"}) is None


# ---------- background work tells no one ----------

def test_a_scheduled_check_that_retries_tells_no_one(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    from datetime import timedelta
    from ferb.policy.targets import Targets
    from tests.support.bus import Subscriber
    from tests.support.schedules import fire, make_schedule, run
    _retrying_first(monkeypatch, "worker", "1", "2")
    make_schedule(daemon, "Check the battery level.", "check the battery", Targets.of(),
                  daemon.daemon.clock.now() + timedelta(minutes=15), "every", 15, "standing")
    client = Subscriber(daemon)
    try:
        install(daemon, [sup("task", "Checking.", task("check the battery level"))],
                {"worker": lambda _: json.dumps({"status": "done", "summary": "The battery is at 64%.",
                                                 "answer": "The battery is at 64%.", "artifacts": [], "evidence": [],
                                                 "watch_met": False})})
        fire(daemon, 15 * 60 + 1)
        recorded = run(daemon, lambda: daemon.daemon.db.conn.execute("SELECT answer FROM task_results").fetchall())
        assert [r["answer"] for r in recorded] == ["The battery is at 64%."]    # it ran to its answer
        assert len(_retries(daemon)) == 2                                       # the retries are recorded
        told = json.dumps(client.events)
        assert "Lost the connection" not in told and "Reconnected" not in told
    finally:
        client.close()


def test_a_learning_call_and_an_attention_check_that_retry_tell_no_one(daemon: RunningDaemon,
                                                                      monkeypatch: pytest.MonkeyPatch) -> None:
    """Neither has a live line: the distillation and the background attention check record the retry and answer."""
    from tests.conftest import FakeDistillRuntime, trail_row
    from tests.support.bus import Subscriber

    class Retrying(FakeDistillRuntime):
        async def events(self, run: RunHandle) -> Any:
            yield RuntimeEvent("retrying", {"message": RETRY.format(1)})
            async for event in super().events(run):
                yield event
    d = daemon.daemon
    client = Subscriber(daemon)
    try:
        trail_row(daemon, "2025-12-30")
        d.distiller.make_runtime = lambda *a, **k: Retrying({"narrative": "", "items": []})
        assert daemon.run(d.distiller.run_day("2025-12-30"), timeout=30)["status"] == "distilled"
        _retrying_first(monkeypatch, "supervisor", "1")
        install(daemon, [json.dumps({"tell": False, "text": "", "source_refs": [], "quotes": []})])
        assert daemon.run(d.supervisor.attention(["trail:x"], [{"data": {"title": "a page"}}], []))["tell"] is False
        assert len(_retries(daemon)) == 2
        told = json.dumps(client.events)
        assert "Lost the connection" not in told and "Reconnected" not in told
    finally:
        client.close()
