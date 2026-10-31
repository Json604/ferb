"""Orchestration decision 17 ("when you're back at your Mac"): a reminder or schedule the planner marks when_active fires
when the owner is at the Mac (the owner-input tap's idle/active signal), not while they're away, and never polls; a
scheduled run that found nothing to tell posts nothing. The real daemon with the scripted model; the owner's presence
is set on the daemon's owner-input object (no event tap runs in tests)."""
import json
from datetime import timedelta
from typing import Any

from ferb.policy.targets import Targets
from tests.conftest import RunningDaemon, chat, install, report, sup, task
from tests.support.schedules import fire, make_schedule, results, run

NO_TIME = {"in_minutes": None, "in_days": None, "day": None, "date": None, "time": None}


def _away(d: RunningDaemon) -> None:
    """The tap runs and the owner's last input was an hour ago."""
    oi = d.daemon.owner_input
    oi.running = True
    oi.last_owner_wall = d.daemon.clock.now().timestamp() - 3600


def _input(d: RunningDaemon) -> None:
    """One input of the owner's now (a return when they were away), on the daemon's loop."""
    run(d, d.daemon.owner_input.owner_event)


def _schedule(words: str, repeat: str = "daily") -> str:
    return json.dumps({"kind": "reply", "text": "I'll remind you.", "task": None, "timers": [], "schedule_changes": [],
                       "schedule": {"request": "remind me that I want to apply to YC", "when": NO_TIME,
                                    "repeat": repeat, "weekday": None, "every_minutes": None, "quote": words,
                                    "targets": {"apps": [], "sites": [], "paths": [], "recipients": []},
                                    "watch": "none", "until": None, "when_active": True}})


def _runs(d: RunningDaemon) -> int:
    return len(run(d, lambda: d.daemon.journal.of_kind("schedule.run")))


def test_a_when_active_schedule_fires_when_the_owner_is_back_and_not_while_they_are_away(daemon: RunningDaemon) -> None:
    words = "remind me every day whenever I'm on my mac that I want to apply to YC"
    install(daemon, [_schedule(words), sup("reply", "Reminder: you want to apply to YC."),
                     sup("reply", "Reminder: you want to apply to YC.")])
    try:
        _away(daemon)
        chat(daemon, words)
        s = run(daemon, daemon.daemon.schedules.all)[0]
        assert s.rule.when_active and s.rule.repeat == "daily" and s.next_run > daemon.daemon.clock.now()
        assert run(daemon, lambda: daemon.daemon.timers.when_active(s.timer_id))
        due_in = (s.next_run - daemon.daemon.clock.now()).total_seconds()
        fire(daemon, due_in + 3600)                         # due an hour ago, the owner still away: held
        assert _runs(daemon) == 0 and results(daemon) == []
        _input(daemon)                                       # back at the Mac
        fire(daemon, 1)
        assert _runs(daemon) == 1
        assert [r["answer"] for r in results(daemon)] == ["Reminder: you want to apply to YC."]
        late = run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.run"))[-1].data["late"]
        assert late is False                                 # waiting for the owner isn't lateness
        nxt = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
        assert nxt.next_run > daemon.daemon.clock.now() and run(daemon, lambda: daemon.daemon.timers.when_active(
            nxt.timer_id))                                   # the next day's run waits for them too
        # the next day, the owner is at the Mac when it comes due: it fires then, no return needed
        daemon.call_soon(daemon.daemon.clock.advance, (nxt.next_run - daemon.daemon.clock.now()).total_seconds() + 5)
        daemon.daemon.owner_input.last_owner_wall = daemon.daemon.clock.now().timestamp() - 10
        fire(daemon, 0)
        assert _runs(daemon) == 2
    finally:
        daemon.daemon.owner_input.running = False


def test_a_reminder_for_when_the_owner_is_back_waits_for_their_next_return(daemon: RunningDaemon) -> None:
    words = "when I'm back at my mac remind me to call Mom"
    install(daemon, [json.dumps({"kind": "reply", "text": "Okay.", "task": None, "schedule": None,
                                 "schedule_changes": [], "timers": [{"when": NO_TIME, "message": "Call Mom",
                                                                     "quote": words, "when_active": True}]})])
    fired: list[dict[str, Any]] = []
    daemon.daemon.events.on({"timer.fired"}, lambda topic, data: fired.append(data))
    try:
        _away(daemon)
        _input(daemon)                                       # the owner is at the Mac, asking now
        chat(daemon, words)
        fire(daemon, 30)
        assert fired == []                                   # said while here: it waits for a return
        daemon.call_soon(daemon.daemon.clock.advance, 600)   # away ten minutes, then back
        _input(daemon)
        fire(daemon, 1)
        assert [f["message"] for f in fired] == ["Call Mom"]
    finally:
        daemon.daemon.owner_input.running = False


def test_without_the_tap_a_when_active_schedule_runs_at_its_time(daemon: RunningDaemon) -> None:
    """FERB can't tell where the owner is (no Input Monitoring, or sensing paused): unknown counts as present."""
    words = "every day whenever I'm on my mac remind me to apply to YC"
    install(daemon, [_schedule(words), sup("reply", "Reminder: you want to apply to YC.")])
    chat(daemon, words)
    s = run(daemon, daemon.daemon.schedules.all)[0]
    fire(daemon, (s.next_run - daemon.daemon.clock.now()).total_seconds() + 1)
    assert _runs(daemon) == 1


def test_a_scheduled_run_that_found_nothing_to_tell_posts_nothing(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    make_schedule(daemon, "tell me if the YC page changed", "every day tell me if the YC page changed", Targets.of(),
                  now + timedelta(minutes=1))
    install(daemon, [sup("task", "Checking.", task("Check the YC page")),
                     sup("task", "Checking.", task("Check the YC page"))],
                    {"worker": [json.dumps(report("done", "Nothing changed since yesterday.", nothing_to_tell=True)),
                                json.dumps(report("done", "The deadline moved to Nov 5.", nothing_to_tell=False))]})
    published: list[dict[str, Any]] = []
    daemon.daemon.events.on({"task.result"}, lambda topic, data: published.append(data))
    fire(daemon, 61)
    assert published == [] and results(daemon) == []         # recorded only
    recorded = run(daemon, lambda: daemon.daemon.db.conn.execute("SELECT answer FROM task_results").fetchall())
    assert [r[0] for r in recorded] == ["Nothing changed since yesterday."]
    fire(daemon, 24 * 3600)
    assert [r["answer"] for r in results(daemon)] == ["The deadline moved to Nov 5."] and len(published) == 1
