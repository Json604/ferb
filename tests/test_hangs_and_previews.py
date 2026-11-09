"""Past-bug guards for the browser hang and the laggy preview (2026-11-08 runs and lab; HAND-435..437, DMN-406..408): a read
that hung says so and what helps; a screenshot after it doesn't wait on the page again; the preview's own capture ends
within the preview's time; the preview looks alongside a read; an unchanged frame isn't sent again and "too long"
doesn't stick; a call naming only an app keeps its window's picture; a locked screen is said; a preview walk never
replaces the worker's tree."""

import asyncio
import time
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.act.browser import ext_bridge
from ferb.act.browser.ext_bridge import Bridge, _Conn, not_responding
from ferb.act.browser.ext_protocol import Result
from ferb.act.browser.pairing import Binding
from ferb.act.locks import ResourceLocks
from ferb.act.mac import screenshot as shotm
from ferb.act.mac.screenshot import CaptureBusy, ScreenOff
from ferb.act.point import PREVIEW_DEADLINE, PreviewTarget, preview_left
from ferb.core.errors import DriverError
from ferb.core.types import Identity
from ferb.daemon.task_preview import TaskPreviews


class _Writer:
    def __init__(self) -> None:
        self.sent: list[bytes] = []

    def write(self, b: bytes) -> None:
        self.sent.append(b)

    async def drain(self) -> None:
        return None


def _bridge() -> Bridge:
    br = Bridge(Binding("i" * 8, "chrome", "Default", Identity.OWNER), None, lambda *a: None, "dev")  # type: ignore[arg-type]
    conn = _Conn(_Writer())                                              # type: ignore[arg-type]
    conn.key, conn.nonce = b"k" * 32, "n" * 32
    br.conn = conn
    return br


async def _answer(br: Bridge, op: str, args: dict[str, Any], result: dict[str, Any], timeout: float = 30) -> Any:
    call = asyncio.create_task(br.call(op, args, timeout=timeout))       # type: ignore[arg-type]
    while not br.pending:
        await asyncio.sleep(0)
    cid = next(iter(br.pending))
    br.pending[cid].set_result(Result(id=cid, gen=br.gen, **result))
    return await call


# decision: HAND-435, HAND-436
def test_a_read_that_hung_says_the_page_isnt_responding_where_and_what_helps() -> None:
    """Live 2026-11-08: the worker saw only "browser error: timeout" and read the stuck tab again, 2-3 times a task."""
    async def check() -> None:
        br = _bridge()
        with pytest.raises(DriverError) as hung:
            await _answer(br, "snapshot", {"tab_id": 3, "snap": 1},
                          {"status": "error", "error": "timeout", "outcome": "failed", "step": "inject",
                           "steps": {"wake_ms": 2, "attach_ms": 4, "inject_ms": 29990}})
        e = hung.value
        assert e.facts == {"reason": "page_not_responding", "step": "inject"}
        assert "isn't responding" in str(e) and "new tab" in str(e) and "reader into the page" in str(e)
        assert getattr(e, "timing") == {"read": {"wake_ms": 2, "attach_ms": 4, "inject_ms": 29990}}
        with pytest.raises(DriverError) as older:                            # an older extension names no step
            await _answer(br, "snapshot", {"tab_id": 3, "snap": 2},
                          {"status": "error", "error": "timeout", "outcome": "failed"})
        assert older.value.facts == {"reason": "page_not_responding"}
        with pytest.raises(DriverError) as tabs:                             # no page in it: no page claim
            await _answer(br, "list_tabs", {}, {"status": "error", "error": "timeout", "outcome": "failed"})
        assert tabs.value.facts == {} and str(tabs.value) == "timeout"
    asyncio.run(check())


# decision: HAND-435
def test_a_screenshot_after_a_read_that_hung_does_not_look_at_the_page_again() -> None:
    """Live: every screenshot timeout (6 of 6) came right after a snapshot timeout on the same tab: 30 s + 30 s."""
    from ferb.act.browser.ext_bridge import Router
    drv = object.__new__(Router)
    asked: list[bool] = []

    async def snapshot(tab: Any) -> Any:
        raise not_responding("snapshot", 30, "inject")

    async def screenshot(tab: Any, looks: bool = True) -> bytes:
        asked.append(looks)
        raise DriverError("stop here")
    drv.snapshot, drv.screenshot = snapshot, screenshot                  # type: ignore[method-assign]
    with pytest.raises(DriverError):
        asyncio.run(drv.visual_snapshot(SimpleNamespace(tab_id="t1")))  # type: ignore[arg-type]
    assert asked == [False]


