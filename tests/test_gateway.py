"""Gateway execution: scopes/tokens, optional annotations, ledger, redaction, preserved tools and stop."""

import os
import sys
import time

import anyio
import pytest

from ferb.core.types import Identity
from tests.conftest import Answers, RunningDaemon, GatewayCap as FakeCap, gateway_request as request, gateway_setup as setup, gateway_tool_call as call
from tests.support.confirm import FerbAnswers


def test_owner_questions_accept_choices_free_text_and_expire():
    from ferb.clock.clock import FakeClock
    from ferb.daemon.questions import Approvals
    from ferb.policy.rules import ActionRequest
    clock = FakeClock()
    questions = Approvals(clock, ttl_s=60)
    action = ActionRequest('owner.ask', 'owner_question', Identity.OWNER, frozenset(), 't', 'w', 'r', frozenset())
    choice = questions.question(action, 'Which account?', ['a@x', 'b@x'])
    questions.resolve(choice.id, True, text='2')
    assert choice.choice == 'b@x'
    free = questions.question(action, 'Which account?', ['a@x', 'b@x'])
    questions.resolve(free.id, True, text='the work one')
    assert free.answer == 'the work one' and free.choice is None
    expired = questions.question(action, 'Continue?', [])
    clock.advance(61)
    with pytest.raises(ValueError, match='expired'):
        questions.resolve(expired.id, True)
    questions.forget_request('r')
    assert not questions.pending() and questions.get(choice.id) is None


async def _tree(d: RunningDaemon):
    return d.daemon.spans.tree("task-1")[0].children


async def _ledger(d: RunningDaemon, capability: str) -> list[str]:
    return [r[0] for r in d.daemon.db.conn.execute("SELECT state FROM action_ledger WHERE capability = ?",
                                                    (capability,)).fetchall()]


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


def test_effect_and_reason_are_annotations_not_execution_conditions(ferb_yes: FerbAnswers, daemon):
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
    """P19: the identical repeat is refused, not run again (the owner decides; tests/test_unknown_retry.py), and the
    refused attempt is recorded too."""
    boom = FakeCap("fake.send2", "post", {"browser:act"}, boom=True)
    grant, _ = setup(daemon, {"browser:act"}, request("post it to alice@x.com"), [boom])
    daemon.call_soon(daemon.daemon.clock.advance, 0)
    r = call(daemon, grant, "fake.send2", why="posting")
    assert r["is_error"] and "outcome unknown" in r["content"]["error"], r
    assert daemon.run(_ledger(daemon, "fake.send2")) == ["unknown"]
    r2 = call(daemon, grant, "fake.send2", why="posting")
    assert r2["is_error"] and "unknown" in r2["content"]["error"] and boom.calls == 1
    assert daemon.run(_ledger(daemon, "fake.send2")) == ["unknown"]
    attempts = [n.span for n in daemon.run(_tree(daemon)) if n.span.input.get("tool") == "fake.send2"]
    assert len(attempts) == 2 and all(s.error for s in attempts)


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
    assert out["tools"] == ["actions_search", "fs_edit", "fs_list", "fs_read", "fs_trash", "fs_write", "owner_ask",
                            "recording_inspect", "recording_stop", "recovery_snapshot"]
    assert out["before"][0] is False and "[now " in out["before"][1], out           # result + time footer
    assert out["after"][0] is True and "revoked" in out["after"][1] and out["after"][2] < 1.0
    assert (daemon.daemon.home / "workspace" / "x.txt").exists()
    assert not (daemon.daemon.home / "workspace" / "y.txt").exists()


def test_tool_call_span_records_action_kind_and_target(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("send the message"), [send])
    assert not call(daemon, grant, send.name, to="mom@example.com")["is_error"]
    spans = [r.span for r in daemon.run(_tree(daemon)) if r.span.input.get("tool") == send.name]
    assert spans[-1].input["kind"] == "send"
    assert spans[-1].input["target"]["recipients"] == ["mom@example.com"]


