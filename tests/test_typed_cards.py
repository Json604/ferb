"""Step 8e end-to-end runs (owner 2026-10-31): typed cards for the notch, each from FERB's own records through the real
daemon, gateway and scripted model; never from the text. A reminder firing (with and without a recorded next time), a
new schedule and reminder, a remembered fact, a payment and a send, the sites and files of a result; the text stays as
it was. Also the three 8d protocol gaps: `chat.status` questions carry the asking task, part and runtime switch; a
FERB.app-only refusal has its own error code; part events carry the card's count."""
import json
from datetime import timedelta
from typing import Any

import pytest
from pydantic import BaseModel

from ferb.cli.client import Client, RpcError
from ferb.core.types import ActionFacts, ActionSpec, RuntimeEvent, Target, ToolResult
from ferb.daemon.gateway import CONFIRM_ASK
from ferb.daemon.server import APP_ONLY_REFUSED
from ferb.orchestrator.plans import Step
from ferb.orchestrator.supervisor import RUNTIME_ASK
from ferb.policy.rules import Kind
from ferb.policy.targets import Targets
from tests.conftest import (FakeCap, RunningDaemon, answer_question, chat, done, install, outcome, report, sup, task,
                            tool)
from tests.support.confirm import FerbAnswers
from tests.support.schedules import fire, make_schedule, mins, results, run, schedule_reply
from tests.test_answers_in_steps import Turn, _part, _parts, _spec, _until


def _cards(event: dict[str, Any], kind: str) -> list[dict[str, Any]]:
    return [c for c in event.get("cards") or [] if c["type"] == kind]


def _reply(events: list[dict[str, Any]]) -> dict[str, Any]:
    return next(e for e in events if e["type"] == "reply")


def _chat_status(d: RunningDaemon) -> dict[str, Any]:
    c = Client(d.sock)
    try:
        return c.call("chat.status")       # type: ignore[no-any-return]
    finally:
        c.close()


# ---------- side entries: reminders, schedules, remembered facts ----------

def test_a_reply_carries_note_and_event_cards_from_the_rows_code_wrote(daemon: RunningDaemon) -> None:
    """A remembered fact is a note card naming the stored item (what Correct / Forget act on); a reminder set is an
    event card with the timer's own time and id. Each names the exact line it stands for, and the text keeps it."""
    said = "remind me in 10 minutes to call Mom, and remember that my dentist is Dr. Rao"
    install(daemon, [json.dumps({"kind": "reply", "text": "Okay.", "remember": [
        {"kind": "person", "subject": "owner", "key": "dentist", "content": "my dentist is Dr. Rao",
         "quote": "my dentist is Dr. Rao"}], "timers": [
        {"when": mins(timedelta(minutes=10)), "message": "call Mom", "quote": "remind me in 10 minutes to call Mom"}]})])
    reply = _reply(chat(daemon, said))
    lines = reply["text"].split("\n")
    [note] = _cards(reply, "note")
    item = run(daemon, lambda: daemon.daemon.memory.get(note["item_id"]))
    assert item is not None and " ".join(item.content.split()) == note["text"] and note["state"] == "saved"
    assert note["line"] in lines and note["line"].startswith("Remembered: ")
    [event] = _cards(reply, "event")
    timer = run(daemon, lambda: list(daemon.daemon.timers.pending()))[0]
    assert event["kind"] == "reminder" and event["id"] == timer.id and event["what"] == "call Mom"
    assert event["when"] == timer.fire_at.isoformat() and "repeat" not in event
    assert event["line"] in lines


def test_a_new_schedule_is_an_event_card_with_its_rule(daemon: RunningDaemon) -> None:
    install(daemon, [schedule_reply(daemon, "write report.md", "every day at 9 write report.md",
                                    daemon.daemon.clock.now() + timedelta(minutes=5), repeat="daily")])
    reply = _reply(chat(daemon, "every day at 9 write report.md"))
    [event] = _cards(reply, "event")
    s = run(daemon, daemon.daemon.schedules.all)[0]
    assert event["kind"] == "schedule" and event["id"] == s.id and event["what"] == "write report.md"
    assert event["repeat"] == s.rule.describe() and event["when"] == s.first_run.isoformat()
    assert event["line"].startswith("🗓 Scheduled: ") and event["line"] in reply["text"].split("\n")


