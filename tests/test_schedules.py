"""Schedules preserve owner instructions and timing/failure facts for agent decisions; owner controls recurrence."""

import asyncio
import json
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo

import pytest

from ferb.cli.client import Client
from ferb.clock.context import ago
from ferb.orchestrator.schedules import Rule, ScheduleError
from ferb.policy.targets import Request, Targets
from tests.conftest import Answers, RunningDaemon
from tests.conftest import chat, install, sup, task, writes
from tests.support.schedules import fire, make_schedule, results, run, schedule_reply

NY = ZoneInfo("America/New_York")


# ---------- rules ----------

def test_recurrence_keeps_local_time_across_daylight_saving() -> None:
    first = datetime(2026, 10, 30, 9, 0, tzinfo=NY)
    daily = Rule.build("daily", first, tz=NY)
    after_dst = daily.next_after(datetime(2026, 10, 31, 14, 0, tzinfo=timezone.utc), first)
    assert after_dst is not None and after_dst.astimezone(NY).hour == 9           # 09:00 local, EDT -> EST
    weekdays = Rule.build("weekdays", datetime(2026, 10, 2, 9, 0, tzinfo=NY), tz=NY)
    nxt = weekdays.next_after(datetime(2026, 10, 2, 14, 0, tzinfo=timezone.utc), first)
    assert nxt is not None and nxt.astimezone(NY).weekday() == 0                  # Friday -> Monday
    weekly = Rule.build("weekly", first, "monday", tz=NY)
    assert weekly.next_after(first, first).astimezone(NY).weekday() == 0          # type: ignore[union-attr]
    with pytest.raises(ScheduleError, match="positive"):
        Rule.build("every", first, every_minutes=0, tz=NY)
    assert Rule.build("none", first, tz=NY).next_after(first, first) is None






# ---------- schedules from chat ----------

