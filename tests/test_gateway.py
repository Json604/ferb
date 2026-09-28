"""Gateway (section 7 step 6) against a real daemon with fake capabilities, and safety test 21.2 through a real
`ferb mcp` shim process (revoked token -> no tool calls, within a second)."""

import os
import sys
import time
from datetime import timedelta

import anyio
import pytest
from pydantic import BaseModel

from ferb.core.types import ActionSpec, Budget, Identity, RiskLevel, Target, Task, ToolContext, ToolResult
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.intent import RequestIntent
from tests.conftest import RunningDaemon


class FakeCap:
    identities = {Identity.OWNER}
    risk, reversible = RiskLevel.MEDIUM, False

    class Input(BaseModel):
        to: str = "alice@x.com"
        text: str = "hi"

    def __init__(self, name: str, kind: str, scopes: set[str], boom: bool = False,
                 reads: tuple[tuple[str, str], ...] = ()) -> None:
        self.name, self.description, self.kind, self.scopes = name, f"fake {kind}", kind, scopes
        self.boom, self.reads, self.calls = boom, reads, 0

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(self.kind, Target(recipients=(getattr(args, "to"),)), content=getattr(args, "text"))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        self.calls += 1
        if self.boom:
            raise ConnectionError("lost ack after Send")
        return ToolResult({"ok": True, "n": self.calls}, read_sources=self.reads)


def setup(d: RunningDaemon, scopes: set[str], intent: RequestIntent | None, caps: list[FakeCap]):
    for c in caps:
        d.daemon.registry.capabilities[c.name] = c
    task = Task("task-1", None, "g", "dev", Identity.OWNER, scopes, "actor", "req-1", [],
                d.daemon.clock.now() + timedelta(minutes=10), Budget())

    async def go():
        root = d.daemon.spans.start("task", task_id=task.id, input={})
        d.daemon.tasks.add(TaskRecord(task, d.daemon.clock.now(), root.id))
        if intent:
            d.daemon.engine.intents.record(intent)
        token = d.daemon.tokens.issue(task.id, "w1", scopes, Identity.OWNER, ttl_s=600)
        return d.daemon.tokens.validate(token), token
    return d.run(go())


async def _tree(d: RunningDaemon):
    return d.daemon.spans.tree("task-1")[0].children


async def _ledger(d: RunningDaemon, capability: str) -> list[str]:
    return [r[0] for r in d.daemon.db.conn.execute("SELECT state FROM action_ledger WHERE capability = ?",
                                                    (capability,)).fetchall()]


def call(d: RunningDaemon, grant, name: str, **args):
    return d.run(d.daemon.gateway.call(grant, name, args))


def test_tools_listed_only_within_scopes_and_unknown_refused(daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"fs:read"}, None, [send])
    names = {t["name"] for t in daemon.daemon.gateway.tools_for(grant)}
    assert "fs.read" in names and "fs.write" not in names and "fake.send" not in names
    r = call(daemon, grant, "fake.send")
    assert r["is_error"] and "not available" in r["content"]["error"] and send.calls == 0
    assert r["footer"].startswith("[now ")


def test_block_without_intent_is_recorded_on_the_span(daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, None, [send])
    r = call(daemon, grant, "fake.send")
    assert r["is_error"] and "blocked by FERB policy" in r["content"]["error"] and send.calls == 0
    spans = daemon.run(_tree(daemon))
    assert spans[0].span.error.startswith("blocked (intent)")


def test_ask_then_approve_then_same_call_runs_once_with_ledger(daemon: RunningDaemon) -> None:
    pay = FakeCap("fake.pay", "pay", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, RequestIntent.from_plan("req-1", {"buy"}), [pay])
    r = call(daemon, grant, "fake.pay")
    ap = r["content"]["needs_owner_approval"]
    assert r["is_error"] and pay.calls == 0
    daemon.call_soon(daemon.daemon.engine.approvals.resolve, ap, True)
    r2 = call(daemon, grant, "fake.pay")
    assert not r2["is_error"] and pay.calls == 1
    assert daemon.run(_ledger(daemon, "fake.pay")) == ["done"]


