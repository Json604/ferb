"""A payment press is checked against the page the owner was asked about, read fresh just before the press (GATE-580;
owner 2026-11-10: "why nto recheck??"). The page's script changes the total while the question waits: nothing is
pressed and the owner is asked again with the page as it is now. Unchanged: pressed once. Approved although FERB
couldn't find the amount: pressed while the control is the same one."""
import asyncio
import json
from datetime import datetime
from typing import Any

from ferb.policy import payments
from tests.conftest import RunningDaemon, gateway_request, gateway_setup
from tests.test_pay_picture import PIXELS, _PictureTab
from tests.test_payments import _asked

CHECKOUT = "https://www.orderly.example/checkout"


class _LivePage:
    """The page as the browser has it now (a script may change it after the snapshot): the driver's fresh reads."""

    def __init__(self, lines: list[str]) -> None:
        self.url, self.lines, self.reads = CHECKOUT, list(lines), 0

    def owned_by(self, tab: str, owner: str) -> str:
        return tab

    async def shows(self, ref: str, text: str) -> dict[str, Any]:
        self.reads += 1
        return {"open": True, "url": self.url, "shows": True}

    async def find(self, ref: str, text: str, offset: int = 0, max_chars: int = 12000) -> dict[str, Any]:
        # HAND-484's shape: the latest snapshot's matching elements first (as old as that snapshot), then the page's
        # lines now; a stale element's words never count as the page now
        stale = [{"element": "s1e9", "role": "button", "label": "Place your order", "text": "Order Total: ₹70.00"}]
        hits = [{"frame_id": 0, "text": line} for line in self.lines if text.casefold() in line.casefold()]
        return {"text": json.dumps(stale + hits), "next_offset": None, "element_matches": len(stale)}


class _CheckoutTab(_PictureTab):
    def __init__(self, page: _LivePage) -> None:
        super().__init__(PIXELS)
        self.driver = page


class _Owner:
    """Answers FERB's questions in turn (each a choice: 1 Yes, 2 No), running `before` first (the page's script)."""

    def __init__(self, d: RunningDaemon, answers: list[tuple[int, Any]]) -> None:
        self.d, self.answers, self.asked = d, list(answers), []
        self._task = asyncio.run_coroutine_threadsafe(self._loop(), d.loop)

    async def _loop(self) -> None:
        approvals = self.d.daemon.state.approvals
        while True:
            for ap in approvals.pending():
                choice, before = self.answers.pop(0) if self.answers else (2, None)
                self.asked.append(ap)
                if before is not None:
                    before()
                approvals.resolve(ap.id, choice == 1, choice)
            await asyncio.sleep(0.02)

    def stop(self) -> None:
        self._task.cancel()


def _press(d: RunningDaemon, grant: Any, shown: list[str]) -> dict[str, Any]:
    args = {"tab": "t1", "submit": True, "effect": "pay", "shown": shown, "destination": {"amount": 70, "currency": "₹"}}
    return d.run(d.daemon.gateway.call(grant, "probe.tab", args), timeout=20)


def _setup(d: RunningDaemon, lines: list[str]) -> tuple[Any, _LivePage, _CheckoutTab]:
    page = _LivePage(lines)
    tab = _CheckoutTab(page)
    grant, _ = gateway_setup(d, {"probe"}, gateway_request("order the batteries on www.orderly.example"), [tab])
    return grant, page, tab


# decision: GATE-580, GATE-578
def test_a_total_changed_by_script_after_the_yes_isnt_pressed_and_the_owner_is_asked_again(
        daemon: RunningDaemon) -> None:
    grant, page, tab = _setup(daemon, ["Items: ₹63.00", "Order Total: ₹70.00"])

    def script() -> None:
        page.lines = ["Items: ₹73.00", "Order Total: ₹80.00"]
    owner = _Owner(daemon, [(1, script), (2, None)])
    try:
        r = _press(daemon, grant, ["₹63.00", "₹70.00"])
    finally:
        owner.stop()
    assert r["is_error"] and tab.ran == []                          # not pressed
    first, again = [a["args"]["question"] for a in _asked(daemon)]
    assert "couldn't find" not in first and "FERB couldn't find ₹70 on this page" in again
    assert owner.asked[1].picture == "shown"                         # a new picture of the page as it is now
    assert daemon.daemon.gateway.payments.active("task-1", "t1") is None


