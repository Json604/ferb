"""A model run that doesn't hand over its answer (RUN-445, RUN-446, ORCH-558; the 2026-11-08 Sonnet planner turns that
ended empty, 28 of 80, and the two that called the made-up tool `x` about 850 times each).

Adapter contracts with the response shapes recorded in that experiment (an end_turn with no blocks billing 459 tokens,
a tool_use stop with no tool block billing 23, a tool_use stop with only an empty thinking block): one follow-up in the
same conversation, then an answer or an error marked `empty`; each usage report says how the call ended. End to end
through the real daemon and gateway with FERB's own API loop over a scripted transport: the planner recovers through
the follow-up, a loop of wrong calls stops at the cap and the planner still answers from one last tool-free call, and
a worker's loop ends its attempt with the reason. Offline: no model is called."""
import asyncio
import json
from types import SimpleNamespace
from typing import Any

import httpx
import pytest

from ferb.core.types import Identity, ModelLimits, ModelRequest, Task
from ferb.orchestrator import schemas as core
from ferb.runtimes.answer_rules import FOLLOW_UP, WRONG_CALLS_CAP
from ferb.runtimes.anthropic_api import AnthropicProvider
from ferb.runtimes.api import ApiRuntime, GatewayBridge
from ferb.runtimes.google_api import GoogleProvider
from ferb.runtimes.openai_api import OpenAIProvider
from ferb.runtimes.schema_shape import Refusals
from tests.conftest import RunningDaemon, chat, install, sup, task, verdict
from tests.support.claude_home import signed_in_home
from tests.support.runtime_fakes import FakeBridge, FakeProvider, Process, Scripted
from tests.test_schema_shape import sample

from ferb.runtimes.claude_sdk import ClaudeRuntime as _Claude  # noqa: E402
CLAUDE_START = _Claude.start             # saved before the offline fixture replaces it (module code runs first)

