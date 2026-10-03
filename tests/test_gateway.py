"""Gateway execution: scopes/tokens, optional annotations, ledger, redaction, preserved tools and stop."""

import os
import sys
import time
from dataclasses import replace
from datetime import timedelta

import anyio
import pytest
from pydantic import BaseModel

from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, Task, ToolContext, ToolResult
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.targets import Request, Targets
from tests.conftest import Answers, RunningDaemon


class FakeCap:
    identities = {Identity.OWNER}
    risk, reversible = RiskLevel.MEDIUM, False

    class Input(BaseModel):
        to: str = "alice@x.com"
        text: str = "hi"

    def __init__(self, name: str, kind: str, scopes: set[str], boom: bool = False,
                 reads: tuple[tuple[str, str], ...] = (), site: str | None = None) -> None:
        self.name, self.description, self.kind, self.scopes = name, f"fake {kind}", kind, scopes
        self.boom, self.reads, self.site, self.calls = boom, reads, site, 0

    def describe(self, args: BaseModel) -> ActionSpec:
        to = getattr(args, "to")
        return ActionSpec(self.kind, Target(recipients=(to,) if to else (), site=self.site), content=getattr(args, "text"))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        self.calls += 1
        if self.boom:
            raise ConnectionError("lost ack after Send")
        return ToolResult({"ok": True, "n": self.calls}, read_sources=self.reads)


def request(words: str = "", **targets: object) -> Request:
    """The facts the gateway keeps about a request: the owner's own words and the targets the model understood."""
    return Request("req-1", words, Targets.of(**targets))   # type: ignore[arg-type]


def setup(d: RunningDaemon, scopes: set[str], req: Request | None, caps: list[FakeCap],
          identity: Identity = Identity.OWNER):
    for c in caps:
        d.daemon.registry.capabilities[c.name] = c
    task = Task("task-1", None, "g", "dev", identity, scopes, "worker", "req-1", [],
                d.daemon.clock.now() + timedelta(minutes=10))

    async def go():
        root = d.daemon.spans.start("task", task_id=task.id, input={})
        d.daemon.tasks.add(TaskRecord(task, d.daemon.clock.now(), root.id))
        if req is not None:
            d.daemon.engine.requests.record(req)
        token = d.daemon.tokens.issue(task.id, "w1", scopes, identity, ttl_s=600)
        return d.daemon.tokens.validate(token), token
    return d.run(go())


async def _tree(d: RunningDaemon):
    return d.daemon.spans.tree("task-1")[0].children


async def _ledger(d: RunningDaemon, capability: str) -> list[str]:
    return [r[0] for r in d.daemon.db.conn.execute("SELECT state FROM action_ledger WHERE capability = ?",
                                                    (capability,)).fetchall()]


def call(d: RunningDaemon, grant, name: str, **args):
    return d.run(d.daemon.gateway.call(grant, name, args), timeout=20)


def past_undo(d: RunningDaemon, grant, name: str, **args):
    """A call that may be held for undo: the fake clock is moved past the window while it waits."""
    import asyncio as aio

    async def go() -> dict:
        job = aio.create_task(d.daemon.gateway.call(grant, name, args))
        for _ in range(60):
            await aio.sleep(0.05)
            d.daemon.clock.advance(1)
            if job.done():
                break
        return await aio.wait_for(job, 5)
    return d.run(go(), timeout=20)


def test_tools_listed_only_within_scopes_and_unknown_refused(daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"fs:read"}, None, [send])
    names = {t["name"] for t in daemon.daemon.gateway.tools_for(grant)}
    assert "fs.read" in names and "fs.write" not in names and "fake.send" not in names
    assert "owner.ask" in names                                                 # every identity, no scopes
    r = call(daemon, grant, "fake.send")
    assert r["is_error"] and "not available" in r["content"]["error"] and send.calls == 0
    assert r["footer"].startswith("[now ")


