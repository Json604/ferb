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
async def test_unoffered_tool_gets_a_tool_error_not_a_violation() -> None:
    script = Scripted([(200, {"calls": [["c1", "shell_run", {"command": "id"}]]}), (200, {"text": "ok"})])
    bridge = FakeBridge()
    events = await run_all(runtime(script, bridge=bridge))
    assert events[-1].kind == "final" and bridge.calls == [] and len(script.requests) == 2
    assert not any(e.kind in ("violation", "tool_call") for e in events)
    result = script.requests[1][1]["results"][0][0]
    assert result[0] == "c1" and result[2] is True and "no tool named 'shell_run' was offered" in result[1]


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
async def test_a_provider_refusal_says_its_plain_cause_before_the_providers_words() -> None:
    """notes/runtimes.md: owner-facing errors in app words, plain cause first (live: "anthropic HTTP 400:
    output_config…"); the provider's own words stay after it."""
    script = Scripted([(400, {"error": {"message": "output_config.format: unsupported schema"}})])
    message = (await run_all(runtime(script)))[-1].data["message"]
    assert message.startswith("it refused the request FERB sent (HTTP 400: output_config.format")


@pytest.mark.anyio
async def test_cut_off_reply_is_an_error() -> None:
    events = await run_all(runtime(Scripted([(200, {"cut": True})])))
    assert events[-1].kind == "error" and "cut off" in events[-1].data["message"]


@pytest.mark.anyio
async def test_missing_key_names_the_fix() -> None:
    with pytest.raises(Unsupported, match="no fake API key: add one in Settings → AI"):
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


class SlowBridge(FakeBridge):
    def __init__(self, gate: asyncio.Event) -> None:
        super().__init__()
        self.gate, self.tasks = gate, []

    async def call(self, run_token, name, arguments):
        self.tasks.append(asyncio.current_task())
        self.calls.append((name, arguments))
        await self.gate.wait()
        return self.result


@pytest.mark.anyio
async def test_consumer_cancellation_propagates() -> None:
    rt = runtime(Scripted([None]))
    run = await rt.start(TASK, REQ, "tok")

    async def consume():
        async for _ in rt.events(run):
            pass
    consumer = asyncio.create_task(consume())
    await asyncio.sleep(0.05)
    asyncio.get_running_loop().create_task(rt.cancel(run))
    consumer.cancel()
    with pytest.raises(asyncio.CancelledError):
        await consumer


@pytest.mark.anyio
async def test_cancel_during_a_slow_tool_call_stops_the_run() -> None:
    gate = asyncio.Event()
    script = Scripted([(200, {"calls": [["c1", "fs_list", {"path": "/"}]]}), (200, {"text": "done"})])
    bridge = SlowBridge(gate)
    rt = runtime(script, bridge=bridge)
    run = await rt.start(TASK, REQ, "tok")
    collected: list = []

    async def consume():
        async for e in rt.events(run):
            collected.append(e)
    consumer = asyncio.create_task(consume())
    await asyncio.sleep(0.05)
    assert bridge.calls
    await rt.cancel(run)
    await asyncio.wait_for(consumer, 2)
    assert len(script.requests) == 1 and not any(e.kind == "final" for e in collected)


@pytest.mark.anyio
async def test_cancel_before_events_makes_no_request() -> None:
    script = Scripted([(200, {"text": "ok"})])
    rt = runtime(script)
    run = await rt.start(TASK, REQ, "tok")
    await rt.cancel(run)
    assert [e async for e in rt.events(run)] == [] and script.requests == []


@pytest.mark.anyio
async def test_gateway_call_runs_on_its_own_task() -> None:
    gate = asyncio.Event()
    gate.set()
    script = Scripted([(200, {"calls": [["c1", "fs_list", {"path": "/"}]]}), (200, {"text": "done"})])
    bridge = SlowBridge(gate)
    seen = []

    async def consume():
        seen.append(asyncio.current_task())
        return await run_all(runtime(script, bridge=bridge))
    events = await asyncio.create_task(consume())
    assert events[-1].kind == "final" and bridge.tasks[0] is not seen[0]


@pytest.mark.anyio
async def test_gateway_cancelling_a_tool_call_does_not_end_the_run() -> None:
    gate = asyncio.Event()
    script = Scripted([(200, {"calls": [["c1", "fs_list", {"path": "/"}]]}), (200, {"text": "done"})])
    bridge = SlowBridge(gate)
    rt = runtime(script, bridge=bridge)
    run = await rt.start(TASK, REQ, "tok")
    consumer = asyncio.create_task(_collect(rt, run))
    await asyncio.sleep(0.05)
    bridge.tasks[0].cancel()
    events = await asyncio.wait_for(consumer, 2)
    assert events[-1].kind == "final"
    assert script.requests[1][1]["results"][0][0][1:3] == ["FERB stopped this tool call", True]


async def _collect(rt, run) -> list:
    return [e async for e in rt.events(run)]


@pytest.mark.anyio
async def test_one_unoffered_tool_is_refused_alone_in_its_turn() -> None:
    script = Scripted([(200, {"calls": [["c1", "fs_list", {"path": "/"}], ["c2", "shell_run", {}]]}),
                       (200, {"text": "ok"})])
    bridge = FakeBridge()
    events = await run_all(runtime(script, bridge=bridge))
    assert events[-1].kind == "final" and bridge.calls == [("fs.list", {"path": "/"})]
    results = script.requests[1][1]["results"][0]
    assert [r[0] for r in results] == ["c1", "c2"] and results[0][2] is False and results[1][2] is True


