"""The owner's own No is not FERB failing (owner 2026-11-11: "also shuodl i t stll be afilure if i stopped it??";
"btw htw failed thign didnt happen in this task when it is part fo thsi row when i declined. why??"). Through the real
daemon, gateway and scripted model: a payment the owner declines on its card (the Don't pay button, or their own words
in the card's box) ends as their decision: cancelled, the worker's own words with no "I couldn't finish" opener, no
code line about the payment that didn't go through. Another condition found false keeps it a failure."""
import asyncio
import json
from typing import Any

from ferb.daemon.gateway import CONFIRM_ASK
from ferb.orchestrator.answer_marks import DECLINED_LINE, NOT_FINISHED
from ferb.orchestrator.parts import STOPPED
from ferb.orchestrator.verify import DECLINED
from ferb.policy.rules import Kind
from ferb.core.types import ActionFacts, ActionSpec, Target, ToolResult
from pydantic import BaseModel
from tests.conftest import FakeCap, RunningDaemon, chat, cond, install, outcome, report, sup, task, tool, verdict
from tests.support.confirm import FerbAnswers

PAY = cond("The batteries are ordered on orderly.example", "paid", site="www.orderly.example")


class _Checkout(FakeCap):
    """The final Place your order click in a task tab, with the page's amount read by code."""
    generic_input = True

    class Input(BaseModel):
        amount: str = "₹72.05"

    def __init__(self) -> None:
        super().__init__("probe.checkout", Kind.CLICK, {"probe"})

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.CLICK, Target(site="www.orderly.example"), content=f"pay:{args.amount}",
                          facts=ActionFacts(page_url="https://www.orderly.example/checkout", page_amount=args.amount,
                                            submits_form=True, tab_chain="t1"))

    async def run(self, args: Any, ctx: Any) -> ToolResult:
        return ToolResult({"ok": True})


class _Words:
    """The owner types in the payment card's box: their words set the card aside (GATE-510, ORCH-538), never a Yes."""

    def __init__(self, d: RunningDaemon) -> None:
        self.d = d
        self._task = asyncio.run_coroutine_threadsafe(self._loop(), d.loop)

    async def _loop(self) -> None:
        approvals = self.d.daemon.state.approvals
        while True:
            for ap in approvals.pending():
                if ap.capability == CONFIRM_ASK:
                    approvals.supersede(ap.id)          # what steering does with the card the owner's words were on
            await asyncio.sleep(0.02)

    def stop(self) -> None:
        self._task.cancel()


def _pays(d: RunningDaemon, answer: str, status: str = "done") -> Any:
    def go(token: str) -> str:
        tool(d.sock, token, "probe.checkout", effect="pay")
        return json.dumps(report(status, answer, reason="You declined the payment." if status != "done" else ""))
    return go


# decision: ORCH-613, ORCH-576
def test_dont_pay_on_the_card_ends_as_the_owners_decision_in_the_workers_words(daemon: RunningDaemon) -> None:
    """The live run 3e5c8c55 (the Don't pay button): the worker reported not done, the check failed it, code's paid
    check was false. Now: cancelled, the worker's own sentence, no opener, no "No payment went through"."""
    daemon.daemon.registry.capabilities["probe.checkout"] = _Checkout()
    said = "The batteries were not ordered because you declined the final payment confirmation."
    install(daemon, [sup("task", "Ordering.", task("Order the batteries", conds=(PAY,)))],
            {"worker": _pays(daemon, said, "not_done"), "worker.verify": verdict(False, next="fail")})
    with FerbAnswers(daemon, yes=False):
        final = outcome(chat(daemon, "order the cheapest AA batteries on orderly"))
    assert final["status"] == "cancelled", final
    assert final["answer"].split("\n\n")[0] == said and NOT_FINISHED not in final["answer"]
    assert "No payment went through" not in final["answer"]
    [result] = final["verification"]
    assert result["met"] is False and result["path"] == DECLINED


# decision: ORCH-613, GATE-510
def test_words_typed_on_the_card_end_the_same_way(daemon: RunningDaemon) -> None:
    """The live run 6812fdb5 (the owner typed "lets cacnel not ordering" in the card's box): the worker closed the
    checkout and reported done, the check accepted; code's paid check was false. Same end as Don't pay."""
    daemon.daemon.registry.capabilities["probe.checkout"] = _Checkout()
    said = "Cancelled. The order was not placed and no payment was made."
    install(daemon, [sup("task", "Ordering.", task("Order the batteries", conds=(PAY,)))],
            {"worker": _pays(daemon, said), "worker.verify": verdict(True, next="accept")})
    owner = _Words(daemon)
    try:
        final = outcome(chat(daemon, "order the cheapest AA batteries on orderly"))
    finally:
        owner.stop()
    assert final["status"] == "cancelled", final
    assert final["answer"].split("\n\n")[0] == said and NOT_FINISHED not in final["answer"]


# decision: ORCH-613
def test_with_no_words_of_the_workers_the_reply_is_codes_one_line(daemon: RunningDaemon) -> None:
    daemon.daemon.registry.capabilities["probe.checkout"] = _Checkout()
    install(daemon, [sup("task", "Ordering.", task("Order the batteries", conds=(PAY,)))],
            {"worker": _pays(daemon, "", "not_done"), "worker.verify": verdict(False, next="fail")})
    with FerbAnswers(daemon, yes=False):
        final = outcome(chat(daemon, "order the cheapest AA batteries on orderly"))
    assert final["status"] == "cancelled" and final["answer"].split("\n\n")[0] == DECLINED_LINE


