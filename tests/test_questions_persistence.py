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
        review = PersonaReview(Config(tmp_path / 'home'), approvals, lambda: persona_proposal(
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


# ---------- the worker's side: the MCP shim and the daemon's view of its connection (review fix 2, owner §8.1) ----------

class _ShimClient:
    """Stands in for the shim's daemon connection: `call` waits until answered, interrupted, or `fail` says how."""

    def __init__(self, fail: BaseException | None = None) -> None:
        import threading
        self.fail, self.interrupted, self.closed = fail, threading.Event(), False

    def call(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        if self.fail is not None:
            raise self.fail
        self.interrupted.wait(2)
        from ferb.core.errors import NotRunning
        raise NotRunning("FERB daemon closed the connection")

    def interrupt(self) -> None:
        self.interrupted.set()

    def close(self) -> None:
        self.closed = True


def _shim(monkeypatch: Any, client: Any = None) -> Any:
    from ferb.cli import mcp
    monkeypatch.setenv("FERB_RUN_TOKEN", "test-token")
    shim = mcp.Shim()
    if client is not None:
        monkeypatch.setattr(shim, "_client", lambda: client)
    return shim


def test_the_mcp_shim_waits_as_long_as_the_call_takes(monkeypatch: Any) -> None:
    """No transport cutoff: an owner's answer may take longer than any number FERB could pick (§8.1)."""
    from ferb.cli import mcp
    seen: dict[str, Any] = {}

    class Recorder:
        def __init__(self, path: Any, role: str = "cli", token: str | None = None, timeout: Any = 10) -> None:
            seen.update(role=role, timeout=timeout)
    monkeypatch.setattr(mcp, "Client", Recorder)
    _shim(monkeypatch)._client()
    assert seen == {"role": "mcp_shim", "timeout": None}


def test_a_broken_connection_is_a_tool_error_with_an_unknown_outcome_never_a_raw_exception(monkeypatch: Any) -> None:
    import anyio
    import mcp.types as t
    for failure in (TimeoutError("timed out"), OSError("socket broke"), ValueError("half a frame")):
        shim = _shim(monkeypatch, _ShimClient(failure))
        result = anyio.run(shim.call_tool, None, t.CallToolRequestParams(name="owner_ask", arguments={}))
        assert result.is_error, failure
        text = result.content[0].text
        assert "outcome is unknown" in text and type(failure).__name__ in text


def test_a_cancelled_call_closes_its_connection_so_the_daemon_sees_the_caller_leave(monkeypatch: Any) -> None:
    import anyio
    import mcp.types as t
    client = _ShimClient()
    shim = _shim(monkeypatch, client)

    async def runtime_gives_up() -> None:
        with anyio.move_on_after(0.2):
            await shim.call_tool(None, t.CallToolRequestParams(name="owner_ask", arguments={"question": "Which?"}))
    started = time.monotonic()
    anyio.run(runtime_gives_up)
    assert client.interrupted.is_set() and time.monotonic() - started < 1.5


def _ask_over_the_socket(daemon: RunningDaemon, token: str, question: str) -> tuple[Any, Any]:
    """A worker's owner.ask as the shim makes it: its own connection, no timeout. (future, client)."""
    import concurrent.futures
    import threading
    box: dict[str, Any] = {}
    ready = threading.Event()

    def run() -> dict[str, Any]:
        client = Client(daemon.sock, role="mcp_shim", token=token, timeout=None)
        box["client"] = client
        ready.set()
        try:
            return client.call("tools.call", {"name": "owner.ask", "arguments": {"question": question}})
        finally:
            client.close()
    future = concurrent.futures.ThreadPoolExecutor(1).submit(run)
    ready.wait(5)
    return future, box["client"]


def _all_questions(daemon: RunningDaemon) -> list[str]:
    async def go() -> list[str]:
        return [ap.id for ap in daemon.daemon.state.approvals.of_request("req-1")]
    return daemon.run(go())


def test_a_retry_after_the_worker_dropped_its_call_waits_on_the_same_question(daemon: RunningDaemon) -> None:
    grant, token = gateway_setup(daemon, set(), gateway_request("tidy my downloads"), [])
    first, client = _ask_over_the_socket(daemon, token, "Which folder should I tidy?")
    [question] = _until(lambda: _pending(daemon), "the worker's question never appeared")
    client.interrupt()                                        # the runtime gave up the call: its connection closes
    _let_waiters_look_again(daemon)
    try:
        first.result(5)
    except Exception:
        pass
    assert _pending(daemon) == [question]                     # still open for the owner
    retry, _ = _ask_over_the_socket(daemon, token, "Which folder should I tidy?")
    _let_waiters_look_again(daemon)
    assert _all_questions(daemon) == [question]               # the retry waits on it: no second question

    async def answer() -> None:
        daemon.daemon._resolve_approval(question, True, text="Downloads")
    daemon.run(answer())
    assert retry.result(5)["content"]["answer"] == "Downloads"


def test_an_answer_given_while_the_worker_was_gone_reaches_its_retry(daemon: RunningDaemon) -> None:
    grant, token = gateway_setup(daemon, set(), gateway_request("tidy my downloads"), [])
    first, client = _ask_over_the_socket(daemon, token, "Which folder should I tidy?")
    [question] = _until(lambda: _pending(daemon), "the worker's question never appeared")
    client.interrupt()
    _let_waiters_look_again(daemon)

    async def answer() -> None:
        daemon.daemon._resolve_approval(question, True, text="Desktop")
    daemon.run(answer())
    retry, _ = _ask_over_the_socket(daemon, token, "Which folder should I tidy?")
    assert retry.result(5)["content"]["answer"] == "Desktop"  # the owner's answer, not a new question
    assert _all_questions(daemon) == [question]
    again, again_client = _ask_over_the_socket(daemon, token, "Which folder should I tidy?")
    _until(lambda: len(_all_questions(daemon)) == 2, "a later, separate ask must be a new question")
    again_client.interrupt()
    _until(lambda: not daemon.daemon.gateway._waiting, "the dropped call kept waiting")


def test_a_claude_worker_never_gives_up_a_silent_tool_call() -> None:
    """Claude Code (bundled 2.1.283) aborts an MCP call that sends nothing for 30 minutes unless
    CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT is 0 ("0 disables", its own message); an owner's answer may take longer (§8.1)."""
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    proxy = SimpleNamespace(base_url=lambda run_id: "http://unused")
    for mode in ("worker", "supervisor"):
        rt = ClaudeRuntime(mode, proxy, "/tmp/ferb-home", shim_cmd=["ferb", "mcp"])   # type: ignore[arg-type]
        assert rt._options("run", "token").env["CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT"] == "0"