def test_act_tools_offer_optional_effect_and_why_annotations(daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act", "fs:read"}, request("email alice@x.com"), [send])
    tools = {t["name"]: t for t in daemon.daemon.gateway.tools_for(grant)}
    assert tools["fake.send"]["input_schema"]["properties"]["effect"]["enum"] == \
        ["navigate", "edit", "send", "pay", "destroy"]
    assert "why" in tools["fake.send"]["input_schema"]["properties"]
    assert "effect" not in tools["fs.read"]["input_schema"]["properties"]      # reading declares nothing
    r = call(daemon, grant, "fake.send", effect="harmless", why="x")
    assert not r["is_error"] and send.calls == 1


def test_effect_and_reason_are_annotations_not_execution_conditions(daemon):
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("send the message"), [send])
    assert not call(daemon, grant, send.name, effect="navigate")["is_error"]
    assert send.calls == 1
    assert daemon.run(_tree(daemon))[-1].span.input["effect"] == "navigate"


def test_navigate_and_edit_run_without_asking(daemon: RunningDaemon) -> None:
    look = FakeCap("fake.look", "read", {"browser:read"})
    edit = FakeCap("fake.edit", "fill_form", {"browser:act"}, site="notes.example")
    grant, _ = setup(daemon, {"browser:read", "browser:act"}, request("tidy my notes"), [look, edit])
    watch = Answers(daemon)
    try:
        assert not call(daemon, grant, "fake.look", to="")["is_error"]
        assert not call(daemon, grant, "fake.edit", to="", effect="edit")["is_error"]
    finally:
        watch.stop()
    assert watch.asked == [] and look.calls == edit.calls == 1
    assert daemon.run(_ledger(daemon, "fake.edit")) == ["done"]              # edit: a ledger row; read: none
    assert daemon.run(_ledger(daemon, "fake.look")) == []


def test_reading_untrusted_content_preserves_shell_tools(daemon: RunningDaemon) -> None:
    web = FakeCap("fake.read_web", "read", {"browser:read"}, reads=(("web", "https://evil.example"),))
    shell = FakeCap("fake.shell", "run_shell", {"shell"})
    grant, _ = setup(daemon, {"browser:read", "shell"}, request("check the page then run the build"), [web, shell])
    assert "fake.shell" in {t["name"] for t in daemon.daemon.gateway.tools_for(grant)}
    assert not call(daemon, grant, "fake.read_web", to="")["is_error"]
    assert "fake.shell" in {t["name"] for t in daemon.daemon.gateway.tools_for(grant)}
    assert not call(daemon, grant, "fake.shell", to="", effect="edit")["is_error"] and shell.calls == 1


def test_exception_is_unknown_and_gateway_records_each_agent_selected_attempt(daemon: RunningDaemon) -> None:
    boom = FakeCap("fake.send2", "post", {"browser:act"}, boom=True)
    grant, _ = setup(daemon, {"browser:act"}, request("post it to alice@x.com"), [boom])
    daemon.call_soon(daemon.daemon.clock.advance, 0)
    r = past_undo(daemon, grant, "fake.send2", why="posting")
    assert r["is_error"] and "outcome unknown" in r["content"]["error"], r
    assert daemon.run(_ledger(daemon, "fake.send2")) == ["unknown"]
    r2 = past_undo(daemon, grant, "fake.send2", why="posting")
    assert r2["is_error"] and "unknown" in r2["content"]["error"] and boom.calls == 2
    assert daemon.run(_ledger(daemon, "fake.send2")) == ["unknown", "unknown"]


def test_revoked_token_cannot_call_tools_through_the_real_shim(daemon: RunningDaemon) -> None:
    """Safety test 21.2 over MCP: a real `ferb mcp` process, then `ferb stop` revokes everything."""
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client

    grant, token = setup(daemon, {"fs:read", "fs:write"}, request("write x.txt"), [])
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
    assert out["tools"] == ["actions_search", "fs_edit", "fs_list", "fs_read", "fs_trash", "fs_write", "owner_ask"]
    assert out["before"][0] is False and "[now " in out["before"][1], out           # result + time footer
    assert out["after"][0] is True and "revoked" in out["after"][1] and out["after"][2] < 1.0
    assert (daemon.daemon.home / "workspace" / "x.txt").exists()
    assert not (daemon.daemon.home / "workspace" / "y.txt").exists()