# decision: DMN-406
def test_a_preview_frame_gives_the_extension_only_the_time_the_preview_has_left() -> None:
    """Live: the daemon cut a preview capture at 8 s, but the extension kept the tab's looks for its own 30 s."""
    from ferb.act.browser.ext_bridge import Router, _Tab
    drv = object.__new__(Router)
    sent: list[tuple[dict[str, Any], float]] = []

    class Br:
        gen, conn, navs = 1, object(), {}

        async def call(self, op: str, args: dict[str, Any], timeout: float = 30) -> dict[str, Any]:
            sent.append((args, timeout))
            return {"withheld": "stop here"}
    tab = SimpleNamespace(tab_id="t1", generation=1, browser="chrome", profile="Default")
    t = _Tab(tab, Br(), 7, "task", "https://a.example/")                 # type: ignore[arg-type]
    drv._tabs = {"t1": t}
    drv.owned_by = lambda alias, owner: tab                              # type: ignore[method-assign]
    _, capture = drv.recording_source(tab, "task")                       # type: ignore[arg-type]

    async def within(deadline: float | None) -> None:
        PREVIEW_DEADLINE.set(deadline)
        with pytest.raises(Exception):
            await capture()
    asyncio.run(within(time.monotonic() + 3.0))
    t.stuck = True                                                       # its last read hung: no page looks
    asyncio.run(within(None))
    (first, budget), (second, recording) = sent
    assert 2.5 < budget <= 3.0 and first == {"tab_id": 7}
    assert recording == ext_bridge.CMD_TIMEOUT_S and second == {"tab_id": 7, "looks": False}


# decision: DMN-406, DMN-403
def test_the_preview_looks_alongside_a_read_but_never_alongside_an_action_or_ahead_of_a_waiting_call() -> None:
    async def check() -> None:
        locks = ResourceLocks()
        async with locks.hold(["tab:b/1"], "worker", reading=True):
            async with locks.try_hold(["tab:b/1"], "preview", during_reads=True) as seen:
                assert seen is True
            async with locks.try_hold(["tab:b/1"], "preview") as plain:        # without during_reads: busy
                assert plain is False
            waiting = asyncio.create_task(locks.hold(["tab:b/1"], "next").__aenter__())
            await asyncio.sleep(0)
            async with locks.try_hold(["tab:b/1"], "preview", during_reads=True) as queued:
                assert queued is False                                         # the worker's next call waits
            waiting.cancel()
        async with locks.hold(["tab:b/1"], "worker"):                          # an action
            async with locks.try_hold(["tab:b/1"], "preview", during_reads=True) as acting:
                assert acting is False
        async with locks.try_hold(["tab:b/1"], "preview", during_reads=True) as free:
            assert free is True and locks.holder("tab:b/1") == "preview"
    asyncio.run(check())


async def _plain(image: bytes) -> bytes:
    return image


def _target(scope: str, capture: Any, label: str = "Notes") -> PreviewTarget:
    async def resolve() -> tuple[dict[str, Any], Any]:
        return {"scope": scope}, capture
    return PreviewTarget(scope, label, resolve)


# decision: DMN-407, DMN-408
def test_an_unchanged_frame_isnt_sent_again_and_too_long_is_said_once() -> None:
    """Live 2026-11-08: 5 times a second the app got and decoded the same frame, and "The capture took too long" stayed
    on every answer until the next fresh frame."""
    async def check() -> None:
        noted: list[tuple[str, dict[str, Any]]] = []
        locks = ResourceLocks()
        previews = TaskPreviews(_plain, locks.try_hold, limit_s=0.2, note=lambda k, d: noted.append((k, d)))
        slow = asyncio.Event()

        async def capture() -> bytes:
            if slow.is_set():
                await asyncio.sleep(5)
            return b"frame"
        previews.declare("t", "t.worker", _target("tab:browser/1", capture), ("tab:browser/1",))
        first = await previews.frame("t")
        assert first["image"] == b"frame" and first["fresh"] and isinstance(first["seq"], int) and first["age_ms"] >= 0
        async with locks.hold(["tab:browser/1"], "worker"):
            again = await previews.frame("t", have=first["seq"])
        assert "image" not in again and again["seq"] == first["seq"] and again["busy"]
        slow.set()
        timed_out = await previews.frame("t", have=first["seq"])
        assert timed_out["problem"]["kind"] == "slow"
        async with locks.hold(["tab:browser/1"], "worker"):
            after = await previews.frame("t", have=first["seq"])
        assert "problem" not in after                                     # said on the answer that ran out only
        assert noted and noted[0][0] == "preview.slow" and noted[0][1]["surface"] == "tab"
        counts = previews.snapshot()
        assert counts["tab_frames"] == 1 and counts["tab_slow"] == 1 and counts["tab_busy"] == 2
        assert counts["max_tab_gap_ms"] >= 0 and "max_tab_gap_ms" not in previews.snapshot()   # a peak per window
    asyncio.run(check())


