"""Owns "one payment, one question" (GATE-204, owner 2026-10-30) for payments in a task's browser tab: which
Yes covers a payment-class call, from the facts code read on the page. Must not judge intent or read the worker's
numbers: the amount and merchant are the page's (`ActionFacts.page_amount` / `page_merchant`, read by the extension),
never what the worker declared; where the page shows none, the question says FERB couldn't read it, and shows the
amount the worker declared marked as the AI's, which FERB couldn't confirm on the page (GATE-575; never keyed).

The Yes covers that payment in this task tree's tab and the tabs it opened (`ActionFacts.tab_chain`: card choice, the
bank's page in a popup) until it completes or PAY_WINDOW_S passes; another tab chain or task asks again. Within it:
- on the checkout's own page, a changed amount (or one read now where the question couldn't read it) is another
  payment and asks again;
- on a bank or verification page (the tab moved to another host, or the input is in a frame of another host), an
  amount or merchant that differs from what was approved stops and asks again, showing both;
- the payment has completed, and the next payment-class call on the checkout asks again, once the chain went to a
  bank page and came back to the checkout's host, once a card form was submitted and another card form is submitted
  there, or once a paying form submit with no card form ran (a one-click checkout: the Yes covers exactly that submit;
  a bank verification page it opens is still covered).
A Mac app's payment is scoped the same way to its app window (bundle and window id) for PAY_WINDOW_S (coordinator
ruling, step 6b).
An approval lives in memory: after a restart FERB asks again (the safe side). A payment with neither a tab nor an app
window asks once per run (`confirm.decide`, SQ1)."""

import re
from dataclasses import dataclass, field
from datetime import datetime
from decimal import Decimal, InvalidOperation
from typing import Callable

from ferb.policy import confirm

PAY_WINDOW_S = 600.0                     # one payment's checkout, bank verification included (owner 2026-10-27/30)
UNREAD = "an amount FERB couldn't read on the page"
# Currency signs as the same currency's code, so "₹248.75" on the shop and "INR 248.75" on the bank page are one amount
_SIGNS = {"₹": "INR", "rs": "INR", "rs.": "INR", "$": "USD", "us$": "USD", "€": "EUR", "£": "GBP", "¥": "JPY",
          "a$": "AUD", "c$": "CAD", "s$": "SGD", "aed": "AED"}
_NUMBER = re.compile(r"\d[\d,\s]*(?:\.\d+)?")


def amount_key(shown: str | None) -> tuple[str, str] | None:
    """An amount as shown, compared: (currency code or "", the number without grouping); None when it holds no number.
    "₹2,48.75"-style Indian grouping and "INR 248.75" compare equal to "₹248.75"."""
    if not shown:
        return None
    text = " ".join(str(shown).split())
    m = _NUMBER.search(text)
    if m is None:
        return None
    try:
        number = Decimal(m.group(0).replace(",", "").replace(" ", "")).normalize()
    except InvalidOperation:
        return None
    before = text[:m.start()].rstrip().split(" ")[-1] if text[:m.start()].strip() else ""
    after = text[m.end():].lstrip().split(" ")[0] if text[m.end():].strip() else ""
    unit = ""
    for token, at_end in ((before, True), (after, False)):     # the words right beside the number
        low = token.casefold()
        unit = next((code for sign, code in sorted(_SIGNS.items(), key=lambda kv: -len(kv[0]))
                     if low == sign or (not sign[0].isalpha() and (low.endswith(sign) if at_end else
                                                                   low.startswith(sign)))), "")
        if not unit and re.fullmatch(r"[A-Z]{3}", token):     # a currency code as written (INR, USD)
            unit = token
        if unit:
            break
    return unit, format(number, "f")


def _same(a: str | None, b: str | None, key: Callable[[str | None], object]) -> bool | None:
    """Whether two page values agree; None when either is unread."""
    ka, kb = key(a), key(b)
    if ka is None or kb is None:
        return None
    if isinstance(ka, tuple) and isinstance(kb, tuple) and (not ka[0] or not kb[0]):
        return bool(ka[1] == kb[1])                      # one side shows no currency: the number decides
    return bool(ka == kb)


