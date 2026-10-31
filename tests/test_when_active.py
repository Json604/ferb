"""Orchestration decision 17 ("when you're back at your Mac", clarified by the owner 2026-10-31): a reminder or schedule
the planner marks when_active fires while the Mac is in use and, held while it's out of use (asleep, displays asleep,
locked, switched away), when it comes back (wake, unlock, login, restart); never on input idleness, and never polls; a
scheduled run that found nothing to tell posts nothing. The real daemon with the scripted model; the Mac's presence is
told to the daemon's presence object (no macOS notifications in tests)."""
import json
from datetime import timedelta
from typing import Any

from ferb.policy.targets import Targets
from tests.conftest import FakeCap, RunningDaemon, chat, install, report, sup, task, tool
from tests.support.schedules import fire, mac_away, mac_back, make_schedule, results, run, watching

NO_TIME = {"in_minutes": None, "in_days": None, "day": None, "date": None, "time": None}


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
        mac_away(daemon)                                     # the Mac is locked
        chat(daemon, words)
        s = run(daemon, daemon.daemon.schedules.all)[0]
        assert s.rule.when_active and s.rule.repeat == "daily" and s.next_run > daemon.daemon.clock.now()
        assert run(daemon, lambda: daemon.daemon.timers.when_active(s.timer_id))
        due_in = (s.next_run - daemon.daemon.clock.now()).total_seconds()
        fire(daemon, due_in + 3600)                         # due an hour ago, the owner still away: held
        assert _runs(daemon) == 0 and results(daemon) == []
        mac_back(daemon)                                     # unlocked: back at the Mac
        fire(daemon, 1)
        assert _runs(daemon) == 1
        assert [r["answer"] for r in results(daemon)] == ["Reminder: you want to apply to YC."]
        late = run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.run"))[-1].data["late"]
        assert late is False                                 # waiting for the owner isn't lateness
        nxt = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
        assert nxt.next_run > daemon.daemon.clock.now() and run(daemon, lambda: daemon.daemon.timers.when_active(
            nxt.timer_id))                                   # the next day's run waits for them too
        # the next day, the Mac is in use when it comes due: it fires then, no return needed
        daemon.call_soon(daemon.daemon.clock.advance, (nxt.next_run - daemon.daemon.clock.now()).total_seconds() + 5)
        fire(daemon, 0)
        assert _runs(daemon) == 2
    finally:
        daemon.daemon.presence.stop()


def test_input_idleness_is_not_away_and_waking_the_mac_is_a_return(daemon: RunningDaemon) -> None:
    """The owner watching a film touches nothing for an hour: still at the Mac, so the run goes at its time. Asleep
    displays hold the next one until the screens wake."""
    words = "remind me every day whenever I'm on my mac that I want to apply to YC"
    install(daemon, [_schedule(words), sup("reply", "Reminder: you want to apply to YC."),
                     sup("reply", "Reminder: you want to apply to YC.")])
    oi = daemon.daemon.owner_input
    try:
        watching(daemon)
        oi.running, oi.last_owner_wall = True, daemon.daemon.clock.now().timestamp() - 3600   # no input for an hour
        chat(daemon, words)
        s = run(daemon, daemon.daemon.schedules.all)[0]
        fire(daemon, (s.next_run - daemon.daemon.clock.now()).total_seconds() + 1)
        assert _runs(daemon) == 1                            # input quiet time doesn't hold it
        nxt = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
        mac_away(daemon, "displays_asleep")
        fire(daemon, (nxt.next_run - daemon.daemon.clock.now()).total_seconds() + 3600)
        assert _runs(daemon) == 1                            # the screens are asleep: held
        mac_back(daemon, "displays_asleep")                  # the screens wake
        fire(daemon, 1)
        assert _runs(daemon) == 2
        backs = run(daemon, lambda: daemon.daemon.journal.of_kind("mac.back_in_use"))
        assert [b.data["signal"] for b in backs] == ["screens_wake"]
    finally:
        oi.running = False
        daemon.daemon.presence.stop()


