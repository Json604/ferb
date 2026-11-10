"""One payment, one question (GATE-204, owner 2026-10-30): a payment in a task's browser tab asks once, with
the amount and merchant code read on the page (never the worker's number); the Yes covers that tab and what it opens
(card choice, the bank page) until the payment completes or 10 minutes pass; another tab or task asks again; a bank
page showing another amount or merchant stops and asks again, showing both; an OTP question repeats the bank page's
amount and merchant. Replaces "same amount, any site, 10 minutes" (test_confirmations, before step 6b). Live case:
Orderly order 400d67fb asked six times in five minutes for one ₹248.75 payment."""
import json
from datetime import datetime, timedelta
from typing import Any

from pydantic import BaseModel

from ferb.core.types import ActionFacts, ActionSpec, Target, ToolResult
from ferb.policy import confirm, payments
from ferb.policy.rules import Kind
from tests.conftest import Answers, FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call


class _Tab(FakeCap):
    """A browser act in a task tab: what it reads on the page arrives as ActionFacts, as `_TabDriver.act_facts`
    reports it (page URL, frame host, the page's amount and merchant, the tab chain, a submitted form's fields)."""
    generic_input = True

    class Input(BaseModel):
        tab: str = "t1"
        host: str = "www.orderly.example"
        amount: str | None = None
        merchant: str | None = None
        frame: str | None = None
        card: bool = False
        submit: bool = False                     # a form submit with no card fields (a one-click checkout)
        code: bool = False                       # the run asks for a login code here (a bank page's OTP)

    def __init__(self, d: RunningDaemon | None = None) -> None:
        super().__init__("probe.tab", Kind.CLICK, {"probe"})
        self.d = d
        self.ran: list[str] = []

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.CLICK, Target(site=args.host),
                          content=f"click:{args.host}:{args.card}:{args.submit}:{args.code}:{len(self.ran)}",
                          facts=ActionFacts(page_url=f"https://{args.host}/checkout", page_amount=args.amount,
                                            page_merchant=args.merchant, frame_host=args.frame, tab_chain=args.tab,
                                            submits_form=True if args.card or args.submit else None,
                                            field_kinds=("card_number", "card_csc") if args.card else ()))

    async def run(self, args: Any, ctx: Any) -> ToolResult:
        self.ran.append(args.host)
        if args.code and self.d is not None:
            await self.d.daemon.gateway.ask_login_code(ctx, f"https://{args.host}")
        return ToolResult({"ok": True})


def _asked(d: RunningDaemon) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        rows = d.daemon.db.conn.execute("SELECT input_json FROM spans WHERE kind = 'tool.call' AND "
                                        "json_extract(input_json, '$.tool') = 'owner.ask' ORDER BY rowid").fetchall()
        return [json.loads(r["input_json"]) for r in rows]
    return d.run(go())


def _pay(d: RunningDaemon, grant: Any, **args: Any) -> dict[str, Any]:
    return gateway_tool_call(d, grant, "probe.tab", effect="pay", **args)


# ---------- the Orderly flow, through the gateway ----------

