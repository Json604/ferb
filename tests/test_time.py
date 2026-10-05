"""Build step 14, offline: durable timers (restart, sleep / overdue, cancel), safety test 19 (a claim left by a crash
is delivered exactly once), reminders from the owner's own words, the time.* tools (sleep isn't a stall; it ends with
the task), sending from FERB's mailbox under owner intent, and
`ferb device`.
"""

import asyncio
import io
import json
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo

import pytest

from ferb.clock.when import When, WhenError, resolve
from ferb.act.browser.ext_bridge import OutcomeUnknown
from ferb.clock.timers import Timers
from ferb.core.types import Identity
from ferb.policy.targets import Request, Targets
from ferb.store.db import Database
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

def test_a_reminder_fires_once_when_due_everywhere_it_should(daemon: RunningDaemon) -> None:
    notes, t = setup_notes(daemon), daemon.daemon.timers
    tm = run(daemon, lambda: t.add("reminder", daemon.daemon.clock.now() + timedelta(minutes=5), "call Mom"))
    assert daemon.run(t.tick()) == 0                                     # not yet
    daemon.call_soon(daemon.daemon.clock.advance, 301)
    assert daemon.run(t.tick()) == 1 and daemon.run(t.tick()) == 0     # once
    assert notes.shown == [("FERB reminder", "call Mom")]
    assert [x["timer_id"] for x in deliveries(daemon)] == [tm.id]
    from ferb.cli.client import Client
    c = Client(daemon.sock)
    try:
        assert [r["message"] for r in c.call("reminders.unseen")["reminders"]] == ["call Mom"]
        assert c.call("reminders.unseen")["reminders"] == []           # shown once in chat
    finally:
        c.close()


def test_timers_survive_a_restart_and_fire_late_with_overdue(tmp_path: Path) -> None:
    from ferb.clock.clock import FakeClock
    from ferb.core.ids import SequentialIds
    db = Database(tmp_path / "f.db")
    db.migrate()
    clock = FakeClock()
    fired: list[Any] = []

    async def consume(t: Any) -> None:
        fired.append(t)
    t1 = Timers(db, clock, SequentialIds(), consume)
    t1.add("reminder", clock.now() + timedelta(minutes=10), "stretch")
    gone = t1.add("reminder", clock.now() + timedelta(minutes=10), "cancelled")
    t1.cancel(gone.id)
    db.close()
    clock.advance(3600)                                                  # FERB down / the Mac asleep for an hour
    db2 = Database(tmp_path / "f.db")
    t2 = Timers(db2, clock, SequentialIds(), consume)
    assert asyncio.run(t2.tick()) == 1
    assert [f.message for f in fired] == ["stretch"] and round(fired[0].overdue_s) == 3000


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

def test_time_tools_set_reminders_and_sleep_without_counting_as_stuck(daemon: RunningDaemon) -> None:
    setup_notes(daemon)
    g = task(daemon, "tt", Identity.OWNER, {"time"}, {"research"})
    r = call(daemon, g, "time.remind_at", in_minutes=60, message="check the oven")
    assert not r["is_error"] and r["content"]["timer"].startswith("tm-")
    assert not call(daemon, g, "time.set_timer", minutes=20, message="tea")["is_error"]
    assert call(daemon, g, "time.remind_at", date="2020-01-01", message="x")["is_error"]           # in the past
    assert call(daemon, g, "time.remind_at", message="x")["is_error"]                              # no time at all
    assert call(daemon, g, "time.sleep_until", in_days=1)["is_error"]                              # past the limit
    waits: list[bool] = []
    sup = daemon.daemon.supervisor
    real = sup.activity.waiting
    sup.activity.waiting = lambda w, on: waits.append(on) or real(w, on)   # type: ignore[method-assign]
    wake = 5

    async def sleep_then_fire() -> dict[str, Any]:
        job = asyncio.create_task(daemon.daemon.gateway.call(g, "time.sleep_until", {"in_minutes": wake}))
        await asyncio.sleep(0.05)
        daemon.daemon.clock.advance(301)
        await daemon.daemon.timers.tick()
        return await asyncio.wait_for(job, 5)
    r = daemon.run(sleep_then_fire())
    assert not r["is_error"] and "woke_at" in r["content"] and waits == [True, False]


