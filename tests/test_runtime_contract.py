# tests/test_runtime_contract.py
"""The same contract for every direct-API adapter (D32): typed answers, tool round trips through FERB's gateway,
violations, normalized usage, errors without the key, retries, images, cut-off replies, and P20 cache placement.
Each provider adds one case: its native responses and the checks on its native requests."""
import json

import pytest

from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.anthropic_api import AnthropicProvider
from ferb.runtimes.api import ApiRuntime
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
    assert [e.kind for e in events] == ["usage", "tool_call", "usage", "final"]
    assert bridge.calls == [("fs.list", {"path": "/tmp"})]
    case["check_result"](script.requests[1][1], image=False)


@pytest.mark.anyio
async def test_image_results_reach_the_model(case) -> None:
    script = Scripted([(200, case["tool"]("fs_list", {"path": "/tmp"})), (200, case["final"])])
    await run(make(case, script, bridge=FakeBridge(result=IMAGE)))
    case["check_result"](script.requests[1][1], image=True)


@pytest.mark.anyio
async def test_unoffered_tool_is_a_violation(case) -> None:
    bridge = FakeBridge()
    script = Scripted([(200, case["tool"]("shell_run", {"command": "id"}))])
    events = await run(make(case, script, bridge=bridge))
    assert events[-1].kind == "violation" and bridge.calls == [] and len(script.requests) == 1


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


# defined after every case is registered, so params=sorted(CASES) sees them all
@pytest.fixture(params=sorted(CASES))
def case(request):
    return CASES[request.param]


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
