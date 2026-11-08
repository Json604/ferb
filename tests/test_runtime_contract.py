# tests/test_runtime_contract.py
"""The same contract for every direct-API adapter (D32): typed answers, tool round trips through FERB's gateway,
violations, normalized usage, errors without the key, retries, images, cut-off replies, and P20 cache placement.
Each provider adds one case: its native responses and the checks on its native requests."""
import json

import pytest

from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.anthropic_api import AnthropicProvider
from ferb.runtimes.api import ApiRuntime
from ferb.runtimes.google_api import GoogleProvider
from ferb.runtimes.openai_api import OpenAIProvider
from tests.support.runtime_fakes import FakeBridge, Scripted

SCHEMA = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"], "additionalProperties": False}
TASK = Task("t.worker", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
REQ = ModelRequest("FERB instructions", ("device facts", "history", "memory", "message"), 2)
IMAGE = {"content": {"shot": 1}, "footer": "", "is_error": False, "image": {"data": "aGVsbG8=", "mime": "image/png"}}


def anthropic_content(name, args):
    return [{"type": "thinking", "thinking": "", "signature": "sig-1"},
            {"type": "tool_use", "id": "toolu_1", "name": name, "input": args}]


def anthropic_tool(name, args):
    return {"content": anthropic_content(name, args), "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 5}}


CASES = {
    "anthropic": {
        "provider": lambda: AnthropicProvider("claude-sonnet-5-5"),
        "final": {"content": [{"type": "text", "text": '{"ok": true}'}], "stop_reason": "end_turn",
                  "usage": {"input_tokens": 100, "cache_creation_input_tokens": 2000, "cache_read_input_tokens": 3000,
                            "output_tokens": 50}},
        "tool": lambda name, args: anthropic_tool(name, args),
        "cut": {"content": [{"type": "text", "text": '{"ok": tr'}], "stop_reason": "max_tokens", "usage": {}},
        "usage": {"input_tokens": 5100, "cached_input_tokens": 3000, "cache_write_tokens": 2000, "output_tokens": 50},
        "key_header": ("x-api-key", "sk-secret"),
    },
}


def check_anthropic_first(body: dict) -> None:
    assert body["system"][0] == {"type": "text", "text": "FERB instructions", "cache_control": {"type": "ephemeral"}}
    content = body["messages"][0]["content"]
    assert [b["text"] for b in content] == ["device facts", "history", "memory", "message"]
    assert content[1]["cache_control"] == {"type": "ephemeral"} and "cache_control" not in content[2]
    assert body["output_config"]["format"] == {"type": "json_schema", "schema": SCHEMA}
    assert json.dumps(body).count('"cache_control"') <= 4
    assert body["max_tokens"] > 0 and body["cache_control"] == {"type": "ephemeral"}


def check_anthropic_result(body: dict, image: bool) -> None:
    result = body["messages"][-1]["content"][0]
    assert result["type"] == "tool_result" and result["tool_use_id"] == "toolu_1"
    kinds = [c["type"] for c in result["content"]]
    assert kinds == (["text", "image"] if image else ["text"])
    assert body["messages"][-2] == {"role": "assistant", "content": anthropic_content("fs_list", {"path": "/tmp"})}


CASES["anthropic"]["check_first"] = check_anthropic_first
CASES["anthropic"]["check_result"] = check_anthropic_result


def make(case, script, mode="worker", bridge=None, schema=SCHEMA) -> ApiRuntime:
    async def key(provider):
        return "sk-secret"
    async def sleep(s):
        pass
    return ApiRuntime(case["provider"](), mode=mode, role="worker", tool_names=["fs.list"], output_schema=schema,
                      bridge=bridge or FakeBridge(), key=key, transport=script.transport(), sleep=sleep)


async def run(rt, token="tok"):
    handle = await rt.start(TASK, REQ, token)
    return [e async for e in rt.events(handle)]


@pytest.mark.anyio
async def test_typed_answer_with_cache_placement(case) -> None:
    script = Scripted([(200, case["final"])])
    events = await run(make(case, script, mode="supervisor"), token="")
    assert events[-1].kind == "final" and json.loads(events[-1].data["text"]) == {"ok": True}
    headers, body = script.requests[0]
    assert headers[case["key_header"][0]].endswith(case["key_header"][1])
    case["check_first"](body)


@pytest.mark.anyio
async def test_tool_call_goes_through_ferb_and_back(case) -> None:
    bridge = FakeBridge()
    script = Scripted([(200, case["tool"]("fs_list", {"path": "/tmp"})), (200, case["final"])])
    events = await run(make(case, script, bridge=bridge))
    # the tool's result going back is a sign of life for the silence guard (ORCH-528)
    assert [e.kind for e in events] == ["usage", "tool_call", "progress", "usage", "final"]
    assert bridge.calls == [("fs.list", {"path": "/tmp"})]
    case["check_result"](script.requests[1][1], image=False)


@pytest.mark.anyio
async def test_image_results_reach_the_model(case) -> None:
    script = Scripted([(200, case["tool"]("fs_list", {"path": "/tmp"})), (200, case["final"])])
    await run(make(case, script, bridge=FakeBridge(result=IMAGE)))
    case["check_result"](script.requests[1][1], image=True)


@pytest.mark.anyio
async def test_unoffered_tool_gets_a_tool_error_and_the_run_goes_on(case) -> None:
    bridge = FakeBridge()
    script = Scripted([(200, case["tool"]("shell_run", {"command": "id"})), (200, case["final"])])
    events = await run(make(case, script, bridge=bridge))
    assert events[-1].kind == "final" and not any(e.kind == "violation" for e in events)
    assert bridge.calls == [] and len(script.requests) == 2              # nothing ran; the model was told why
    assert "no tool named 'shell_run' was offered" in json.dumps(script.requests[1][1])


# decision: RUN-415
@pytest.mark.anyio
async def test_a_supervisor_run_with_a_run_token_looks_up_through_ferb(case) -> None:
    """The planner's lookups (ORCH-516): a one-turn run given a token is offered its tools and calls them through
    FERB's gateway; without one it has none."""
    bridge = FakeBridge()
    script = Scripted([(200, case["tool"]("fs_list", {"path": "/tmp"})), (200, case["final"])])
    events = await run(make(case, script, mode="supervisor", bridge=bridge), token="tok")
    assert [e.kind for e in events if e.kind not in ("text_delta", "progress")] == ["usage", "tool_call", "usage",
                                                                                   "final"]
    assert bridge.calls == [("fs.list", {"path": "/tmp"})]
    plain = Scripted([(200, case["final"])])
    await run(make(case, plain, mode="supervisor", bridge=FakeBridge()), token="")
    assert "fs_list" not in json.dumps(plain.requests[0][1])


@pytest.mark.anyio
async def test_usage_is_normalized(case) -> None:
    events = await run(make(case, Scripted([(200, case["final"])]), mode="supervisor"), token="")
    usage = events[0].data
    assert {k: usage[k] for k in case["usage"]} == case["usage"] and usage["billing"] == "api"


@pytest.mark.anyio
async def test_cut_off_reply_is_an_error(case) -> None:
    events = await run(make(case, Scripted([(200, case["cut"])]), mode="supervisor"), token="")
    assert events[-1].kind == "error" and not any(e.kind == "final" for e in events)


@pytest.mark.anyio
async def test_provider_error_never_contains_the_key(case) -> None:
    events = await run(make(case, Scripted([(401, {"error": {"message": "key sk-secret is invalid"}})]), mode="supervisor"),
                       token="")
    assert events[-1].kind == "error" and "401" in events[-1].data["message"]
    assert "sk-secret" not in events[-1].data["message"]


@pytest.mark.anyio
@pytest.mark.parametrize("stop", ["pause_turn", "model_context_window_exceeded"])
async def test_anthropic_early_stop_is_an_error(stop) -> None:
    reply = {"content": [{"type": "text", "text": '{"ok": tr'}], "stop_reason": stop, "usage": {}}
    events = await run(make(CASES["anthropic"], Scripted([(200, reply)]), mode="supervisor"), token="")
    assert events[-1].kind == "error" and stop in events[-1].data["message"]
    assert not any(e.kind == "final" for e in events)
    # the documented window-exceeded stop is marked as an overflow (fix wave 6b round 1: the checker's fallback)
    assert events[-1].data.get("overflow", False) is (stop == "model_context_window_exceeded")


@pytest.mark.anyio
@pytest.mark.parametrize("status, kind, message, overflow", [
    (400, "invalid_request_error", "prompt is too long: 1012345 tokens > 1000000 maximum", True),
    (400, "invalid_request_error", "input length and `max_tokens` exceed context limit: 990000 + 32000 > 1000000, "
                                   "decrease input length or `max_tokens` and try again", True),
    (400, "invalid_request_error", "This model does not support assistant message prefill.", False),
    (413, "request_too_large", "prompt is too long: 1012345 tokens > 1000000 maximum", False),
])
async def test_anthropic_over_long_prompt_rejection_is_marked_as_an_overflow(status, kind, message, overflow) -> None:
    """Fix wave 6b round 2 (N1): the Messages API rejects an input over the model's window with a 400
    invalid_request_error whose message is "prompt is too long: N tokens > M maximum", or, when the input and
    max_tokens together exceed it, "input length and `max_tokens` exceed context limit: …". Anthropic's own client
    defines both (Claude Code 2.1.283, bundled in the pinned claude-agent-sdk 0.2.160: `"Prompt is too long"`,
    `/prompt is too long[^0-9]*(\\d+)\\s*tokens?\\s*>\\s*(\\d+)/i`, and its `max_tokens_context_overflow` class). Only
    that status, type and prefix count; any other rejection stays an ordinary error."""
    body = {"type": "error", "error": {"type": kind, "message": message}, "request_id": "req_1"}
    # a 400 that isn't one is sent once more without the typed format (RUN-427), which the provider refuses alike
    events = await run(make(CASES["anthropic"], Scripted([(status, body)] * 2), mode="supervisor"), token="")
    assert events[-1].kind == "error" and events[-1].data.get("overflow", False) is overflow


@pytest.mark.anyio
async def test_googles_token_count_rejection_stays_unmarked() -> None:
    """Google documents no error code or message for an input over a model's token limit, and its own client (Gemini CLI
    0.55.1) counts tokens before sending instead of reading the rejection: with nothing to cite, FERB doesn't mark it
    (an accepted residual)."""
    body = {"error": {"code": 400, "status": "INVALID_ARGUMENT",
                      "message": "The input token count (1200000) exceeds the maximum number of tokens allowed "
                                 "(1048576)."}}
    events = await run(make(CASES["google"], Scripted([(400, body)] * 2), mode="supervisor"), token="")  # RUN-427
    assert events[-1].kind == "error" and "overflow" not in events[-1].data


@pytest.mark.anyio
@pytest.mark.parametrize("code, overflow", [("context_length_exceeded", True), ("invalid_value", False)])
async def test_openai_window_exceeded_error_is_marked_as_an_overflow(code, overflow) -> None:
    """OpenAI documents error code context_length_exceeded for an input longer than the model's window."""
    body = {"error": {"message": "This model's maximum context length is 400000 tokens.",
                      "type": "invalid_request_error", "code": code}}
    events = await run(make(CASES["openai"], Scripted([(400, body)] * 2), mode="supervisor"), token="")  # RUN-427
    assert events[-1].kind == "error" and "400" in events[-1].data["message"]
    assert events[-1].data.get("overflow", False) is overflow

# decision: ORCH-524, RUN-418
@pytest.mark.anyio
@pytest.mark.parametrize("case, status, body, image", [
    ("anthropic", 400, {"type": "error", "error": {"type": "invalid_request_error", "message":
     "messages.0.content.1.image.source.base64: image exceeds 5 MB maximum: 6291456 bytes > 5242880 bytes"}}, True),
    ("anthropic", 400, {"type": "error", "error": {"type": "invalid_request_error",
                                                   "message": "Could not process image"}}, True),
    ("anthropic", 400, {"type": "error", "error": {"type": "invalid_request_error",
                                                   "message": "Schemas contains too many parameters with union types"}},
     False),
    ("anthropic", 429, {"type": "error", "error": {"type": "rate_limit_error", "message": "rate limited"}}, False),
    ("openai", 400, {"error": {"message": "Invalid image.", "type": "invalid_request_error",
                               "code": "invalid_image"}}, True),
    ("openai", 400, {"error": {"message": "Invalid schema for response_format", "type": "invalid_request_error",
                               "code": "invalid_value"}}, False),
    ("google", 400, {"error": {"code": 400, "status": "INVALID_ARGUMENT",
                               "message": "Unable to process input image."}}, False),
])
async def test_only_a_providers_own_image_rejection_is_marked_as_one(case, status, body, image) -> None:
    """Past bug (R3): any planner error with a screenshot was taken for an image refusal. The adapter now marks only
    the provider's own image rejection (OpenAI's image error codes; the shapes Anthropic's own client classifies as image
    errors); a schema, rate-limit or Google error (no documented image code) stays an ordinary error."""
    events = await run(make(CASES[case], Scripted([(status, body)] * 5), mode="supervisor"), token="")
    assert events[-1].kind == "error" and events[-1].data.get("image", False) is image


def openai_tool(name, args):
    return {"status": "completed", "output": [{"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": name,
                                               "arguments": json.dumps(args)}], "usage": {"input_tokens": 10}}


def check_openai_first(body: dict) -> None:
    developer, user = body["input"]
    assert developer["role"] == "developer"
    assert developer["content"][0]["prompt_cache_breakpoint"] == {"mode": "explicit"}
    assert [b["text"] for b in user["content"]] == ["device facts", "history", "memory", "message"]
    assert user["content"][1]["prompt_cache_breakpoint"] == {"mode": "explicit"}
    assert "prompt_cache_breakpoint" not in user["content"][2]
    assert body["text"]["format"] == {"type": "json_schema", "name": "ferb_result", "schema": SCHEMA, "strict": True}
    assert body["store"] is False and body["prompt_cache_key"] == "ferb:worker:supervisor"
    assert body["reasoning"] == {"effort": "low"} and "instructions" not in body
    assert body["include"] == ["reasoning.encrypted_content"]


def check_openai_result(body: dict, image: bool) -> None:
    items = body["input"]
    assert any(i.get("type") == "function_call" and i["call_id"] == "call_1" for i in items)   # replayed output
    out = items[-1]
    assert out["type"] == "function_call_output" and out["call_id"] == "call_1"
    if image:
        assert [c["type"] for c in out["output"]] == ["input_text", "input_image"]
        assert out["output"][1]["image_url"] == "data:image/png;base64,aGVsbG8="
    else:
        assert isinstance(out["output"], str)


CASES["openai"] = {
    "provider": lambda: OpenAIProvider("gpt-6.1-sol"),
    "final": {"status": "completed", "output": [
        {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "enc"},
        {"type": "message", "id": "msg_1", "role": "assistant", "content": [{"type": "output_text", "text": '{"ok": true}'}]}],
        "usage": {"input_tokens": 5100, "input_tokens_details": {"cached_tokens": 3000, "cache_write_tokens": 2000},
                  "output_tokens": 50, "output_tokens_details": {"reasoning_tokens": 20}}},
    "tool": openai_tool,
    "cut": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}, "output": [], "usage": {}},
    "usage": {"input_tokens": 5100, "cached_input_tokens": 3000, "cache_write_tokens": 2000, "output_tokens": 50},
    "key_header": ("authorization", "Bearer sk-secret"),
    "check_first": check_openai_first,
    "check_result": check_openai_result,
}