def merchant_key(shown: str | None) -> str | None:
    text = " ".join(str(shown or "").split()).casefold()
    return text or None


@dataclass(frozen=True)
class PayFacts:
    """What code read about one payment-class call in a tab: the tab chain, the page's and the input frame's hosts,
    the amount and merchant on the page, and whether it submits a card form."""
    chain: str
    host: str
    frame_host: str | None = None
    amount: str | None = None
    merchant: str | None = None
    submits_card: bool = False
    submits: bool = False                 # it submits a form (a card form or not)


@dataclass
class Approval:
    cid: str                              # the owner's Yes (a confirmations row)
    chain: str
    host: str                             # the checkout page's host when approved
    amount: str | None
    merchant: str | None
    at: datetime
    bank_host: str | None = None          # the bank page the chain went to, and what it showed
    bank_amount: str | None = None
    bank_merchant: str | None = None
    bank_done: bool = False               # a call ran on the bank page (the tab went there)
    cards: int = 0                        # card forms submitted under this Yes
    paid_one_click: bool = False          # a paying submit with no card form ran (one-click checkout)
    done: bool = False


@dataclass(frozen=True)
class Check:
    """A payment-class call against the tree's approvals: covered by `approval`, else a Need to ask."""
    approval: Approval | None = None
    need: confirm.Need | None = None


@dataclass
class Payments:
    now: Callable[[], datetime]
    _by: dict[tuple[str, str], Approval] = field(default_factory=dict)

    def _active(self, tree: str, chain: str) -> Approval | None:
        a = self._by.get((tree, chain))
        if a is None or a.done or (self.now() - a.at).total_seconds() > PAY_WINDOW_S:
            return None
        return a

    def check(self, tree: str, facts: PayFacts, values: dict[str, str]) -> Check:
        """Whether this call is covered; `values` are the question's other facts (site, account)."""
        a = self._active(tree, facts.chain)
        if a is None:
            return Check(need=need(facts, values))
        bank = facts.host != a.host or bool(facts.frame_host and facts.frame_host != a.host)
        if bank:
            amount = _same(facts.amount, a.amount, amount_key)
            merchant = _same(facts.merchant, a.merchant, merchant_key)
            if amount is False or merchant is False:     # the bank page shows another payment: ask, showing both
                return Check(need=need(facts, values, approved=a))
            return Check(approval=a)
        if a.bank_done or a.paid_one_click:
            return Check(need=need(facts, values))       # back from the bank page, or after a one-click submit: done
        if facts.submits_card and a.cards:
            return Check(need=need(facts, values))       # a second card form: another payment
        if facts.amount is not None and _same(facts.amount, a.amount, amount_key) is not True:
            return Check(need=need(facts, values))       # a changed amount, or read only now
        return Check(approval=a)

    def approve(self, tree: str, facts: PayFacts, cid: str) -> Approval:
        a = Approval(cid, facts.chain, facts.host, facts.amount, facts.merchant, self.now())
        self._by[(tree, facts.chain)] = a
        return a

    def observe(self, tree: str, chain: str | None, host: str | None, amount: str | None,
                merchant: str | None) -> None:
        """Any call in an approved chain: the bank page it reached and what that page showed (for the OTP question)."""
        a = self._active(tree, chain or "") if chain else None
        if a is None or not host or host == a.host:
            return
        a.bank_host = host
        a.bank_amount = amount or a.bank_amount
        a.bank_merchant = merchant or a.bank_merchant

    def ran(self, tree: str, facts: PayFacts, ok: bool) -> None:
        """A covered or approved call ran: a submitted card form counts; a call on the bank page marks that the chain
        reached it, so the checkout's next payment-class call is a new payment."""
        a = self._active(tree, facts.chain)
        if a is None or not ok:
            return
        if facts.submits_card and facts.host == a.host:
            a.cards += 1
        elif facts.submits and facts.host == a.host and not facts.frame_host and not a.cards and not a.bank_done:
            a.paid_one_click = True                      # a one-click checkout: this submit was the payment
        if facts.host != a.host:
            self.observe(tree, facts.chain, facts.host, facts.amount, facts.merchant)
            a.bank_done = True

    def for_otp(self, tree: str, host: str) -> Approval | None:
        """The approval an OTP question at `host` belongs to: the one whose bank page is there, else the tree's only
        active one."""
        active = [a for (t, chain), a in self._by.items() if t == tree and self._active(t, chain) is a]
        exact = [a for a in active if a.bank_host == host or a.host == host]
        if exact:
            return exact[-1]
        return active[0] if len(active) == 1 else None

    def forget(self, trees: set[str]) -> None:
        for k in [k for k in self._by if k[0] in trees]:
            del self._by[k]


