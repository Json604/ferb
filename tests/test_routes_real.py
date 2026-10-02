"""Build step 16, real (pytest -m real; Codex only): the real planner follows the route FERB chose.
Planning only: each task is stopped as soon as its plan is shown, so nothing is done in the owner's apps or accounts.
1. "make a note in Notes …" with Notes installed -> "Using the Notes app", a Mac-app plan.
2. "play lofi music on youtube" with a paired Brave "Work" profile -> "Using YouTube on the web … Brave (Work)", a
   browser plan for that profile.
"""

import shutil
import tempfile
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.cli.client import Client
from ferb.clock.clock import SystemClock
from ferb.policy import services as svc
from tests.conftest import RunningDaemon, only_tools

pytestmark = pytest.mark.real


@pytest.fixture
def codex() -> Iterator[RunningDaemon]:
    base = Path(tempfile.mkdtemp(prefix="frt-", dir="/tmp"))
    d = RunningDaemon(base / "home", clock=SystemClock())
    d.daemon.use_identity_browser(_StandIn())                          # FERB's browser tools (planning only: never run)
    only_tools(d, "fs.", "time.", "browser.", "ferb_browser.", "mac.apps", "mac.open", "mac.snapshot", "mac.act")
    d.daemon.config.set("runtimes", "orchestrator", "codex")
    d.daemon.config.set("runtimes", "worker", "codex")
    env = svc.Environment([("com.apple.Notes", "Notes")], [("brave", "Default", "Work")])

    def environment(words: str) -> dict[str, Any]:
        named = svc.services_named(words, env)
        stable, live = svc.summary(env, "26.0", named, set())
        return {"stable": stable, "live": live}
    d.daemon.supervisor.environment = environment                                  # type: ignore[union-attr]
    d.daemon.supervisor.apps = lambda: env.apps                                    # type: ignore[union-attr]
    sup = d.daemon.supervisor
    sup.availability = sup._default_availability                   # type: ignore[union-attr]  # about routes, not pairing
    yield d
    d.stop()
    shutil.rmtree(base, ignore_errors=True)


class _StandIn:
    """FERB's browser for planning-only tests: tasks are stopped at the plan, so nothing is ever called."""
    secrets = None

    async def release(self, owner: str) -> None:
        return None

    async def shutdown(self) -> None:
        return None


def plan_of(d: RunningDaemon, text: str) -> dict[str, Any]:
    """The task as chat shows it first (identity, goal); the task is stopped right away."""
    c = Client(d.sock, timeout=900)
    first: Any = {}
    try:
        for ev in c.stream("chat.send", {"session_id": "s", "text": text}):
            first = first or ev
            if ev["type"] == "plan":
                c2 = Client(d.sock)
                c2.call("task.stop", {"task_ids": [ev["task_id"]]})
                c2.close()
    finally:
        c.close()
    return first


def test_the_notes_app_is_an_owner_task(codex: RunningDaemon) -> None:
    ev = plan_of(codex, "make a note in Notes titled 'ferb route test' saying hello")
    assert ev.get("identity") == "owner" and "Notes" in ev.get("goal", ""), ev


def test_a_plain_request_on_a_site_is_a_ferb_task(codex: RunningDaemon) -> None:
    ev = plan_of(codex, "play some lofi music on youtube")
    assert ev.get("identity") == "ferb", (ev, codex.run(_why(codex)))


def test_the_owners_profile_when_they_say_so(codex: RunningDaemon) -> None:
    ev = plan_of(codex, "in my Work profile, play some lofi music on youtube")
    assert ev.get("identity") == "owner" and "Work" in ev.get("goal", ""), ev


async def _why(d: RunningDaemon) -> list[Any]:
    return [e.data for e in d.daemon._journal().entries() if e.kind in ("task.repair", "chat.failed")]
