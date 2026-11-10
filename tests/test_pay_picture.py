"""The payment question's one-time picture of the payment page (GATE-578, owner 2026-11-10: "what if we send a one
time screenhsot of the cehckout/paymetn page with this pay X money"; GATE-579). Taken when FERB asks, held in
memory by the question's id for FERB.app only, taken once, gone after the answer and at the task's end; never in the
database, journal, trace or a file. A model-visible screenshot of the same page is still masked (GATE-206)."""
import asyncio
import base64
import time
from typing import Any

import pytest

from ferb.act.evidence.mask import pixel
from ferb.act.point import PreviewTarget
from ferb.core.errors import Withheld
from ferb.daemon import pay_picture
from ferb.daemon.questions import card_parts
from tests.conftest import Answers, RunningDaemon, gateway_request, gateway_setup
from tests.support.images import black, solid_jpeg
from tests.test_payments import _Tab

PIXELS = b"\xff\xd8PAYMENT-PAGE-PIXELS-7731"


class _PictureTab(_Tab):
    """A task tab whose surface (`preview_target`, as the browser tools declare it) captures `frame`."""

    def __init__(self, frame: Any) -> None:
        super().__init__()
        self.frame = frame

    def preview_target(self, args: Any, owner: str, observed: Any = None) -> PreviewTarget | None:
        async def capture() -> bytes:
            if isinstance(self.frame, BaseException):
                raise self.frame
            return bytes(self.frame)

        async def resolve() -> tuple[dict[str, Any], Any]:
            return {"tab": args.tab}, capture
        return PreviewTarget(f"tab:probe/{args.tab}", "the browser", resolve)


def _pending(daemon: RunningDaemon) -> Any:
    approvals = daemon.daemon.state.approvals
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        waiting = list(approvals.pending())
        if waiting:
            return waiting[0]
        time.sleep(0.02)
    raise AssertionError("no question was asked")


def _pay_in_background(daemon: RunningDaemon, grant: Any, tab: str) -> Any:
    args = {"tab": tab, "submit": True, "effect": "pay", "shown": ["₹70.00"],
            "destination": {"amount": 70, "currency": "₹"}}
    return asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.call(grant, "probe.tab", args), daemon.loop)


# decision: GATE-578, GATE-579, DMN-23
def test_the_payment_picture_reaches_ferb_app_once_and_is_never_recorded(daemon: RunningDaemon) -> None:
    from ferb.cli.client import Client, RpcError
    tab = _PictureTab(PIXELS)
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("order the batteries on www.orderly.example"), [tab])
    pictures = daemon.daemon.gateway.hooks.pictures
    approvals = daemon.daemon.state.approvals

    paid = _pay_in_background(daemon, grant, "t1")
    ap = _pending(daemon)
    assert card_parts(ap).get("picture") is True and "picture_problem" not in card_parts(ap)
    assert ap.summary == "Pay ₹70?"
    c = Client(daemon.sock)
    try:
        with pytest.raises(RpcError) as refused:                      # any other program as the owner: refused
            c.call("questions.picture", {"approval_id": ap.id})
        assert "FERB.app only" in str(refused.value)
    finally:
        c.close()
    daemon.daemon.server.is_app = lambda token: token is not None     # FERB.app's signature, stood in for
    c = Client(daemon.sock)
    try:
        first = c.call("questions.picture", {"approval_id": ap.id})
        again = c.call("questions.picture", {"approval_id": ap.id})
    finally:
        c.close()
    assert base64.b64decode(first["image"]) == PIXELS and again == {}   # one time
    daemon.call_soon(approvals.resolve, ap.id, True, 1)
    assert not paid.result(10)["is_error"]

    paid = _pay_in_background(daemon, grant, "t2")                     # never fetched: gone with the answer
    second = _pending(daemon)
    assert pictures.has(second.id)
    daemon.call_soon(approvals.resolve, second.id, False, 2)
    assert paid.result(10)["is_error"] and not pictures.has(second.id)

    pictures.hold("ap-x", "task-1", PIXELS)                            # and with its task, whatever else happened
    daemon.call_soon(daemon.daemon.gateway.forget_tasks, {"task-1"})
    assert not pictures.has("ap-x")

    encoded = base64.b64encode(PIXELS)
    leaked = [p for p in daemon.daemon.home.rglob("*") if p.is_file()
              and (b"PAYMENT-PAGE-PIXELS" in (data := p.read_bytes()) or encoded in data)]
    assert not leaked                                                  # db (and its WAL), journal, logs, files

    async def rows() -> int:
        conn = daemon.daemon.db.conn
        return int(conn.execute("SELECT (SELECT COUNT(*) FROM spans WHERE input_json LIKE '%PAYMENT-PAGE%' OR "
                                "output_json LIKE '%PAYMENT-PAGE%') + (SELECT COUNT(*) FROM journal WHERE "
                                "data_json LIKE '%PAYMENT-PAGE%')").fetchone()[0])
    assert daemon.run(rows()) == 0


# decision: GATE-579
def test_a_page_that_couldnt_be_captured_says_so_and_never_blocks_the_question(daemon: RunningDaemon) -> None:
    tab = _PictureTab(Withheld("pixels withheld: the extension didn't say where the page's secret fields are"))
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("order the batteries on www.orderly.example"), [tab])
    owner = Answers(daemon, granted=True, choice=1)
    try:
        assert not _pay_in_background(daemon, grant, "t1").result(15)["is_error"]
    finally:
        owner.stop()
    asked = owner.asked[0]
    assert card_parts(asked)["picture_problem"] == pay_picture.NOT_CAPTURED and "picture" not in card_parts(asked)


# decision: GATE-578, GATE-206
def test_a_model_visible_screenshot_of_the_same_page_is_still_masked(monkeypatch: pytest.MonkeyPatch) -> None:
    """The owner's exception covers only the owner's picture: the worker's screenshot of the same checkout page is
    painted over where its card field is, and the payment picture adds no text-recognition pass of its own."""
    from tests.test_pixels_to_model import _same, _state, _tab_driver
    page = solid_jpeg(200, 100)
    card_field = _state((10, 10, 30, 10))                              # a card number box, in CSS px (2x capture)
    d, ref, _ = _tab_driver({"data": base64.b64encode(page).decode(), "mask": _same(card_field)})
    model_sees = asyncio.run(d.screenshot(ref))
    assert black(pixel(model_sees, 40, 25))

    def no_pass(*a: Any, **k: Any) -> Any:
        raise AssertionError("the owner's picture gets no text-recognition pass")
    monkeypatch.setattr("ferb.sanitize.images.ImageRedactor", no_pass)
    locks = daemon_free_locks()
    owner_sees = asyncio.run(pay_picture.capture(_PictureTab(page), _Tab.Input(), "task-1", locks, [], "picture:t"))
    assert owner_sees == page


def daemon_free_locks() -> Any:
    from ferb.act.locks import ResourceLocks
    return ResourceLocks().hold