TASK = Task("t.empty", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
REQ = ModelRequest("FERB instructions", ("stable facts", "the message"), 1)
SCHEMA = {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"],
          "additionalProperties": False}
ANSWER = {"text": "Hello"}


async def key(name: str) -> str:
    return "sk-test"


# ---------- Anthropic: the recorded empty shapes, streamed as the planner's calls are ----------

def _start(output_tokens: int = 1) -> dict[str, Any]:
    return {"type": "message_start", "message": {"id": "msg_1", "role": "assistant", "content": [], "usage": {
        "input_tokens": 2, "cache_read_input_tokens": 39558, "output_tokens": output_tokens}}}


def _stop(reason: str, output_tokens: int) -> list[dict[str, Any]]:
    return [{"type": "message_delta", "delta": {"stop_reason": reason, "stop_details": None},
             "usage": {"output_tokens": output_tokens, "output_tokens_details": {"thinking_tokens": 0}}},
            {"type": "message_stop"}]


THINKING = {"type": "thinking", "thinking": "", "signature": "sig" * 8}
EMPTY = {        # the shapes the experiment's raw streams showed (empty-reply-exp, arm A)
    "end_turn_no_blocks": ([_start(), *_stop("end_turn", 459)], []),
    "tool_use_no_block": ([_start(), *_stop("tool_use", 23)], []),
    "tool_use_thinking_only": ([_start(), {"type": "content_block_start", "index": 0,
                                           "content_block": {"type": "thinking", "thinking": ""}},
                                {"type": "content_block_delta", "index": 0,
                                 "delta": {"type": "signature_delta", "signature": THINKING["signature"]}},
                                {"type": "content_block_stop", "index": 0}, *_stop("tool_use", 41)], [THINKING]),
}
ANSWERED = [_start(), {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
            {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": json.dumps(ANSWER)}},
            {"type": "content_block_stop", "index": 0}, *_stop("end_turn", 12)]


class Streams:
    """Answers each request with the next recorded stream (SSE), and keeps each request's body."""
    def __init__(self, streams: list[list[dict[str, Any]]]) -> None:
        self.streams, self.bodies = list(streams), []

    def transport(self) -> httpx.MockTransport:
        async def handle(request: httpx.Request) -> httpx.Response:
            self.bodies.append(json.loads(request.content))
            data = "".join("data: " + json.dumps(e) + "\n\n" for e in self.streams.pop(0)).encode()
            return httpx.Response(200, headers={"content-type": "text/event-stream"}, content=data)
        return httpx.MockTransport(handle)


def api(provider: Any, transport: httpx.AsyncBaseTransport, mode: str = "supervisor", bridge: Any = None,
        tools: list[str] | None = None, schema: dict[str, Any] | None = SCHEMA) -> ApiRuntime:
    return ApiRuntime(provider, mode=mode, role="orchestrator", tool_names=tools or [], output_schema=schema,  # type: ignore[arg-type]
                      bridge=bridge, key=key, transport=transport, refusals=Refusals())


async def events_of(rt: ApiRuntime) -> list[Any]:
    handle = await rt.start(TASK, REQ, "")
    return [e for e in [e async for e in rt.events(handle)] if e.kind != "progress"]


def usage(events: list[Any]) -> list[tuple[str, list[str], str]]:
    return [(e.data["stop"], e.data["blocks"], e.data.get("follow_up", "")) for e in events if e.kind == "usage"]


# decision: RUN-446, PRM-334
@pytest.mark.anyio
@pytest.mark.parametrize("shape", sorted(EMPTY))
async def test_an_empty_anthropic_answer_gets_one_follow_up_in_the_same_conversation(shape: str) -> None:
    stream, returned = EMPTY[shape]
    streams = Streams([stream, ANSWERED])
    events = await events_of(api(AnthropicProvider("claude-sonnet-5-5"), streams.transport()))
    assert json.loads(next(e for e in events if e.kind == "final").data["text"]) == ANSWER
    assert not any(e.kind == "error" for e in events)
    assistant, follow = streams.bodies[1]["messages"][-2:]
    assert assistant == {"role": "assistant", "content": returned}        # the empty turn stays as it came back
    assert follow == {"role": "user", "content": [{"type": "text", "text": FOLLOW_UP}]}
    stop = "end_turn" if shape.startswith("end_turn") else "tool_use"
    assert usage(events) == [(stop, [b["type"] for b in returned], "asked"), ("end_turn", ["text"], "fixed")]

    twice = Streams([stream, stream])                                    # empty again: the run ends, marked
    events = await events_of(api(AnthropicProvider("claude-sonnet-5-5"), twice.transport()))
    assert len(twice.bodies) == 2 and events[-1].kind == "error" and events[-1].data["empty"] is True
    assert [u[2] for u in usage(events)] == ["asked", "failed"]
    assert not any(e.kind == "final" for e in events)


# decision: RUN-446
@pytest.mark.anyio
async def test_an_empty_openai_answer_with_only_reasoning_gets_the_follow_up() -> None:
    reasoning = {"type": "reasoning", "id": "rs_1", "summary": []}
    script = Scripted([(200, {"status": "completed", "output": [reasoning], "usage": {"output_tokens": 30}}),
                       (200, {"status": "completed", "usage": {"output_tokens": 9}, "output": [
                           {"type": "message", "content": [{"type": "output_text", "text": json.dumps(ANSWER)}]}]})])
    events = await events_of(api(OpenAIProvider("gpt-6.1-sol"), script.transport()))
    assert json.loads(next(e for e in events if e.kind == "final").data["text"]) == ANSWER
    sent = script.requests[1][1]["input"]
    assert sent[-2] == reasoning and sent[-1]["role"] == "user" and FOLLOW_UP in json.dumps(sent[-1])
    assert usage(events) == [("completed", ["reasoning"], "asked"), ("completed", ["message"], "fixed")]


# decision: RUN-446
@pytest.mark.anyio
async def test_an_empty_gemini_answer_is_not_sent_back_and_the_follow_up_comes_after_the_last_user_turn() -> None:
    empty = {"candidates": [{"index": 0, "content": {"role": "model", "parts": []}, "finishReason": "STOP"}],
             "usageMetadata": {"candidatesTokenCount": 0}}
    answer = {"candidates": [{"index": 0, "content": {"role": "model", "parts": [{"text": json.dumps(ANSWER)}]},
                              "finishReason": "STOP"}], "usageMetadata": {"candidatesTokenCount": 5}}
    script = Scripted([(200, empty), (200, answer)])
    events = await events_of(api(GoogleProvider("gemini-3.8-flash"), script.transport()))
    assert json.loads(next(e for e in events if e.kind == "final").data["text"]) == ANSWER
    contents = script.requests[1][1]["contents"]
    assert [c["role"] for c in contents] == ["user", "user"]             # no model turn with no parts
    assert contents[-1]["parts"] == [{"text": FOLLOW_UP}]
    assert usage(events) == [("STOP", [], "asked"), ("STOP", ["text"], "fixed")]


# decision: RUN-445
@pytest.mark.anyio
async def test_a_made_up_tool_is_reported_as_not_offered_refused_and_never_run() -> None:
    made_up = {"content": [{"type": "tool_use", "id": "tu_1", "name": "x", "input": {}}], "stop_reason": "tool_use",
               "usage": {"output_tokens": 24}}
    answer = {"content": [{"type": "text", "text": json.dumps(ANSWER)}], "stop_reason": "end_turn",
              "usage": {"output_tokens": 9}}
    bridge = FakeBridge()
    script = Scripted([(200, made_up), (200, answer)])
    rt = api(AnthropicProvider("claude-sonnet-5-5"), script.transport(), mode="worker", bridge=bridge,
             tools=["fs.list"])
    handle = await rt.start(TASK, REQ, "token")
    events = [e async for e in rt.events(handle)]
    assert [e.data for e in events if e.kind == "tool_call"] == [{"tool": "x", "arguments": {}, "offered": False}]
    assert bridge.calls == []                                            # nothing ran
    result = script.requests[1][1]["messages"][-1]["content"][0]
    assert result["type"] == "tool_result" and result["is_error"] is True
    assert events[-1].kind == "final"
    assert usage(events)[0] == ("tool_use", ["tool_use"], "")


# ---------- Claude Agent SDK: the same follow-up through Claude Code's own input ----------

class Client:
    """Claude Code as the SDK client: each query (the request, then each follow-up) is answered with the next scripted
    turn: its messages, Claude Code's replay of the user message, then its result."""
    made: list["Client"] = []
    turns: list[list[Any]] = []

    def __init__(self, options: Any) -> None:
        self.options, self.queue, self.sent = options, asyncio.Queue(), []
        Client.made.append(self)

    async def connect(self) -> None:
        pass

    async def query(self, frames: Any) -> None:
        from claude_agent_sdk import UserMessage
        got = [f async for f in frames]
        self.sent.extend(got)
        for frame in got:
            if frame.get("uuid"):
                self.queue.put_nowait(UserMessage(frame["message"]["content"], uuid=frame["uuid"]))
        for message in Client.turns.pop(0):
            self.queue.put_nowait(message)

    async def receive_messages(self) -> Any:
        while True:
            yield await self.queue.get()

    async def disconnect(self) -> None:
        pass


def claude(monkeypatch: pytest.MonkeyPatch, tools: list[str] | None = None) -> _Claude:
    import claude_agent_sdk
    monkeypatch.setattr(claude_agent_sdk, "ClaudeSDKClient", Client)
    proxy = SimpleNamespace(seen=lambda ident: set(), forget=lambda ident: None, base_url=lambda ident: "http://unused")
    return _Claude("supervisor", proxy, signed_in_home(), tool_names=tools,  # type: ignore[arg-type]
                   output_schema=SCHEMA, model="sonnet", refusals=Refusals())


# decision: RUN-446, RUN-445
@pytest.mark.anyio
async def test_claude_code_ending_without_an_answer_gets_the_follow_up_once(monkeypatch: pytest.MonkeyPatch) -> None:
    from claude_agent_sdk import AssistantMessage, ResultMessage, ToolUseBlock
    rt = claude(monkeypatch)
    Client.made, Client.turns = [], [
        [AssistantMessage([ToolUseBlock("tu_1", "x", {})], "sonnet"),
         ResultMessage("success", 1, 1, False, 1, "s", stop_reason="end_turn", result="")],
        [ResultMessage("success", 1, 1, False, 1, "s", stop_reason="end_turn", structured_output=ANSWER)]]
    handle = await CLAUDE_START(rt, TASK, REQ, "")
    events = [e async for e in rt.events(handle)]
    assert [e.data for e in events if e.kind == "tool_call"] == [{"tool": "x", "arguments": {}, "offered": False}]
    assert Client.made[0].sent[-1]["message"]["content"] == FOLLOW_UP
    assert json.loads(next(e for e in events if e.kind == "final").data["text"]) == ANSWER
    assert usage(events) == [("end_turn", ["tool_use"], "asked"), ("end_turn", [], "fixed")]

    rt = claude(monkeypatch)
    empty = ResultMessage("success", 1, 1, False, 1, "s", stop_reason="end_turn", result="")
    Client.made, Client.turns = [], [[empty], [empty]]
    handle = await CLAUDE_START(rt, TASK, REQ, "")
    events = [e async for e in rt.events(handle)]
    assert events[-1].kind == "error" and events[-1].data["empty"] is True
    assert [u[2] for u in usage(events)] == ["asked", "failed"]


# ---------- Codex: a turn completed with no answer ----------

# decision: RUN-446
@pytest.mark.anyio
async def test_a_codex_turn_completed_without_an_answer_is_marked_empty(tmp_path: Any,
                                                                       monkeypatch: pytest.MonkeyPatch) -> None:
    from tests.test_codex_streaming import begin
    proc = Process(phase="commentary")                # only commentary: the turn completes with no final answer
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    proc.complete()
    got = [ev async for ev in stream.events(rt, run)]
    assert got[-1].kind == "error" and got[-1].data.get("empty") is True

    failed = Process(fail=True)                       # a failed turn stays its own error
    (tmp_path / "failed").mkdir()
    stream, rt, run, _ = await begin(tmp_path / "failed", monkeypatch, failed)
    failed.complete()
    got = [ev async for ev in stream.events(rt, run)]
    assert got[-1].kind == "error" and "empty" not in got[-1].data


# ---------- End to end: the real daemon and gateway, FERB's own API loop, a scripted transport ----------

class Provider(FakeProvider):
    limits = ModelLimits("fake-model", 1_000_000, 100_000, "test double")


REPLY = sample(core.SUPERVISOR_SCHEMA, False) | {"kind": "reply", "text": "Your dentist is Dr. Rao."}


def text(value: Any) -> tuple[int, dict[str, Any]]:
    return 200, {"text": value if isinstance(value, str) else json.dumps(value)}


def calls(*named: tuple[str, dict[str, Any]]) -> tuple[int, dict[str, Any]]:
    return 200, {"calls": [[f"c{i}", name, args] for i, (name, args) in enumerate(named)]}


def through_api(d: RunningDaemon, planner: Scripted | None, worker: Scripted | None = None,
                script_workers: dict[str, Any] | None = None, supervisor: list[Any] | None = None) -> Any:
    """The planner (and the worker, when given) run FERB's own API loop through the daemon's gateway; every other role
    stays the scripted runtime."""
    script = install(d, supervisor or [], script_workers or {})
    scripted = d.daemon.supervisor.make_runtime
    bridge = GatewayBridge(d.daemon.gateway, d.daemon.tokens)

    async def no_wait(seconds: float) -> None:
        return None

    def make(role: str, *, mode: str, tool_names: list[str], schema: Any, effort: Any = None) -> Any:
        own = planner if role == "orchestrator" else worker if role.startswith("worker") else None
        if own is None:
            return scripted(role, mode=mode, tool_names=tool_names, schema=schema, effort=effort)
        return ApiRuntime(Provider(), mode=mode, role=role, tool_names=tool_names,  # type: ignore[arg-type]
                          output_schema=schema, bridge=bridge, key=key, transport=own.transport(), sleep=no_wait)
    d.daemon.supervisor.make_runtime = make
    return script


def spans(d: RunningDaemon, kind: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [{"input": json.loads(r["input_json"] or "{}"), "output": json.loads(r["output_json"] or "{}"),
                 "error": r["error"]} for r in d.daemon.db.conn.execute(
            "SELECT input_json, output_json, error FROM spans WHERE kind = ? ORDER BY rowid", (kind,))]
    return d.run(go())                    # type: ignore[no-any-return]


def replies(events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [e for e in events if e["type"] == "reply"]


# decision: RUN-446, ORCH-558
def test_a_planner_that_goes_empty_after_a_lookup_answers_on_the_follow_up(daemon: RunningDaemon) -> None:
    planner = Scripted([calls(("memory_read", {"query": "dentist"})), text(""), text(REPLY)])
    through_api(daemon, planner)
    events = chat(daemon, "who is my dentist?")
    assert [r["text"] for r in replies(events)] == [REPLY["text"]] and "status" not in replies(events)[0]
    assert len(planner.requests) == 3
    assert planner.requests[2][1]["updates"] == [FOLLOW_UP]               # asked once, in the same conversation
    assert [u["output"].get("follow_up") for u in spans(daemon, "model.usage")] == [None, "asked", "fixed"]
    assert all("last_call" not in t["input"] for t in spans(daemon, "model.turn"))   # no last call was needed


# decision: RUN-445, ORCH-558
def test_a_planner_looping_on_a_made_up_tool_stops_at_the_cap_and_answers_from_one_last_call(
        daemon: RunningDaemon) -> None:
    planner = Scripted([calls(("x", {})) for _ in range(WRONG_CALLS_CAP)] + [text(REPLY)])
    through_api(daemon, planner)
    events = chat(daemon, "who is my dentist?")
    assert [r["text"] for r in replies(events)] == [REPLY["text"]]
    assert len(planner.requests) == WRONG_CALLS_CAP + 1                  # nothing more after the cap
    last = planner.requests[-1][1]
    assert last["tools"] == [] and "updates" not in last and last["results"] == []   # a fresh call, no tools
    [loop] = spans(daemon, "model.tool_loop")
    assert loop["output"]["wrong_calls"] == WRONG_CALLS_CAP and loop["output"]["made_up"] == WRONG_CALLS_CAP
    assert [t["input"].get("last_call") for t in spans(daemon, "model.turn")] == [None, "tool_loop"]


# decision: RUN-445, ORCH-558, ORCH-516
def test_lookups_refused_after_they_are_spent_count_toward_the_cap(daemon: RunningDaemon) -> None:
    lookups = [calls(("memory_read", {"query": f"topic {i}"})) for i in range(10 + WRONG_CALLS_CAP)]
    planner = Scripted(lookups + [text(REPLY)])
    through_api(daemon, planner)
    events = chat(daemon, "what do you remember about me?")
    assert [r["text"] for r in replies(events)] == [REPLY["text"]]
    assert len(planner.requests) == 10 + WRONG_CALLS_CAP + 1             # 10 ran, WRONG_CALLS_CAP refused, last call
    [loop] = spans(daemon, "model.tool_loop")
    assert loop["output"]["refused_lookups"] == WRONG_CALLS_CAP and loop["output"]["made_up"] == 0
    assert planner.requests[-1][1]["tools"] == []
    assert not daemon.daemon.gateway._refused_over                      # the count went with the call


# decision: RUN-446, ORCH-558
def test_a_planner_empty_twice_and_empty_again_on_its_last_call_fails_the_turn_plainly(daemon: RunningDaemon) -> None:
    planner = Scripted([text(""), text(""), text(""), text("")])
    through_api(daemon, planner)
    events = chat(daemon, "who is my dentist?")
    [reply] = replies(events)
    assert reply["status"] == "failed" and "without an answer" in reply["text"]
    assert len(planner.requests) == 4                                    # two calls, each with its one follow-up
    assert [t["input"].get("last_call") for t in spans(daemon, "model.turn")] == [None, "empty"]


# decision: RUN-445
def test_a_worker_looping_on_a_made_up_tool_ends_its_attempt_with_the_reason(daemon: RunningDaemon) -> None:
    worker = Scripted([calls(("x", {})) for _ in range(WRONG_CALLS_CAP)])
    through_api(daemon, None, worker, {"worker.verify": verdict(False, next="fail", reason="stopped")},
                [sup("task", "On it.", task("Tidy the desktop"))])
    chat(daemon, "tidy my desktop")
    assert len(worker.requests) == WRONG_CALLS_CAP                        # stopped at the cap, nothing after
    [loop] = spans(daemon, "model.tool_loop")
    assert loop["output"]["made_up"] == WRONG_CALLS_CAP
    [run] = [s for s in spans(daemon, "worker") if "kept calling tools" in json.dumps(s["output"])]
    assert run["output"]["status"] == "failed" and "ToolLoop" in (run["error"] or "")