@pytest.mark.anyio
async def test_key_at_the_truncation_point_is_fully_replaced() -> None:
    message = "x" * 295 + "sk-secret" + "y" * 50
    events = await run_all(runtime(Scripted([(401, {"error": {"message": message}})])))
    text = events[-1].data["message"]
    assert "sk-secre" not in text and "[key]" in text


@pytest.mark.anyio
async def test_run_repr_and_cancel_hide_and_close() -> None:
    rt = runtime(Scripted([]))
    run = await rt.start(TASK, REQ, "tok")
    assert "sk-secret" not in repr(run)
    await rt.cancel(run)
    assert run.state["run"].client.is_closed
    assert [e async for e in rt.events(run)] == []       # closing twice is harmless


def test_inline_refs_keeps_siblings_of_a_reference() -> None:
    schema = {"properties": {"a": {"$ref": "#/$defs/A", "description": "the a"}}, "$defs": {"A": {"type": "string"}}}
    assert inline_refs(schema) == {"properties": {"a": {"type": "string", "description": "the a"}}}


@pytest.mark.anyio
async def test_cancel_while_the_consumer_handles_the_tool_call_event_skips_the_bridge() -> None:
    script = Scripted([(200, {"calls": [["c1", "fs_list", {"path": "/"}]]}), (200, {"text": "done"})])
    bridge = FakeBridge()
    rt = runtime(script, bridge=bridge)
    run = await rt.start(TASK, REQ, "tok")
    events = []
    async for e in rt.events(run):
        events.append(e)
        if e.kind == "tool_call":
            await rt.cancel(run)                      # the consumer awaits while handling the event
    assert bridge.calls == [] and len(script.requests) == 1 and not any(e.kind == "final" for e in events)


@pytest.mark.anyio
async def test_owner_correction_replaces_stale_final_in_same_run():
    script = Scripted([(200, {"text": "stale"}), (200, {"text": "updated"})])
    rt = runtime(script)
    run = await rt.start(TASK, REQ, "tok")
    events = []
    async for event in rt.events(run):
        events.append(event)
        if event.kind == "usage" and len(script.requests) == 1:
            await rt.continue_(run, "Keep it under 300")
            await rt.continue_(run, "With two breads")
    assert [e.data["text"] for e in events if e.kind == "final"] == ["updated"]
    assert script.requests[1][1]["updates"] == ["Keep it under 300", "With two breads"]
    with pytest.raises(Unsupported):
        await rt.continue_(run, "too late")


@pytest.mark.anyio
async def test_owner_correction_preserves_started_tool_and_skips_remaining_stale_batch():
    gate = asyncio.Event()
    script = Scripted([(200, {"calls": [["c1", "fs_list", {"path": "/a"}],
                                       ["c2", "fs_list", {"path": "/b"}]]}), (200, {"text": "updated"})])
    bridge = SlowBridge(gate)
    rt = runtime(script, bridge=bridge)
    run = await rt.start(TASK, REQ, "tok")
    consumer = asyncio.create_task(_collect(rt, run))
    while not bridge.calls:
        await asyncio.sleep(0)
    try:
        await rt.continue_(run, "Use /c instead")
    finally:
        gate.set()
    events = await asyncio.wait_for(consumer, 2)
    assert bridge.calls == [("fs.list", {"path": "/a"})]
    assert not bridge.tasks[0].cancelled()
    results = script.requests[1][1]["results"][0]
    assert results[0][2] is False and results[1][2] is True
    assert "not executed" in results[1][1]
    assert script.requests[1][1]["updates"] == ["Use /c instead"]
    assert events[-1].data == {"text": "updated"}


@pytest.mark.anyio
async def test_owner_correction_at_tool_event_prevents_unstarted_call():
    script = Scripted([(200, {"calls": [["c1", "fs_list", {"path": "/a"}]]}), (200, {"text": "updated"})])
    bridge = FakeBridge()
    rt = runtime(script, bridge=bridge)
    run = await rt.start(TASK, REQ, "tok")
    async for event in rt.events(run):
        if event.kind == "tool_call":
            await rt.continue_(run, "Use /b instead")
    assert bridge.calls == []
    assert script.requests[1][1]["results"][0][0][2] is True


@pytest.mark.anyio
async def test_owner_update_during_http_is_retained_without_cancelling_request():
    entered, release = asyncio.Event(), asyncio.Event()
    script = Scripted([(200, {"text": "stale"}), (200, {"text": "updated"})])
    rt = runtime(script)
    request_tasks = []

    async def handle(request):
        request_tasks.append(asyncio.current_task())
        if len(request_tasks) == 1:
            entered.set()
            await release.wait()
        return await script._handle(request)

    rt.transport = httpx.MockTransport(handle)
    run = await rt.start(TASK, REQ, "tok")
    consumer = asyncio.create_task(_collect(rt, run))
    await entered.wait()
    await rt.continue_(run, "Keep it under 300")
    release.set()
    events = await asyncio.wait_for(consumer, 2)
    assert not request_tasks[0].cancelled()
    assert "updates" not in script.requests[0][1]
    assert script.requests[1][1]["updates"] == ["Keep it under 300"]
    assert [e.data["text"] for e in events if e.kind == "final"] == ["updated"]


@pytest.mark.anyio
async def test_terminal_event_rejects_updates_before_generator_cleanup():
    rt = runtime(Scripted([(200, {"text": "done"})]))
    run = await rt.start(TASK, REQ, "tok")
    async for event in rt.events(run):
        if event.kind == "final":
            with pytest.raises(Unsupported):
                await rt.continue_(run, "too late")