def need(facts: PayFacts, values: dict[str, str], approved: Approval | None = None) -> confirm.Need:
    """The payment question: the amount and merchant read on the page (or that FERB couldn't read them); on a bank page
    that differs from the approved payment, both."""
    shown = {"amount": facts.amount or UNREAD, "merchant": facts.merchant or "",
             "site": values.get("site", ""), "account": values.get("account", ""), "recipients": "",
             "chain": facts.chain}
    # GATE-575: with no amount code read, the AI's own stated amount (`declared`, from the worker's destination) is
    # shown, marked as one FERB couldn't confirm on the page; it never keys the Yes and never replaces an amount read
    if facts.amount is None and values.get("declared"):
        shown["declared"] = values["declared"]
    if approved is not None:
        shown["approved"] = (approved.amount or UNREAD) + (f" to {approved.merchant}" if approved.merchant else "")
        shown["bank"] = facts.frame_host if facts.frame_host and facts.frame_host != approved.host else facts.host
    k = confirm.key(confirm.ALWAYS["pay"], "tab", facts.chain, amount_key(facts.amount) or facts.amount,
                    merchant_key(facts.merchant), shown.get("bank"))
    return confirm.Need(confirm.ALWAYS["pay"], k, shown, fresh=True)


def otp_card(site: str, a: Approval) -> tuple[str, str, str]:
    """GATE-513 (owner 2026-11-07: "otp needed to pay x on y"): a code asked on a payment's page says in its question
    what it pays: the amount and merchant that page shows (code read them), else the payment the owner approved, and
    that this page's couldn't be read. (question, why line, exact part); the place stays first (re-review R1)."""
    said = ("Your bank sent a one-time code to approve this payment. Type it only if it's the payment you approved. "
            "FERB enters it on this page once; FERB's AI never sees it.")      # GATE-566
    if a.bank_amount:
        paid = a.bank_amount + (f" to {a.bank_merchant}" if a.bank_merchant else "")
        return (f"{site}: code to pay {paid}", said,
                a.bank_amount + (f" → {a.bank_merchant}" if a.bank_merchant else "") + f" · {site}")
    approved = (a.amount or UNREAD) + (f" to {a.merchant}" if a.merchant else "")
    return (f"{site}: code for the payment you approved ({approved})", "FERB couldn't read the amount on this page. "
            + said, (a.amount or UNREAD) + (f" → {a.merchant}" if a.merchant else "") + f" · {site}")


def otp_line(a: Approval | None) -> str:
    """What an OTP (or card detail) question on a payment's later page repeats: the amount and merchant that page shows
    when code read them, else the payment the owner approved and that this page's couldn't be read."""
    if a is None:
        return ""
    if a.bank_amount:
        return f"This page shows {a.bank_amount}" + (f" to {a.bank_merchant}" if a.bank_merchant else "") + "."
    approved = (a.amount or UNREAD) + (f" to {a.merchant}" if a.merchant else "")
    return f"For the payment you approved ({approved}); FERB couldn't read the amount on this page."