def test_a_refused_side_entry_has_no_card(daemon: RunningDaemon) -> None:
    install(daemon, [json.dumps({"kind": "reply", "text": "Okay.", "remember": [
        {"kind": "person", "subject": "owner", "key": "dentist", "content": "Your dentist is Dr. Rao",
         "quote": "words the owner never said"}], "timers": []})])
    reply = _reply(chat(daemon, "when is my dentist appointment?"))
    assert "cards" not in reply


def test_a_one_ask_task_carries_its_receipt_cards_in_the_outcome(daemon: RunningDaemon) -> None:
    plan = json.loads(sup("task", "Checking.", task("check the gate number")))
    plan["timers"] = [{"when": mins(timedelta(minutes=10)), "message": "stretch",
                       "quote": "remind me in 10 minutes to stretch"}]
    install(daemon, [json.dumps(plan)], {"worker": done("Your gate is B12.")})
    final = outcome(chat(daemon, "check the gate number and remind me in 10 minutes to stretch"))
    [event] = _cards(final, "event")
    assert event["what"] == "stretch" and event["line"] in final["lines"]


def test_a_multi_part_first_reply_carries_its_receipt_cards_once(daemon: RunningDaemon) -> None:
    spec = _spec("Visa news and Trump's term", ["B1/B2 news is reported", "The end of Trump's term is given"],
                 [_part("p1", "any news about the b1b2 visa", "B1/B2 visa news", step="visa", conds=(0,)),
                  _part("p2", "when is trump's term ending", "When Trump's term ends", step="term", conds=(1,))],
                 [Step(id="visa", kind="do", goal="Check B1/B2 visa news"),
                  Step(id="term", kind="do", goal="Find when Trump's term ends")])
    plan = json.loads(sup("task", "I'll check both.", spec))
    plan["timers"] = [{"when": mins(timedelta(minutes=10)), "message": "stretch",
                       "quote": "remind me in 10 minutes to stretch"}]
    install(daemon, [json.dumps(plan)], {"step.visa": done("No new rules."), "step.term": done("January 2029.")})
    events = chat(daemon, "any news about the b1b2 visa and when is trump's term ending, "
                          "remind me in 10 minutes to stretch")
    first = next(e for e in events if e["type"] == "plan")
    [event] = _cards(first, "event")
    assert event["line"] in first["receipts"]
    assert not _cards(outcome(events), "event")            # said in the first reply, not again at the end
    # protocol gap 3: each part event carries the card's count after it
    counts = [(e["part"], e["done"], e["total"]) for e in _parts(events) if e["state"] == "done"]
    assert sorted(c[1] for c in counts) == [1, 2] and all(c[2] == 2 for c in counts)


# ---------- a reminder firing ----------

YC = ("While YC Winter 2027 applications remain open, remind me that I want to apply. The window is open until "
      "Nov 3, 9:30 AM IST.")


def test_a_repeating_schedules_run_is_a_reminder_card_counting_down_to_its_next_run(daemon: RunningDaemon) -> None:
    """The countdown is to the next run FERB recorded; the deadline in the request's words is not read."""
    now = daemon.daemon.clock.now()
    s = make_schedule(daemon, YC, YC, Targets.of(), now + timedelta(minutes=1), repeat="every", every=180)
    install(daemon, [sup("reply", "Reminder: you wanted to apply to YC Winter 2027.")])
    fire(daemon, 61)
    [result] = results(daemon)
    [card] = _cards(result, "reminder")
    after = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
    assert card["title"] == YC and card["schedule_id"] == s.id and card["repeat"] == "every 180 minutes"
    assert card["next_run"] == after.next_run.isoformat() and card["due"] == s.first_run.isoformat()
    assert set(card) == {"type", "title", "schedule_id", "due", "repeat", "next_run"}     # no deadline from the words
    assert result["answer"].startswith("Reminder: you wanted to apply")                    # FERB's text under it


