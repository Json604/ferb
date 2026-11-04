"""The intro's Try beat (kb area app): FERB suggests one first task that fits what it saw on this Mac (a
solid general task when it saw nothing clear), then shows that task's plan as rows without doing any of it. The preview
cannot act by construction: the planner's turn alone, with no run token, no worker and no tool call, and nothing kept
as a task or as chat; both methods are FERB.app's only (its code signature is stood in for here)."""
import asyncio
import json
import threading
import time
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from ferb.core.types import ModelLimits, RunHandle
from ferb.daemon.areas import intro
from ferb.daemon.areas.intro import GENERAL_APPS, GENERAL_TASKS
from ferb.daemon.server import LONG_WAITS
from ferb.orchestrator.plans import Plan, Step
from tests.conftest import FakeDistillRuntime, RunningDaemon, chat, install, sup, task

STEPS = ["Look through Downloads for old installers", "Find the largest files on the Desktop",
         "List the apps not opened in months"]
SECRET = "ghp_abcdefghijklmnopqrstuvwxyz0123456789"
SEEN = {"summary": f"You seem to code most days in an editor (token {SECRET}).", "shown": "",
        "facts": [{"id": "m1", "text": "The owner mostly codes on this Mac."}], "at": "2026-11-01T10:00:00+00:00"}


class Silent:
    """A model that never answers."""
    name = "fake"
    limits = ModelLimits("fake-model", 1_000_000, 100_000, "test double")

    async def start(self, task: Any, request: Any, token: str) -> RunHandle:
        return RunHandle("r", "fake", task.id, {})

    async def events(self, run: RunHandle) -> Any:
        await asyncio.sleep(3600)
        yield None

    async def cancel(self, run: RunHandle) -> None:
        return None


def _as_app(daemon: RunningDaemon) -> Client:
    daemon.daemon.server.is_app = lambda token: token is not None      # FERB.app's signature, stood in for
    return Client(daemon.sock, timeout=30)


def _planned(daemon: RunningDaemon) -> Any:
    spec = task("Find three things on this Mac to tidy up")
    spec["plan"] = Plan(steps=[Step(id=f"s{i}", kind="do", goal=goal) for i, goal in enumerate(STEPS, 1)]).model_dump()
    return install(daemon, [sup("task", "I'll look for three things you could tidy up.", spec)])


def _preview(daemon: RunningDaemon, request: str = GENERAL_TASKS[0]) -> list[dict[str, Any]]:
    c = _as_app(daemon)
    try:
        return list(c.stream("intro.preview", {"request": request}))
    finally:
        c.close()


# the general tasks as the app gets them: each with its one main app ("" is the default browser)
GENERAL = [{"request": t, "app": a} for t, a in zip(GENERAL_TASKS, GENERAL_APPS)]


def _suggest(daemon: RunningDaemon) -> dict[str, Any]:
    c = _as_app(daemon)
    try:
        return c.call("intro.suggest", {})
    finally:
        c.close()


def _until(check: Any, timeout: float = 10.0) -> None:
    end = time.monotonic() + timeout
    while not check():
        assert time.monotonic() < end, "timed out"
        time.sleep(0.02)


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


# decision: ORCH-500, ORCH-516
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


# decision: ORCH-500
def test_preview_shows_the_planned_steps_as_parts(daemon: RunningDaemon) -> None:
    _planned(daemon)
    events = _preview(daemon)
    plan = events[0]
    assert plan["type"] == "plan" and "tidy up" in plan["text"]
    parts = [e for e in events if e["type"] == "part"]
    assert [p["label"] for p in parts] == STEPS
    assert len({p["part"] for p in parts}) == 3 and all(p["state"] == "preview" for p in parts)
    assert events[-1]["type"] == "done"


