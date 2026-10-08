"""Shared schedule test support: run code on the daemon's loop, make and fire a schedule, read waiting answers, and the
planner's reply that creates a schedule."""
import json
import time
from datetime import datetime, timedelta
from typing import Any

from ferb.cli.client import Client
from ferb.orchestrator.schedules import Rule
from ferb.policy.targets import Targets
from tests.conftest import RunningDaemon


def run(d: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return d.run(go())


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