@pytest.mark.anyio
async def test_openai_malformed_arguments_return_an_error_result() -> None:
    bad = {"status": "completed", "output": [{"type": "function_call", "id": "fc_1", "call_id": "call_1",
                                              "name": "fs_list", "arguments": "{not json"}], "usage": {}}
    bridge = FakeBridge()
    script = Scripted([(200, bad), (200, CASES["openai"]["final"])])
    events = await run(make(CASES["openai"], script, bridge=bridge))
    assert bridge.calls == [] and events[-1].kind == "final"
    assert "not a JSON object" in script.requests[1][1]["input"][-1]["output"]


def test_gpt_5_5_uses_the_routing_key_without_breakpoints() -> None:
    state = OpenAIProvider("gpt-5.5").begin(REQ, [], SCHEMA, None, "ferb:orchestrator:supervisor")
    blocks = [b for item in state["body"]["input"] for b in item["content"]]
    assert not any("prompt_cache_breakpoint" in b for b in blocks)
    assert state["body"]["prompt_cache_key"] == "ferb:orchestrator:supervisor"


@pytest.mark.anyio
@pytest.mark.parametrize("status", ["failed", "cancelled", "in_progress", "queued", None])
async def test_openai_non_completed_status_is_an_error(status) -> None:
    reply: dict = {"output": [{"type": "message", "role": "assistant",
                               "content": [{"type": "output_text", "text": '{"ok": tr'}]}], "usage": {}}
    if status:
        reply["status"] = status
    if status == "failed":
        reply["error"] = {"code": "server_error", "message": "boom"}
    events = await run(make(CASES["openai"], Scripted([(200, reply)]), mode="supervisor"), token="")
    assert events[-1].kind == "error" and not any(e.kind == "final" for e in events)
    assert f"did not complete ({status})" in events[-1].data["message"] or status == "failed"
    if status == "failed":
        assert "boom" in events[-1].data["message"]


