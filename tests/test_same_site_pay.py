"""GATE-582 (owner 2026-11-12, closing T-1060): a form posted to the page's own site from a page showing a money amount
is a payment by code's facts, card field or not. Live cause: all four real Amazon "Place your order" clicks (POST to
www.amazon.in, no card field) were asked only because the worker declared pay."""
import json
from typing import Any

from pydantic import BaseModel

from ferb.core.types import ActionFacts, ActionSpec, Target, ToolResult
from ferb.policy import confirm
from ferb.policy.rules import Kind
from tests.conftest import Answers, FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call

PRODUCT = "https://www.amazon.in/Nippo-3DG-Gold-Battery-Pack/dp/B0C73N5QVG"
CHECKOUT = "https://www.amazon.in/checkout/p/p-000-0000000-0000000/spc?pipelineType=Chewbacca&isBuyNow=1"
# The page_amounts the extension recorded on those pages (tasks 3e5c8c55, 6812fdb5, 9b96e830, 2026-11-11), as written
PRODUCT_AMOUNTS = ["₹67.00", "₹232.00", "₹3,502.00", "₹17,999.00", "₹65.05", "₹ 65", "05 ₹", "₹16.26", "₹299",
                   "₹11,853.65", "₹85.00", "₹179.00", "₹17.90", "₹329.05", "₹220.00", "₹90.00", "20,699 ₹"]
CHECKOUT_AMOUNTS = ["₹65.05", "₹0.00", "₹7.00", "₹72.05", "₹19.00", "₹ 65", "₹16.26"]
# An eraser's product page at Buy Now (tree 62252d57, 2026-11-12): 39 amounts, so none read as the price. The page's
# amounts as recorded; the address is invented (PROJ-500)
ERASER = "https://www.amazon.in/Clear-Eraser-Pack-of-5/dp/B000000000"
ERASER_AMOUNTS = ["₹132.05", "₹166.95", "₹65.05", "₹67.00", "₹3,502.00", "₹17,999.00", "₹40.00", "₹ 40", "00 ₹",
                  "₹8.00", "₹80", "₹8", "₹2.00", "20 ₹", "55.00 ₹", "₹2.75", "₹95.00", "₹ 95", "₹55.00", "₹ 55",
                  "₹60.00", "₹80.00", "854 ₹", "100.00 ₹", "₹5.00", "2 ₹", "60.00 ₹", "₹3.00", "₹800.00", "₹50.00",
                  "₹649.00", "₹ 649", "₹64.90", "₹999.00", "₹87.07", "₹ 87", "₹4.35", "₹100.00", "5 INR"]


class _Form(FakeCap):
    """A browser click that submits a form, with the facts the extension records for it (ActionFacts)."""
    generic_input = True

    class Input(BaseModel):
        tab: str = "t1"
        url: str = "https://www.shop.example/checkout"
        action_host: str | None = "www.shop.example"
        method: str = "POST"
        shown: list[str] = []
        label: str = "Place your order"

    def __init__(self) -> None:
        super().__init__("probe.form", Kind.CLICK, {"probe"})
        self.ran: list[str] = []

    def describe(self, args: Any) -> ActionSpec:
        host = args.url.split("/")[2]
        return ActionSpec(Kind.CLICK, Target(site=host), content=f"click:{args.tab}:{args.label}:{len(self.ran)}",
                          facts=ActionFacts(submits_form=True, form_action_host=args.action_host,
                                            form_method=args.method, page_url=args.url, tab_chain=args.tab,
                                            page_amounts=tuple(args.shown), control_label=args.label,
                                            target_role="button"))

    async def run(self, args: Any, ctx: Any) -> ToolResult:
        self.ran.append(args.label)
        return ToolResult({"ok": True})