def test_a_tasks_sleep_ends_with_the_task_but_its_reminders_stay(daemon: RunningDaemon) -> None:
    t = daemon.daemon.timers
    now = daemon.daemon.clock.now()
    s = run(daemon, lambda: t.add("sleep", now + timedelta(minutes=5), task_id="x1"))
    r = run(daemon, lambda: t.add("reminder", now + timedelta(minutes=5), "keep", task_id="x1"))
    run(daemon, lambda: t.cancel_task("x1"))
    assert run(daemon, lambda: t.get(s.id)).state == "cancelled" and run(daemon, lambda: t.get(r.id)).state == "pending"


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
    states = run(daemon, lambda: [x.intent.state for x in daemon.daemon.ledger.for_task("m3")])
    assert states == ["unknown"]


# ---------- ferb device ----------

def test_ferb_device_shows_the_profile_with_current_driver_status(tmp_path: Path) -> None:
    from ferb.cli.admin import device
    from ferb.config import Config
    from ferb.device.profile import ProfileStore
    from ferb.device.scan import scan
    home = tmp_path / "h"
    Config(home).set("device", "id", "dev1")
    ProfileStore(home).save(scan("dev1"), "test")
    out = io.StringIO()

    def no_daemon() -> Any:
        raise ConnectionError
    assert device(home, "show", no_daemon, out) == 0
    text = out.getvalue()
    assert "apps" in text and "not built yet" not in text


# ---------- when: the model says what the owner said, code computes the time ----------

IST, NY = ZoneInfo("Asia/Kolkata"), ZoneInfo("America/New_York")


def w(**k: Any) -> When:
    return When(**k)


def local(at: datetime, zone: ZoneInfo) -> str:
    return f"{at.astimezone(zone):%a %d %b %Y %H:%M %Z}"


def test_next_friday_and_weekdays() -> None:
    wed = datetime(2026, 9, 30, 12, 0, tzinfo=IST)                                   # Wednesday
    assert local(resolve(w(day="friday", time="09:00"), wed, IST), IST) == "Fri 02 Oct 2026 09:00 IST"
    assert local(resolve(w(day="next_friday", time="17:00"), wed, IST), IST) == "Fri 09 Oct 2026 17:00 IST"
    fri = datetime(2026, 10, 2, 10, 0, tzinfo=IST)
    assert local(resolve(w(day="friday", time="17:00"), fri, IST), IST) == "Fri 02 Oct 2026 17:00 IST"  # still ahead
    assert local(resolve(w(day="friday", time="09:00"), fri, IST), IST) == "Fri 09 Oct 2026 09:00 IST"  # gone: next
    assert local(resolve(w(day="next_friday", time="09:00"), fri, IST), IST) == "Fri 09 Oct 2026 09:00 IST"
    sun = datetime(2026, 10, 4, 10, 0, tzinfo=IST)
    assert local(resolve(w(day="next_monday", time="09:00"), sun, IST), IST) == "Mon 05 Oct 2026 09:00 IST"
    assert local(resolve(w(time="08:00"), wed, IST), IST) == "Thu 01 Oct 2026 08:00 IST"   # a time alone: next one


def test_end_of_month_and_dates() -> None:
    sep30 = datetime(2026, 9, 30, 12, 0, tzinfo=IST)
    assert local(resolve(w(day="tomorrow", time="09:00"), sep30, IST), IST) == "Thu 01 Oct 2026 09:00 IST"
    assert local(resolve(w(in_days=2, time="09:00"), sep30, IST), IST) == "Fri 02 Oct 2026 09:00 IST"
    assert local(resolve(w(day="end_of_month", time="17:00"), sep30, IST), IST) == "Wed 30 Sep 2026 17:00 IST"
    assert local(resolve(w(day="end_of_month", time="09:00"), sep30, IST), IST) == "Sat 31 Oct 2026 09:00 IST"
    leap = datetime(2028, 2, 10, 12, 0, tzinfo=IST)
    assert local(resolve(w(day="end_of_month", time="09:00"), leap, IST), IST) == "Tue 29 Feb 2028 09:00 IST"
    assert local(resolve(w(date="10-31", time="18:30"), sep30, IST), IST) == "Sat 31 Oct 2026 18:30 IST"
    assert local(resolve(w(date="01-15", time="09:00"), sep30, IST), IST) == "Fri 15 Jan 2027 09:00 IST"
    for bad in (w(date="02-30"), w(date="2026-01-01"), w(), w(time="25:00"), w(in_minutes=-5),
                w(date="2028-01-01")):                                              # impossible, past, none, missing time
        with pytest.raises(WhenError):
            resolve(bad, sep30, IST)


