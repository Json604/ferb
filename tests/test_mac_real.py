"""Build step 7, real Mac (pytest -m real), read-only and invisible:
- the owner-input tap tells FERB's marked events from the owner's (an unmarked mouse-move to where the pointer
  already is stands in for the owner; nothing moves);
- an Accessibility snapshot of Finder through the real driver (read only; contents are asserted, never printed).
"""

import asyncio
import time

import pytest

from ferb.act.gui_lock import GuiLock
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.input import RealInput
from ferb.sense.watchers.owner_input import FERB_EVENT_MARK, OwnerInput

pytestmark = pytest.mark.real


def _still_move(marked: bool) -> None:
    import Quartz
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)
    here = Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))
    ev = Quartz.CGEventCreateMouseEvent(src, Quartz.kCGEventMouseMoved, here, Quartz.kCGMouseButtonLeft)
    if marked:
        Quartz.CGEventSetIntegerValueField(ev, Quartz.kCGEventSourceUserData, FERB_EVENT_MARK)
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)


def test_owner_input_tap_ignores_ferbs_marked_events_and_sees_the_owners() -> None:
    oi = OwnerInput(quiet_s=0.5)
    if not oi.start():
        pytest.skip("no event tap (this process needs Accessibility or Input Monitoring)")
    try:
        time.sleep(0.6)
        if oi.active():
            pytest.skip("the owner is using the mouse/keyboard right now")
        for _ in range(5):
            _still_move(marked=True)
        time.sleep(0.2)
        assert not oi.active()                                   # FERB's own input never counts as the owner
        _still_move(marked=False)
        time.sleep(0.2)
        assert oi.active()                                       # anything unmarked is the owner
        time.sleep(0.6)
        assert not oi.active()                                   # quiet period over
    finally:
        oi.stop()


def test_accessibility_snapshot_of_finder_is_structured() -> None:
    d = MacComputerDriver("dev", RealInput(lambda: False, GuiLock()))
    app, out = asyncio.run(d.snapshot("com.apple.finder"))
    assert app.bundle_id == "com.apple.finder" and "elements" in out
    assert all({"id", "role", "label"} <= set(e) for e in out["elements"])
    assert all(e["id"].startswith("s1e") for e in out["elements"])
