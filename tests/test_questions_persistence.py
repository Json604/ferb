"""Owner questions never expire while their task waits (spec §8.1, L3): no clock ends a question; it closes when it is
answered or its task ends. The setup profile question ("Getting to know you", P28) is asked once and sends no banner."""
import asyncio
import time
from types import SimpleNamespace
from typing import Any, Callable

from ferb.cli.client import Client
from ferb.core.types import Identity
from ferb.device.persona import persona_proposal
from tests.conftest import RunningDaemon, gateway_request, gateway_setup

LONGER_THAN_THE_OLD_EXPIRY = 601          # seconds: the removed 600 s question lifetime, plus one


def _pending(d: RunningDaemon) -> list[str]:
    async def go() -> list[str]:
        return [ap.id for ap in d.daemon.state.approvals.pending()]
    return d.run(go())


def _until(check: Callable[[], Any], what: str) -> Any:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if found := check():
            return found
        time.sleep(0.02)
    raise AssertionError(what)


def _let_waiters_look_again(d: RunningDaemon) -> None:
    """A waiting call checks its question every hold_poll_s: let it look several times after the clock moved."""
    time.sleep(4 * d.daemon.gateway.hold_poll_s)


def test_a_worker_question_older_than_600_s_is_still_pending_and_its_answer_reaches_the_task(
        daemon: RunningDaemon) -> None:
    grant, _ = gateway_setup(daemon, set(), gateway_request("tidy my downloads"), [])
    asked = asyncio.run_coroutine_threadsafe(
        daemon.daemon.gateway.call(grant, "owner.ask", {"question": "Which folder should I tidy?"}), daemon.loop)
    [question] = _until(lambda: _pending(daemon), "the worker's question never appeared")
    daemon.call_soon(daemon.daemon.clock.advance, LONGER_THAN_THE_OLD_EXPIRY)
    _let_waiters_look_again(daemon)
    assert not asked.done(), "the task stopped waiting for the owner's answer"
    assert _pending(daemon) == [question]
    client = Client(daemon.sock)
    try:
        assert [a["approval_id"] for a in client.call("chat.status", {"peek": True})["approvals"]] == [question]
        assert client.call("chat.answer", {"approval_id": question, "text": "Downloads"})["state"] == "granted"
    finally:
        client.close()
    result = asked.result(5)
    assert not result["is_error"] and result["content"]["answer"] == "Downloads", result


def test_a_plan_question_older_than_600_s_still_waits_for_the_owner(daemon: RunningDaemon) -> None:
    sup = daemon.daemon.supervisor
    assert sup is not None
    worker = SimpleNamespace(id="tree-q.worker", request_id="req-q", identity=Identity.OWNER, scopes=set())
    tree = SimpleNamespace(id="tree-q", worker=worker, stop=None)
    asked = asyncio.run_coroutine_threadsafe(
        sup._plan_question(tree, "Which folder first?", ["Documents", "Desktop"]), daemon.loop)
    [question] = _until(lambda: _pending(daemon), "the plan's question never appeared")
    daemon.call_soon(daemon.daemon.clock.advance, LONGER_THAN_THE_OLD_EXPIRY)
    _let_waiters_look_again(daemon)
    assert not asked.done() and _pending(daemon) == [question]

    async def answer() -> None:
        daemon.daemon._resolve_approval(question, True, text="Desktop")
    daemon.run(answer())
    result = asked.result(5)
    assert result["status"] == "done" and result["answer"] == "Desktop"


def test_a_question_closes_when_its_task_ends(daemon: RunningDaemon) -> None:
    """Without an expiry, the end of the task is what closes its question (the request is forgotten)."""
    grant, _ = gateway_setup(daemon, set(), gateway_request("tidy my downloads"), [])
    asked = asyncio.run_coroutine_threadsafe(
        daemon.daemon.gateway.call(grant, "owner.ask", {"question": "Which folder should I tidy?"}), daemon.loop)
    _until(lambda: _pending(daemon), "the worker's question never appeared")

    async def task_ends() -> None:
        daemon.daemon.state.forget_request("req-1")
    daemon.run(task_ends())
    assert asked.result(5)["content"].get("declined") is True
    assert _pending(daemon) == []


def _persona_daemon(daemon: RunningDaemon) -> tuple[list[tuple[str, dict[str, Any]]], list[tuple[str, str]]]:
    def collect(home: Any, device: Any, activity: Any, excluded: Any) -> dict[str, Any]:
        return persona_proposal([{'bundle_id': 'code', 'app': 'Code', 'uses': 7}], '2026-10-11')
    daemon.daemon.persona_scan = collect
    published: list[tuple[str, dict[str, Any]]] = []
    banners: list[tuple[str, str]] = []
    real_publish = daemon.daemon._publish

    def publish(topic: str, data: dict[str, Any]) -> None:
        published.append((topic, data))
        real_publish(topic, data)

    async def banner(title: str, text: str) -> None:
        banners.append((title, text))
    daemon.daemon._publish = publish                     # type: ignore[method-assign]
    daemon.daemon.notify = banner
    return published, banners


def test_one_hundred_status_polls_show_one_profile_question_published_once_with_no_banner(
        daemon: RunningDaemon) -> None:
    published, banners = _persona_daemon(daemon)
    client = Client(daemon.sock)
    try:
        client.call("chat.attach")

        async def scanned() -> None:
            await daemon.daemon.persona.task
            await asyncio.sleep(0)                        # a banner would be scheduled on the loop by now
        daemon.run(scanned())
        seen: set[str] = set()
        for poll in range(100):
            if poll % 10 == 0:                            # the notch polls every 5 s: well past the old 600 s
                daemon.call_soon(daemon.daemon.clock.advance, LONGER_THAN_THE_OLD_EXPIRY)
            approvals = client.call("chat.status", {"peek": True})["approvals"]
            seen.update(a["approval_id"] for a in approvals if a["capability"] == "setup.persona")
        assert len(seen) == 1, seen
        asked = [d for topic, d in published if topic == "approval.requested" and d["capability"] == "setup.persona"]
        assert [d["approval_id"] for d in asked] == list(seen)
        daemon.run(asyncio.sleep(0.05))
        assert banners == []
    finally:
        client.close()


def test_the_profile_question_is_asked_once_across_status_and_start(tmp_path: Any) -> None:
    from ferb.clock.clock import FakeClock
    from ferb.config import Config
    from ferb.daemon.persona import PersonaReview
    from ferb.daemon.questions import Approvals

    async def scenario() -> None:
        clock = FakeClock()
        approvals = Approvals()
        events: list[tuple[str, dict[str, Any]]] = []
        review = PersonaReview(Config(tmp_path / 'home'), approvals, clock, lambda: persona_proposal(
            [{'app': 'Code', 'bundle_id': 'code', 'uses': 3, 'last_used': '2026-10-10'}], '2026-10-11'),
            remember=lambda text: None, publish=lambda topic, data: events.append((topic, data)),
            record=lambda kind, data: None)
        review.start()
        assert review.task is not None
        await review.task
        first = review.status()['approval']['approval_id']
        for _ in range(100):
            clock.advance(LONGER_THAN_THE_OLD_EXPIRY)
            assert review.status()['approval']['approval_id'] == first
            assert review.start()['approval']['approval_id'] == first
        assert [topic for topic, _ in events] == ['approval.requested']
        assert [ap.id for ap in approvals.pending()] == [first]
    asyncio.run(scenario())
