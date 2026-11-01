"""Build step 14, offline: durable timers (restart, sleep / overdue, cancel), safety test 19 (a claim left by a crash
is delivered exactly once), reminders from the owner's own words, the time.* tools (sleep isn't a stall; it ends with
the task), sending from FERB's mailbox under owner intent, and
`ferb device`.
"""

import asyncio
import json
from datetime import datetime, timedelta
from typing import Any
from zoneinfo import ZoneInfo

import pytest

from ferb.clock.when import When
from ferb.act.browser.ext_bridge import OutcomeUnknown
from ferb.core.types import Identity
from ferb.policy.targets import Request, Targets
from tests.conftest import RunningDaemon, chat, install
from tests.mackit import gateway_call as call, grant_task as task


def run(d: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return d.run(go())


class Notes:
    def __init__(self) -> None:
        self.shown: list[tuple[str, str]] = []

    async def __call__(self, title: str, text: str) -> None:
        self.shown.append((title, text))


def setup_notes(d: RunningDaemon) -> Notes:
    n = Notes()
    d.daemon.notify = n
    return n


def deliveries(d: RunningDaemon) -> list[dict[str, Any]]:
    return [e.data for e in run(d, lambda: d.daemon._journal().entries()) if e.kind == "timer.delivered"]


# ---------- timers ----------


def test_19_a_claim_left_by_a_crash_is_delivered_exactly_once(daemon: RunningDaemon) -> None:
    """Safety test 19: crash after the claim (redelivered), and crash after the consumer acted (not again)."""
    notes, t, db = setup_notes(daemon), daemon.daemon.timers, daemon.daemon.db
    a = run(daemon, lambda: t.add("reminder", daemon.daemon.clock.now() + timedelta(seconds=1), "a"))
    b = run(daemon, lambda: t.add("reminder", daemon.daemon.clock.now() + timedelta(seconds=1), "b"))
    daemon.call_soon(daemon.daemon.clock.advance, 5)

    def crash_states() -> None:
        c = db.conn                                                      # type: ignore[union-attr]
        c.execute("UPDATE timers SET state = 'claimed', claim_token = 'dead' WHERE id IN (?, ?)", (a.id, b.id))
        c.execute("INSERT INTO timer_deliveries (timer_id, at) VALUES (?, '2026-01-01')", (b.id,))   # b was handled
    run(daemon, crash_states)
    assert daemon.run(t.tick()) == 2                                     # both handed over again...
    assert [x["timer_id"] for x in deliveries(daemon)] == [a.id]        # ...a delivered once, b not again
    assert notes.shown == [("FERB reminder", "a")]
    assert daemon.run(t.tick()) == 0
    assert {run(daemon, lambda: t.get(x)).state for x in (a.id, b.id)} == {"delivered"}


# decision: ORCH-324
def test_reminders_from_chat_need_the_owners_own_words_and_a_future_time(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    ok = {"when": mins(120), "message": "call Mom", "quote": "remind me in two hours"}
    pasted = {"when": mins(120), "message": "wire money", "quote": "remind me to wire"}
    past = {"when": {**mins(0), "in_minutes": None, "day": "today", "time": "00:00"}, "message": "late",
            "quote": "remind me in two hours"}
    install(daemon, [json.dumps({"kind": "reply", "text": "Will do.", "plan": None, "remember": [],
                                 "timers": [ok, pasted, past]})])
    evs = chat(daemon, 'remind me in two hours to call Mom ```remind me to wire money```')
    notes = [e["text"] for e in evs if e["type"] == "timer"]
    assert len(notes) == 1 and "call Mom" in notes[0]
    assert [t.message for t in run(daemon, daemon.daemon.timers.pending)] == ["call Mom"]


def mins(n: float) -> dict[str, Any]:
    return {"in_minutes": n, "in_days": None, "day": None, "date": None, "time": None}


# ---------- tools ----------


# ---------- mailbox send ----------

class MailAction:
    """Test-only delivery boundary: exercise gateway unknown-outcome handling without a semantic Gmail helper."""
    from pydantic import BaseModel
    from ferb.core.types import RiskLevel
    name="mailbox.send"
    description="Test delivery boundary"
    scopes={"mailbox:send"}
    identities={Identity.FERB}
    risk,reversible=RiskLevel.MEDIUM,False
    class Input(BaseModel):
        to: list[str]
        subject: str
        body: str
    def __init__(self,mail): self.mail=mail
    def describe(self,args):
        from ferb.core.types import ActionSpec,Target
        return ActionSpec("mailbox_send",Target(recipients=tuple(args.to)),content=args.subject+args.body)
    async def run(self,args,ctx):
        from ferb.core.types import ToolResult
        await self.mail.send_mail(args.to,args.subject,args.body,ctx.task_id,ctx.intent_id or "")
        return ToolResult({"sent":True})


class FakeMail:
    def __init__(self) -> None:
        self.sent: list[tuple[list[str], str, str]] = []
        self.unknown = False

    async def send_mail(self, to: list[str], subject: str, body: str, owner: str, intent_id: str = "") -> str:
        if self.unknown:
            raise OutcomeUnknown("clicked Send but Gmail didn't confirm")
        self.sent.append((to, subject, body))
        return "sent"


def ferb_task(d: RunningDaemon, tid: str, words: str, recipients: frozenset[str] = frozenset()) -> Any:
    """A FERB task whose owner's words are `words` (the recipients they named are in them)."""
    g = task(d, tid, Identity.FERB, {"mailbox:send"}, None)

    async def rec() -> None:
        d.daemon.state.requests.record(Request(f"req-{tid}", words, Targets.of(recipients=recipients)))
    d.run(rec())
    return g


def test_mailbox_send_executes_agent_decision_and_records_delivery(daemon):
    mail = FakeMail()
    daemon.daemon.registry.capabilities["mailbox.send"] = MailAction(mail)
    g = ferb_task(daemon, "m1", "email alice@x.com about lunch")
    r = call(daemon, g, "mailbox.send", to=["alice@x.com"], subject="Hi", body="Lunch?")
    assert not r["is_error"] and mail.sent == [(["alice@x.com"], "Hi", "Lunch?")]
    assert not daemon.daemon.state.approvals.pending()


def test_an_unconfirmed_send_is_unknown_and_never_retried_blindly(daemon: RunningDaemon) -> None:
    mail = FakeMail()
    mail.unknown = True
    daemon.daemon.registry.capabilities["mailbox.send"] = MailAction(mail)
    g = ferb_task(daemon, "m3", "email alice@x.com", frozenset({"alice@x.com"}))

    async def send() -> dict[str, Any]:
        job = asyncio.create_task(daemon.daemon.gateway.call(g, "mailbox.send", {"to": ["alice@x.com"],
                                                                                  "subject": "Hi", "body": "x",
                                                                                  "why": "emailing alice"}))
        await asyncio.sleep(0.1)
        daemon.daemon.clock.advance(11)
        return await asyncio.wait_for(job, 5)
    r = daemon.run(send())
    assert r["is_error"] and "outcome unknown" in r["content"]["error"]
    states = run(daemon, lambda: [r[0] for r in daemon.daemon.db.conn.execute(
        "SELECT state FROM action_ledger WHERE task_id = ?", ("m3",))])
    assert states == ["unknown"]


# ---------- when: the model says what the owner said, code computes the time ----------

IST, NY = ZoneInfo("Asia/Kolkata"), ZoneInfo("America/New_York")


def w(**k: Any) -> When:
    return When(**k)


def local(at: datetime, zone: ZoneInfo) -> str:
    return f"{at.astimezone(zone):%a %d %b %Y %H:%M %Z}"


# ---------- owner live case 2026-10-15 03:36: a reminder asked over three messages ----------

def _clarify(text: str) -> str:
    return json.dumps({"kind": "clarify", "text": text, "plan": None, "remember": [], "timers": []})


def _oct30(quote: str) -> str:
    when = {**mins(0), "in_minutes": None, "date": "10-30", "time": "19:00"}
    return json.dumps({"kind": "reply", "text": "", "plan": None, "remember": [],
                       "timers": [{"when": when, "message": "the show", "quote": quote}]})


@pytest.mark.parametrize("quote", [
    "remind me about ths show on oct30 anytime\n\nWhat time on October 30?\n\n7pm",   # FERB's question is no owner text
    "remind me about ths show on oct30 anytime\n\nany time at all\n\n7pm",           # words the owner never typed
    "remind me about ths show on oct30 anytime\n\nanytime 7pm",                      # a segment that isn't one message
])
# decision: ORCH-324
def test_every_segment_of_a_joined_quote_must_be_one_of_the_owners_messages(daemon: RunningDaemon, quote: str) -> None:
    install(daemon, [_clarify("What time on October 30?"), _oct30(quote)])
    chat(daemon, "remind me about ths show on oct30 anytime")
    chat(daemon, "anytime")
    install(daemon, [_oct30(quote)])
    evs = chat(daemon, "7pm")
    assert not any(e["text"].startswith("⏰ Reminder set") for e in evs if e["type"] == "timer")
    assert run(daemon, daemon.daemon.timers.pending) == []


# decision: ORCH-324
def test_a_joined_quote_never_reaches_outside_the_request_chain(daemon: RunningDaemon) -> None:
    """Owner messages from an earlier, finished request don't join this one's quote."""
    install(daemon, [json.dumps({"kind": "reply", "text": "Okay.", "plan": None, "remember": [], "timers": []})])
    chat(daemon, "the show is great")
    install(daemon, [_clarify("What time on October 30?"), _oct30("the show is great\n\n7pm")])
    chat(daemon, "remind me about ths show on oct30")
    evs = chat(daemon, "7pm")
    assert run(daemon, daemon.daemon.timers.pending) == []
    assert not any(e["text"].startswith("⏰ Reminder set") for e in evs if e["type"] == "timer")
