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


# decision: GATE-582
def test_a_same_site_post_on_a_page_with_amounts_asks_as_a_payment_and_nothing_else_does(daemon: RunningDaemon) -> None:
    form = _Form()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("order it on www.shop.example"), [form])
    owner = Answers(daemon, granted=False, choice=2)
    try:
        r = gateway_tool_call(daemon, grant, "probe.form", shown=["₹499.00", "₹40.00"])        # no card field
        assert r["is_error"] and form.ran == [] and len(_pays(daemon)) == 1
        for i, case in enumerate(({"method": "GET", "shown": ["₹499.00"]},                    # a search, a filter
                                  {"shown": []},                                              # no amount shown
                                  {"action_host": "api.razorpay.com", "shown": ["₹499.00"]},  # another site's form
                                  {"action_host": None, "shown": ["₹499.00"], "url": "https://www.shop.example/x",
                                   "method": "GET"})):
            r = gateway_tool_call(daemon, grant, "probe.form", tab=f"t{i + 2}", **case)
            assert not r["is_error"], (case, r)
        assert len(_pays(daemon)) == 1 and len(form.ran) == 4
        r = gateway_tool_call(daemon, grant, "probe.form", tab="t9", url="https://pay.shop.example/go",
                              shown=["₹499.00"])                                              # same site, other host
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
        r = gateway_tool_call(daemon, grant, "probe.form", tab="t2", shown=["₹499.00"])
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
            buy = gateway_tool_call(daemon, grant, "probe.form", tab=chain, url=PRODUCT, action_host="www.amazon.in",
                                    shown=PRODUCT_AMOUNTS, label="Buy Now")
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