@pytest.mark.anyio
async def test_openai_refusal_is_an_error() -> None:
    reply = {"status": "completed", "usage": {}, "output": [
        {"type": "message", "role": "assistant", "content": [{"type": "refusal", "refusal": "I can't help with that."}]}]}
    events = await run(make(CASES["openai"], Scripted([(200, reply)]), mode="supervisor"), token="")
    assert events[-1].kind == "error" and not any(e.kind == "final" for e in events)


def google_tool(name, args):
    return {"candidates": [{"content": {"role": "model", "parts": [
        {"functionCall": {"name": name, "args": args, "id": "fc1"}, "thoughtSignature": "sig1"}]},
        "finishReason": "STOP"}], "usageMetadata": {"promptTokenCount": 10}}


def check_google_first(body: dict) -> None:
    assert body["systemInstruction"] == {"parts": [{"text": "FERB instructions"}]}
    assert [p["text"] for p in body["contents"][0]["parts"]] == ["device facts", "history", "memory", "message"]
    assert body["generationConfig"]["responseMimeType"] == "application/json"
    assert body["generationConfig"]["responseJsonSchema"] == SCHEMA
    assert body["generationConfig"]["thinkingConfig"] == {"thinkingLevel": "low"}


def check_google_result(body: dict, image: bool) -> None:
    model_turn, reply = body["contents"][-2], body["contents"][-1]
    assert model_turn["parts"][0]["thoughtSignature"] == "sig1"              # passed back exactly as received
    response = reply["parts"][0]["functionResponse"]
    assert response["name"] == "fs_list" and response["id"] == "fc1"
    assert ("parts" in response) == image
    if image:
        assert response["parts"][0]["inlineData"] == {"mimeType": "image/png", "data": "aGVsbG8="}