def test_send_waits_out_the_undo_window_and_undo_cancels(daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"},
                     RequestIntent.from_plan("req-1", {"email"}, recipients=frozenset({"alice@x.com"})), [send])
    fut = __import__("asyncio").run_coroutine_threadsafe(daemon.daemon.gateway.call(grant, "fake.send", {}),
                                                         daemon.loop)
    time.sleep(0.3)
    assert send.calls == 0 and not fut.done()                       # held
    daemon.call_soon(daemon.daemon.clock.advance, 11)                # window passes -> released
    assert not fut.result(5)["is_error"] and send.calls == 1
    fut2 = __import__("asyncio").run_coroutine_threadsafe(
        daemon.daemon.gateway.call(grant, "fake.send", {"text": "second"}), daemon.loop)
    time.sleep(0.3)
    hold = daemon.daemon.engine.holds.pending()[0]
    daemon.call_soon(daemon.daemon.engine.undo, hold.id)
    r = fut2.result(5)
    assert r["is_error"] and "cancelled by the owner" in r["content"]["error"] and send.calls == 1


def test_reading_untrusted_content_removes_shell_like_tools(daemon: RunningDaemon) -> None:
    web = FakeCap("fake.read_web", "read", {"browser:read"}, reads=(("web", "https://evil.example"),))
    shell = FakeCap("fake.shell", "run_shell", {"shell"})
    grant, _ = setup(daemon, {"browser:read", "shell"},
                     RequestIntent.from_plan("req-1", {"run_command"}), [web, shell])
    assert "fake.shell" in {t["name"] for t in daemon.daemon.gateway.tools_for(grant)}
    assert not call(daemon, grant, "fake.read_web")["is_error"]
    assert "fake.shell" not in {t["name"] for t in daemon.daemon.gateway.tools_for(grant)}
    assert call(daemon, grant, "fake.shell")["is_error"] and shell.calls == 0


def test_exception_after_start_is_unknown_and_a_blind_retry_is_refused(daemon: RunningDaemon) -> None:
    boom = FakeCap("fake.send2", "post", {"browser:act"}, boom=True)
    grant, _ = setup(daemon, {"browser:act"},
                     RequestIntent.from_plan("req-1", {"post"}, recipients=frozenset({"alice@x.com"})), [boom])
    daemon.call_soon(daemon.daemon.clock.advance, 0)
    fut = __import__("asyncio").run_coroutine_threadsafe(daemon.daemon.gateway.call(grant, "fake.send2", {}),
                                                         daemon.loop)
    time.sleep(0.3)
    daemon.call_soon(daemon.daemon.clock.advance, 11)
    r = fut.result(5)
    assert r["is_error"] and "outcome unknown" in r["content"]["error"], r
    assert daemon.run(_ledger(daemon, "fake.send2")) == ["unknown"]
    fut = __import__("asyncio").run_coroutine_threadsafe(daemon.daemon.gateway.call(grant, "fake.send2", {}),
                                                         daemon.loop)
    time.sleep(0.3)
    daemon.call_soon(daemon.daemon.clock.advance, 11)
    r2 = fut.result(5)
    assert r2["is_error"] and "reconcile" in r2["content"]["error"] and boom.calls == 1


def test_revoked_token_cannot_call_tools_through_the_real_shim(daemon: RunningDaemon) -> None:
    """Safety test 21.2 over MCP: a real `ferb mcp` process, then `ferb stop` revokes everything."""
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client

    grant, token = setup(daemon, {"fs:read", "fs:write"}, RequestIntent.from_plan("req-1", {"edit_file"}), [])
    params = StdioServerParameters(command=sys.executable, args=["-m", "ferb.cli.mcp"],
                                   env={**os.environ, "FERB_RUN_TOKEN": token, "FERB_HOME": str(daemon.daemon.home)})
    out: dict = {}

    async def scenario() -> None:
        async with stdio_client(params) as (r, w), ClientSession(r, w) as s:
            await s.initialize()
            out["tools"] = sorted(t.name for t in (await s.list_tools()).tools)
            ok = await s.call_tool("fs_write", {"path": "x.txt", "content": "hi"})
            out["before"] = (ok.is_error, ok.content[0].text)
            from ferb.cli.client import Client
            c = Client(daemon.sock)
            t0 = time.monotonic()
            c.call("stop.engage")
            refused = await s.call_tool("fs_write", {"path": "y.txt", "content": "no"})
            out["after"] = (refused.is_error, refused.content[0].text, time.monotonic() - t0)
            c.call("stop.lift")
            c.close()

    anyio.run(scenario)
    assert out["tools"] == ["fs_edit", "fs_list", "fs_read", "fs_trash", "fs_write"]
    assert out["before"][0] is False and "[now " in out["before"][1], out           # result + time footer
    assert out["after"][0] is True and "revoked" in out["after"][1] and out["after"][2] < 1.0
    assert (daemon.daemon.home / "workspace" / "x.txt").exists()
    assert not (daemon.daemon.home / "workspace" / "y.txt").exists()