# decision: GATE-580
def test_the_owner_says_yes_to_the_changed_page_and_it_is_pressed_once(daemon: RunningDaemon) -> None:
    grant, page, tab = _setup(daemon, ["Order Total: ₹70.00"])

    def script() -> None:
        page.lines = ["Order Total: ₹80.00"]
    owner = _Owner(daemon, [(1, script), (1, None)])
    try:
        r = _press(daemon, grant, ["₹70.00"])
    finally:
        owner.stop()
    assert not r["is_error"] and tab.ran == ["www.orderly.example"] and len(_asked(daemon)) == 2


# decision: GATE-580, GATE-204
def test_an_unchanged_page_is_pressed_once_after_one_question(daemon: RunningDaemon) -> None:
    grant, page, tab = _setup(daemon, ["Items: ₹63.00", "Order Total: ₹70.00"])
    owner = _Owner(daemon, [(1, None)])
    try:
        r = _press(daemon, grant, ["₹63.00", "₹70.00"])
    finally:
        owner.stop()
    assert not r["is_error"] and tab.ran == ["www.orderly.example"] and len(_asked(daemon)) == 1
    assert page.reads == 1                                           # read fresh once, just before the press


# decision: GATE-580
def test_approved_although_not_found_is_pressed_while_the_control_is_the_same(daemon: RunningDaemon) -> None:
    grant, page, tab = _setup(daemon, ["Order Total: ₹69.00"])

    def script() -> None:
        page.lines = ["Order Total: ₹81.00"]
    owner = _Owner(daemon, [(1, script)])
    try:
        r = _press(daemon, grant, ["₹69.00"])                         # the card said FERB couldn't find ₹70
    finally:
        owner.stop()
    assert not r["is_error"] and tab.ran == ["www.orderly.example"] and len(_asked(daemon)) == 1
    assert "FERB couldn't find ₹70 on this page" in _asked(daemon)[0]["args"]["question"]


# decision: GATE-580
def test_a_control_moved_to_another_page_is_asked_again(daemon: RunningDaemon) -> None:
    grant, page, tab = _setup(daemon, ["Order Total: ₹70.00"])

    def script() -> None:
        page.url = "https://www.orderly.example/upsell#offer"        # the app changed the page without a load
    owner = _Owner(daemon, [(1, script), (2, None)])
    try:
        r = _press(daemon, grant, ["₹70.00"])
    finally:
        owner.stop()
    assert r["is_error"] and tab.ran == [] and len(_asked(daemon)) == 2


# decision: GATE-580
def test_the_same_change_is_asked_about_once_then_the_worker_is_told() -> None:
    """A page that flips back and forth: one question per distinct observed change, then a refusal to the worker."""
    p = payments.Payments(lambda: datetime(2026, 11, 10, 12, 0))
    facts = payments.PayFacts("t1", "shop.example", shown=("₹70.00",), declared="₹70", page=CHECKOUT, control="Pay")
    a = p.approve("tree", facts, "c1")
    assert a.seen and a.declared == "₹70"
    gone = payments.Pressed(CHECKOUT + "#top", ("₹80.00",))
    assert payments.pressed_change(a, facts, payments.Pressed(CHECKOUT + "#top", ("₹70.00",)), True) is None
    assert payments.pressed_change(a, facts, gone, True) == "amount"
    assert payments.pressed_change(a, facts, payments.Pressed(CHECKOUT, None), False) == "amount"   # unreadable
    assert payments.pressed_change(a, facts, payments.Pressed("https://shop.example/other", ("₹70",)), True) == "page"
    other = payments.PayFacts("t1", "shop.example", control="Choose card")   # another control: GATE-204's scope
    assert payments.pressed_change(a, other, payments.Pressed("https://shop.example/card", ("₹70",)), False) is None
    bank = payments.PayFacts("t1", "bank.example")
    assert payments.pressed_change(a, bank, gone, False) is None     # a bank page keeps GATE-204/216's rules
    assert payments.pressed_change(a, facts, None, True) is None     # no fresh read (a Mac app): nothing to compare
    state = ("amount", CHECKOUT, ("₹80.00",))
    assert p.first_change("tree", "t1", state) and not p.first_change("tree", "t1", state)
    assert payments.digits_of("₹1,24,000.50") == "000" and payments.digits_of("₹70") == "70"