# decision: DMN-500
@pytest.mark.parametrize("state", ["not_started", "failed"])
def test_suggest_falls_back_to_the_general_task_without_a_scan(daemon: RunningDaemon, state: str) -> None:
    def no_model(*args: Any, **kwargs: Any) -> Any:
        raise AssertionError("no model call without a scan")
    daemon.daemon._make_runtime = no_model                                  # type: ignore[method-assign]
    _persona(daemon, error="OSError: the scan failed" if state == "failed" else None)
    assert _suggest(daemon) == {"suggestions": GENERAL, "fits": False}
    assert _journal(daemon, "intro.suggested")[-1].data == {"fits": False, "from_scan": 0, "why": f"scan {state}"}
    # a reply with fewer than two usable tasks (a blank one, and the same one twice) is topped up from the general ones
    rt = FakeDistillRuntime({"requests": [{"request": "Tidy my Desktop screenshots into a folder", "app": "Finder"},
                                          {"request": "", "app": "Notes"},
                                          {"request": "tidy my desktop  screenshots into a folder", "app": "Finder"}]})
    daemon.daemon._make_runtime = lambda *a, **k: rt                        # type: ignore[method-assign]
    _persona(daemon, seen=SEEN)
    assert _suggest(daemon) == {"suggestions": [{"request": "Tidy my Desktop screenshots into a folder", "app": "Finder"},
                                                GENERAL[0]], "fits": True}
    assert _journal(daemon, "intro.suggested")[-1].data == {"fits": True, "from_scan": 1, "why": "topped up"}


# decision: APP-693
# decision: DMN-500
def test_suggest_uses_the_scan_when_seen(daemon: RunningDaemon) -> None:
    asked: list[dict[str, Any]] = []
    rt = FakeDistillRuntime({"requests": [
        {"request": "Sort the files in my Downloads folder into project folders", "app": "Finder"},
        {"request": "Compare three code editors' pricing pages and summarize the differences",
         "app": f"Google Chrome {SECRET} " + "x" * 80}]})

    def made(*args: Any, **kwargs: Any) -> Any:
        asked.append(kwargs)
        return rt
    daemon.daemon._make_runtime = made                                      # type: ignore[method-assign]
    _persona(daemon, seen=SEEN)
    suggested = _suggest(daemon)
    assert suggested["fits"] is True
    assert [s["request"] for s in suggested["suggestions"]] == [
        "Sort the files in my Downloads folder into project folders",
        "Compare three code editors' pricing pages and summarize the differences"]
    apps = [s["app"] for s in suggested["suggestions"]]
    assert apps[0] == "Finder"                                              # the model's app, as macOS names it
    assert apps[1].startswith("Google Chrome") and SECRET not in apps[1]    # redacted like the request
    assert len(apps[1]) <= intro.APP_NAME_MAX
    assert asked and all(k["tool_names"] == [] for k in asked)              # the call can use no tool
    seen = str(rt.contexts)
    assert "code most days" in seen and SECRET not in seen                  # redacted before it reaches a model
    rows = _journal(daemon, "intro.suggested")
    assert rows[-1].data == {"fits": True, "from_scan": 2}
    assert "code most days" not in json.dumps([r.data for r in rows])       # the journal never keeps the summary
    rt.answer = RuntimeError("the service is down")                         # a model failure never reaches the app
    assert _suggest(daemon) == {"suggestions": GENERAL, "fits": False}
    assert _journal(daemon, "intro.suggested")[-1].data == {"fits": False, "from_scan": 0, "why": "RuntimeError"}


def test_a_long_suggestion_is_cut_at_a_word_boundary(daemon: RunningDaemon) -> None:
    """Code bounds what the model's suggestion can put in the notch: at most REQUEST_MAX characters, whole words only,
    with no ellipsis added."""
    long = " ".join(f"step{i} of the example cleanup" for i in range(40))
    rt = FakeDistillRuntime({"requests": [{"request": long, "app": "Finder"},
                                          {"request": "x" * 400, "app": "Notes"}]})
    daemon.daemon._make_runtime = lambda *a, **k: rt                        # type: ignore[method-assign]
    _persona(daemon, seen=SEEN)
    first, second = [s["request"] for s in _suggest(daemon)["suggestions"]]
    assert len(first) <= intro.REQUEST_MAX and long.startswith(first)
    assert long[len(first)] == " " and not first.endswith((" ", "…", "..."))   # whole words, nothing added
    assert second == "x" * intro.REQUEST_MAX                                # one long word: cut at the bound


# decision: DMN-500
def test_suggest_gives_the_general_tasks_when_the_model_stalls(daemon: RunningDaemon,
                                                                monkeypatch: pytest.MonkeyPatch) -> None:
    """The call ends just before the app stops waiting (SUGGEST_LIMIT_S, under server.LONG_WAITS): a model that never
    answers still gives the app the two general tasks."""
    assert intro.SUGGEST_LIMIT_S < LONG_WAITS["intro.suggest"]
    monkeypatch.setattr(intro, "SUGGEST_LIMIT_S", 0.3)
    daemon.daemon._make_runtime = lambda *a, **k: Silent()                  # type: ignore[method-assign]
    _persona(daemon, seen=SEEN)
    assert _suggest(daemon) == {"suggestions": GENERAL, "fits": False}
    assert _journal(daemon, "intro.suggested")[-1].data == {"fits": False, "from_scan": 0, "why": "timed out"}