# decision: DMN-407, DMN-408, HAND-437
def test_a_call_naming_only_the_app_keeps_this_steps_window_of_it_and_a_locked_screen_is_said() -> None:
    """Live 2026-11-08: a PowerPoint slide show driven by key presses showed "Working in PowerPoint" and no picture."""
    async def check() -> None:
        previews = TaskPreviews(_plain, ResourceLocks().try_hold)
        state = {"raise": None}

        async def capture() -> bytes:
            if state["raise"] is not None:
                raise state["raise"]
            return b"slide"
        previews.declare("t", "t.worker", _target("app:PowerPoint", capture, "PowerPoint"))
        assert (await previews.frame("t"))["image"] == b"slide"
        previews.declare("t", "t.worker", PreviewTarget("app:PowerPoint", "PowerPoint"))   # a key press: app only
        assert (await previews.frame("t"))["fresh"] is True
        state["raise"] = ScreenOff("locked")
        locked = await previews.frame("t")
        assert locked["problem"]["kind"] == "screen_locked" and locked["image"] == b"slide"
        state["raise"] = CaptureBusy("the worker's screenshot")
        busy = await previews.frame("t")
        assert busy["busy"] is True and busy["image"] == b"slide"
        counts = previews.snapshot()
        assert counts["window_frames"] == 2 and counts["window_locked"] == 1 and counts["window_busy"] == 1
    asyncio.run(check())


# decision: HAND-437, DMN-406
def test_a_locked_screen_is_said_and_a_preview_capture_never_waits_for_another(monkeypatch: Any) -> None:
    w = shotm.Window(1, 2, (0, 0, 10, 10), "")
    import ferb.act.mac.accessibility as axm
    monkeypatch.setattr(axm, "window_minimized", lambda pid, bounds: False)
    monkeypatch.setattr(shotm, "_capture_once", lambda w, edge: (b"jpeg", 1, 1))
    monkeypatch.setattr(shotm, "screen_off", lambda: "locked")
    with pytest.raises(ScreenOff) as off:
        shotm.capture(w)
    assert "locked" in str(off.value) and isinstance(off.value, RuntimeError)   # a worker's call: Refused with it
    monkeypatch.setattr(shotm, "screen_off", lambda: None)
    assert shotm.capture(w) == (b"jpeg", 1, 1)
    with shotm._capturing:                                                # the worker's own screenshot is capturing
        token = PREVIEW_DEADLINE.set(time.monotonic() + 5)
        try:
            assert preview_left() is not None
            with pytest.raises(CaptureBusy):
                shotm.capture(w)
        finally:
            PREVIEW_DEADLINE.reset(token)


# decision: DMN-406
def test_a_preview_walk_is_bounded_by_its_time_and_never_replaces_the_workers_tree(monkeypatch: Any) -> None:
    """The preview's walk replaced the app's cached tree, which the worker's later secret-field reads use."""
    from tests.mackit import vision
    d, ax, shots, *_ = vision(monkeypatch)
    budgets: list[Any] = []
    read = ax.read_tree

    def tracked(*args: Any, budget_s: Any = None, **kw: Any) -> Any:
        budgets.append(budget_s)
        return read(*args, **kw)
    ax.read_tree = tracked

    async def check() -> None:
        await d.snapshot('Notes')
        _, shot, _ = await d.screenshot('Notes')
        state = d._state['com.apple.Notes']
        tree, budgets[:] = state.tree, []
        _, capture = d.recording_source('Notes', shot.id)
        PREVIEW_DEADLINE.set(time.monotonic() + 8)
        assert await capture() == shots.jpeg
        assert state.tree is tree                                       # the worker's tree is still its own
        assert len(budgets) == 2 and all(b is not None and 0 < b <= 8 for b in budgets)
    asyncio.run(check())