# decision: GATE-204, GATE-216
def test_the_orderly_flow_asks_once_and_the_otp_question_repeats_the_bank_page(daemon: RunningDaemon) -> None:
    """Pay click → card choice → card form (in the gateway's frame) → bank page (navigated) with its OTP → its submit:
    one FERB question showing the page's ₹248.75 and Orderly, one hidden code question repeating the bank page's
    amount. Then the next payment on the checkout, and another tab, ask again."""
    tab = _Tab(daemon)
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("order food under 200 rupees on orderly.example"), [tab])
    owner = Answers(daemon, granted=True, choice=1)
    try:
        for step in ({"amount": "Pay ₹248.75", "merchant": "Orderly"},                 # one-click Pay
                     {"amount": "₹248.75", "merchant": "Orderly"},                     # the card choice
                     {"frame": "api.razorpay.com", "card": True},                     # card form in the gateway's frame
                     {"host": "secure.bank.example", "amount": "INR 248.75", "merchant": "ORDERLY",
                      "code": True},                                                  # bank page: its OTP
                     {"host": "secure.bank.example", "amount": "INR 248.75"}):  # bank page: submit
            r = _pay(daemon, grant, **step)
            assert not r["is_error"], r
        confirm_questions = _asked(daemon)
        assert len(confirm_questions) == 1
        question = confirm_questions[0]["args"]["question"]
        assert "₹248.75" in question and "Orderly" in question
        codes = [ap for ap in owner.asked if ap.kind == "owner_login_code"]
        assert len(codes) == 1 and codes[0].summary == "secure.bank.example: code to pay INR 248.75 to ORDERLY"   # GATE-513
        assert len(tab.ran) == 5
        r = _pay(daemon, grant, amount="Pay ₹248.75", merchant="Orderly")         # back on the checkout: a new payment
        assert not r["is_error"] and len(_asked(daemon)) == 2
        r = _pay(daemon, grant, tab="t2", amount="Pay ₹248.75", merchant="Orderly")   # another tab: asks again
        assert not r["is_error"] and len(_asked(daemon)) == 3
    finally:
        owner.stop()


# decision: GATE-204, GATE-217
def test_a_bank_page_with_another_amount_stops_and_asks_showing_both(daemon: RunningDaemon) -> None:
    tab = _Tab()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("order food"), [tab])
    owner = Answers(daemon, granted=True, choice=1)
    try:
        assert not _pay(daemon, grant, amount="₹248.75", merchant="Orderly")["is_error"]
        owner.stop()
        owner = Answers(daemon, granted=True, choice=2)
        r = _pay(daemon, grant, host="bank.example", amount="INR 999.00", merchant="Evil Ltd")
        assert r["is_error"] and len(tab.ran) == 1
        asked = _asked(daemon)
        assert len(asked) == 2
        second = asked[1]["args"]["question"]
        assert "INR 999.00" in second and "you approved ₹248.75 to Orderly" in second and "bank.example" in second
    finally:
        owner.stop()


# decision: GATE-204, GATE-217
def test_a_changed_amount_on_the_checkout_asks_again_and_an_unread_one_says_so(daemon: RunningDaemon) -> None:
    tab = _Tab()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("buy it"), [tab])
    owner = Answers(daemon, granted=True, choice=1)
    try:
        assert not _pay(daemon, grant)["is_error"]                         # no amount on the page
        assert "an amount FERB couldn't read on the page" in _asked(daemon)[0]["args"]["question"]
        assert not _pay(daemon, grant)["is_error"] and len(_asked(daemon)) == 1   # still unread: covered
        assert not _pay(daemon, grant, amount="$20.00")["is_error"]        # read now: asked, showing it
        assert len(_asked(daemon)) == 2 and "$20.00" in _asked(daemon)[1]["args"]["question"]
        assert not _pay(daemon, grant, amount="USD 20")["is_error"] and len(_asked(daemon)) == 2   # the same amount
        assert not _pay(daemon, grant, amount="$25.00")["is_error"] and len(_asked(daemon)) == 3   # changed
    finally:
        owner.stop()


# decision: GATE-204, GATE-216
def test_the_question_never_shows_the_workers_number(daemon: RunningDaemon) -> None:
    tab = _Tab()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("order food"), [tab])
    owner = Answers(daemon, granted=True, choice=1)
    try:
        r = gateway_tool_call(daemon, grant, "probe.tab", effect="pay", amount="₹248.75",
                              destination={"amount": 1.0, "currency": "INR"})
    finally:
        owner.stop()
    assert not r["is_error"]
    question = _asked(daemon)[0]["args"]["question"]
    assert "₹248.75" in question and "INR 1" not in question