def _pays(d: RunningDaemon) -> list[dict[str, Any]]:
    """The payment questions FERB asked, as recorded (their owner.ask spans' arguments)."""
    async def go() -> list[dict[str, Any]]:
        rows = d.daemon.db.conn.execute("SELECT input_json FROM spans WHERE kind = 'tool.call' AND "
                                        "json_extract(input_json, '$.tool') = 'owner.ask' ORDER BY rowid").fetchall()
        asks = [json.loads(r["input_json"])["args"] for r in rows]
        return [a for a in asks if any(n.get("rule") == "floor:pay" for n in a.get("confirm") or [])]
    return d.run(go())


def _asked_after_the_amount(d: RunningDaemon, grant: Any, **args: Any) -> dict[str, Any]:
    """GATE-584: a payment with no amount read or stated goes back to the worker once, unasked; the same call again
    (still no amount) is asked."""
    asked = len(_pays(d))
    back = gateway_tool_call(d, grant, "probe.form", **args)
    assert back["is_error"] and "amount" in back["content"]["error"] and len(_pays(d)) == asked
    return gateway_tool_call(d, grant, "probe.form", **args)


# decision: GATE-582
def test_a_same_site_post_on_a_page_with_amounts_asks_as_a_payment_and_nothing_else_does(daemon: RunningDaemon) -> None:
    form = _Form()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("order it on www.shop.example"), [form])
    owner = Answers(daemon, granted=False, choice=2)
    try:
        r = _asked_after_the_amount(daemon, grant, shown=["₹499.00", "₹40.00"])                 # no card field
        assert r["is_error"] and form.ran == [] and len(_pays(daemon)) == 1
        for i, case in enumerate(({"method": "GET", "shown": ["₹499.00"]},                    # a search, a filter
                                  {"shown": []},                                              # no amount shown
                                  {"action_host": "api.razorpay.com", "shown": ["₹499.00"]},  # another site's form
                                  {"action_host": None, "shown": ["₹499.00"], "url": "https://www.shop.example/x",
                                   "method": "GET"})):
            r = gateway_tool_call(daemon, grant, "probe.form", tab=f"t{i + 2}", **case)
            assert not r["is_error"], (case, r)
        assert len(_pays(daemon)) == 1 and len(form.ran) == 4
        r = _asked_after_the_amount(daemon, grant, tab="t9", url="https://pay.shop.example/go",
                                    shown=["₹499.00"])                                        # same site, other host
        assert r["is_error"] and len(_pays(daemon)) == 2
    finally:
        owner.stop()


# decision: GATE-582
def test_the_owner_can_turn_it_off_in_config(daemon: RunningDaemon) -> None:
    daemon.daemon.config.set("policy", "pay_same_site_forms", False)
    form = _Form()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("order it on www.shop.example"), [form])
    owner = Answers(daemon, granted=False, choice=2)
    try:
        r = gateway_tool_call(daemon, grant, "probe.form", shown=["₹499.00"])
        assert not r["is_error"] and _pays(daemon) == []
        daemon.daemon.config.set("policy", "pay_same_site_forms", True)                    # read at each call
        r = _asked_after_the_amount(daemon, grant, tab="t2", shown=["₹499.00"])
        assert r["is_error"] and len(_pays(daemon)) == 1
    finally:
        owner.stop()


def test_a_site_is_its_registrable_domain() -> None:
    assert confirm.registrable("www.amazon.in") == confirm.registrable("pay.amazon.in") == "amazon.in"
    assert confirm.registrable("www.amazon.co.uk") == "amazon.co.uk" != confirm.registrable("amazon.in")
    assert confirm.registrable("192.168.1.5") == "192.168.1.5" != confirm.registrable("10.0.1.5")


