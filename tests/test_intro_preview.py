"""The intro's Try beat (notes/setup-intro.md step 6): FERB suggests one first task that fits what it saw on this Mac (a
solid general task when it saw nothing clear), then shows that task's plan as rows without doing any of it. The preview
cannot act by construction: the planner's turn alone, with no run token, no worker and no tool call, and nothing kept
as a task or as chat; both methods are FERB.app's only (its code signature is stood in for here)."""
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from ferb.daemon.areas.intro import GENERAL_TASK
from ferb.orchestrator.plans import Plan, Step
from tests.conftest import FakeDistillRuntime, RunningDaemon, install, sup, task

STEPS = ["Look through Downloads for old installers", "Find the largest files on the Desktop",
         "List the apps not opened in months"]
SECRET = "ghp_abcdefghijklmnopqrstuvwxyz0123456789"


def _as_app(daemon: RunningDaemon) -> Client:
    daemon.daemon.server.is_app = lambda token: token is not None      # FERB.app's signature, stood in for
    return Client(daemon.sock, timeout=30)


def _planned(daemon: RunningDaemon) -> Any:
    spec = task("Find three things on this Mac to tidy up")
    spec["plan"] = Plan(steps=[Step(id=f"s{i}", kind="do", goal=goal) for i, goal in enumerate(STEPS, 1)]).model_dump()
    return install(daemon, [sup("task", "I'll look for three things you could tidy up.", spec)])


def _preview(daemon: RunningDaemon, request: str = GENERAL_TASK) -> list[dict[str, Any]]:
    c = _as_app(daemon)
    try:
        return list(c.stream("intro.preview", {"request": request}))
    finally:
        c.close()


def _persona(daemon: RunningDaemon, *, seen: dict[str, Any] | None = None, error: str | None = None) -> None:
    async def go() -> None:
        review = daemon.daemon._mac_profile_service()
        if seen is not None:
            review.state.set_seen(seen)
        review.error = error
    daemon.run(go())


def _journal(daemon: RunningDaemon, kind: str) -> list[Any]:
    async def read() -> list[Any]:
        return list(daemon.daemon.journal.entries_of(kind))
    return list(daemon.run(read()))


def _latest_task(daemon: RunningDaemon) -> str | None:
    async def read() -> str | None:
        return daemon.daemon.tasks.latest_root()
    return daemon.run(read())  # type: ignore[no-any-return]


def test_preview_runs_no_worker_and_issues_no_run_token(daemon: RunningDaemon) -> None:
    script = _planned(daemon)
    issued: list[Any] = []
    tokens = daemon.daemon.tokens
    issue = tokens.issue

    def counted(*args: Any, **kwargs: Any) -> str:
        issued.append(args)
        return issue(*args, **kwargs)
    tokens.issue = counted                                                  # type: ignore[method-assign]
    topics: list[str] = []
    bus = daemon.daemon.bus
    publish = bus.publish

    def heard(topic: str, data: Any = None) -> Any:
        topics.append(topic)
        return publish(topic, data)
    bus.publish = heard                                                     # type: ignore[method-assign]
    events = _preview(daemon)
    assert issued == []                                                     # no RunGrant: nothing could call a tool
    assert "tool.called" not in topics and not any(t.startswith("task.") for t in topics)
    assert events and all(e.get("preview") is True for e in events)
    assert [e["type"] for e in events] == ["plan", "part", "part", "part", "done"]
    assert [key for _, key, _, _ in script.contexts] == ["supervisor"]      # the planner's turn only, no worker
    assert all(tools == [] for _, _, _, tools in script.contexts)           # and it had no tools
    assert not _latest_task(daemon)                                         # no task in the owner's list
    assert not _journal(daemon, "chat.message") and not _journal(daemon, "chat.plan")   # nor in any chat
    assert len(_journal(daemon, "intro.preview")) == 1


def test_preview_shows_the_planned_steps_as_parts(daemon: RunningDaemon) -> None:
    _planned(daemon)
    events = _preview(daemon)
    plan = events[0]
    assert plan["type"] == "plan" and "tidy up" in plan["text"]
    parts = [e for e in events if e["type"] == "part"]
    assert [p["label"] for p in parts] == STEPS
    assert len({p["part"] for p in parts}) == 3 and all(p["state"] == "preview" for p in parts)
    assert events[-1]["type"] == "done"


@pytest.mark.parametrize("state", ["not_started", "failed"])
def test_suggest_falls_back_to_the_general_task_without_a_scan(daemon: RunningDaemon, state: str) -> None:
    def no_model(*args: Any, **kwargs: Any) -> Any:
        raise AssertionError("no model call without a scan")
    daemon.daemon._make_runtime = no_model                                  # type: ignore[method-assign]
    _persona(daemon, error="OSError: the scan failed" if state == "failed" else None)
    c = _as_app(daemon)
    try:
        answer = c.call("intro.suggest", {})
    finally:
        c.close()
    assert answer == {"request": GENERAL_TASK, "fits": False}


def test_suggest_uses_the_scan_when_seen(daemon: RunningDaemon) -> None:
    rt = FakeDistillRuntime({"request": "Sort the files in my Downloads folder into project folders"})
    daemon.daemon._make_runtime = lambda *a, **k: rt                        # type: ignore[method-assign]
    _persona(daemon, seen={"summary": f"You seem to code most days in an editor (token {SECRET}).",
                           "shown": "", "facts": [{"id": "m1", "text": "The owner mostly codes on this Mac."}],
                           "at": "2026-11-01T10:00:00+00:00"})
    c = _as_app(daemon)
    try:
        answer = c.call("intro.suggest", {})
    finally:
        c.close()
    assert answer == {"request": "Sort the files in my Downloads folder into project folders", "fits": True}
    seen = str(rt.contexts)
    assert "code most days" in seen and SECRET not in seen                  # redacted before it reaches a model
    rt.answer = RuntimeError("the service is down")                         # a model failure never reaches the app
    c = _as_app(daemon)
    try:
        assert c.call("intro.suggest", {}) == {"request": GENERAL_TASK, "fits": False}
    finally:
        c.close()


def test_intro_methods_refuse_any_other_program(daemon: RunningDaemon) -> None:
    c = Client(daemon.sock)
    try:
        with pytest.raises(RpcError) as suggested:
            c.call("intro.suggest", {})
        assert "FERB.app only" in str(suggested.value)
        with pytest.raises(RpcError) as previewed:
            list(c.stream("intro.preview", {"request": GENERAL_TASK}))
        assert "FERB.app only" in str(previewed.value)
    finally:
        c.close()
