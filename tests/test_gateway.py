"""Gateway (section 7 step 6) against a real daemon with fake capabilities, and safety test 21.2 through a real
`ferb mcp` shim process (revoked token -> no tool calls, within a second)."""

import os
import sys
import time
from datetime import timedelta

import anyio
import pytest
from pydantic import BaseModel

from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, Task, ToolContext, ToolResult
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
                d.daemon.clock.now() + timedelta(minutes=10))

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


def test_the_call_waits_for_the_owner_and_payment_asks_every_time(daemon: RunningDaemon) -> None:
    """10.3: the tool call waits for the answer and carries on; a payment (the irreversible floor) asks every time,
    even the same payment in the same request; declining blocks without running."""
    from tests.conftest import Answers
    pay = FakeCap("fake.pay", "pay", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, RequestIntent.from_plan("req-1", {"buy"}), [pay])
    yes = Answers(daemon)
    try:
        assert not call(daemon, grant, "fake.pay")["is_error"] and pay.calls == 1
        assert not call(daemon, grant, "fake.pay")["is_error"] and pay.calls == 2
        assert len(yes.asked) == 2                                          # asked both times
    finally:
        yes.stop()
    assert daemon.run(_ledger(daemon, "fake.pay")) == ["done", "done"]
    no = Answers(daemon, granted=False)
    try:
        r = call(daemon, grant, "fake.pay")
        assert r["is_error"] and "declined" in r["content"]["error"] and pay.calls == 2
    finally:
        no.stop()


def test_one_approval_covers_the_same_target_and_type_for_the_rest_of_the_request(daemon: RunningDaemon) -> None:
    from tests.conftest import Answers
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"},
                     RequestIntent.from_plan("req-1", {"email"}, recipients=frozenset({"alice@x.com"})), [send])
    yes = Answers(daemon)
    import asyncio as aio

    async def send_to(to: str, text: str) -> dict:
        job = aio.create_task(daemon.daemon.gateway.call(grant, "fake.send", {"to": to, "text": text}))
        for _ in range(50):                                              # past the undo window
            await aio.sleep(0.05)
            daemon.daemon.clock.advance(1)
            if job.done():
                break
        return await aio.wait_for(job, 5)
    try:
        for text in ("one", "two", "three"):                             # an unnamed recipient: asked once
            assert not daemon.run(send_to("bob@y.com", text), timeout=20)["is_error"]
        scope = [ap for ap in yes.asked if ap.kind != "send_again"]
        assert len(scope) == 1 and send.calls == 3
        again = [ap for ap in yes.asked if ap.kind == "send_again"]     # one message per person per task (9.4)
        assert len(again) == 2 and "“one”" in again[0].reason and "“two”" in again[0].reason
        assert not daemon.run(send_to("carol@z.com", "x"), timeout=20)["is_error"]
        assert len([ap for ap in yes.asked if ap.kind != "send_again"]) == 2   # a different recipient: a new question
    finally:
        yes.stop()


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
    from tests.conftest import Answers
    yes = Answers(daemon)                                            # "send alice another one?" (9.4): yes
    try:
        fut2 = __import__("asyncio").run_coroutine_threadsafe(
            daemon.daemon.gateway.call(grant, "fake.send", {"text": "second"}), daemon.loop)
        time.sleep(0.5)
        hold = daemon.daemon.engine.holds.pending()[0]
        daemon.call_soon(daemon.daemon.engine.undo, hold.id)
        r = fut2.result(5)
    finally:
        yes.stop()
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
