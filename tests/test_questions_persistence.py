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
from pathlib import Path
from tests.support.claude_home import signed_in_home

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
        rt = ClaudeRuntime(mode, proxy, signed_in_home(), shim_cmd=["ferb", "mcp"])   # type: ignore[arg-type]
        assert rt._options("run", Path("/token")).env["CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT"] == "0"


# ---------- review round 2: a call the runtime dropped silently (Codex), reworded asks, released callers ----------

def test_the_shim_names_the_runtimes_call_to_the_daemon(monkeypatch: Any) -> None:
    import anyio
    import mcp.types as t
    sent: list[dict[str, Any]] = []

    class Answering(_ShimClient):
        def call(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
            sent.append(params)
            return {"content": {"answer": "ok"}, "footer": "", "is_error": False}
    shim = _shim(monkeypatch, Answering())
    params = t.CallToolRequestParams.model_validate({"name": "owner_ask", "arguments": {},
                                                     "_meta": {"callId": "call_1", "threadId": "th"}})
    anyio.run(shim.call_tool, None, params)
    assert sent[-1]["call_id"] == "th/call_1"
    anyio.run(shim.call_tool, None, t.CallToolRequestParams(name="owner_ask", arguments={}))
    assert "call_id" not in sent[-1]                          # a runtime that names no call (Claude)


def _ask_with_call_id(daemon: RunningDaemon, token: str, question: str, call_id: str) -> tuple[Any, Any]:
    import concurrent.futures
    import threading
    box: dict[str, Any] = {}
    ready = threading.Event()

    def run() -> dict[str, Any]:
        client = Client(daemon.sock, role="mcp_shim", token=token, timeout=None)
        box["client"] = client
        ready.set()
        try:
            return client.call("tools.call", {"name": "owner.ask", "arguments": {"question": question},
                                              "call_id": call_id})
        finally:
            client.close()
    future = concurrent.futures.ThreadPoolExecutor(1).submit(run)
    ready.wait(5)
    return future, box["client"]


def test_a_call_codex_dropped_is_released_and_its_question_waits_for_the_retry(daemon: RunningDaemon) -> None:
    """Codex drops a timed-out call without telling the shim: the daemon, told by the Codex adapter, ends that call
    (its shim connection closes, so the shim's thread ends too); the question stays open and untaken, an answer given
    meanwhile stays on it, and the retry receives it."""
    from ferb.core.errors import NotRunning
    grant, token = gateway_setup(daemon, set(), gateway_request("tidy my downloads"), [])
    dropped, _ = _ask_with_call_id(daemon, token, "Which folder should I tidy?", "th/call_1")
    [question] = _until(lambda: _pending(daemon), "the worker's question never appeared")
    assert not daemon.daemon.server.abandon("another-task", "th/call_1")    # only the task's own call
    assert daemon.daemon.server.abandon(grant.task_id, "th/call_1")
    try:
        dropped.result(5)
        raise AssertionError("the dropped call was answered")
    except NotRunning:
        pass                                                    # its connection closed: the shim's thread ends
    _until(lambda: not daemon.daemon.gateway._waiting, "the released call kept waiting")

    async def answer() -> bool:
        daemon.daemon._resolve_approval(question, True, text="Downloads")
        approval = daemon.daemon.state.approvals.get(question)
        return bool(approval is not None and approval.taken)
    assert daemon.run(answer()) is False                      # nobody took it: no caller is waiting
    retry, _ = _ask_with_call_id(daemon, token, "Which folder should I tidy?", "th/call_2")
    assert retry.result(5)["content"]["answer"] == "Downloads"
    assert _all_questions(daemon) == [question]


def test_a_reworded_retry_waits_on_the_question_the_owner_already_sees(daemon: RunningDaemon) -> None:
    """The orchestrator rewords a worker's question each time it routes it: a retry is matched on the worker's own
    question and options, before any rewording, and is not routed again."""
    grant, token = gateway_setup(daemon, set(), gateway_request("tidy my downloads"), [])
    routed: list[str] = []

    async def reword(task_id: str, question: str, options: list[str]) -> dict[str, Any]:
        routed.append(question)
        return {"ask_owner": True, "question": f"Reworded {len(routed)}: {question}", "answer": "",
                "display_text": ""}
    daemon.daemon.gateway.route_question = reword
    first, client = _ask_over_the_socket(daemon, token, "Which folder should I tidy?")
    [question] = _until(lambda: _pending(daemon), "the worker's question never appeared")
    client.interrupt()
    _until(lambda: not daemon.daemon.gateway._waiting, "the dropped call kept waiting")
    retry, _ = _ask_over_the_socket(daemon, token, "Which folder should I tidy?")
    _until(lambda: daemon.daemon.gateway._waiting, "the retry never waited")
    assert _all_questions(daemon) == [question] and routed == ["Which folder should I tidy?"]

    async def answer() -> None:
        daemon.daemon._resolve_approval(question, True, text="Downloads")
    daemon.run(answer())
    assert retry.result(5)["content"]["answer"] == "Downloads"


def test_an_answer_given_as_the_call_is_released_is_never_taken_by_it(daemon: RunningDaemon) -> None:
    """The release and the owner's answer land before the waiting call looks again: the released call must not take
    the answer; it stays on the question for the next waiter."""
    grant, token = gateway_setup(daemon, set(), gateway_request("tidy my downloads"), [])
    dropped, _ = _ask_with_call_id(daemon, token, "Which folder should I tidy?", "th/call_1")
    [question] = _until(lambda: _pending(daemon), "the worker's question never appeared")

    async def release_and_answer() -> None:              # one turn of the daemon's loop: no poll in between
        assert daemon.daemon.server.abandon(grant.task_id, "th/call_1")
        daemon.daemon._resolve_approval(question, True, text="Desktop")
    daemon.run(release_and_answer())
    _until(lambda: not daemon.daemon.gateway._waiting, "the released call kept waiting")

    async def taken() -> bool:
        approval = daemon.daemon.state.approvals.get(question)
        return bool(approval is not None and approval.taken)
    assert daemon.run(taken()) is False
    retry, _ = _ask_with_call_id(daemon, token, "Which folder should I tidy?", "th/call_2")
    assert retry.result(5)["content"]["answer"] == "Desktop"
    try:
        dropped.result(5)
    except Exception:
        pass


def test_releasing_a_call_marks_it_left_at_once_and_closes_only_that_tasks_connection(tmp_path: Any) -> None:
    import asyncio
    from ferb.clock.clock import FakeClock
    from ferb.daemon.auth import RunTokens
    from ferb.daemon.server import Server

    class Writer:
        closed = False

        def close(self) -> None:
            self.closed = True
    server = Server(tmp_path / "s.sock", RunTokens(FakeClock()))
    left, writer = asyncio.Event(), Writer()
    server._shim_calls[("task-1", "th/call_1")] = [(left, writer)]        # type: ignore[list-item]
    assert not server.abandon("task-2", "th/call_1") and not left.is_set() and not writer.closed
    assert server.abandon("task-1", "th/call_1")
    assert left.is_set() and writer.closed                                 # before any waiter looks again
    assert not server.abandon("task-1", "th/call_1")                       # once


# ---------- review round 3: P19 stays exact, questions stay their task's, duplicate call ids, a lost result ----------

def _ask_args(daemon: RunningDaemon, token: str, args: dict[str, Any], call_id: str | None = None,
              name: str = "owner.ask") -> tuple[Any, Any]:
    """A worker's call as the shim makes it (its own connection, no timeout): (future, client)."""
    import concurrent.futures
    import threading
    box: dict[str, Any] = {}
    ready = threading.Event()

    def run() -> dict[str, Any]:
        client = Client(daemon.sock, role="mcp_shim", token=token, timeout=None)
        box["client"] = client
        ready.set()
        try:
            params: dict[str, Any] = {"name": name, "arguments": args}
            if call_id:
                params["call_id"] = call_id
            return client.call("tools.call", params)
        finally:
            client.close()
    future = concurrent.futures.ThreadPoolExecutor(1).submit(run)
    ready.wait(5)
    return future, box["client"]


def _owner_questions(daemon: RunningDaemon) -> list[Any]:
    # a P19 retry question is FERB's own Yes/No and carries the capability ferb.retry (Task 7, L3)
    async def go() -> list[Any]:
        return [ap for ap in daemon.daemon.state.approvals.of_request("req-1")
                if ap.capability in ("owner.ask", "ferb.retry")]
    return daemon.run(go())


def test_a_p19_yes_to_an_older_question_never_approves_a_later_attempt(ferb_yes: Any, daemon: RunningDaemon) -> None:
    """Reviewer's reproduction (round 2): the owner's Yes to "could make it happen twice", given while the worker was
    gone, must not approve a third attempt after another attempt ran meanwhile (P19, RETRY_STALE)."""
    from tests.conftest import FakeCap, gateway_tool_call
    ferb_yes.expect("floor:pay")
    boom = FakeCap("probe.pay", "pay", {"probe"}, boom=True)
    grant, token = gateway_setup(daemon, {"probe"}, gateway_request("pay x@y.com $5"), [boom])
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")             # A1: unknown
    ref = gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")["content"]["retry_action"]
    dropped, client = _ask_args(daemon, token, {"question": "Pay again?", "retry_action": ref})
    [q1] = _until(lambda: [a for a in _owner_questions(daemon) if a.state == "pending"], "no first question")
    assert "twice" in q1.reason
    client.interrupt()                                                          # the runtime gave up the call
    _until(lambda: not daemon.daemon.gateway._waiting, "the dropped call kept waiting")

    async def another_attempt() -> None:                    # meanwhile another Yes let a second attempt run
        daemon.daemon.ledger.approve_retry(ref, "span-other")
    daemon.run(another_attempt())
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")             # A2: unknown too
    assert boom.calls == 2

    async def owner_answers_the_older_question() -> None:  # nobody waits on it now
        daemon.daemon.state.approvals.resolve(q1.id, True, 1)
    daemon.run(owner_answers_the_older_question())
    retry, retry_client = _ask_args(daemon, token, {"question": "Pay again?", "retry_action": ref})
    newer = _until(lambda: [a for a in _owner_questions(daemon) if a.state == "pending"], "the newer question")
    assert [a.id for a in newer] != [q1.id] and "twice" not in newer[0].reason   # it names the attempts as they are
    assert not retry.done()                                  # the older Yes approved nothing
    retry_client.interrupt()
    _until(lambda: not daemon.daemon.gateway._waiting, "the retry kept waiting")


def test_a_sibling_task_never_takes_another_tasks_question(daemon: RunningDaemon) -> None:
    """Reviewer's reproduction (round 2), asserts flipped: two tasks of one request ask the same words; each gets its
    own question, and the owner's Yes to the first task's question never reaches the second (rule 5)."""
    from datetime import timedelta
    from ferb.core.types import Task
    from ferb.orchestrator.tree import TaskRecord
    grant, token_a = gateway_setup(daemon, set(), gateway_request("send the invoice and tidy downloads"), [])
    task_b = Task("task-2", None, "g2", "dev", Identity.OWNER, set(), "worker", "req-1", [],
                  daemon.daemon.clock.now() + timedelta(minutes=10))

    async def add_b() -> str:
        root = daemon.daemon.spans.start("task", task_id=task_b.id, input={})
        daemon.daemon.tasks.add(TaskRecord(task_b, daemon.daemon.clock.now(), root.id))
        return daemon.daemon.tokens.issue(task_b.id, "w2", set(), Identity.OWNER, ttl_s=600)
    token_b = daemon.run(add_b())
    routed: list[tuple[str, str]] = []

    async def reword(task_id: str, question: str, options: list[str]) -> dict[str, Any]:
        routed.append((task_id, question))
        ctx = {"task-1": "Send the invoice email to bob@example.com now", "task-2": "Delete the 40 old files"}[task_id]
        return {"ask_owner": True, "question": f"{ctx}?", "answer": "", "display_text": ""}
    daemon.daemon.gateway.route_question = reword
    a, client = _ask_args(daemon, token_a, {"question": "Should I go ahead?", "options": ["Yes", "No"]})
    [q] = _until(lambda: [x for x in _owner_questions(daemon) if x.state == "pending"], "no first question")
    client.interrupt()
    _until(lambda: not daemon.daemon.gateway._waiting, "the first task kept waiting")
    b, b_client = _ask_args(daemon, token_b, {"question": "Should I go ahead?", "options": ["Yes", "No"]})
    _until(lambda: daemon.daemon.gateway._waiting, "the second task never waited")

    async def owner_yes() -> None:
        daemon.daemon._resolve_approval(q.id, True, 1)
    daemon.run(owner_yes())
    _let_waiters_look_again(daemon)
    assert routed == [("task-1", "Should I go ahead?"), ("task-2", "Should I go ahead?")]
    assert len(_owner_questions(daemon)) == 2 and not b.done()   # its own question, still waiting for its answer
    b_client.interrupt()
    _until(lambda: not daemon.daemon.gateway._waiting, "the second task kept waiting")


def test_two_live_calls_with_one_call_id_can_each_be_released(daemon: RunningDaemon) -> None:
    grant, token = gateway_setup(daemon, set(), gateway_request("tidy"), [])
    _ask_args(daemon, token, {"question": "Q A?"}, call_id="th/call_1")
    _until(lambda: len(_owner_questions(daemon)) == 1, "no first question")
    _ask_args(daemon, token, {"question": "Q B?"}, call_id="th/call_1")
    _until(lambda: len(_owner_questions(daemon)) == 2, "no second question")
    assert daemon.daemon.server.abandon(grant.task_id, "th/call_1")
    assert daemon.daemon.server.abandon(grant.task_id, "th/call_1")      # the other one, too
    assert not daemon.daemon.server.abandon(grant.task_id, "th/call_1")
    _until(lambda: not daemon.daemon.gateway._waiting, "a released call kept waiting")


def test_an_action_that_finished_after_its_caller_left_isnt_run_again_by_the_retry(daemon: RunningDaemon) -> None:
    """An action that outlasts the runtime's own per-call timeout still finishes (Codex: 300 s) and its result goes
    nowhere. The same task's identical retry (same tool, same arguments) gets that result, marked as from the earlier
    attempt, instead of a second send."""
    import asyncio as aio
    from tests.conftest import FakeCap

    class SlowSend(FakeCap):
        release: Any = None

        async def run(self, args: Any, ctx: Any) -> Any:
            await self.release.wait()
            return await super().run(args, ctx)
    send = SlowSend("probe.send", "send", {"probe"}, content={"sent": True})
    grant, token = gateway_setup(daemon, {"probe"}, gateway_request("send hi to alice@x.com"), [send])

    async def event() -> aio.Event:
        return aio.Event()
    send.release = daemon.run(event())
    args = {"to": "alice@x.com", "text": "hi"}
    dropped, _ = _ask_args(daemon, token, args, call_id="th/call_1", name="probe.send")
    _until(lambda: daemon.daemon.gateway.in_flight({grant.task_id}), "the send never started")
    assert daemon.daemon.server.abandon(grant.task_id, "th/call_1")      # the runtime gave up on it
    daemon.call_soon(send.release.set)                                   # it finishes anyway
    _until(lambda: not daemon.daemon.gateway.in_flight({grant.task_id}), "the send never finished")
    retry, _ = _ask_args(daemon, token, args, call_id="th/call_2", name="probe.send")
    got = retry.result(5)
    assert send.calls == 1, "the identical retry sent it again"
    assert got["content"]["sent"] is True and got["content"]["completed_in_an_earlier_attempt"] is True
    again, _ = _ask_args(daemon, token, args, name="probe.send")         # a later, separate send runs as usual
    assert again.result(5)["content"].get("completed_in_an_earlier_attempt") is None and send.calls == 2


def test_a_reused_p19_question_settles_against_the_attempts_it_named(ferb_yes: Any, daemon: RunningDaemon) -> None:
    """Reviewer's probe (round 3): A1 is settled failed by reconcile and an identical A2 ends unknown, so FERB's retry
    question reads exactly as before and the retry waits on Q1. The owner's Yes to Q1 named A1 alone: settled against
    that stored list it approves nothing (RETRY_STALE), never the attempt list as it is now."""
    from ferb.daemon.gateway import RETRY_STALE
    from tests.conftest import FakeCap, gateway_tool_call
    ferb_yes.expect("floor:pay", 2)
    boom = FakeCap("probe.pay", "pay", {"probe"}, boom=True)
    grant, token = gateway_setup(daemon, {"probe"}, gateway_request("pay x@y.com $5"), [boom])
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")             # A1: unknown
    ref = gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")["content"]["retry_action"]
    dropped, client = _ask_args(daemon, token, {"question": "Pay again?", "retry_action": ref})
    [q1] = _until(lambda: [a for a in _owner_questions(daemon) if a.state == "pending"], "no first question")
    client.interrupt()
    _until(lambda: not daemon.daemon.gateway._waiting, "the dropped call kept waiting")

    async def settle_a1_failed() -> None:
        daemon.daemon.ledger.reconcile(ref, "failed", ["probe-evidence"])
    daemon.run(settle_a1_failed())
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")             # A2: unknown
    assert boom.calls == 2

    async def owner_yes_to_q1() -> None:
        daemon.daemon.state.approvals.resolve(q1.id, True, 1)
    daemon.run(owner_yes_to_q1())
    retry, _ = _ask_args(daemon, token, {"question": "Pay again?", "retry_action": ref})
    got = retry.result(5)["content"]
    assert [a.id for a in _owner_questions(daemon)] == [q1.id]             # the same words: it waited on Q1
    assert got.get("retry_approved") is False and got.get("why_not") == RETRY_STALE, got


def test_a_result_finishing_after_its_task_ended_is_not_kept(daemon: RunningDaemon) -> None:
    """Review minor M1: the task ends (as the supervisor ends it) while the abandoned action still runs; when it then
    finishes, nothing is kept for a retry that can't come (or a later resumed plan)."""
    import asyncio as aio
    from tests.conftest import FakeCap

    class SlowSend(FakeCap):
        release: Any = None

        async def run(self, args: Any, ctx: Any) -> Any:
            await self.release.wait()
            return await super().run(args, ctx)
    send = SlowSend("probe.send", "send", {"probe"}, content={"sent": True})
    grant, token = gateway_setup(daemon, {"probe"}, gateway_request("send hi to alice@x.com"), [send])

    async def event() -> aio.Event:
        return aio.Event()
    send.release = daemon.run(event())
    _ask_args(daemon, token, {"to": "alice@x.com", "text": "hi"}, call_id="th/call_1", name="probe.send")
    _until(lambda: daemon.daemon.gateway.in_flight({grant.task_id}), "the send never started")
    assert daemon.daemon.server.abandon(grant.task_id, "th/call_1")

    def task_ends() -> None:                                 # supervisor: forget_tasks, then the task leaves the tree
        daemon.daemon.gateway.forget_tasks({grant.task_id})
        daemon.daemon.tasks.remove(grant.task_id)
    daemon.call_soon(task_ends)
    daemon.call_soon(send.release.set)
    _until(lambda: not daemon.daemon.gateway.in_flight({grant.task_id}), "the send never finished")
    assert send.calls == 1 and daemon.daemon.gateway._undelivered == {}
