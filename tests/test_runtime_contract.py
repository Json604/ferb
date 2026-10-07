# tests/test_runtime_contract.py
"""The same contract for every direct-API adapter (D32): typed answers, tool round trips through FERB's gateway,
violations, normalized usage, errors without the key, retries, images, cut-off replies, and P20 cache placement.
Each provider adds one case: its native responses and the checks on its native requests."""
import json

import pytest

from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.anthropic_api import AnthropicProvider
from ferb.runtimes.api import ApiRuntime
from tests.support.runtime_fakes import FakeBridge, Scripted

SCHEMA = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"], "additionalProperties": False}
TASK = Task("t.worker", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
REQ = ModelRequest("FERB instructions", ("device facts", "history", "memory", "message"), 2)
IMAGE = {"content": {"shot": 1}, "footer": "", "is_error": False, "image": {"data": "aGVsbG8=", "mime": "image/png"}}


def anthropic_tool(name, args):
    return {"content": [{"type": "tool_use", "id": "toolu_1", "name": name, "input": args}], "stop_reason": "tool_use",
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
    assert body["max_tokens"] > 0 and body["cache_control"] == {"type": "ephemeral"}


def check_anthropic_result(body: dict, image: bool) -> None:
    result = body["messages"][-1]["content"][0]
    assert result["type"] == "tool_result" and result["tool_use_id"] == "toolu_1"
    kinds = [c["type"] for c in result["content"]]
    assert kinds == (["text", "image"] if image else ["text"])
    assert body["messages"][-2]["role"] == "assistant"                      # the model's turn went back unchanged


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


@pytest.fixture(params=sorted(CASES))
def case(request):
    return CASES[request.param]


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
