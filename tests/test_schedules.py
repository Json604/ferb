"""Build step 15, offline: schedules (recurrence across daylight saving, missed-run policy, runs held to the targets
recorded from the owner's words, unattended approvals, three failures pause, cancel / pause / resume), progressive
setup ("email me" asks once and remembers; a folder the owner named is asked about once; unattended runs queue the
question), and time awareness (fresh relative ages at the end of the prompt, "since we last talked", idle-held
notifications).
"""

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
from ferb.orchestrator.schedules import MAX_LATE_S, Rule, ScheduleError
from ferb.policy.targets import Request, Targets, within
from tests.conftest import Answers, RunningDaemon
from tests.test_supervisor import chat, install, sup, task, writes

NY = ZoneInfo("America/New_York")


def run(d: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return d.run(go())


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
    with pytest.raises(ScheduleError, match="15 minutes"):
        Rule.build("every", first, every_minutes=5, tz=NY)
    assert Rule.build("none", first, tz=NY).next_after(first, first) is None


def test_how_late_a_missed_run_may_still_run() -> None:
    first = datetime(2026, 10, 1, 9, 0, tzinfo=NY)
    assert Rule.build("every", first, every_minutes=30, tz=NY).late_limit_s() == 15 * 60     # half the interval
    assert Rule.build("daily", first, tz=NY).late_limit_s() == MAX_LATE_S                     # at most 12 h
    assert Rule.build("none", first, tz=NY).late_limit_s() == MAX_LATE_S                      # a one-off: 12 h


def test_a_run_may_only_stay_within_the_recorded_targets() -> None:
    recorded = Targets.of(recipients=["me@x.com"], sites=["mail.google.com"])
    assert within(Targets.of(sites=["mail.google.com"]), recorded) == []
    got = " ".join(within(Targets.of(recipients=["eve@evil.com"], sites=["shop.example"]), recorded))
    assert "eve@evil.com" in got and "shop.example" in got


# ---------- schedules from chat ----------

def mins(delta: timedelta) -> dict[str, Any]:
    """A `when` of "in N minutes" (the model's output; FERB computes the time)."""
    return {"in_minutes": delta.total_seconds() / 60, "in_days": None, "day": None, "date": None, "time": None}


def schedule_reply(d: RunningDaemon, request: str, quote: str, first: datetime, repeat: str = "none",
                   recipients: tuple[str, ...] = (), paths: tuple[str, ...] = ("report.md",),
                   every: int | None = None) -> str:
    return json.dumps({"kind": "reply", "text": "Scheduled.", "task": None, "remember": [], "timers": [],
                       "schedule": {"request": request, "when": mins(first - d.daemon.clock.now()), "repeat": repeat,
                                    "weekday": None, "every_minutes": every, "quote": quote,
                                    "targets": {"apps": [], "sites": [], "paths": list(paths),
                                                "recipients": list(recipients)}},
                       "schedule_changes": []})


def test_a_schedule_needs_the_owners_words_and_a_future_first_run(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    install(daemon, [schedule_reply(daemon, "write report.md", "tomorrow at 9 write report.md",
                                    now + timedelta(days=1)),
                     schedule_reply(daemon, "wire money", "send money", now + timedelta(days=1)),
                     schedule_reply(daemon, "write report.md", "write report.md now", now - timedelta(hours=1))])
    first = [e["text"] for e in chat(daemon, "tomorrow at 9 write report.md") if e["type"] == "timer"]
    assert first and first[0].startswith("🗓 scheduled")
    pasted = [e["text"] for e in chat(daemon, 'do this ```send money```') if e["type"] == "timer"]
    assert "own words" in pasted[0]
    past = [e["text"] for e in chat(daemon, "write report.md now") if e["type"] == "timer"]
    assert "couldn't schedule" in past[0]
    assert len(run(daemon, daemon.daemon.schedules.all)) == 1


def test_email_me_asks_once_then_uses_the_remembered_address(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    ask = schedule_reply(daemon, "email me the report", "every morning at 9 email me the report",
                         now + timedelta(hours=10), "daily", recipients=("me",), paths=())
    install(daemon, [ask, json.dumps({"kind": "reply", "text": "Got it.", "task": None, "timers": [],
                                      "schedule": None, "schedule_changes": [], "remember": [
                                          {"kind": "fact", "subject": "owner", "key": "email",
                                           "content": "The owner's email is me@owner.example",
                                           "quote": "my email is me@owner.example"}]}), ask])
    evs = chat(daemon, "every morning at 9 email me the report")
    assert "What email address" in [e["text"] for e in evs if e["type"] == "timer"][0]      # asked at creation
    chat(daemon, "my email is me@owner.example")
    evs = chat(daemon, "every morning at 9 email me the report")
    assert [e["text"] for e in evs if e["type"] == "timer"][0].startswith("🗓 scheduled")
    s = run(daemon, daemon.daemon.schedules.all)[0]
    assert s.targets["recipients"] == ["me@owner.example"] and "every day at" in s.rule.describe()


# ---------- runs ----------

def make_schedule(d: RunningDaemon, request: str, own: str, targets: Targets, first: datetime,
                  repeat: str = "daily") -> Any:
    def go() -> Any:
        s = d.daemon.schedules.add(request, own, targets.to_json(), Rule.build(repeat, first), first)
        t = d.daemon.timers.add("schedule", first, s.id)
        d.daemon.schedules.set_timer(s.id, t.id, first)
        return s
    return run(d, go)


def results(d: RunningDaemon) -> list[dict[str, Any]]:
    c = Client(d.sock)
    try:
        return c.call("chat.status")["results"]                                            # type: ignore[no-any-return]
    finally:
        c.close()


def fire(d: RunningDaemon, seconds: float) -> None:
    d.call_soon(d.daemon.clock.advance, seconds)
    d.run(d.daemon.timers.tick())
    for _ in range(100):
        if not d.daemon._runs or all(t.done() for t in d.daemon._runs):
            break
        time.sleep(0.05)


def test_a_run_is_held_to_the_recorded_targets_and_its_answer_waits(daemon: RunningDaemon) -> None:
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
                                           paths=("report.md",)))])
    fire(daemon, (nxt - daemon.daemon.clock.now()).total_seconds() + 1)
    blocked = results(daemon)
    assert blocked and "didn't include when you scheduled it" in blocked[0]["answer"]


