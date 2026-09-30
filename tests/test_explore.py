"""Exploring beyond the plan (owner decisions 2026-09-30, sections 10.2 / 10.3).
- opening and reading apps / sites outside the plan is allowed with a short stated reason, recorded in the trace;
  without one it's refused; apps on the exclusion list stay refused;
- acting outside the plan's apps / sites asks the owner in the worker's plain words (plus what FERB would do, from
  code); a yes adds that app / site for the rest of the request; a no refuses it;
- never offered when policy forbids the action anyway (owner data to an unrelated site: 21.9);
- exploring counts toward the runaway guards like any tool call.
"""

import asyncio
import json
from typing import Any

from pydantic import BaseModel

from ferb.cli.chat import question
from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.policy.intent import RequestIntent
from ferb.policy.rules import Kind
from tests.conftest import Answers, RunningDaemon
from tests.test_gateway import setup

BRAVE, WA = "com.brave.browser", "net.whatsapp.whatsapp"


class At:
    """A fake tool acting on (or reading) whatever app / site it's given."""
    identities, risk, reversible = {Identity.OWNER}, RiskLevel.MEDIUM, False

    class Input(BaseModel):
        site: str
        text: str = "hi"

    def __init__(self, name: str, kind: str, scopes: set[str]) -> None:
        self.name, self.description, self.kind, self.scopes, self.calls = name, name, kind, scopes, 0

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(self.kind, Target(site=getattr(args, "site")), content=getattr(args, "text"))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        self.calls += 1
        return ToolResult({"ok": True})


def rig(d: RunningDaemon, *caps: At, verbs: frozenset[str] = frozenset({"use_app"})) -> Any:
    intent = RequestIntent.from_plan("req-1", set(verbs), sites=frozenset({BRAVE}))
    grant, _ = setup(d, {"mac:read", "mac:act"}, intent, list(caps))   # type: ignore[arg-type]
    return grant


def call(d: RunningDaemon, grant: Any, name: str, **args: Any) -> dict[str, Any]:
    return d.run(d.daemon.gateway.call(grant, name, args), timeout=20)   # type: ignore[no-any-return]


async def _spans(d: RunningDaemon) -> list[dict[str, Any]]:
    return [json.loads(r[0]) for r in d.daemon.db.conn.execute(          # type: ignore[union-attr]
        "SELECT input_json FROM spans WHERE kind = 'tool.call' ORDER BY started_at")]


def test_looking_beyond_the_plan_needs_a_stated_reason_which_is_recorded(daemon: RunningDaemon) -> None:
    look = At("fake.look", Kind.READ, {"mac:read"})
    grant = rig(daemon, look)
    r = call(daemon, grant, "fake.look", site=WA)
    assert r["is_error"] and "short reason" in r["content"]["error"] and look.calls == 0
    ok = call(daemon, grant, "fake.look", site=WA, why="Brave had no call button; checking the WhatsApp app")
    assert not ok["is_error"] and look.calls == 1
    assert not call(daemon, grant, "fake.look", site=BRAVE)["is_error"]          # in the plan: no reason needed
    spans = daemon.run(_spans(daemon))
    assert spans[-2]["why"] == "Brave had no call button; checking the WhatsApp app"
    assert "why" not in spans[-2]["args"]                                        # the tool never sees it


def test_the_tool_list_offers_a_why_field(daemon: RunningDaemon) -> None:
    grant = rig(daemon, At("fake.look", Kind.READ, {"mac:read"}))
    tool = next(t for t in daemon.daemon.gateway.tools_for(grant) if t["name"] == "fake.look")
    assert "why" in tool["input_schema"]["properties"]


def test_acting_beyond_the_plan_asks_in_plain_words_and_a_yes_extends_the_plan(daemon: RunningDaemon) -> None:
    act = At("fake.act", Kind.MAC_INPUT, {"mac:act"})
    grant = rig(daemon, act)
    daemon.daemon.gateway.name_of = lambda t: "WhatsApp" if t == WA else t
    r = call(daemon, grant, "fake.act", site=WA)
    assert r["is_error"] and "FERB asks them" in r["content"]["error"] and act.calls == 0
    why = "Your Brave had no call button. The WhatsApp app does. Call Rahul from there?"
    yes = Answers(daemon)
    try:
        assert not call(daemon, grant, "fake.act", site=WA, why=why)["is_error"] and act.calls == 1
        asked = [ap for ap in yes.asked if ap.kind == "route_change"]
        assert [ap.reason for ap in asked] == [why + " (FERB would click and type in WhatsApp.)"]
        assert WA in daemon.daemon.engine.intents.get("req-1").sites        # type: ignore[union-attr]
        assert not call(daemon, grant, "fake.act", site=WA)["is_error"]     # in the plan now: no reason, no question
        assert len([ap for ap in yes.asked if ap.kind == "route_change"]) == 1 and act.calls == 2
    finally:
        yes.stop()