def test_test_mode_allowlist_blocks_unlisted_recipient_and_allows_listed(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("message people"), [send])
    daemon.daemon.gateway.test_send_allowlist = {"ok@example.com"}
    try:
        blocked = call(daemon, grant, "fake.send", to="attacker@evil.example", effect="send")
        assert blocked["is_error"] and "allowlist" in str(blocked["content"])
        ok = call(daemon, grant, "fake.send", to="ok@example.com", effect="send")
        assert not ok["is_error"]
    finally:
        daemon.daemon.gateway.test_send_allowlist = None
    assert send.calls == 1  # only the allowed one executed


def test_allowlist_refusal_leaves_no_ledger_row_and_span_carries_error(daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("message people"), [send])
    daemon.daemon.gateway.test_send_allowlist = {"ok@example.com"}
    try:
        blocked = call(daemon, grant, "fake.send", to="attacker@evil.example")
    finally:
        daemon.daemon.gateway.test_send_allowlist = None
    assert blocked["is_error"] and blocked["content"]["recipients"] == ["attacker@evil.example"]
    assert daemon.run(_ledger(daemon, "fake.send")) == []
    spans = [r.span for r in daemon.run(_tree(daemon)) if r.span.input.get("tool") == "fake.send"]
    assert "allowlist" in str(spans[-1].error) and send.calls == 0


def test_allowlist_refuses_send_without_checkable_recipient_only_in_test_mode(daemon: RunningDaemon) -> None:
    send = FakeCap("fake.send", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("message people"), [send])
    daemon.daemon.gateway.test_send_allowlist = {"ok@example.com"}
    try:
        blocked = call(daemon, grant, "fake.send", to="")
        assert blocked["is_error"] and blocked["content"]["recipients"] == [] and "recipient" in str(blocked["content"]["error"])
        assert send.calls == 0
    finally:
        daemon.daemon.gateway.test_send_allowlist = None
    assert not call(daemon, grant, "fake.send", to="")["is_error"]
    assert send.calls == 1


def test_allowlist_never_affects_non_send_kinds(daemon: RunningDaemon) -> None:
    look = FakeCap("fake.look", "read", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("look"), [look])
    daemon.daemon.gateway.test_send_allowlist = {"ok@example.com"}
    try:
        assert not call(daemon, grant, "fake.look", to="anyone@evil.example")["is_error"]
    finally:
        daemon.daemon.gateway.test_send_allowlist = None
    assert look.calls == 1


class _ContactCap(FakeCap):
    """FakeCap whose destination is `contact` (WhatsApp/iMessage style), plus optional recipients."""

    class Input(FakeCap.Input):
        contact: str = ""

    def describe(self, args):  # type: ignore[no-untyped-def]
        from ferb.core.types import ActionSpec, Target
        to = getattr(args, "to")
        return ActionSpec(self.kind, Target(recipients=(to,) if to else (), contact=getattr(args, "contact") or None),
                          content=getattr(args, "text"))


def test_allowlist_checks_contact_even_when_recipients_are_listed(daemon: RunningDaemon) -> None:
    send = _ContactCap("fake.msg", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("message"), [send])
    daemon.daemon.gateway.test_send_allowlist = {"ok@example.com"}
    try:
        blocked = call(daemon, grant, "fake.msg", to="ok@example.com", contact="+15550000")
        assert blocked["is_error"] and blocked["content"]["recipients"] == ["+15550000"] and send.calls == 0
    finally:
        daemon.daemon.gateway.test_send_allowlist = None


def test_allowlist_allows_contact_only_destination_when_listed(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    send = _ContactCap("fake.msg", "send", {"browser:act"})
    grant, _ = setup(daemon, {"browser:act"}, request("message"), [send])
    daemon.daemon.gateway.test_send_allowlist = {"+15550000"}
    try:
        assert not call(daemon, grant, "fake.msg", to="", contact="+15550000")["is_error"]
        assert send.calls == 1
    finally:
        daemon.daemon.gateway.test_send_allowlist = None
