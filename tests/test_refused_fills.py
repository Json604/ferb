"""A sealed fill the page refused (owner 2026-11-09, an application task: "i think it also doesnt communixate on a wrong pass
or code ... IT SHUODL"; then "Twice"). The site's page stayed on its code step after the owner's code, and FERB asked for a
4th and a 5th code with cards identical to the first, telling the owner nothing. Now: the page asking again for the
same field after a sealed fill and a submit is a fact; the owner is told at once with the page's own words, the next
card says the last value wasn't accepted, and after two fresh asks FERB asks no more for that field in the task."""
import asyncio
import time
from typing import Any

import pytest
from pydantic import BaseModel

from ferb.core.errors import Refused
from ferb.core.types import ActionSpec, Identity, Target, ToolContext
from ferb.policy.rules import Kind
from tests.conftest import FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call
from tests.support.hidden_input import asked

PAGE = "https://apply.example.com/retrieve/enter-code/"
FIELD = {"id": "s2e5", "role": "textbox", "label": "12345", "sensitive": True, "filled": False,
         "field": "one_time_code"}


class _Snap(FakeCap):
    """A tab's read, as browser.snapshot returns it: url, elements, text."""

    class Input(BaseModel):
        tab: str = "t1"

    def __init__(self, page: dict[str, Any]) -> None:
        super().__init__("probe.snapshot", Kind.READ, {"probe"}, reads=(("web", PAGE),), content=page)

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.READ, Target(site="apply.example.com"))


def _record(daemon: RunningDaemon, task_id: str, tool: str, args: dict[str, Any], output: dict[str, Any],
            **more: Any) -> None:
    async def go() -> None:
        spans = daemon.daemon.spans
        span = spans.start("tool.call", task_id=task_id, input={"tool": tool, "args": args} | more)
        spans.finish(span, output=output)
    daemon.run(go())


def _ctx(task_id: str, span_id: str | None = None) -> ToolContext:
    return ToolContext(task_id, "w1", "r1", Identity.OWNER, frozenset({"browser:act"}), "dev", span_id, "/tmp", (), ())


def _refused_once(daemon: RunningDaemon) -> tuple[Any, dict[str, Any]]:
    snap = _Snap({"url": PAGE, "elements": [FIELD],
                  "text": "Enter the code you just received\nYour code:\nThat code didn't work, try again later."})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("reopen my saved application"), [snap])
    tid = grant.task_id
    _record(daemon, tid, "browser.snapshot", {"tab": "t1"},
            {"url": PAGE, "elements": [FIELD], "text": "Enter the code you just received\nYour code:"})
    facts = {"field_kinds": ["one_time_code"], "page_url": PAGE, "control_label": "12345", "tab_chain": "t1"}
    _record(daemon, tid, "browser.login_code", {"tab": "t1", "element": "s1e5"}, {"ok": True, "detail": "filled"},
            facts=facts, why="Reopen the saved draft with the code just emailed to the owner")
    _record(daemon, tid, "browser.act", {"tab": "t1", "element": "s1e6", "action": "click"}, {"ok": True},
            facts={"submits_form": True, "page_url": PAGE, "tab_chain": "t1"})
    return grant, gateway_tool_call(daemon, grant, "probe.snapshot", tab="t1")


def _notices(daemon: RunningDaemon) -> list[Any]:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        rows = daemon.run(_rows(daemon))
        if rows:
            return rows
        time.sleep(0.02)
    return []


async def _rows(daemon: RunningDaemon) -> list[Any]:
    return [dict(r) for r in daemon.daemon.db.conn.execute("SELECT text, quotes_json FROM attention_notices")]


# decision: GATE-562, GATE-563
def test_a_page_that_asks_again_after_a_sealed_fill_is_told_to_the_owner_at_once(daemon: RunningDaemon) -> None:
    grant, result = _refused_once(daemon)
    fact = result["content"]["sealed_fill_refused"]
    assert fact["times"] == 1 and "That code didn't work" in fact["page_says"]
    assert "Enter the code" not in fact["page_says"]                 # only what is new on the page
    [notice] = _notices(daemon)
    assert "apply.example.com" in notice["text"] and "That code didn't work" in notice["quotes_json"]
    again = gateway_tool_call(daemon, grant, "probe.snapshot", tab="t1")   # the same refusal is told once
    assert "sealed_fill_refused" not in again["content"] and len(_notices(daemon)) == 1


# decision: GATE-559, GATE-562, GATE-563
def test_after_a_refusal_the_card_says_so_and_ferb_asks_at_most_twice_more(daemon: RunningDaemon) -> None:
    grant, _ = _refused_once(daemon)
    for _ in range(2):
        waiting = asyncio.run_coroutine_threadsafe(
            daemon.daemon.gateway.ask_login_code(_ctx(grant.task_id), "https://apply.example.com"), daemon.loop)
        ap = asked(daemon, "owner_login_code")
        try:
            assert "That code didn't work" in ap.detail                  # the page's words, on the fresh ask
        finally:
            daemon.call_soon(daemon.daemon.state.approvals.resolve, ap.id, False)
            assert waiting.result(5) is None
    with pytest.raises(Refused) as stopped:                             # a third fresh ask: none, the step stops
        daemon.run(daemon.daemon.gateway.ask_login_code(_ctx(grant.task_id), "https://apply.example.com"))
    assert "That code didn't work" in str(stopped.value)
    assert [ap for ap in daemon.daemon.state.approvals.pending() if ap.kind == "owner_login_code"] == []


# decision: GATE-559
def test_the_login_code_card_shows_the_workers_reason_and_the_pages_label(daemon: RunningDaemon) -> None:
    """Past bug (an application task, 2026-11-09): the card said only "For <the site>: FERB needs a login code." five times,
    without the worker's reason (where the code was sent) or the page's own words for the field."""
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("reopen my saved application"), [])

    async def call() -> str:
        span = daemon.daemon.spans.start("tool.call", task_id=grant.task_id, input={
            "tool": "browser.login_code", "args": {"tab": "t1", "element": "s1e5"},
            "why": "Reopen the saved draft; the site emailed the code to the owner's inbox",
            "facts": {"field_kinds": ["one_time_code"], "page_url": PAGE, "control_label": "Your One Time Passcode"}})
        return span.id
    span_id = daemon.run(call())
    waiting = asyncio.run_coroutine_threadsafe(
        daemon.daemon.gateway.ask_login_code(_ctx(grant.task_id, span_id), "https://apply.example.com"), daemon.loop)
    ap = asked(daemon, "owner_login_code")
    try:
        assert "emailed the code to the owner's inbox" in ap.why
        assert "Your One Time Passcode" in ap.detail
    finally:
        daemon.call_soon(daemon.daemon.state.approvals.resolve, ap.id, False)
        assert waiting.result(5) is None
