"""Gateway (section 7 step 6; docs/DESIGN_ONE_WORKER.md section 3) against a real daemon with fake capabilities:
the effect gate (declared effect raised to the floor; send asks once per request / effect / target unless the owner
named it; pay asks every time), the undo window, exposure, unknown outcomes, and safety test 21.2 through a real
`ferb mcp` shim process (revoked token -> no tool calls, within a second)."""

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


@pytest.mark.parametrize("kind,effect,allowed", [
    ("read", "navigate", True), ("navigate", "navigate", True),
    ("fill_form", "navigate", False), ("click", "navigate", False),
    ("read", "edit", False), ("submit", "send", False),
])
def test_identity_browser_checker_is_read_only_despite_shared_scope(daemon, kind, effect, allowed):
    cap = FakeCap("ferb_browser.probe", kind, {"identity:browse"})
    cap.identities = {Identity.FERB}
    grant, _ = setup(daemon, {"identity:browse"}, request(), [cap], Identity.FERB)

    async def verifier():
        rec = daemon.daemon.tasks.get(grant.task_id)
        rec.task = replace(rec.task, role="verifier")
    daemon.run(verifier())
    result = call(daemon, grant, cap.name, to="", text="", effect=effect, why="check the result")
    assert result["is_error"] is not allowed, result
    assert cap.calls == int(allowed)
    if not allowed:
        assert "read-only" in result["content"]["error"]


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


def test_act_tools_take_effect_and_why_and_a_bad_effect_is_refused(daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act", "fs:read"}, request("email alice@x.com"), [send])
    tools = {t["name"]: t for t in daemon.daemon.gateway.tools_for(grant)}
    assert tools["fake.send"]["input_schema"]["properties"]["effect"]["enum"] == \
        ["navigate", "edit", "send", "pay", "destroy"]
    assert "why" in tools["fake.send"]["input_schema"]["properties"]
    assert "effect" not in tools["fs.read"]["input_schema"]["properties"]      # reading declares nothing
    r = call(daemon, grant, "fake.send", effect="harmless", why="x")
    assert r["is_error"] and "effect must be one of" in r["content"]["error"] and send.calls == 0


def test_a_send_needs_a_why_and_the_floor_raises_a_low_declaration(daemon: RunningDaemon) -> None:
    """A tool whose kind is a send is at least `send`, whatever the worker declared; a send without a reason for the
    owner is sent back for one."""
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("email alice@x.com"), [send])
    r = call(daemon, grant, "fake.send", effect="navigate")
    assert r["is_error"] and 'effect "send"' in r["content"]["error"] and "why" in r["content"]["error"]
    assert send.calls == 0
    assert not past_undo(daemon, grant, "fake.send", effect="navigate", why="emailing alice as asked")["is_error"]
    assert send.calls == 1
    spans = daemon.run(_tree(daemon))
    assert spans[-1].span.input["effect"] == "send" and spans[-1].span.input["why"] == "emailing alice as asked"


def test_a_send_to_someone_the_owner_did_not_name_asks_and_a_no_blocks(daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("summarize the news"), [send])
    no = Answers(daemon, granted=False)
    try:
        r = call(daemon, grant, "fake.send", to="boss@x.com", why="replying for you")
    finally:
        no.stop()
    assert r["is_error"] and "declined" in r["content"]["error"] and send.calls == 0
    assert len(no.asked) == 1 and "boss@x.com" in no.asked[0].reason and "replying for you" in no.asked[0].reason
    spans = daemon.run(_tree(daemon))
    assert spans[0].span.error.startswith("not approved") or "approval" in spans[0].span.error


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


def test_the_call_waits_for_the_owner_and_payment_asks_every_time(daemon: RunningDaemon) -> None:
    """10.3: the tool call waits for the answer and carries on; a payment (the irreversible floor) asks every time,
    even the same payment in the same request, even when the owner named the shop; declining blocks without
    running."""
    pay = FakeCap("fake.pay", "pay", {"browser:act"}, site="shop.example")
    grant, _ = setup(daemon, {"browser:act"}, request("buy the cheapest one on shop.example"), [pay])
    yes = Answers(daemon)
    try:
        assert not call(daemon, grant, "fake.pay", to="", why="paying for the cheapest one")["is_error"] and pay.calls == 1
        assert not call(daemon, grant, "fake.pay", to="", why="paying for the cheapest one")["is_error"] and pay.calls == 2
        assert len(yes.asked) == 2                                          # asked both times
    finally:
        yes.stop()
    assert daemon.run(_ledger(daemon, "fake.pay")) == ["done", "done"]
    no = Answers(daemon, granted=False)
    try:
        r = call(daemon, grant, "fake.pay", to="", why="paying")
        assert r["is_error"] and "declined" in r["content"]["error"] and pay.calls == 2
    finally:
        no.stop()