# decision: ORCH-613
def test_another_condition_found_false_keeps_it_a_failure(daemon: RunningDaemon) -> None:
    """The decline explains the payment only: a second condition the check found not met is FERB's failure."""
    daemon.daemon.registry.capabilities["probe.checkout"] = _Checkout()
    install(daemon, [sup("task", "Ordering.", task("Order the batteries and save the receipt",
                                                    conds=(PAY, cond("The receipt is saved"))))],
            {"worker": _pays(daemon, "Not ordered; the receipt page wouldn't load.", "not_done"),
             "worker.verify": verdict(False, False, next="fail")})
    with FerbAnswers(daemon, yes=False):
        final = outcome(chat(daemon, "order the batteries and save the receipt"))
    assert final["status"] == "failed" and final["answer"].startswith(NOT_FINISHED)


class _BuyNow(_Checkout):
    """A checkout's first form (Buy Now): a form submission on the same site that needs no Yes and runs."""

    def __init__(self) -> None:
        FakeCap.__init__(self, "probe.buy_now", Kind.CLICK, {"probe"})

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.CLICK, Target(site="www.orderly.example"),
                          facts=ActionFacts(page_url="https://www.orderly.example/item", submits_form=True,
                                            tab_chain="t1"))


def _parts(daemon: RunningDaemon, order: dict[str, Any], worker: Any, verify: str) -> list[dict[str, Any]]:
    """A two-part request: the order (its condition `order`) and a laptop comparison that is done."""
    from ferb.orchestrator.plans import Plan, Step
    spec = task("Order the batteries and compare laptops", conds=(order, cond("Three laptops are compared")))
    spec["parts"] = [{"id": "order", "quote": "order the batteries", "label": "Battery order", "direct": None,
                      "answer_step": "order", "conditions": [0]},
                     {"id": "laptops", "quote": "compare laptops", "label": "Laptop comparison", "direct": None,
                      "answer_step": "laptops", "conditions": [1]}]
    spec["plan"] = Plan(steps=[Step(id="order", kind="do", goal="Order the batteries"),
                               Step(id="laptops", kind="do", goal="Compare three laptops")]).model_dump()
    install(daemon, [sup("task", "Ordering and comparing.", spec)],
            {"step.order": worker, "step.laptops": json.dumps(report("done", "A, B and C.")),
             "step.order.verify": verify, "step.laptops.verify": verdict(True)})
    with FerbAnswers(daemon, yes=False):
        return chat(daemon, "order the batteries and compare laptops")


def _told_as_the_owners_decision(events: list[dict[str, Any]], said: str) -> None:
    """The row is the owner's decision in the worker's words (no "Couldn't do:"), counted apart, and the task ends
    done because the other part is (owner 2026-11-11: "should we have a stoped by me color???")."""
    row = [e for e in events if e["type"] == "part" and e["part"] == "order"][-1]
    assert row["state"] == STOPPED and row["text"] == said, row
    final = outcome(events)
    assert final["status"] == "done", final
    assert final["card"]["line"] == "1 of 2 done \u00b7 1 stopped by you", final["card"]
    assert final["answer"].splitlines()[0] == f"Battery order: {said}", final["answer"]


# decision: ORCH-613, ORCH-614, ORCH-319
def test_a_declined_part_is_the_owners_decision_in_its_row(daemon: RunningDaemon) -> None:
    """The same No inside a multi-part task: the row tells the worker's words as the owner's decision, never
    "Couldn't do", and the task's end isn't failed because of it."""
    daemon.daemon.registry.capabilities["probe.checkout"] = _Checkout()
    said = "No batteries were ordered: you declined the payment."
    _told_as_the_owners_decision(_parts(daemon, PAY, _pays(daemon, said, "not_done"), verdict(False, next="fail")),
                                 said)


# decision: ORCH-613, ORCH-614, ORCH-615, ORCH-576
def test_a_declined_order_after_the_checkouts_first_form_is_still_the_owners_decision(daemon: RunningDaemon) -> None:
    """The live run 9b96e830 (2026-11-11 11:43): the order's condition was "submitted on amazon.in"; the Buy Now form
    had gone through, so code's check read the submission as made, the checker found the order not placed and failed
    it, and the row said "Couldn't do: …" in red. The latest submission at that site is the one the owner said no to:
    their decision."""
    daemon.daemon.registry.capabilities["probe.checkout"] = _Checkout()
    daemon.daemon.registry.capabilities["probe.buy_now"] = _BuyNow()
    said = "I picked AA batteries for ₹72.05, but you declined the final purchase confirmation, so nothing was ordered."

    def worker(token: str) -> str:
        tool(daemon.sock, token, "probe.buy_now")
        tool(daemon.sock, token, "probe.checkout", effect="pay")
        return json.dumps(report("not_done", said, reason="You declined the payment."))
    submitted = cond("An item is ordered on orderly.example", "submitted", site="orderly.example")
    _told_as_the_owners_decision(_parts(daemon, submitted, worker, verdict(False, next="fail")), said)


# decision: ORCH-613, ORCH-576
def test_a_payment_that_was_never_asked_about_is_still_codes_failure(daemon: RunningDaemon) -> None:
    """No card, no decline: the worker claims it paid and nothing ran; code's false paid check fails it as before."""
    install(daemon, [sup("task", "Ordering.", task("Order the batteries", conds=(PAY,)))],
            {"worker": json.dumps(report("done", "Ordered.")), "worker.verify": verdict(True, next="accept")})
    final = outcome(chat(daemon, "order the cheapest AA batteries on orderly"))
    assert final["status"] == "failed" and "No payment went through on www.orderly.example." in final["answer"]