CASES["google"] = {
    "provider": lambda: GoogleProvider("gemini-3.8-flash"),
    "final": {"candidates": [{"content": {"role": "model", "parts": [{"text": '{"ok": true}'}]}, "finishReason": "STOP"}],
              "usageMetadata": {"promptTokenCount": 5000, "cachedContentTokenCount": 3000, "candidatesTokenCount": 30,
                                "thoughtsTokenCount": 20, "toolUsePromptTokenCount": 100}},
    "tool": google_tool,
    "cut": {"candidates": [{"content": {"role": "model", "parts": [{"text": '{"ok'}]}, "finishReason": "MAX_TOKENS"}],
            "usageMetadata": {}},
    "usage": {"input_tokens": 5100, "cached_input_tokens": 3000, "cache_write_tokens": 0, "output_tokens": 50},
    "key_header": ("x-goog-api-key", "sk-secret"),
    "check_first": check_google_first,
    "check_result": check_google_result,
}


# defined after every case is registered, so params=sorted(CASES) sees them all
@pytest.fixture(params=sorted(CASES))
def case(request):
    return CASES[request.param]


def test_google_tools_use_json_schema_parameters() -> None:
    from ferb.runtimes.api import ToolSpec
    state = GoogleProvider().begin(REQ, [ToolSpec("fs_list", "List.", {"type": "object"})], None, None, "k")
    declaration = state["body"]["tools"][0]["functionDeclarations"][0]
    assert declaration == {"name": "fs_list", "description": "List.", "parametersJsonSchema": {"type": "object"}}


