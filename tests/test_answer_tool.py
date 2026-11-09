"""The planner's answer tool where its typed format isn't used (RUN-447; the 2026-11-08 measurement: with a non-strict
`ferb_answer` tool whose input schema is the core answer schema, 0 of 135 first calls ended empty, against 54 of 135
with the schema stated in the instructions).

Adapter contracts with recorded stream shapes (Anthropic, OpenAI, Gemini): a call to the answer tool is the answer and
streams as the reply's draft from the call's start; a text answer is still accepted; an answer outside the form is told
back once through the call's own result and the other calls of that reply never run; an empty call still gets the one
follow-up; the answer tool is never a wrong call and never reaches the gateway; it is offered only where the form is
stated. End to end through the real daemon and gateway with FERB's API loop and the Anthropic adapter on a scripted
transport: the planner answers through the tool after a lookup, and a follow-up question is answered from a lookup of
the earlier answer. Nothing here pins prompt wording. Offline: no model is called."""
import json
from typing import Any

import httpx
import pytest

from ferb.core.structured import check_final
from ferb.core.types import Identity, ModelRequest, Task
from ferb.orchestrator import schemas as core
from ferb.orchestrator.schemas import SupervisorOutcome
from ferb.runtimes.answer_rules import ANSWER_TOOL, FOLLOW_UP, WRONG_CALLS_CAP
from ferb.runtimes.anthropic_api import AnthropicProvider
from ferb.runtimes.api import ApiRuntime, GatewayBridge, inline_refs
from ferb.runtimes.google_api import GoogleProvider
from ferb.runtimes.openai_api import OpenAIProvider
from ferb.runtimes.schema_shape import Refusals, for_gemini
from tests.conftest import RunningDaemon, chat, install
from tests.support.runtime_fakes import FakeBridge
from tests.test_answer_rules import EMPTY, _start, _stop
from tests.test_schema_shape import sample
from tests.test_typed_fallback import GRAMMAR