def test_a_one_time_schedules_run_has_no_countdown(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    s = make_schedule(daemon, YC, YC, Targets.of(), now + timedelta(minutes=1), repeat="none")
    install(daemon, [sup("reply", "Reminder: apply to YC.")])
    fire(daemon, 61)
    [result] = results(daemon)
    [card] = _cards(result, "reminder")
    assert card["schedule_id"] == s.id and "next_run" not in card and "repeat" not in card


# ---------- payments and sends ----------

class _Checkout(FakeCap):
    """A pay click in a task tab: what the page shows reaches the gateway as ActionFacts read by code."""
    generic_input = True

    class Input(BaseModel):
        amount: str = "₹137.39"
        merchant: str = "Zomato"

    def __init__(self) -> None:
        super().__init__("probe.checkout", Kind.CLICK, {"probe"})

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.CLICK, Target(site="www.zomato.com"), content=f"pay:{args.amount}",
                          facts=ActionFacts(page_url="https://www.zomato.com/checkout", page_amount=args.amount,
                                            page_merchant=args.merchant, tab_chain="t1"))

    async def run(self, args: Any, ctx: Any) -> ToolResult:
        return ToolResult({"ok": True})


def test_a_payment_is_an_expense_row_from_the_page_facts_never_the_report(ferb_yes: FerbAnswers,
                                                                         daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:pay")
    daemon.daemon.registry.capabilities["probe.checkout"] = _Checkout()

    def pays(token: str) -> str:
        r = tool(daemon.sock, token, "probe.checkout", effect="pay")
        assert not r["is_error"], r
        return done("Ordered paneer tikka for ₹999 from Domino's, paid with card 4111 1111 1111 1111.")
    install(daemon, [sup("task", "Ordering.", task("order paneer tikka on zomato"))], {"worker": pays})
    final = outcome(chat(daemon, "order paneer tikka on zomato"))
    assert _cards(final, "payment") == [{"type": "payment", "amount": "₹137.39", "merchant": "Zomato",
                                         "site": "www.zomato.com"}]
    assert "4111" not in json.dumps(final["cards"])


def test_a_payment_the_owner_refused_has_no_card(daemon: RunningDaemon) -> None:
    daemon.daemon.registry.capabilities["probe.checkout"] = _Checkout()

    def pays(token: str) -> str:
        tool(daemon.sock, token, "probe.checkout", effect="pay")
        return done("Paid ₹137.39 to Zomato.")
    install(daemon, [sup("task", "Ordering.", task("order paneer tikka on zomato"))], {"worker": pays})
    with FerbAnswers(daemon, yes=False):
        final = outcome(chat(daemon, "order paneer tikka on zomato"))
    assert not _cards(final, "payment")


def test_a_send_is_a_contact_card_from_the_calls_checked_destination(ferb_yes: FerbAnswers,
                                                                    daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:new_recipient")
    daemon.daemon.registry.capabilities["probe.send"] = FakeCap("probe.send", "send", {"probe"})

    def sends(token: str) -> str:
        r = tool(daemon.sock, token, "probe.send", to="alex@work.example", site="mail.google.com")
        assert not r["is_error"], r
        return done("Sent it to Priya (priya@home.example) on WhatsApp.")
    install(daemon, [sup("task", "Sending.", task("email alex the report"))], {"worker": sends})
    final = outcome(chat(daemon, "email alex the report"))
    assert _cards(final, "send") == [{"type": "send", "kind": "send", "recipient": "alex@work.example",
                                      "site": "mail.google.com"}]


# ---------- sites and files ----------

def test_sites_and_files_are_link_cards_and_file_chips_from_the_records(daemon: RunningDaemon) -> None:
    """A site the records say the task reached is a link card standing for the "Sites contacted" line; a site only
    named in the answer is not; the task's file is a file chip."""
    path = daemon.daemon.home / "workspace" / "notes.md"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("notes")

    def works(token: str) -> str:
        task_id = run(daemon, lambda: daemon.daemon.tokens.validate(token).task_id)
        daemon.call_soon(lambda: daemon.daemon.journal.append(
            "shell.network", {"task_id": task_id, "destinations": [{"host": "kartikey.xyz"}]}))
        return json.dumps(report("done", "Saved notes from kartikey.xyz and also see https://elsewhere.example.",
                                 artifacts=(str(path),)))
    install(daemon, [sup("task", "Reading.", task("save notes from kartikey.xyz"))], {"worker": works})
    final = outcome(chat(daemon, "save notes from kartikey.xyz"))
    links = _cards(final, "link")
    assert links == [{"type": "link", "host": "kartikey.xyz", "line": "Sites contacted: kartikey.xyz"}]
    assert links[0]["line"] in final["lines"] and links[0]["line"] in final["answer"]
    assert _cards(final, "file") == [{"type": "file", "path": str(path), "name": "notes.md"}]
    stored = _chat_status(daemon)["results"]
    assert all(r.get("cards") == final["cards"] for r in stored if r["task_id"] == final["task_id"])


# ---------- the 8d protocol gaps ----------

def test_chat_status_questions_carry_the_asking_task_and_part(daemon: RunningDaemon) -> None:
    """Gap 1: after an app restart a step's question still lands in its row: `chat.status` lists the task that asked
    (a step's task id) and the part."""
    spec = _spec("Book Toit and order lunch", ["A table at Toit is booked", "Paneer tikka is ordered"],
                 [_part("book", "book a table at Toit for tonight", "Table at Toit tonight", step="book", conds=(0,)),
                  _part("food", "order paneer tikka from Zomato", "Paneer tikka from Zomato", step="food", conds=(1,))],
                 [Step(id="book", kind="do", goal="Book a table at Toit tonight"),
                  Step(id="food", kind="do", goal="Order paneer tikka on Zomato")])
    install(daemon, [sup("task", "I'll book Toit and order lunch.", spec)],
            {"step.book": [json.dumps(report("not_done", reason="Two times are free.",
                                             question="Which time: 8:00 or 9:30 pm?")), done("Booked 8:00 pm.")],
             "step.food": done("Ordered paneer tikka.")})
    turn = Turn(daemon, "book a table at Toit for tonight and order paneer tikka from Zomato for lunch")
    _until(lambda: any(a.capability != CONFIRM_ASK for a in run(daemon, daemon.daemon.state.approvals.pending)))
    [listed] = _chat_status(daemon)["approvals"]
    assert listed["part"] == "book" and listed["source_task_id"].endswith(".step.book")
    answer_question(daemon, text="8:00 pm")
    turn.done()


def test_chat_status_questions_carry_the_runtime_switch(daemon: RunningDaemon) -> None:
    daemon.daemon.supervisor.worker_types = lambda: ["default", "claude"]
    limit = RuntimeEvent("error", {"message": "You've hit your usage limit.", "account": "usage_limit"})
    install(daemon, [sup("task", "On it.", task("Tidy the desktop"))], {"worker": [limit, done("Tidied.")]})
    turn = Turn(daemon, "tidy my desktop")
    _until(lambda: any(a.capability == RUNTIME_ASK for a in run(daemon, daemon.daemon.state.approvals.pending)))
    [listed] = _chat_status(daemon)["approvals"]
    assert listed["runtime_switch"]["choices"] == ["claude"] and listed["runtime_switch"]["role"] == "worker"
    assert listed["source_task_id"].startswith(listed["task_id"])
    answer_question(daemon, RUNTIME_ASK, choice="Stop")
    turn.done()


def test_an_app_only_refusal_has_its_own_error_code(daemon: RunningDaemon) -> None:
    """Gap 2: the app tells "only FERB.app" from "this daemon doesn't know the method" by the code, not the words."""
    c = Client(daemon.sock)
    try:
        with pytest.raises(RpcError) as refused:
            c.call("tasks.preview", {"task_id": "nope"})
    finally:
        c.close()
    assert refused.value.code == APP_ONLY_REFUSED
