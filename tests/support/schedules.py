"""Shared schedule test support: run code on the daemon's loop, make and fire a schedule, read waiting answers, the
planner's reply that creates a schedule, and a test daemon of its own (for a restart)."""
import json
import time
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

from ferb.cli.client import Client
from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.orchestrator.schedules import Rule
from ferb.policy.targets import Targets
from tests.conftest import RunningDaemon
from tests.support import egress


def run(d: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return d.run(go())


def mins(delta: timedelta) -> dict[str, Any]:
    """A `when` of "in N minutes" (the model's output; FERB computes the time)."""
    return {"in_minutes": delta.total_seconds() / 60, "in_days": None, "day": None, "date": None, "time": None}


def schedule_reply(d: RunningDaemon, request: str, quote: str, first: datetime, repeat: str = "none",
                   recipients: tuple[str, ...] = (), paths: tuple[str, ...] = ("report.md",),
                   every: int | None = None, watch: str = "none") -> str:
    return json.dumps({"kind": "reply", "text": "Scheduled.", "task": None, "remember": [], "timers": [],
                       "schedule": {"request": request, "when": mins(first - d.daemon.clock.now()), "repeat": repeat,
                                    "weekday": None, "every_minutes": every, "quote": quote,
                                    "targets": {"apps": [], "sites": [], "paths": list(paths),
                                                "recipients": list(recipients)}, "watch": watch},
                       "schedule_changes": []})


def make_schedule(d: RunningDaemon, request: str, own: str, targets: Targets, first: datetime,
                  repeat: str = "daily", every: int | None = None, watch: str = "none",
                  standing: list[dict[str, Any]] | None = None) -> Any:
    """`standing`: the send scopes recorded with it (as `_schedule_create` records the owner's stated ones, D30)."""
    def go() -> Any:
        recorded = targets.to_json() | ({"standing_sends": standing} if standing else {})
        s = d.daemon.schedules.add(request, own, recorded, Rule.build(repeat, first, every_minutes=every), first,
                                   watch)
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


def own_daemon(home: Path, ids: str) -> RunningDaemon:
    """A test daemon the test stops itself, set up as the `daemon` fixture's is; a second one on the same home is FERB
    after a restart (its own id prefix, so its new rows never collide with the first one's)."""
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds(ids), watch_owner_input=False)
    d.daemon._permissions = lambda: {"accessibility": True, "screen": True}   # type: ignore[method-assign]
    egress.install(getattr(d.daemon, "proxy", None))
    if d.daemon.supervisor is not None:
        d.daemon.supervisor.availability = d.daemon.supervisor._default_availability
    return d
