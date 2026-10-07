# tests/test_api_runtime.py
"""The direct-API runtime: tools only through FERB's gateway, retries, errors without the key, cancellation (D32)."""
import asyncio
import json

import httpx
import pytest

from ferb.core.errors import Unsupported
from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.api import ApiRuntime, inline_refs
from tests.support.runtime_fakes import FakeBridge, FakeProvider, Scripted

TASK = Task("t.worker", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
REQ = ModelRequest("instructions", ("facts", "history", "message"), 2)


def runtime(script: Scripted, *, mode="worker", bridge=None, key="sk-secret", sleeps=None, **kw) -> ApiRuntime:
    async def key_source(provider):
        return key
    async def sleep(s):
        (sleeps if sleeps is not None else []).append(s)
    return ApiRuntime(FakeProvider(), mode=mode, role="worker", tool_names=["fs.list"], output_schema=None,
                      bridge=bridge or FakeBridge(), key=key_source, transport=script.transport(), sleep=sleep, **kw)


async def run_all(rt, request=REQ, token="tok") -> list:
    run = await rt.start(TASK, request, token)
    return [e async for e in rt.events(run)]


@pytest.mark.anyio
async def test_tool_call_goes_through_the_bridge_and_back() -> None:
    script = Scripted([(200, {"calls": [["c1", "fs_list", {"path": "/tmp"}]]}), (200, {"text": "done"})])
    bridge = FakeBridge()
    events = await run_all(runtime(script, bridge=bridge))
    assert [e.kind for e in events] == ["usage", "tool_call", "usage", "final"]
    assert bridge.calls == [("fs.list", {"path": "/tmp"})]                    # mcp name mapped back to the capability
    assert script.requests[1][1]["results"][0][0][:3] == ["c1", '{"entries": ["a.txt"]}\n', False]
    assert events[-1].data == {"text": "done"}                                # no tools_seen: tools are never deferred


@pytest.mark.anyio
async def test_request_carries_the_cacheable_prefix_and_role_key() -> None:
    script = Scripted([(200, {"text": "ok"})])
    await run_all(runtime(script, mode="supervisor"), token="")
    headers, body = script.requests[0]
    assert body["parts"][:3] == ["facts", "history", "message"] and body["cacheable"] == 2
    assert body["cache_key"] == "ferb:worker:supervisor" and body["effort"] == "low" and body["tools"] == []
    assert headers["x-key"] == "sk-secret"


@pytest.mark.anyio
async def test_unoffered_tool_is_a_violation() -> None:
    script = Scripted([(200, {"calls": [["c1", "shell_run", {"command": "id"}]]})])
    bridge = FakeBridge()
    events = await run_all(runtime(script, bridge=bridge))
    assert events[-1].kind == "violation" and bridge.calls == [] and len(script.requests) == 1


@pytest.mark.anyio
async def test_bad_arguments_never_reach_the_gateway() -> None:
    script = Scripted([(200, {"calls": [["c1", "fs_list", "not an object"]]}), (200, {"text": "ok"})])
    bridge = FakeBridge()
    events = await run_all(runtime(script, bridge=bridge))
    assert bridge.calls == [] and events[-1].kind == "final"
    result = script.requests[1][1]["results"][0][0]
    assert result[2] is True and "not a JSON object" in result[1]


@pytest.mark.anyio
async def test_retries_then_succeeds() -> None:
    sleeps: list[float] = []
    script = Scripted([(429, {"error": {"message": "slow down"}}), (503, {}), (200, {"text": "ok"})])
    events = await run_all(runtime(script, sleeps=sleeps))
    assert events[-1].kind == "final" and sleeps == [1.0, 2.0]


@pytest.mark.anyio
async def test_network_failure_after_retries_is_an_error() -> None:
    async def boom(request):
        raise httpx.ConnectError("down")
    rt = runtime(Scripted([]), retries=2)
    rt.transport = httpx.MockTransport(boom)
    events = await run_all(rt)
    assert events[-1].kind == "error" and "network error" in events[-1].data["message"]


@pytest.mark.anyio
async def test_errors_name_the_status_never_the_key() -> None:
    script = Scripted([(401, {"error": {"message": "bad key sk-secret was rejected"}})])
    events = await run_all(runtime(script))
    message = events[-1].data["message"]
    assert events[-1].kind == "error" and "401" in message and "sk-secret" not in message


@pytest.mark.anyio
async def test_cut_off_reply_is_an_error() -> None:
    events = await run_all(runtime(Scripted([(200, {"cut": True})])))
    assert events[-1].kind == "error" and "cut off" in events[-1].data["message"]


@pytest.mark.anyio
async def test_missing_key_names_the_fix() -> None:
    with pytest.raises(Unsupported, match="ferb setup --key fake"):
        await runtime(Scripted([]), key=None).start(TASK, REQ, "tok")


@pytest.mark.anyio
async def test_cancel_stops_a_slow_call() -> None:
    rt = runtime(Scripted([None]))
    run = await rt.start(TASK, REQ, "tok")
    collected: list = []
    async def consume():
        async for e in rt.events(run):
            collected.append(e)
    consumer = asyncio.create_task(consume())
    await asyncio.sleep(0.05)
    await rt.cancel(run)
    await asyncio.wait_for(consumer, 2)
    assert not any(e.kind == "final" for e in collected)


def test_inline_refs_resolves_local_definitions() -> None:
    schema = {"type": "object", "properties": {"a": {"$ref": "#/$defs/A"}}, "$defs": {"A": {"type": "string"}}}
    assert inline_refs(schema) == {"type": "object", "properties": {"a": {"type": "string"}}}