# decision: ORCH-500
def test_preview_leaves_out_the_owners_running_tasks_and_open_questions(daemon: RunningDaemon) -> None:
    """On "Replay intro" with work under way, the preview's planner turn sees none of it (no running task, open
    question or earlier chat), so it plans the demo request and can't answer or control the owner's task."""
    running = task("Rename the quarterly invoices by client")
    running["plan"] = Plan(steps=[Step(id="pick", kind="ask", question="Which folder holds the invoices?",
                                       options=["Documents", "Desktop"])]).model_dump()
    demo = task("Find three things on this Mac to tidy up")
    demo["plan"] = Plan(steps=[Step(id=f"s{i}", kind="do", goal=g) for i, g in enumerate(STEPS, 1)]).model_dump()
    script = install(daemon, [sup("task", "Asking you which folder.", running),
                              sup("task", "I'll look for three things you could tidy up.", demo)])
    turn = threading.Thread(target=lambda: chat(daemon, "Rename the quarterly invoices by client"))
    turn.start()
    try:
        _until(lambda: any(ap.capability == "plan.ask" for ap in daemon.daemon.state.approvals.pending()))
        events = _preview(daemon)
        assert [p["label"] for p in events if p["type"] == "part"] == STEPS
        context = [c for _, key, c, _ in script.contexts if key == "supervisor"][-1]
        assert "quarterly invoices" not in context and "Which folder holds the invoices" not in context
        assert GENERAL_TASKS[0] in context
    finally:
        for tree in list(daemon.daemon.supervisor._tree_of.values()):
            daemon.daemon.supervisor.stop_tree(tree.id)
        turn.join(10)


# decision: ORCH-500
def test_preview_leaves_out_the_owners_notices_schedules_and_reminders(daemon: RunningDaemon) -> None:
    """The preview's planner turn sees none of the owner's live state: a notice FERB posted to the owner, a schedule and
    a reminder stay out of it, while an owner chat's turn still gets all three."""
    from datetime import timedelta

    from ferb.policy.targets import Targets
    from tests.support.schedules import make_schedule
    now = daemon.daemon.clock.now()
    make_schedule(daemon, "Summarize the example newsletter", "every morning summarize the example newsletter",
                  Targets(), now + timedelta(hours=2))

    async def owner_state() -> None:
        daemon.daemon.timers.add("reminder", now + timedelta(hours=3), "water the example fern")
        daemon.daemon.supervisor.journal.append("chat.notice", {"id": "n-example", "text": "Your example parcel moved.",
                                                                "quotes": []})
    daemon.run(owner_state())
    owners = ("example newsletter", "water the example fern", "Your example parcel moved")
    script = install(daemon, [sup("reply", "Noted.")])
    chat(daemon, "what's on today?")
    seen = [c for _, key, c, _ in script.contexts if key == "supervisor"][-1]
    assert all(part in seen for part in owners)                             # an owner chat's turn gets them
    script = _planned(daemon)
    events = _preview(daemon)
    assert [p["label"] for p in events if p["type"] == "part"] == STEPS
    context = [c for _, key, c, _ in script.contexts if key == "supervisor"][-1]
    assert not [part for part in owners if part in context]                 # the preview's turn gets none


# decision: ORCH-500
def test_chat_send_refuses_ferbs_own_sessions(daemon: RunningDaemon) -> None:
    c = Client(daemon.sock)
    try:
        with pytest.raises(RpcError) as refused:
            list(c.stream("chat.send", {"session_id": intro.INTRO_SESSION, "text": "hello"}))
        assert "FERB's own sessions" in str(refused.value)
    finally:
        c.close()


# decision: ORCH-500
# decision: DMN-500
def test_intro_methods_refuse_any_other_program(daemon: RunningDaemon) -> None:
    c = Client(daemon.sock)
    try:
        with pytest.raises(RpcError) as suggested:
            c.call("intro.suggest", {})
        assert "FERB.app only" in str(suggested.value)
        with pytest.raises(RpcError) as previewed:
            list(c.stream("intro.preview", {"request": GENERAL_TASKS[0]}))
        assert "FERB.app only" in str(previewed.value)
    finally:
        c.close()
