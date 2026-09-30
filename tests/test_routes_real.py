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
    only_tools(d, "fs.", "time.", "browser.", "mac.apps", "mac.open", "mac.snapshot", "mac.act")   # about app routes
    d.daemon.config.set("runtimes", "orchestrator", "codex")
    d.daemon.config.set("runtimes", "worker", "codex")
    env = svc.Environment([("com.apple.Notes", "Notes")], [("brave", "Default", "Work")])

    def environment(words: str) -> dict[str, Any]:
        named = svc.services_named(words, env)
        stable, live = svc.summary(env, "26.0", named, set())
        return {"routes": [r for s in named if (r := svc.route(s, env))], "stable": stable, "live": live}
    d.daemon.supervisor.environment = environment                                  # type: ignore[union-attr]
    d.daemon.supervisor.apps = lambda: env.apps                                    # type: ignore[union-attr]
    sup = d.daemon.supervisor
    sup.availability = sup._default_availability                   # type: ignore[union-attr]  # about routes, not pairing
    yield d
    d.stop()
    shutil.rmtree(base, ignore_errors=True)


def plan_of(d: RunningDaemon, text: str) -> str:
    """What chat shows first; a planned task is stopped right away."""
    c, first = Client(d.sock, timeout=900), ""
    try:
        for ev in c.stream("chat.send", {"session_id": "s", "text": text}):
            first = first or str(ev.get("text", ""))
            if ev["type"] == "plan":
                c2 = Client(d.sock)
                c2.call("task.stop", {"task_ids": [ev["task_id"]]})
                c2.close()
    finally:
        c.close()
    return first


def test_the_notes_app_route(codex: RunningDaemon) -> None:
    text = plan_of(codex, "make a note in Notes titled 'ferb route test' saying hello")
    assert text.startswith("Using the Notes app"), text


def test_a_web_route_in_the_paired_brave_profile(codex: RunningDaemon) -> None:
    text = plan_of(codex, "play some lofi music on youtube")
    assert text.startswith("Using YouTube on the web (youtube.com) in your Brave (Work) profile"), text