def test_the_daemon_starting_in_a_new_login_session_is_a_return(daemon: RunningDaemon) -> None:
    """A restart or a login brings the daemon up in a new login session: what waited for the owner's return fires;
    the same session again (an update, a crash) is no return."""
    words = "when I'm back at my mac remind me to call Mom"
    install(daemon, [json.dumps({"kind": "reply", "text": "Okay.", "task": None, "schedule": None,
                                 "schedule_changes": [], "timers": [{"when": NO_TIME, "message": "Call Mom",
                                                                     "quote": words, "when_active": True}]})])
    fired: list[dict[str, Any]] = []
    daemon.daemon.events.on({"timer.fired"}, lambda topic, data: fired.append(data))
    try:
        p = watching(daemon)
        p.returned("unlock")                                 # the owner is at the Mac, asking now
        run(daemon, lambda: None)
        chat(daemon, words)
        daemon.call_soon(daemon.daemon.clock.advance, 600)
        run(daemon, lambda: daemon.daemon._mac_started(False))   # a daemon restart in the same session
        fire(daemon, 1)
        assert fired == []
        run(daemon, lambda: daemon.daemon._mac_started(True))    # after a restart of the Mac: a new login session
        run(daemon, lambda: None)
        fire(daemon, 1)
        assert [f["message"] for f in fired] == ["Call Mom"]
    finally:
        daemon.daemon.presence.stop()


def test_a_reminder_for_when_the_owner_is_back_waits_for_their_next_return(daemon: RunningDaemon) -> None:
    words = "when I'm back at my mac remind me to call Mom"
    install(daemon, [json.dumps({"kind": "reply", "text": "Okay.", "task": None, "schedule": None,
                                 "schedule_changes": [], "timers": [{"when": NO_TIME, "message": "Call Mom",
                                                                     "quote": words, "when_active": True}]})])
    fired: list[dict[str, Any]] = []
    daemon.daemon.events.on({"timer.fired"}, lambda topic, data: fired.append(data))
    try:
        mac_away(daemon)
        mac_back(daemon)                                     # the owner is at the Mac, asking now
        chat(daemon, words)
        fire(daemon, 30)
        assert fired == []                                   # said while here: it waits for a return
        daemon.call_soon(daemon.daemon.clock.advance, 600)   # locked ten minutes, then back
        mac_away(daemon)
        mac_back(daemon)
        fire(daemon, 1)
        assert [f["message"] for f in fired] == ["Call Mom"]
    finally:
        daemon.daemon.presence.stop()


def test_without_the_presence_signal_a_when_active_schedule_runs_at_its_time(daemon: RunningDaemon) -> None:
    """FERB can't tell whether the Mac is in use (sensing paused): unknown counts as present."""
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


def test_a_scheduled_run_that_acted_posts_whatever_its_worker_said(daemon: RunningDaemon) -> None:
    """Decision 17 ("a run that did nothing posts nothing") rests on facts, not only the worker's word: a run whose tree
    has an acting call that ran (here a form submitted) posts even when its worker reported nothing_to_tell."""
    from ferb.policy.rules import Kind
    submit = FakeCap("probe.submit", Kind.SUBMIT, {"probe"})
    daemon.daemon.registry.capabilities[submit.name] = submit
    now = daemon.daemon.clock.now()
    make_schedule(daemon, "renew the library loan", "every day renew the library loan", Targets.of(),
                  now + timedelta(minutes=1))

    def worker(token: str) -> str:
        tool(daemon.sock, token, "probe.submit", site="kartikey.xyz", text="renew")
        return json.dumps(report("done", "Renewed.", nothing_to_tell=True))
    install(daemon, [sup("task", "Renewing.", task("Renew the library loan"))], {"worker": worker})
    published: list[dict[str, Any]] = []
    daemon.daemon.events.on({"task.result"}, lambda topic, data: published.append(data))
    fire(daemon, 61)
    assert submit.calls == 1
    assert [r["answer"] for r in results(daemon)] == ["Renewed."] and len(published) == 1
