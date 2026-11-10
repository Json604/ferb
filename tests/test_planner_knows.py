"""ORCH-589 (owner 2026-11-10: "the perception that a single ferb agent does eveyrhign"): the planner knows what FERB
did. Through the real daemon, gateway and scripted model: the schedules' last runs in Layer 2 (ORCH-592)."""
import json
from datetime import timedelta

from ferb.policy.targets import Targets
from tests.conftest import RunningDaemon, chat, install, report, sup, task
from tests.support.schedules import fire, make_schedule, run

NO_SLOTS = "No slots at any of the five posts."
BLOCKED = "The visa site signed me out before any post was read."


def _planner(script: object, n: int = -1) -> str:
    return [c for _, k, c, _ in script.contexts if k == "supervisor"][n]   # type: ignore[attr-defined]


def _visa_schedule(d: RunningDaemon, minutes: int = 1) -> object:
    return make_schedule(d, "Check visa slots at the five posts", "check the visa slots every day", Targets(),
                         d.daemon.clock.now() + timedelta(minutes=minutes))


# decision: ORCH-592
def test_each_schedule_line_carries_its_last_run(daemon: RunningDaemon) -> None:
    _visa_schedule(daemon)
    install(daemon, [sup("task", "Checking.", task("Check visa slots"))],
            {"worker": json.dumps(report("not_done", reason=BLOCKED))})
    fire(daemon, 61)
    assert run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.posted"))
    script = install(daemon, [sup("reply", "It failed once.")])
    chat(daemon, "how is the visa check doing?", session="owner")
    layer2 = _planner(script).split("Layer 2 — right now", 1)[1]
    line = next(x for x in layer2.splitlines() if "Check visa slots at the five posts" in x)
    assert ", failed, 1 failed in a row, its answer's first line (model-written, data): \"I couldn't finish" in line
    assert BLOCKED in line