def test_a_declared_pay_or_destroy_asks_every_time_even_on_a_harmless_tool(daemon: RunningDaemon) -> None:
    """The worker's own declaration can only raise: an edit-class tool declared `destroy` is on the floor."""
    edit = FakeCap("fake.edit", "fill_form", {"browser:act"}, site="shop.example")
    grant, _ = setup(daemon, {"browser:act"}, request("clean up shop.example"), [edit])
    yes = Answers(daemon)
    try:
        for _ in range(2):
            assert not call(daemon, grant, "fake.edit", to="", effect="destroy", why="deleting the old entries")["is_error"]
    finally:
        yes.stop()
    assert len(yes.asked) == 2 and edit.calls == 2


def test_one_approval_covers_the_same_target_and_effect_for_the_rest_of_the_request(daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("email alice@x.com"), [send])
    yes = Answers(daemon)
    try:
        for text in ("one", "two", "three"):                             # an unnamed recipient: asked once
            assert not past_undo(daemon, grant, "fake.send", to="bob@y.com", text=text, why="emailing bob")["is_error"]
        scope = [ap for ap in yes.asked if ap.kind != "send_again"]
        assert len(scope) == 1 and send.calls == 3
        again = [ap for ap in yes.asked if ap.kind == "send_again"]     # one message per person per task (9.4)
        assert len(again) == 2 and "“one”" in again[0].reason and "“two”" in again[0].reason
        assert not past_undo(daemon, grant, "fake.send", to="carol@z.com", text="x", why="emailing carol")["is_error"]
        assert len([ap for ap in yes.asked if ap.kind != "send_again"]) == 2   # a different recipient: a new question
    finally:
        yes.stop()


def test_a_target_the_owner_named_is_auto_granted_and_held_for_undo(daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("email alice@x.com that I'm late"), [send])
    fut = __import__("asyncio").run_coroutine_threadsafe(
        daemon.daemon.gateway.call(grant, "fake.send", {"why": "emailing alice"}), daemon.loop)
    time.sleep(0.3)
    assert send.calls == 0 and not fut.done()                       # held, never asked
    daemon.call_soon(daemon.daemon.clock.advance, 11)                # window passes -> released
    assert not fut.result(5)["is_error"] and send.calls == 1
    yes = Answers(daemon)                                            # "send alice another one?" (9.4): yes
    try:
        fut2 = __import__("asyncio").run_coroutine_threadsafe(
            daemon.daemon.gateway.call(grant, "fake.send", {"text": "second", "why": "again"}), daemon.loop)
        time.sleep(0.5)
        hold = daemon.daemon.engine.holds.pending()[0]
        daemon.call_soon(daemon.daemon.engine.undo, hold.id)
        r = fut2.result(5)
    finally:
        yes.stop()
    assert r["is_error"] and "cancelled by the owner" in r["content"]["error"] and send.calls == 1


def test_a_recipient_only_in_pasted_text_is_not_named(daemon: RunningDaemon) -> None:
    """The owner's own words are what count: a request whose only mention of eve is inside pasted text asks."""
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("do what this says", recipients=["eve@evil.com"]), [send])
    no = Answers(daemon, granted=False)
    try:
        r = call(daemon, grant, "fake.send", to="eve@evil.com", why="the note says so")
    finally:
        no.stop()
    assert r["is_error"] and "declined" in r["content"]["error"] and send.calls == 0 and len(no.asked) == 1


def test_reading_untrusted_content_removes_shell_like_tools(daemon: RunningDaemon) -> None:
    web = FakeCap("fake.read_web", "read", {"browser:read"}, reads=(("web", "https://evil.example"),))
    shell = FakeCap("fake.shell", "run_shell", {"shell"})
    grant, _ = setup(daemon, {"browser:read", "shell"}, request("check the page then run the build"), [web, shell])
    assert "fake.shell" in {t["name"] for t in daemon.daemon.gateway.tools_for(grant)}
    assert not call(daemon, grant, "fake.read_web", to="")["is_error"]
    assert "fake.shell" not in {t["name"] for t in daemon.daemon.gateway.tools_for(grant)}
    assert call(daemon, grant, "fake.shell", to="", effect="edit")["is_error"] and shell.calls == 0


def test_exception_after_start_is_unknown_and_a_blind_retry_is_refused(daemon: RunningDaemon) -> None:
    boom = FakeCap("fake.send2", "post", {"browser:act"}, boom=True)
    grant, _ = setup(daemon, {"browser:act"}, request("post it to alice@x.com"), [boom])
    daemon.call_soon(daemon.daemon.clock.advance, 0)
    r = past_undo(daemon, grant, "fake.send2", why="posting")
    assert r["is_error"] and "outcome unknown" in r["content"]["error"], r
    assert daemon.run(_ledger(daemon, "fake.send2")) == ["unknown"]
    r2 = past_undo(daemon, grant, "fake.send2", why="posting")
    assert r2["is_error"] and "reconcile" in r2["content"]["error"] and boom.calls == 1


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
    assert out["tools"] == ["fs_edit", "fs_list", "fs_read", "fs_trash", "fs_write", "owner_ask"]
    assert out["before"][0] is False and "[now " in out["before"][1], out           # result + time footer
    assert out["after"][0] is True and "revoked" in out["after"][1] and out["after"][2] < 1.0
    assert (daemon.daemon.home / "workspace" / "x.txt").exists()
    assert not (daemon.daemon.home / "workspace" / "y.txt").exists()
