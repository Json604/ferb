"""L34 Phase G: FERB.app installs an update only when nothing runs that a restart would cut. `update.prepare` says what
it waits for, or holds new work, quits FERB Voice and stops the daemon at once. A loop resting between rounds and a plan
waiting for its time don't hold it (they resume after the restart, L31); the owner's Quit and Install stops even with a
task running."""

import asyncio
from datetime import timedelta
from types import SimpleNamespace
from typing import Any, Callable, Iterator

import pytest

from ferb.cli.client import Client, RpcError
from ferb.daemon import auth
from ferb.orchestrator.plans import Plan, Step
from tests.conftest import RunningDaemon

LOOP = Plan(steps=[Step(id="keep", kind="loop", goal="Keep the tab playing")])


def _call(daemon: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    c = Client(daemon.sock)
    try:
        return c.call(method, params or {})
    finally:
        c.close()


def _on_loop(daemon: RunningDaemon, fn: Callable[[], Any]) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


@pytest.fixture
def quits(daemon: RunningDaemon) -> list[int]:
    """FERB Voice is never really quit from a test (the owner's own may be running)."""
    calls: list[int] = []
    daemon.daemon._quit_voice_app = lambda: calls.append(1)            # type: ignore[method-assign]
    return calls


@pytest.fixture
def trees(daemon: RunningDaemon) -> Iterator[Callable[..., None]]:
    """Running trees as run_tree registers them (and their plans' step states); removed before the daemon stops."""
    sv = daemon.daemon.supervisor
    assert sv is not None
    seeded: list[str] = []

    def seed(tree_id: str, goal: str, plan: Plan | None = None, running: tuple[str, ...] = (),
             waiting: tuple[str, ...] = (), resting: tuple[str, ...] = ()) -> None:
        def go() -> None:
            sv._tree_of[tree_id] = SimpleNamespace(id=tree_id, spec=SimpleNamespace(goal=goal), watch=False, stop=None)
            if plan is not None:
                sv.plans.create(tree_id, plan, {"spec": {}, "request_id": "r-" + tree_id, "words": goal,
                                                "payload": goal}, None)
                sv._plan_runs[tree_id] = SimpleNamespace(wake=lambda: None)
                for step in running:
                    sv.plans.begin(tree_id, step)
                for step in waiting:
                    sv.plans.finish(tree_id, step, {"status": "waiting"})
            sv._resting.update(f"{tree_id}.loop.{step}" for step in resting)
        _on_loop(daemon, go)
        seeded.append(tree_id)
    yield seed

    def clear() -> None:
        for tree_id in seeded:
            sv._tree_of.pop(tree_id, None)
            sv._plan_runs.pop(tree_id, None)
            sv._resting.difference_update({r for r in set(sv._resting) if r.startswith(tree_id + ".")})
    _on_loop(daemon, clear)


def _prepared(daemon: RunningDaemon) -> list[Any]:
    return _on_loop(daemon, lambda: daemon.daemon.journal.of_kind("update.prepared"))   # type: ignore[union-attr]


def test_an_idle_ferb_stops_for_the_update(daemon: RunningDaemon, quits: list[int]) -> None:
    assert _call(daemon, "update.prepare", {"version": "0.28.0"}) == {"ready": True, "running": []}
    assert daemon.daemon._stop.is_set() and daemon.daemon._shutting_down
    assert quits == [1]
    assert _prepared(daemon)[-1].data == {"version": "0.28.0", "interrupted": []}
    c = Client(daemon.sock, timeout=10)
    try:
        with pytest.raises(RpcError, match="shutting down"):
            list(c.stream("chat.send", {"session_id": "s1", "text": "rename my invoices"}))
    finally:
        c.close()


def test_a_running_task_holds_the_update_and_nothing_stops(daemon: RunningDaemon, quits: list[int],
                                                           trees: Callable[..., None]) -> None:
    trees("tree-a", "Rename the invoices")
    assert _call(daemon, "update.prepare", {"version": "0.28.0"}) == {
        "ready": False, "running": [{"task_id": "tree-a", "goal": "Rename the invoices"}]}
    assert not daemon.daemon._stop.is_set() and not daemon.daemon._shutting_down and quits == []
    assert _prepared(daemon) == []


def test_a_loop_resting_between_rounds_does_not_hold_the_update(daemon: RunningDaemon, quits: list[int],
                                                                trees: Callable[..., None]) -> None:
    trees("tree-l", "Keep the tab playing", LOOP, running=("keep",), resting=("keep",))
    assert _call(daemon, "update.prepare", {})["ready"] is True


def test_a_loop_in_its_round_holds_the_update(daemon: RunningDaemon, quits: list[int],
                                              trees: Callable[..., None]) -> None:
    trees("tree-r", "Keep the tab playing", LOOP, running=("keep",))
    reply = _call(daemon, "update.prepare", {})
    assert reply["ready"] is False and reply["running"][0]["task_id"] == "tree-r"


def test_a_plan_waiting_for_its_time_does_not_hold_the_update(daemon: RunningDaemon, quits: list[int],
                                                              trees: Callable[..., None]) -> None:
    plan = Plan(steps=[Step(id="later", kind="wait", goal="Wait until 9", until="2026-10-26T09:00:00+00:00"),
                       Step(id="post", kind="do", goal="Post the update", depends_on=["later"])])
    trees("tree-w", "Post at 9", plan, waiting=("later",))
    assert _call(daemon, "update.prepare", {})["ready"] is True


def test_a_plan_doing_a_step_beside_its_resting_loop_holds_the_update(daemon: RunningDaemon, quits: list[int],
                                                                      trees: Callable[..., None]) -> None:
    plan = Plan(steps=[Step(id="keep", kind="loop", goal="Keep it playing"), Step(id="sum", kind="do", goal="Summarize")])
    trees("tree-d", "Two things", plan, running=("keep", "sum"), resting=("keep",))
    assert _call(daemon, "update.prepare", {})["ready"] is False


def test_a_tree_whose_plan_has_not_started_holds_the_update(daemon: RunningDaemon, quits: list[int],
                                                            trees: Callable[..., None]) -> None:
    trees("tree-p", "Plan something")                     # its planner turn is still running: no plan run yet
    assert _call(daemon, "update.prepare", {})["ready"] is False


def test_a_ferb_process_still_running_holds_the_update(daemon: RunningDaemon, quits: list[int]) -> None:
    proc = SimpleNamespace(running=True, owner_task="tree-old.worker", command="npm run dev")
    shells = daemon.daemon.shells
    assert shells is not None
    shells.list = lambda: [proc]                                       # type: ignore[method-assign]
    try:
        reply = _call(daemon, "update.prepare", {})
    finally:
        del shells.list                                                # the real list again for the daemon's shutdown
    assert reply == {"ready": False, "running": [{"task_id": "tree-old.worker", "goal": "process: npm run dev"}]}


def test_the_owners_quit_and_install_stops_even_with_a_task_running(daemon: RunningDaemon, quits: list[int],
                                                                    trees: Callable[..., None]) -> None:
    trees("tree-a", "Rename the invoices")
    reply = _call(daemon, "update.prepare", {"version": "0.28.0", "even_if_busy": True})
    assert reply["ready"] is True and reply["running"][0]["task_id"] == "tree-a"
    assert daemon.daemon._stop.is_set()
    assert _prepared(daemon)[-1].data["interrupted"] == ["tree-a"]


def test_nothing_new_starts_once_the_update_holds(daemon: RunningDaemon, quits: list[int]) -> None:
    from ferb.orchestrator.schemas import TaskSpec
    from tests.conftest import task
    _call(daemon, "update.prepare", {})
    sv = daemon.daemon.supervisor
    assert sv is not None

    async def start() -> None:
        gen = sv.run_tree(TaskSpec.model_validate(task("Rename the invoices")), "x", "req-held")
        try:
            await asyncio.wait_for(gen.__anext__(), 0.3)
        finally:
            await gen.aclose()
    with pytest.raises(asyncio.TimeoutError):
        daemon.run(start())


def test_a_timer_that_fires_during_shutdown_is_kept_for_after_the_restart(daemon: RunningDaemon) -> None:
    """Before, a timer due while FERB shut down was acknowledged unhandled: a reminder was lost."""
    d = daemon.daemon
    assert d.timers is not None
    d._shutting_down = True
    timer = _on_loop(daemon, lambda: d.timers.add("reminder", d.clock.now() - timedelta(seconds=1), "Call the bank"))
    assert daemon.run(d.timers.tick()) == 0          # the consumer refused it; later timers still go (step 2)
    assert _on_loop(daemon, lambda: d.timers.get(timer.id).state) == "claimed"
    d._shutting_down = False
    daemon.run(d.timers.tick())
    assert _on_loop(daemon, lambda: d.timers.get(timer.id).state) == "delivered"


def test_only_the_owners_connection_may_stop_ferb_for_an_update() -> None:
    assert auth.allowed(auth.Role.CLI, "update.prepare")
    for role in (auth.Role.MCP_SHIM, auth.Role.NATIVE_HOST):
        assert not auth.allowed(role, "update.prepare")