# decision: GATE-582, GATE-204, GATE-217
def test_the_recorded_amazon_orders_ask_at_buy_now_and_again_at_place_your_order(daemon: RunningDaemon) -> None:
    """The six recorded facts (Buy Now, then Place your order, in three tab chains), the worker's declarations as
    recorded (none at Buy Now; pay with ₹72.05 at Place your order). With every Yes: Buy Now now asks (no amount read,
    none declared), its Yes is spent by that one-click submit (GATE-217), and Place your order asks again with the
    AI's ₹72.05, found on the page. No rule lets the first Yes cover the second."""
    form = _Form()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("order the Nippo AA batteries on amazon.in"), [form])
    owner = Answers(daemon, granted=True, choice=1)
    try:
        for chain in ("t1", "t18", "t3"):
            for _ in range(2):                           # GATE-584: sent back once for the amount, then asked
                buy = gateway_tool_call(daemon, grant, "probe.form", tab=chain, url=PRODUCT,
                                        action_host="www.amazon.in", shown=PRODUCT_AMOUNTS, label="Buy Now")
            place = gateway_tool_call(daemon, grant, "probe.form", tab=chain, url=CHECKOUT,
                                      action_host="www.amazon.in", shown=CHECKOUT_AMOUNTS, label="Place your order",
                                      effect="pay", destination={"amount": 72.05, "currency": "INR",
                                                                 "item": "1 × Nippo 3DG Gold AA Battery, pack of 4"})
            assert not buy["is_error"] and not place["is_error"]
    finally:
        owner.stop()
    assert len(_pays(daemon)) == 6 and form.ran == ["Buy Now", "Place your order"] * 3
    assert [a.summary for a in owner.asked] == ["Pay? FERB couldn't read the amount", "Pay INR 72.05?"] * 3
    assert [a.exact for a in owner.asked] == ["an amount FERB couldn't read on the page · amazon.in",
                                              "INR 72.05 · amazon.in"] * 3


# decision: GATE-583, GATE-584, GATE-578
def test_a_payment_with_no_amount_goes_back_once_for_it_and_its_card_then_says_it(daemon: RunningDaemon) -> None:
    """Tonight's Buy Now (39 amounts on the page, none read, none declared): the call goes back to the worker unrun with
    the fact and no question; the retry with ₹40 asks "Pay ₹40?" (₹40.00 is on the page); a retry still without an
    amount asks the unread card, never a second bounce; an amount the page doesn't show says so."""
    form = _Form()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("buy the cheapest eraser on amazon.in"), [form])
    owner = Answers(daemon, granted=False, choice=2)
    buy = dict(url=ERASER, action_host="www.amazon.in", shown=ERASER_AMOUNTS, label="Buy Now")
    try:
        r = gateway_tool_call(daemon, grant, "probe.form", tab="t5", **buy)
        assert r["is_error"] and form.ran == [] and _pays(daemon) == []
        fact = r["content"]["error"] + " " + r["content"]["next"]
        assert "amount" in fact and "floor:" not in fact and "probe.form" not in fact
        r = gateway_tool_call(daemon, grant, "probe.form", tab="t5", effect="pay",
                              destination={"amount": 40, "currency": "₹"}, **buy)
        assert r["is_error"] and form.ran == [] and len(_pays(daemon)) == 1
        r = gateway_tool_call(daemon, grant, "probe.form", tab="t6", **buy)                  # another payment: back once
        assert r["is_error"] and len(_pays(daemon)) == 1
        r = gateway_tool_call(daemon, grant, "probe.form", tab="t6", **buy)                  # still none: the unread card
        assert r["is_error"] and len(_pays(daemon)) == 2
        gateway_tool_call(daemon, grant, "probe.form", tab="t7", **buy)
        r = gateway_tool_call(daemon, grant, "probe.form", tab="t7", effect="pay",
                              destination={"amount": 41, "currency": "₹"}, **buy)            # not among the page's
        assert r["is_error"] and len(_pays(daemon)) == 3 and form.ran == []
    finally:
        owner.stop()
    assert [(a.summary, a.exact) for a in owner.asked] == [
        ("Pay ₹40?", "₹40 · amazon.in"),
        ("Pay? FERB couldn't read the amount", "an amount FERB couldn't read on the page · amazon.in"),
        ("Pay ₹41?", "₹41 · amazon.in")]
    assert "couldn't find ₹41" in owner.asked[2].detail and "couldn't find" not in owner.asked[0].detail