@pytest.mark.anyio
@pytest.mark.parametrize("finish", ["OTHER", "UNEXPECTED_TOOL_CALL", "SAFETY", "LANGUAGE", "MALFORMED_FUNCTION_CALL"])
async def test_google_non_stop_finish_reason_is_an_error(finish) -> None:
    reply = {"candidates": [{"content": {"role": "model", "parts": [{"text": '{"ok": tr'}]}, "finishReason": finish}],
             "usageMetadata": {}}
    events = await run(make(CASES["google"], Scripted([(200, reply)]), mode="supervisor"), token="")
    assert events[-1].kind == "error" and not any(e.kind == "final" for e in events)
    assert f"the model stopped ({finish})" in events[-1].data["message"]


@pytest.mark.anyio
async def test_google_missing_finish_reason_is_a_normal_end() -> None:
    reply = {"candidates": [{"content": {"role": "model", "parts": [{"text": '{"ok": true}'}]}}], "usageMetadata": {}}
    events = await run(make(CASES["google"], Scripted([(200, reply)]), mode="supervisor"), token="")
    assert events[-1].kind == "final"


@pytest.mark.anyio
@pytest.mark.parametrize('initial', ['final', 'tool'])
async def test_owner_updates_are_native_user_messages_after_prior_turn(case, initial):
    first = case['final'] if initial == 'final' else case['tool']('fs_list', {'path': '/tmp'})
    script = Scripted([(200, first), (200, case['final'])])
    bridge = FakeBridge()
    rt = make(case, script, bridge=bridge)
    handle = await rt.start(TASK, REQ, 'tok')
    events = []
    async for event in rt.events(handle):
        events.append(event)
        if event.kind == 'usage' and len(script.requests) == 1:
            await rt.continue_(handle, 'Keep it under 300')
            await rt.continue_(handle, 'Two breads')
    body = script.requests[1][1]
    history = body.get('messages', body.get('input', body.get('contents')))
    assert [item['role'] for item in history[-2:]] == ['user', 'user']
    assert 'Keep it under 300' in json.dumps(history[-2])
    assert 'Two breads' in json.dumps(history[-1])
    assert bridge.calls == []
    if initial == 'tool':
        assert 'not executed' in json.dumps(history[-3])
    else:
        assert 'ok' in json.dumps(history[-3])
    assert len([event for event in events if event.kind == 'final']) == 1
