"""The owner's last word on a purchase wins (ORCH-629). Since GATE-582 a checkout's first same-site form (Buy Now) is
a payment call too: a Yes there and a No at Place your order must leave no payment card and no "paid", and end as the
owner's decision (ORCH-613); a Yes at both is one payment, carded once with the final page's amount. Through the real
daemon, gateway and scripted model."""
import asyncio
import json
from typing import Any

from pydantic import BaseModel

from ferb.core.types import ActionFacts, ActionSpec, Target, ToolResult
from ferb.daemon.gateway import CONFIRM_ASK, NO, YES
from ferb.orchestrator import cards
from ferb.orchestrator.verify import DECLINED
from ferb.policy.rules import Kind
from tests.conftest import FakeCap, RunningDaemon, chat, cond, install, outcome, report, sup, task, tool, verdict

PAY = cond("The batteries are ordered on orderly.example", "paid", site="www.orderly.example")


class _Step(FakeCap):
    """A same-site form submit in the task's tab on a page showing money: Buy Now (no amount read) or Place your
    order (its amount read by code)."""
    generic_input = True

    class Input(BaseModel):
        final: bool = False

    def __init__(self) -> None:
        super().__init__("probe.shop", Kind.CLICK, {"probe"})

    def describe(self, args: Any) -> ActionSpec:
        page = "https://www.orderly.example/" + ("checkout" if args.final else "item")
        return ActionSpec(Kind.CLICK, Target(site="www.orderly.example"), content=f"shop:{args.final}",
                          facts=ActionFacts(page_url=page, submits_form=True, form_method="POST",
                                            form_action_host="www.orderly.example", tab_chain="t1",
                                            page_amount="₹72.05" if args.final else None,
                                            page_amounts=("₹65.05", "₹7.00", "₹72.05") if args.final
                                            else ("₹65.05", "₹299")))

    async def run(self, args: Any, ctx: Any) -> ToolResult:
        return ToolResult({"ok": True})


class _Owner:
    """Answers FERB's payment questions in turn: `answers[i]` for the i-th question (True: Yes)."""

    def __init__(self, d: RunningDaemon, answers: list[bool]) -> None:
        self.d, self.answers, self.asked = d, answers, 0
        self._task = asyncio.run_coroutine_threadsafe(self._loop(), d.loop)

    async def _loop(self) -> None:
        approvals = self.d.daemon.state.approvals
        while True:
            for ap in approvals.pending():
                if ap.capability != CONFIRM_ASK:
                    continue
                yes = self.answers[min(self.asked, len(self.answers) - 1)]
                approvals.resolve(ap.id, True, ap.options.index(YES if yes else NO) + 1)
                self.asked += 1
            await asyncio.sleep(0.02)

    def stop(self) -> None:
        self._task.cancel()


def _run(d: RunningDaemon, answers: list[bool], said: str, status: str, check: str) -> tuple[dict[str, Any], int]:
    d.daemon.registry.capabilities["probe.shop"] = _Step()

    def worker(token: str) -> str:
        tool(d.sock, token, "probe.shop")                       # Buy Now: no amount read or stated, sent back (GATE-584)
        tool(d.sock, token, "probe.shop", effect="pay", destination={"amount": 65.05, "currency": "₹"})   # stated
        tool(d.sock, token, "probe.shop", final=True, effect="pay")   # Place your order
        return json.dumps(report(status, said, reason="You declined the payment." if status != "done" else ""))
    install(d, [sup("task", "Ordering.", task("Order the batteries", conds=(PAY,)))],
            {"worker": worker, "worker.verify": verdict(status == "done", next=check)})
    owner = _Owner(d, answers)
    try:
        final = outcome(chat(d, "order the cheapest AA batteries on orderly"))
    finally:
        owner.stop()
    return final, owner.asked


async def _paid(d: RunningDaemon, tree: str) -> int:
    return len(cards.paid(d.daemon.spans, tree))


def _payment_cards(final: dict[str, Any]) -> list[dict[str, Any]]:
    return [c for c in final.get("cards") or [] if c["type"] == "payment"]


# decision: ORCH-629, ORCH-613, GATE-582
def test_yes_at_buy_now_and_no_at_place_your_order_is_no_payment_and_the_owners_decision(daemon: RunningDaemon) -> None:
    said = "Nothing was ordered: you declined the final payment."
    final, asked = _run(daemon, [True, False], said, "not_done", "fail")
    assert asked == 2
    assert final["status"] == "cancelled", final                # the owner's decision, not FERB failing
    [result] = final["verification"]
    assert result["met"] is False and result["path"] == DECLINED
    assert _payment_cards(final) == [] and "No payment went through" not in final["answer"]
    assert daemon.run(_paid(daemon, final["task_id"])) == 0                 # code's paid check counts no payment


# decision: ORCH-629, GATE-582
def test_yes_at_both_steps_is_one_payment_carded_with_the_final_amount(daemon: RunningDaemon) -> None:
    final, asked = _run(daemon, [True], "Ordered the batteries for ₹72.05.", "done", "accept")
    assert asked == 2
    assert final["status"] == "done", final
    [result] = final["verification"]
    assert result["met"] is True
    assert daemon.run(_paid(daemon, final["task_id"])) == 2                 # both steps under the owner's Yes
    assert _payment_cards(final) == [{"type": "payment", "amount": "₹72.05", "site": "www.orderly.example"}]