def test_a_schedule_needs_the_owners_words_and_a_future_first_run(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    install(daemon, [schedule_reply(daemon, "write report.md", "tomorrow at 9 write report.md",
                                    now + timedelta(days=1)),
                     schedule_reply(daemon, "wire money", "send money", now + timedelta(days=1)),
                     schedule_reply(daemon, "write report.md", "write report.md now", now - timedelta(hours=1))])
    first = [e["text"] for e in chat(daemon, "tomorrow at 9 write report.md") if e["type"] == "timer"]
    assert first and first[0].startswith("🗓 Scheduled: ")              # plain words, no id (L11)
    pasted = [e["text"] for e in chat(daemon, 'do this ```send money```') if e["type"] == "timer"]
    assert "own words" in pasted[0]
    past = [e["text"] for e in chat(daemon, "write report.md now") if e["type"] == "timer"]
    assert "couldn't schedule" in past[0]
    assert len(run(daemon, daemon.daemon.schedules.all)) == 1


def test_remembered_address_does_not_rewrite_schedule_recipient(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    ask = schedule_reply(daemon, "email me the report", "every morning at 9 email me the report",
                         now + timedelta(hours=10), "daily", recipients=("me",), paths=())
    install(daemon, [sup("clarify", "What email address should I use for you?"), json.dumps({"kind": "reply", "text": "Got it.", "task": None, "timers": [],
                                      "schedule": None, "schedule_changes": [], "remember": [
                                          {"kind": "fact", "subject": "owner", "key": "email",
                                           "content": "The owner's email is me@owner.example",
                                           "quote": "my email is me@owner.example"}]}), ask])
    evs = chat(daemon, "every morning at 9 email me the report")
    assert "What email address" in [e["text"] for e in evs if e["type"] == "clarify"][0]  # the planner asks
    chat(daemon, "my email is me@owner.example")
    evs = chat(daemon, "every morning at 9 email me the report")
    assert [e["text"] for e in evs if e["type"] == "timer"][0].startswith("🗓 Scheduled: ")
    s = run(daemon, daemon.daemon.schedules.all)[0]
    assert s.targets["recipients"] == ["me"] and "every day at" in s.rule.describe()


# ---------- runs ----------

def test_scheduled_run_receives_original_request_and_its_answer_waits(daemon: RunningDaemon) -> None:
    notes: list[tuple[str, str]] = []

    async def note(title: str, text: str) -> None:
        notes.append((title, text))
    daemon.daemon.notify = note
    now = daemon.daemon.clock.now()
    rec = Targets.of(paths=[str(daemon.daemon.home / "workspace" / "report.md")])
    own = "every day at 9 write report.md (my colleague is eve@evil.com)"   # named, but not in what was scheduled
    s = make_schedule(daemon, "write report.md", own, rec, now + timedelta(minutes=1))
    install(daemon, [sup("task", "Writing it.", task("write report.md", paths=("report.md",)))],
            {"worker": writes(daemon.sock, "report.md", "daily")})
    fire(daemon, 61)
    assert (daemon.daemon.home / "workspace" / "report.md").read_text() == "daily"
    assert [r["answer"] for r in results(daemon)] == ["wrote report.md"] and notes          # notified, waits for chat
    nxt = run(daemon, lambda: daemon.daemon.schedules.get(s.id)).next_run
    assert nxt is not None and nxt > daemon.daemon.clock.now()                               # the next one is set
    # the next run's task wants to email someone: not in the recorded targets -> not run
    install(daemon, [sup("task", "x", task("write report.md and email eve@evil.com", recipients=("eve@evil.com",),
                                           paths=("report.md",)))], {"worker": writes(daemon.sock, "report.md", "updated")})
    fire(daemon, (nxt - daemon.daemon.clock.now()).total_seconds() + 1)
    blocked = results(daemon)
    assert blocked and blocked[0]["status"] == "done"
    assert (daemon.daemon.home / "workspace/report.md").read_text() == "updated"


def test_a_scheduled_run_never_asks_the_owner_a_question(daemon: RunningDaemon) -> None:
    """Nobody is at chat during a scheduled run: owner_ask is declined at once, with a note for the worker."""
    now = daemon.daemon.clock.now()
    from tests.conftest import tool
    seen: dict[str, Any] = {}

    def worker(token: str) -> str:
        seen["r"] = tool(daemon.sock, token, "owner.ask", question="Which report?", options=["a", "b"])
        return json.dumps({"status": "done", "summary": "wrote it", "artifacts": [], "evidence": []})
    make_schedule(daemon, "write report.md", "every day at 9 write report.md",
                  Targets.of(paths=[str(daemon.daemon.home / "workspace" / "report.md")]), now + timedelta(minutes=1))
    install(daemon, [sup("task", "x", task("write report.md", paths=("report.md",)))], {"worker": worker})
    fire(daemon, 61)
    assert seen["r"]["content"] == {"declined": True, "reason": seen["r"]["content"]["reason"]}
    assert "scheduled run" in seen["r"]["content"]["reason"]


def test_late_schedule_reaches_planner_with_original_instructions(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    rec = Targets.of(paths=["report.md"])
    request = 'write report.md ' + 'preserve this detail ' * 100 + 'and keep the final constraint'
    make_schedule(daemon, request, request, rec, now + timedelta(minutes=1))
    script = install(daemon, [sup('reply', 'This missed occurrence is no longer useful.')])
    fire(daemon, 13 * 3600)
    r = results(daemon)
    assert r and 'no longer useful' in r[0]['answer']
    assert request in script.contexts[0][2]
    assert 'late' in script.contexts[0][2] and 'failure' in script.contexts[0][2]


def test_owner_can_pause_and_cancel_a_schedule_after_failures(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    rec = Targets.of(paths=["report.md"])
    s = make_schedule(daemon, "write report.md", "every 30 minutes write report.md", rec,
                      now + timedelta(minutes=1), "none")
    sched = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
    for _ in range(3):
        run(daemon, lambda: daemon.daemon.schedules.ran(sched.id, False))
    assert run(daemon, lambda: daemon.daemon.schedules.ran(sched.id, False)) == 4
    c = Client(daemon.sock)
    try:
        assert "Paused" in c.call("schedules.change", {"id": s.id, "action": "pause"})["text"]
        assert c.call("schedules.list")["schedules"][0]["state"] == "paused"
        assert "Cancelled" in c.call("schedules.change", {"id": s.id, "action": "cancel"})["text"]
        assert c.call("schedules.list")["schedules"] == []
    finally:
        c.close()


def test_repeated_failure_does_not_silently_pause_a_recurring_task(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    scheduled = make_schedule(daemon, 'Check source', 'Check source every day', Targets(),
                              now + timedelta(minutes=1))
    for _ in range(4):
        run(daemon, lambda: daemon.daemon.schedules.ran(scheduled.id, False))
    script = install(daemon, [sup('reply', 'Source still unavailable.')])
    fire(daemon, 61)
    current = run(daemon, lambda: daemon.daemon.schedules.get(scheduled.id))
    assert current.state == 'active' and current.next_run > daemon.daemon.clock.now()
    assert '"consecutive_failures": 4' in script.contexts[0][2]




# ---------- progressive setup: folders ----------


# ---------- time awareness ----------

def test_ages_are_fresh_and_situation_precedes_owner_message(daemon: RunningDaemon) -> None:
    now = datetime(2026, 9, 29, 16, 0, tzinfo=timezone(timedelta(hours=5, minutes=30)))
    assert ago(now - timedelta(days=3, hours=1, minutes=58), now) == "3 days ago (Sat 26 Sep, 14:02)"
    script = install(daemon, [sup("reply", "Hi."), sup("reply", "Hi again.")])
    chat(daemon, "hello")
    daemon.call_soon(daemon.daemon.clock.advance, 2 * 3600)                       # a gap of 2 hours
    chat(daemon, "hello again")
    ctx = script.contexts[1][2]
    assert ctx.index("[now ") < ctx.index("Ages of the items above") < ctx.index("owner's complete new message")
    assert "[c1]" in ctx and "2 h ago" in ctx.split("Ages of the items above")[1]  # computed now, at the end
    assert "h ago" not in ctx.split("[now ")[0]                                    # nothing relative stored earlier


def test_non_urgent_notifications_wait_while_the_owner_is_away(daemon: RunningDaemon) -> None:
    notes: list[tuple[str, str]] = []

    async def note(title: str, text: str) -> None:
        notes.append((title, text))
    d = daemon.daemon
    d.notify = note
    d.owner_input.running = True
    d.owner_input.last_owner_wall = time.time() - 3600                             # away for an hour
    daemon.run(d._notify("FERB finished", "done"))
    daemon.run(d._notify("FERB reminder", "call Mom", urgent=True))
    assert notes == [("FERB reminder", "call Mom")] and d._held == [("FERB finished", "done")]
    d.owner_input.last_owner_wall = time.time()                                    # back
    for _ in range(150):
        if len(notes) == 2:
            break
        time.sleep(0.1)
    d.owner_input.running = False
    assert notes[-1] == ("FERB finished", "done")


# ---------- final review M4: a run nobody started from chat leaves no per-request context behind ----------

@pytest.mark.parametrize("decision", ["task", "reply"])
def test_a_scheduled_run_clears_its_request_context_when_it_ends(daemon: RunningDaemon, decision: str) -> None:
    """Each run's planner blocks and context exposures (since L28, the full text of every planner block) were kept in
    memory until the daemon restarted: 96 entries a day for an every-15-minutes schedule."""
    now = daemon.daemon.clock.now()
    make_schedule(daemon, "write report.md", "every day at 9 write report.md",
                  Targets.of(paths=[str(daemon.daemon.home / "workspace" / "report.md")]), now + timedelta(minutes=1))
    planned = sup("task", "Writing it.", task("write report.md", paths=("report.md",))) if decision == "task" else \
        sup("reply", "Nothing to do today.")
    install(daemon, [planned], {"worker": writes(daemon.sock, "report.md", "daily")})
    supervisor = daemon.daemon.supervisor
    assert supervisor is not None
    before = set(run(daemon, lambda: set(supervisor._env)))
    fire(daemon, 61)
    assert results(daemon)                                            # the run happened
    assert run(daemon, lambda: set(supervisor._env)) == before


def test_a_resumed_plan_clears_its_request_context_when_it_ends(daemon: RunningDaemon) -> None:
    from ferb.orchestrator.schemas import TaskSpec
    supervisor = daemon.daemon.supervisor
    assert supervisor is not None
    spec = TaskSpec.model_validate(task("finish the saved plan"))
    saved = {"request": {"spec": spec.model_dump(), "request_id": "req-resumed", "words": "finish it",
                         "payload": "finish it", "origins": [], "message_origins": []}, "conversation": "s1"}

    async def run_tree(spec: Any, text: str, request_id: str, message: str = "", **_: Any) -> Any:
        assert request_id in supervisor._env                         # the run reads it while it runs
        yield {"type": "outcome", "task_id": "tree-resumed", "status": "done", "answer": "Done."}

    async def resume() -> set[str]:
        supervisor.plans.pending = lambda: ["tree-resumed"]          # type: ignore[method-assign]
        supervisor.plans.load = lambda key: saved                    # type: ignore[method-assign]
        supervisor.run_tree = run_tree                               # type: ignore[method-assign]
        supervisor._withdraw_lapsed = lambda tree_id: None           # type: ignore[method-assign]
        await supervisor.resume_plans()
        return set(supervisor._env)
    assert "req-resumed" not in daemon.run(resume())