def test_across_daylight_saving_changes() -> None:
    before_fall = datetime(2026, 10, 31, 12, 0, tzinfo=NY)                          # EDT; clocks go back 1 Nov
    assert local(resolve(w(day="tomorrow", time="09:00"), before_fall, NY), NY) == "Sun 01 Nov 2026 09:00 EST"
    assert local(resolve(w(in_minutes=24 * 60), before_fall, NY), NY) == "Sun 01 Nov 2026 11:00 EST"  # exact span
    before_spring = datetime(2026, 3, 7, 12, 0, tzinfo=NY)                          # EST; clocks go forward 8 Mar
    assert local(resolve(w(day="tomorrow", time="09:00"), before_spring, NY), NY) == "Sun 08 Mar 2026 09:00 EDT"
    assert local(resolve(w(day="tomorrow", time="02:30"), before_spring, NY), NY) == "Sun 08 Mar 2026 03:30 EDT"


def test_the_confirmation_shows_the_time_code_computed(daemon: RunningDaemon) -> None:
    when = {**mins(0), "in_minutes": None, "day": "tomorrow", "time": "17:00"}
    install(daemon, [json.dumps({"kind": "reply", "text": "Will do.", "plan": None, "remember": [],
                                 "timers": [{"when": when, "message": "call Mom", "quote": "tomorrow at 5 pm"}]})])
    notes = [e["text"] for e in chat(daemon, "remind me tomorrow at 5 pm to call Mom") if e["type"] == "timer"]
    expected = resolve(When(**when), daemon.daemon.clock.now())
    assert notes == [f"⏰ reminder {run(daemon, daemon.daemon.timers.pending)[0].id} set for "
                     f"{expected.astimezone():%a %d %b %H:%M}: call Mom"]


def test_a_corrected_time_word_is_remembered_and_the_wrong_reminder_replaced(daemon: RunningDaemon) -> None:
    """Fuzzy words have no fixed rule: FERB guesses, shows the date; the owner's correction is kept as their meaning."""
    nxt = {**mins(0), "in_minutes": None, "day": "next_friday", "time": "17:00"}
    this = {**nxt, "day": "friday"}
    first = json.dumps({"kind": "reply", "text": "Will do.", "plan": None, "remember": [],
                        "timers": [{"when": nxt, "message": "call Mom", "quote": "next friday at 5"}]})
    install(daemon, [first])
    chat(daemon, "remind me next friday at 5 to call Mom")
    wrong = run(daemon, daemon.daemon.timers.pending)[0]
    fix = json.dumps({"kind": "reply", "text": "Fixed.", "plan": None,
                      "remember": [{"kind": "preference", "subject": "owner", "key": "time:next friday",
                                    "content": "the coming Friday, not next week's", "quote": "I meant this friday"}],
                      "reminder_cancels": [{"id": wrong.id, "quote": "I meant this friday"}],
                      "timers": [{"when": this, "message": "call Mom", "quote": "I meant this friday"}]})
    install(daemon, [fix])
    evs = chat(daemon, "no, by next friday I meant this friday")
    notes = [e["text"] for e in evs if e["type"] == "timer"]
    assert notes[0] == f"⏰ cancelled reminder {wrong.id}: call Mom" and notes[1].startswith("⏰ reminder")
    left = run(daemon, daemon.daemon.timers.pending)
    assert len(left) == 1 and left[0].fire_at == resolve(When(**this), daemon.daemon.clock.now())
    facts = run(daemon, daemon.daemon._facts)
    assert "When the owner says “next friday”, they mean: the coming Friday, not next week's" in facts
    assert f"Reminder {left[0].id}: call Mom" in facts


def test_a_test_reminder_survives_typo_fixing_and_a_refusal_is_told(daemon: RunningDaemon) -> None:
    """2026-10-01: "…what the reminder looks like" — the typo fix read "looks" as the Books app, the quote no longer
    matched, the reminder was dropped, and the reply still said it was set."""
    daemon.daemon.supervisor.apps = lambda: [("com.apple.iBooksX", "Books")]          # type: ignore[union-attr]
    said = "Can you send a test reminder in one minute so that I can see what the reminder looks like"
    install(daemon, [json.dumps({"kind": "reply", "text": "I'll send you a test reminder in one minute.", "plan": None,
                                 "remember": [], "timers": [{"when": mins(1), "message": "test", "quote": said}]})])
    evs = chat(daemon, said)
    assert [e["type"] for e in evs] == ["timer", "reply"] and evs[0]["text"].startswith("⏰ reminder")
    assert len(run(daemon, daemon.daemon.timers.pending)) == 1
    install(daemon, [json.dumps({"kind": "reply", "text": "I'll remind you.", "plan": None, "remember": [],
                                 "timers": [{"when": mins(1), "message": "x", "quote": "words never said"}]})])
    evs = chat(daemon, "remind me in a minute")
    assert evs[-1]["text"].startswith("I couldn't set that reminder")                # never "I'll remind you"