def test_a_scheduled_run_never_asks_the_owner_a_question(daemon: RunningDaemon) -> None:
    """Nobody is at chat during a scheduled run: owner_ask is declined at once, with a note for the worker."""
    now = daemon.daemon.clock.now()
    from tests.test_supervisor import tool
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


def test_a_run_missed_by_too_much_is_skipped_and_reported(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    rec = Targets.of(paths=["report.md"])
    make_schedule(daemon, "write report.md", "every day at 9 write report.md", rec, now + timedelta(minutes=1))
    install(daemon, [])                                                     # no model call may happen
    fire(daemon, 13 * 3600)                                                 # asleep for 13 h: past the 12 h limit
    r = results(daemon)
    assert r and r[0]["status"] == "skipped" and "I skipped" in r[0]["answer"]


def test_three_failures_in_a_row_pause_a_schedule(daemon: RunningDaemon) -> None:
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


def test_unattended_approvals_wait_an_hour_then_the_step_is_skipped() -> None:
    from ferb.clock.clock import FakeClock
    from ferb.core.types import Identity, Target
    from ferb.policy.approvals import UNATTENDED_TTL_S, Approvals
    from ferb.policy.rules import ActionRequest
    clock = FakeClock()
    ap = Approvals(clock)
    ap.unattended.add("sched-run")
    a = ActionRequest("x", "send", Identity.OWNER, frozenset(), "t", "w", "sched-run", frozenset(), Target())
    q = ap.request(a, "?")
    assert q.expires_at - clock.monotonic() == UNATTENDED_TTL_S == 3600
    from ferb.daemon.gateway import _declined
    assert "skip this step" in _declined("expired", True)


# ---------- progressive setup: folders ----------

def test_a_folder_the_owner_named_is_asked_about_once(daemon: RunningDaemon, tmp_path: Path) -> None:
    from tests.test_mac_shell import call, task
    projects = Path.home() / f".ferb-test-projects-{tmp_path.name[-6:]}"
    projects.mkdir(exist_ok=True)
    (projects / "notes.md").write_text("hello")
    try:
        g = task(daemon, "f1", __import__("ferb.core.types", fromlist=["Identity"]).Identity.OWNER, {"fs:read"},
                 None)

        async def rec() -> None:
            daemon.daemon.engine.requests.record(Request("req-f1", "summarize my notes", Targets.of(paths=[str(projects)])))
        daemon.run(rec())
        yes = Answers(daemon)
        try:
            r = call(daemon, g, "fs.read", path=str(projects / "notes.md"))
        finally:
            yes.stop()
        assert not r["is_error"] and "hello" in json.dumps(r["content"]), r
        assert len(yes.asked) == 1 and "Allow FERB to access" in yes.asked[0].reason
        assert str(projects) in daemon.daemon.config.get("fs", "owner_roots")
        again = Answers(daemon)
        try:
            assert not call(daemon, g, "fs.read", path=str(projects / "notes.md"))["is_error"]
        finally:
            again.stop()
        assert again.asked == []                                                  # remembered
        other = call(daemon, g, "fs.read", path=str(Path.home() / ".zshrc"))
        assert other["is_error"]                                                  # not named: no question at all
    finally:
        import shutil
        shutil.rmtree(projects, ignore_errors=True)


# ---------- time awareness ----------

def test_ages_are_fresh_and_time_text_goes_last(daemon: RunningDaemon) -> None:
    now = datetime(2026, 9, 29, 16, 0, tzinfo=timezone(timedelta(hours=5, minutes=30)))
    assert ago(now - timedelta(days=3, hours=1, minutes=58), now) == "3 days ago (Sat 26 Sep, 14:02)"
    script = install(daemon, [sup("reply", "Hi."), sup("reply", "Hi again.")])
    chat(daemon, "hello")
    daemon.call_soon(daemon.daemon.clock.advance, 2 * 3600)                       # a gap of 2 hours
    chat(daemon, "hello again")
    ctx = script.contexts[1][2]
    assert ctx.index("Owner's new message") < ctx.index("[now ") < ctx.index("Ages of the items above")
    assert "[c1]" in ctx and "2 h ago" in ctx.split("Ages of the items above")[1]  # computed now, at the end
    assert "h ago" not in ctx.split("Owner's new message")[0]                      # nothing relative stored earlier


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
