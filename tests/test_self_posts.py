"""ORCH-588: what FERB posts in the owner's chat by itself (a scheduled run's result, a reminder arriving) is in the
planner's conversation, in order and marked as FERB's own post. Live 2026-11-10 12:04: the owner asked "why couldn't
you finish??" under a failed scheduled visa check; the planner, which never saw that post, answered about an Amazon
order from 10:25."""
import json
from datetime import timedelta

from ferb.policy.targets import Targets
from tests.conftest import RunningDaemon, chat, install, report, sup, task
from tests.support.schedules import fire, make_schedule, run

FAILED = "The visa site blocked every attempt, so no slots were checked."


def _conversation(script: object, n: int) -> str:
    context = [c for _, k, c, _ in script.contexts if k == "supervisor"][n]   # type: ignore[attr-defined]
    start = context.index("Layer 5 — conversation")
    return context[start:context.index("Layer 5 — owner's", start)]


# decision: ORCH-588
def test_a_scheduled_runs_post_is_the_last_thing_the_planner_sees_before_the_owners_question(
        daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    install(daemon, [sup("reply", "Noted.")])
    chat(daemon, "the visa check is every morning", session="owner")
    make_schedule(daemon, "Check visa slots", "Check visa slots every day", Targets(), now + timedelta(minutes=1))
    script = install(daemon, [sup("task", "Checking.", task("Check visa slots")),
                              sup("reply", "The visa site blocked every attempt.")],
                     {"worker": json.dumps(report("not_done", reason=FAILED))})
    fire(daemon, 61)
    assert run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.posted"))
    chat(daemon, "why couldn't you finish??", session="owner")
    history = _conversation(script, 1)
    assert "posted by itself: scheduled run" in history and FAILED in history
    assert history.rindex("posted by itself") > history.rindex("the visa check is every morning")


# decision: ORCH-588
def test_a_reminder_that_arrived_is_in_the_conversation(daemon: RunningDaemon) -> None:
    run(daemon, lambda: daemon.daemon.journal.append("timer.delivered", {
        "timer_id": "tm-1", "kind": "reminder", "message": "Take the iron tablet.", "overdue_s": 0}))
    script = install(daemon, [sup("reply", "You took it at 2 pm?")])
    chat(daemon, "did i take it?", session="owner")
    history = _conversation(script, 0)
    assert "reminder arrived" in history and "Take the iron tablet." in history