# decision: GATE-204, GATE-217
def test_a_payment_yes_lapses_after_ten_minutes_and_a_no_is_final(daemon: RunningDaemon) -> None:
    tab = _Tab()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("order food on www.orderly.example"), [tab])
    owner = Answers(daemon, granted=True, choice=1)
    try:
        assert not _pay(daemon, grant, amount="₹248.75")["is_error"]
        assert not _pay(daemon, grant, amount="₹248.75")["is_error"] and len(_asked(daemon)) == 1
        daemon.call_soon(daemon.daemon.clock.advance, 11 * 60)
        import time
        time.sleep(0.05)
        assert not _pay(daemon, grant, amount="₹248.75")["is_error"] and len(_asked(daemon)) == 2
        owner.stop()
        owner = Answers(daemon, granted=True, choice=2)
        r = _pay(daemon, grant, tab="t9", amount="₹50")
        again = _pay(daemon, grant, tab="t9", amount="₹50")
        assert r["is_error"] and again["is_error"] and "already said no" in again["content"]["error"]
        assert len(_asked(daemon)) == 3                                    # the No is final for that payment
    finally:
        owner.stop()


# decision: GATE-202, GATE-214
def test_a_card_form_submitted_is_a_payment_without_any_declaration(daemon: RunningDaemon) -> None:
    """Decision 1: the facts raise the question; the worker's declaration (here: none, then `navigate`) only adds."""
    tab = _Tab()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("buy it"), [tab])
    owner = Answers(daemon, granted=False, choice=2)
    try:
        r = gateway_tool_call(daemon, grant, "probe.tab", card=True, amount="$5")
        r2 = gateway_tool_call(daemon, grant, "probe.tab", tab="t2", card=True, amount="$5", effect="navigate")
    finally:
        owner.stop()
    assert r["is_error"] and r2["is_error"] and tab.ran == []
    assert [a["args"]["confirm"][0]["rule"] for a in _asked(daemon)] == ["floor:pay", "floor:pay"]


# decision: GATE-575, GATE-204
def test_with_no_amount_read_the_card_shows_the_ais_amount_marked_unconfirmed(daemon: RunningDaemon) -> None:
    """Owner 2026-11-10 (Amazon cash-on-delivery order): the card said only "FERB couldn't read the amount" although
    the AI had stated ₹70. With no amount code read, the question and card show the AI's amount, marked as one FERB
    couldn't confirm on the page; an amount code read stands alone, and the Yes is never keyed by the AI's number."""
    tab = _Tab()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("order the batteries on www.orderly.example"), [tab])
    owner = Answers(daemon, granted=True, choice=1)
    try:
        assert not gateway_tool_call(daemon, grant, "probe.tab", submit=True, effect="pay",
                                     destination={"amount": 70, "currency": "₹"})["is_error"]
        assert not gateway_tool_call(daemon, grant, "probe.tab", tab="t2", submit=True, amount="₹65", effect="pay",
                                     destination={"amount": 70, "currency": "₹"})["is_error"]
    finally:
        owner.stop()
    unread, read = [a["args"] for a in _asked(daemon)]
    assert "₹70" in unread["question"] and "couldn't confirm" in unread["question"]
    assert "₹65" in read["question"] and "₹70" not in read["question"]
    assert "70" not in unread["confirm"][0]["key"]                      # never keyed by the AI's number
    need = payments.need(payments.PayFacts("t1", "www.orderly.example"),
                         {"declared": "₹70", "site": "orderly.example"})
    card = confirm.card([need])
    assert "₹70" in card.question and "₹70" in card.exact and "couldn't confirm" in card.question


# ---------- the rule on its own ----------

# decision: GATE-217
def test_amounts_compare_by_currency_and_number() -> None:
    assert payments.amount_key("₹248.75") == payments.amount_key("INR 248.75") == ("INR", "248.75")
    assert payments.amount_key("Pay $1,020.50 now") == ("USD", "1020.5")
    assert payments.amount_key("Total") is None and payments.amount_key(None) is None
    assert payments._same("248.75", "₹248.75", payments.amount_key) is True     # one side shows no currency
    assert payments._same("₹248.75", "$248.75", payments.amount_key) is False