TASK = Task("t.answer", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
REQ = ModelRequest("FERB instructions", ("stable facts", "the message"), 1)
SCHEMA = core.SUPERVISOR_SCHEMA


def reply(text: str) -> dict[str, Any]:
    """A plain reply in the core form, its text last (as the schema orders it), so the draft can stream it."""
    value = sample(SCHEMA, False) | {
        "kind": "reply", "show": [], "about_tasks": [],
        **{k: [] for k in ("remember", "timers", "schedules", "schedule_changes", "reminder_cancels", "forget",
                           "conversation_changes")}}
    return {**{k: v for k, v in value.items() if k != "text"}, "text": text}


ANSWER = reply("Your dentist is Dr. Rao.")
LOOKUP_TOOL = {"name": "memory.read", "description": "Read memory.",
               "input_schema": {"type": "object", "properties": {"query": {"type": "string"}}}}


async def key(name: str) -> str:
    return "sk-test"


def sse(events: list[dict[str, Any]]) -> httpx.Response:
    data = "".join("data: " + json.dumps(e) + "\n\n" for e in events).encode()
    return httpx.Response(200, headers={"content-type": "text/event-stream"}, content=data)


class Wire:
    """Answers each request with the next item: a list of stream events (SSE), or (status, JSON body). Keeps each
    request's body."""
    def __init__(self, items: list[Any]) -> None:
        self.items, self.bodies = list(items), []

    def transport(self) -> httpx.MockTransport:
        async def handle(request: httpx.Request) -> httpx.Response:
            self.bodies.append(json.loads(request.content))
            item = self.items.pop(0)
            return sse(item) if isinstance(item, list) else httpx.Response(item[0], json=item[1])
        return httpx.MockTransport(handle)


# ---------- Anthropic streams ----------

def tool_block(index: int, name: str, value: Any, chunk: int = 23) -> list[dict[str, Any]]:
    raw = value if isinstance(value, str) else json.dumps(value)
    return [{"type": "content_block_start", "index": index,
             "content_block": {"type": "tool_use", "id": f"tu_{index}", "name": name, "input": {}}},
            *[{"type": "content_block_delta", "index": index,
               "delta": {"type": "input_json_delta", "partial_json": raw[i:i + chunk]}}
              for i in range(0, len(raw), chunk)],
            {"type": "content_block_stop", "index": index}]


def text_block(index: int, text: str) -> list[dict[str, Any]]:
    return [{"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}},
            {"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": text}},
            {"type": "content_block_stop", "index": index}]


def answered(value: Any, before: str = "", also: list[tuple[str, Any]] | None = None) -> list[dict[str, Any]]:
    """A reply that calls the answer tool (after `before` text, and with `also`'s calls after it)."""
    blocks = text_block(0, before) if before else []
    blocks += tool_block(len(blocks) and 1, ANSWER_TOOL, value)
    for i, (name, args) in enumerate(also or []):
        blocks += tool_block(2 + i, name, args)
    return [_start(), *blocks, *_stop("tool_use", 700)]


def text_answer(value: Any) -> list[dict[str, Any]]:
    return [_start(), *text_block(0, json.dumps(value)), *_stop("end_turn", 600)]


def looked_up(name: str, args: dict[str, Any]) -> list[dict[str, Any]]:
    return [_start(), *tool_block(0, name, args), *_stop("tool_use", 40)]


def runtime(provider: Any, wire: Wire, *, stated: bool = True, planner: bool = True, tools: list[str] | None = None,
            bridge: Any = None, refusals: Refusals | None = None) -> ApiRuntime:
    refusals = refusals if refusals is not None else Refusals()
    rt = ApiRuntime(provider, mode="supervisor", role="orchestrator", tool_names=tools or [], output_schema=SCHEMA,
                    bridge=bridge, key=key, transport=wire.transport(), refusals=refusals, planner=planner)
    if stated:
        refusals.remember(rt.format_key)          # the provider refused the typed format earlier (RUN-427)
    return rt


async def events_of(rt: ApiRuntime, token: str = "") -> list[Any]:
    handle = await rt.start(TASK, REQ, token)
    return [e for e in [e async for e in rt.events(handle)] if e.kind != "progress"]


def final(events: list[Any]) -> Any:
    return json.loads(next(e for e in events if e.kind == "final").data["text"])


def drafted(events: list[Any]) -> str:
    """The text the reply's draft is fed: what streamed after the last restart."""
    out = ""
    for e in events:
        if e.kind == "text_delta":
            out = "" if e.data.get("restart") else out + e.data["text"]
    return out


def via(events: list[Any]) -> list[str]:
    return [e.data.get("answer_via", "") for e in events if e.kind == "usage"]


def offered(body: dict[str, Any]) -> list[str]:
    tools = body.get("tools") or []
    return [t.get("name") for t in tools] + [d["name"] for t in tools for d in t.get("functionDeclarations", [])]


# decision: RUN-447, PRM-336, PRM-337
@pytest.mark.anyio
async def test_the_answer_tools_call_is_the_answer_and_streams_as_the_draft_from_its_start() -> None:
    wire = Wire([answered(ANSWER, before="Let me answer that.")])
    events = await events_of(runtime(AnthropicProvider("claude-sonnet-5-5"), wire))
    assert final(events) == ANSWER and SupervisorOutcome.model_validate(final(events))
    body = wire.bodies[0]
    [tool] = [t for t in body["tools"] if t["name"] == ANSWER_TOOL]
    assert tool["input_schema"] == inline_refs(SCHEMA) and "strict" not in tool      # the core schema, not enforced
    assert body["tool_choice"] == {"type": "auto"} and "format" not in body.get("output_config", {})
    assert json.dumps(SCHEMA["properties"]["kind"]) not in body["system"][-1]["text"]   # no stated schema beside it
    assert body["system"][-1]["cache_control"] == {"type": "ephemeral", "ttl": "1h"}     # still cached
    # the draft gets the call's arguments from its start; the text before it was withdrawn (restart)
    assert json.loads(drafted(events)) == ANSWER
    restart = next(i for i, e in enumerate(events) if e.data.get("restart"))
    assert "Let me answer" in "".join(e.data.get("text", "") for e in events[:restart] if e.kind == "text_delta")
    assert not any(e.kind == "tool_call" for e in events)                    # never a tool call, never a wrong one
    assert via(events) == ["tool"]


# decision: RUN-447
@pytest.mark.anyio
async def test_a_text_answer_is_still_accepted_with_the_tool_offered() -> None:
    wire = Wire([text_answer(ANSWER)])
    events = await events_of(runtime(AnthropicProvider("claude-sonnet-5-5"), wire))
    assert final(events) == ANSWER and ANSWER_TOOL in offered(wire.bodies[0])
    assert json.loads(drafted(events)) == ANSWER and via(events) == ["text"]


# decision: RUN-447, RUN-440
@pytest.mark.anyio
async def test_an_answer_outside_the_form_is_told_back_once_and_the_calls_beside_it_never_run() -> None:
    bridge = FakeBridge([LOOKUP_TOOL])
    wire = Wire([answered({"kind": "reply"}, also=[("memory_read", {"query": "dentist"})]), answered(ANSWER)])
    rt = runtime(AnthropicProvider("claude-sonnet-5-5"), wire, tools=["memory.read"], bridge=bridge)
    events = await events_of(rt, "token")
    assert final(events) == ANSWER
    assert bridge.calls == []                                            # the lookup beside the answer didn't run
    results = wire.bodies[1]["messages"][-1]["content"]
    assert [r["tool_use_id"] for r in results] == ["tu_0", "tu_2"] and all(r["is_error"] for r in results)
    assert "doesn't fit" in results[0]["content"][0]["text"]            # the problem, as the answer call's result
    assert [e.data.get("repair") for e in events if e.kind == "usage"] == ["asked", "fixed"]
    assert via(events) == ["tool", "tool"] and not any(e.kind == "tool_call" for e in events)

    twice = Wire([answered({"kind": "reply"}), answered({"kind": "reply"})])     # a second miss: the caller's check
    events = await events_of(runtime(AnthropicProvider("claude-sonnet-5-5"), twice))
    assert len(twice.bodies) == 2
    with pytest.raises(ValueError):
        check_final(next(e for e in events if e.kind == "final").data["text"], SCHEMA)


# decision: RUN-447, RUN-446
@pytest.mark.anyio
@pytest.mark.parametrize("shape", sorted(EMPTY))
async def test_an_empty_call_still_gets_the_follow_up_with_the_tool_offered(shape: str) -> None:
    stream, returned = EMPTY[shape]
    wire = Wire([stream, answered(ANSWER)])
    events = await events_of(runtime(AnthropicProvider("claude-sonnet-5-5"), wire))
    assert final(events) == ANSWER and json.loads(drafted(events)) == ANSWER
    assistant, follow = wire.bodies[1]["messages"][-2:]
    assert assistant == {"role": "assistant", "content": returned}
    assert follow == {"role": "user", "content": [{"type": "text", "text": FOLLOW_UP}]}
    assert [e.data.get("follow_up") for e in events if e.kind == "usage"] == ["asked", "fixed"]


# decision: RUN-447, RUN-445
@pytest.mark.anyio
async def test_the_answer_tool_is_never_a_wrong_call_and_never_reaches_the_gateway() -> None:
    bridge = FakeBridge([LOOKUP_TOOL])
    wire = Wire([looked_up("x", {}), looked_up("memory_read", {"query": "dentist"}), answered(ANSWER)])
    rt = runtime(AnthropicProvider("claude-sonnet-5-5"), wire, tools=["memory.read"], bridge=bridge)
    events = await events_of(rt, "token")
    assert final(events) == ANSWER
    assert [(e.data["tool"], e.data.get("offered", True)) for e in events if e.kind == "tool_call"] == [
        ("x", False), ("memory_read", True)]                              # only the made-up name counts
    assert bridge.calls == [("memory.read", {"query": "dentist"})]


# decision: RUN-447
@pytest.mark.anyio
async def test_only_a_stated_planner_gets_the_tool_and_a_refusal_offers_it_on_the_resend() -> None:
    wire = Wire([(400, GRAMMAR), answered(ANSWER)])
    events = await events_of(runtime(AnthropicProvider("claude-sonnet-5-5"), wire, stated=False))
    strict, resent = wire.bodies
    assert ANSWER_TOOL not in offered(strict) and "format" in strict["output_config"]   # enforced: no answer tool
    assert ANSWER_TOOL in offered(resent) and "format" not in resent.get("output_config", {})
    assert final(events) == ANSWER
    assert [(e.data["typed"], e.data.get("answer_via")) for e in events if e.kind == "usage"] == [
        ("stated_after_refusal", "tool")]

    # another role with a stated form keeps the schema stated, no tool (RUN-427 unchanged there)
    other = Wire([text_answer(ANSWER)])
    events = await events_of(runtime(AnthropicProvider("claude-sonnet-5-5"), other, planner=False))
    assert final(events) == ANSWER and ANSWER_TOOL not in offered(other.bodies[0])
    assert json.loads(other.bodies[0]["system"][-1]["text"].rsplit("\n", 1)[-1]) == SCHEMA
    assert via(events) == [""]


# ---------- OpenAI and Gemini: the same loop, each provider's own form ----------

# decision: RUN-447
@pytest.mark.anyio
async def test_openai_streams_the_answer_calls_arguments_when_its_form_is_stated() -> None:
    raw = json.dumps(ANSWER)
    item = {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": ANSWER_TOOL, "arguments": ""}
    stream = [{"type": "response.output_item.added", "output_index": 0, "item": item},
              *[{"type": "response.function_call_arguments.delta", "item_id": "fc_1", "output_index": 0,
                 "delta": raw[i:i + 31]} for i in range(0, len(raw), 31)],
              {"type": "response.completed", "response": {"status": "completed", "usage": {"output_tokens": 9},
                                                          "output": [{**item, "arguments": raw}]}}]
    wire = Wire([stream])
    events = await events_of(runtime(OpenAIProvider("gpt-6.1-sol"), wire))
    assert final(events) == ANSWER and json.loads(drafted(events)) == ANSWER
    [tool] = [t for t in wire.bodies[0]["tools"] if t["name"] == ANSWER_TOOL]
    assert tool["strict"] is False and tool["parameters"] == inline_refs(SCHEMA) and "text" not in wire.bodies[0]


# decision: RUN-447, RUN-408
@pytest.mark.anyio
async def test_gemini_offers_the_tool_in_its_form_and_its_whole_call_is_the_answer() -> None:
    def chunk(parts: list[dict[str, Any]], finish: str = "") -> dict[str, Any]:
        return {"candidates": [{"index": 0, "content": {"role": "model", "parts": parts},
                                **({"finishReason": finish} if finish else {})}],
                "usageMetadata": {"candidatesTokenCount": 7}}
    wire = Wire([[chunk([{"text": "Answering now."}]),
                  chunk([{"functionCall": {"name": ANSWER_TOOL, "args": ANSWER}, "thoughtSignature": "s"}], "STOP")]])
    events = await events_of(runtime(GoogleProvider("gemini-3.8-flash"), wire))
    assert final(events) == ANSWER and json.loads(drafted(events)) == ANSWER
    body = wire.bodies[0]
    [declared] = [d for t in body["tools"] for d in t["functionDeclarations"] if d["name"] == ANSWER_TOOL]
    assert declared["parametersJsonSchema"] == for_gemini(inline_refs(SCHEMA)).wire
    assert "generationConfig" not in body or "responseJsonSchema" not in body["generationConfig"]
    assert via(events) == ["tool"] and not any(e.kind == "tool_call" for e in events)


# ---------- End to end: the real daemon and gateway, the Anthropic adapter, a scripted transport ----------

def through_anthropic(d: RunningDaemon, wire: Wire) -> None:
    """The planner runs FERB's API loop with the Anthropic adapter, its typed format already refused (RUN-427), its
    lookups through the daemon's gateway; every other role stays the scripted runtime."""
    install(d, [], {})
    scripted = d.daemon.supervisor.make_runtime
    bridge = GatewayBridge(d.daemon.gateway, d.daemon.tokens)
    refusals = Refusals()

    def make(role: str, *, mode: str, tool_names: list[str], schema: Any, effort: Any = None) -> Any:
        if schema is not SCHEMA:
            return scripted(role, mode=mode, tool_names=tool_names, schema=schema, effort=effort)
        rt = ApiRuntime(AnthropicProvider("claude-sonnet-5-5"), mode=mode, role=role,  # type: ignore[arg-type]
                        tool_names=tool_names, output_schema=schema, bridge=bridge, key=key,
                        transport=wire.transport(), refusals=refusals, planner=True)
        refusals.remember(rt.format_key)
        return rt
    d.daemon.supervisor.make_runtime = make


def spans(d: RunningDaemon, kind: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [json.loads(r["output_json"] or "{}") for r in d.daemon.db.conn.execute(
            "SELECT output_json FROM spans WHERE kind = ? ORDER BY rowid", (kind,))]
    return d.run(go())                    # type: ignore[no-any-return]


def replies(events: list[dict[str, Any]]) -> list[str]:
    return [e["text"] for e in events if e["type"] == "reply"]


# decision: RUN-447, ORCH-516
def test_the_planner_answers_through_the_tool_after_a_lookup(daemon: RunningDaemon) -> None:
    wire = Wire([looked_up("memory_read", {"query": "dentist"}), answered(ANSWER, before="Found it.")])
    through_anthropic(daemon, wire)
    events = chat(daemon, "who is my dentist?")
    assert replies(events) == [ANSWER["text"]]
    assert "".join(e["text"] for e in events if e["type"] == "reply_delta") == ANSWER["text"]   # streamed draft
    assert len(wire.bodies) == 2 and ANSWER_TOOL in offered(wire.bodies[0])
    assert wire.bodies[1]["messages"][-1]["content"][0]["type"] == "tool_result"         # the lookup ran and went back
    assert [u.get("answer_via") for u in spans(daemon, "model.usage")] == [None, "tool"]
    assert not spans(daemon, "model.tool_loop")


# decision: RUN-447, PRM-336, ORCH-516
def test_a_follow_up_about_an_earlier_answer_is_answered_from_its_lookup(daemon: RunningDaemon) -> None:
    fact = "Sky Dental is on 4th Cross Road and open until 7 pm."
    wire = Wire([answered(reply(fact)),
                 looked_up("chat_history", {"query": "Sky Dental"}),
                 answered(reply("It's on 4th Cross Road, open until 7 pm."))])
    through_anthropic(daemon, wire)
    assert replies(chat(daemon, "where is sky dental?")) == [fact]
    assert replies(chat(daemon, "where was that dentist again?")) == ["It's on 4th Cross Road, open until 7 pm."]
    [result] = wire.bodies[2]["messages"][-1]["content"]
    assert result["type"] == "tool_result" and not result["is_error"]
    assert "4th Cross Road" in result["content"][0]["text"]               # the earlier answer, read through the gateway
    assert [u.get("answer_via") for u in spans(daemon, "model.usage")] == ["tool", None, "tool"]


# decision: PRM-338, ORCH-558, RUN-447
def test_the_last_call_offers_no_lookup_only_the_answer_tool(daemon: RunningDaemon) -> None:
    """The last call after the wrong-call cap (ORCH-558) has no lookups; where the form is stated the answer tool stays,
    as the note says ("no lookup is offered now"), and the answer through it is the reply."""
    wire = Wire([looked_up("x", {}) for _ in range(WRONG_CALLS_CAP)] + [answered(ANSWER)])
    through_anthropic(daemon, wire)
    assert replies(chat(daemon, "who is my dentist?")) == [ANSWER["text"]]
    assert "memory_read" in offered(wire.bodies[0])
    assert offered(wire.bodies[-1]) == [ANSWER_TOOL]