def test_a_no_keeps_the_plan_as_it_was(daemon: RunningDaemon) -> None:
    act = At("fake.act", Kind.MAC_INPUT, {"mac:act"})
    grant = rig(daemon, act)
    no = Answers(daemon, granted=False)
    try:
        r = call(daemon, grant, "fake.act", site=WA, why="The WhatsApp app has a call button. Use it?")
    finally:
        no.stop()
    assert r["is_error"] and "stay within the plan" in r["content"]["error"] and act.calls == 0
    assert WA not in daemon.daemon.engine.intents.get("req-1").sites          # type: ignore[union-attr]


def test_never_offered_when_policy_forbids_it_anyway(daemon: RunningDaemon) -> None:
    from ferb.policy.rules import DataLabel
    act, pay = At("fake.act", Kind.POST, {"mac:act"}), At("fake.pay", Kind.PAY, {"mac:act"})
    grant = rig(daemon, act, pay, verbs=frozenset({"use_app", "post"}))
    daemon.daemon.gateway.taint.labels_for = lambda tree, content: frozenset(          # type: ignore[method-assign]
        {DataLabel("owner_account", "mail.google.com")})
    watch = Answers(daemon)
    try:
        r = call(daemon, grant, "fake.act", site="evil.example", text="acct 99887766554433",
                 why="The page there takes the number. Post it?")
    finally:
        watch.stop()
    assert r["is_error"] and "blocked by FERB policy" in r["content"]["error"] and act.calls == 0
    assert watch.asked == []                                                   # the owner was never asked
    r = call(daemon, grant, "fake.pay", site=WA, why="Pay there?")
    assert r["is_error"] and "was not requested" in r["content"]["error"]      # a kind never asked for


def test_excluded_apps_stay_out_even_for_looking(daemon: RunningDaemon, monkeypatch: Any) -> None:
    from tests.test_mac_shell import call as mcall, task
    from tests.test_mac_vision import _fake_mac
    _fake_mac(daemon, monkeypatch)
    g = task(daemon, "x", Identity.OWNER, {"mac:read", "mac:act", "screen"}, {"use_app"})
    for tool, args in (("mac.open", {"bundle_id": "com.apple.Passwords"}), ("mac.snapshot", {"app": "Passwords"}),
                       ("mac.screenshot", {"app": "Passwords"})):
        r = mcall(daemon, g, tool, **args, why="looking for the password")
        assert r["is_error"] and "exclusion" in r["content"]["error"], (tool, r)


def test_exploring_counts_toward_the_runaway_guards(daemon: RunningDaemon) -> None:
    look = At("fake.look", Kind.READ, {"mac:read"})
    grant = rig(daemon, look)
    counted: list[bool] = []
    real = daemon.daemon.gateway.observer

    class Count(type(real)):                                                  # type: ignore[misc]
        def tool_call(self, g: Any, done: bool) -> None:
            if not done:
                counted.append(True)
            real.tool_call(g, done)
    daemon.daemon.gateway.observer = Count.__new__(Count)
    daemon.daemon.gateway.observer.__dict__.update(real.__dict__)
    for i in range(3):
        assert not call(daemon, grant, "fake.look", site=f"site{i}.example", why="looking around")["is_error"]
    assert len(counted) == 3


def test_the_route_question_reads_plainly() -> None:
    q = question({"kind": "route_change", "reason": "Your Brave had no call button. The WhatsApp app does. Call "
                                                    "Rahul from there? (FERB would start a call in WhatsApp.)"})
    assert q == ("   ? Your Brave had no call button. The WhatsApp app does. Call Rahul from there? (FERB would "
                 "start a call in WhatsApp.) [y/N] ")