# decision: GATE-216, GATE-217
def test_the_rule_covers_a_tab_chain_and_completes_after_the_bank_page() -> None:
    now = {"t": datetime(2026, 10, 30, 12, 0)}
    p = payments.Payments(lambda: now["t"])
    shop = payments.PayFacts("t1", "shop.example", amount="$5", merchant="Shop")
    first = p.check("tree", shop, {})
    assert first.need is not None and first.need.fresh and first.need.values["amount"] == "$5"
    p.approve("tree", shop, "c1")
    assert p.check("tree", shop, {}).approval is not None
    assert p.check("other-tree", shop, {}).need is not None             # another task asks again
    assert p.check("tree", payments.PayFacts("t2", "shop.example", amount="$5"), {}).need is not None
    bank = payments.PayFacts("t1", "bank.example", amount="USD 5")
    assert p.check("tree", bank, {}).approval is not None
    p.ran("tree", bank, True)
    assert p.for_otp("tree", "bank.example") is not None
    assert p.check("tree", shop, {}).need is not None                   # back on the shop: that payment completed
    p.approve("tree", shop, "c2")
    card = payments.PayFacts("t1", "shop.example", amount="$5", submits_card=True)
    assert p.check("tree", card, {}).approval is not None
    p.ran("tree", card, True)
    assert p.check("tree", card, {}).need is not None                   # a second card form: another payment
    now["t"] += timedelta(seconds=601)
    assert p.check("tree", shop, {}).need is not None                   # lapsed
    p.forget({"tree"})
    assert not p._by


# decision: GATE-204
def test_an_otp_line_repeats_the_bank_page_or_says_it_couldnt_read_it() -> None:
    a = payments.Approval("c", "t1", "shop.example", "₹248.75", "Orderly", datetime(2026, 10, 30))
    assert "couldn't read the amount on this page" in payments.otp_line(a) and "₹248.75 to Orderly" in \
        payments.otp_line(a)
    a.bank_amount, a.bank_merchant = "INR 248.75", "ORDERLY"
    assert payments.otp_line(a) == "This page shows INR 248.75 to ORDERLY."
    assert payments.otp_line(None) == ""


# decision: GATE-217
def test_a_one_click_checkout_yes_covers_that_one_paying_submit(daemon: RunningDaemon) -> None:
    """Coordinator ruling (step 6b): a paying form submit with no card form and no bank page in the chain is the
    payment itself; the next one asks again. Its bank verification page, if it opens one, is still covered."""
    tab = _Tab()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("reorder my usual"), [tab])
    owner = Answers(daemon, granted=True, choice=1)
    try:
        assert not _pay(daemon, grant, submit=True, amount="₹248.75", merchant="Orderly")["is_error"]
        assert not _pay(daemon, grant, host="bank.example", amount="INR 248.75")["is_error"]
        assert len(_asked(daemon)) == 1                                   # its verification page: covered
        assert not _pay(daemon, grant, submit=True, amount="₹248.75", merchant="Orderly")["is_error"]
        assert len(_asked(daemon)) == 2                                   # a second order: asked again
    finally:
        owner.stop()


class _MacPay(FakeCap):
    """A Mac click in an app window; its stable identity names the window (as `driver.click_identity` does)."""
    generic_input = True

    class Input(BaseModel):
        window: int = 7
        point: int = 0

    def __init__(self) -> None:
        super().__init__("mac.fakepay", Kind.MAC_INPUT, {"probe"})

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.MAC_INPUT, Target(site="com.apple.AppStore"), content=f"{args.window}:{args.point}")

    def ledger_args(self, args: Any, hashed: dict[str, Any]) -> dict[str, Any]:
        return {"at": {"window": args.window, "point": [args.point, 0]}}


# decision: GATE-216
def test_a_mac_payment_is_scoped_to_its_app_window(daemon: RunningDaemon) -> None:
    """Coordinator ruling (step 6b): like a tab, one Yes covers that app window for 10 minutes."""
    mac = _MacPay()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("buy the app"), [mac])
    owner = Answers(daemon, granted=True, choice=1)
    try:
        for point in (1, 2):
            assert not gateway_tool_call(daemon, grant, "mac.fakepay", effect="pay", point=point)["is_error"]
        assert len(_asked(daemon)) == 1
        assert not gateway_tool_call(daemon, grant, "mac.fakepay", effect="pay", window=8)["is_error"]
        assert len(_asked(daemon)) == 2                                   # another window asks
    finally:
        owner.stop()
    assert mac.calls == 3
