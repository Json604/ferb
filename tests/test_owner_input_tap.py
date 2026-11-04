"""Live 2026-11-01/02: Input Monitoring was missing when the daemon started (4 starts journaled owner_input.unavailable),
and the owner-input tap was started only at a start, so after the grant FERB still couldn't tell the owner was using the
Mac until a restart. Meanwhile "the owner is active" read False: foreground input never waited for the owner, and a tab
sound change the owner caused could be recorded as near FERB's call. The tap now starts as soon as the grant is there,
each change is journaled with why, and "can't tell" counts as the owner using the Mac (SENSE-406)."""

import asyncio
import threading
from pathlib import Path
from types import SimpleNamespace
from typing import Any

from ferb.sense.manager import INPUT_MONITORING_OFF, Sense
from tests.conftest import RunningDaemon


class _Tap:
    """The owner-input tap's surface Sense uses, with the grant switchable and no Quartz."""

    def __init__(self) -> None:
        self.granted, self.running, self.started = False, False, threading.Event()

    def allowed(self) -> bool:
        return self.granted

    def start(self) -> bool:
        self.running = self.granted
        if self.running:
            self.started.set()
        return self.running

    def stop(self) -> None:
        self.running = False


# decision: SENSE-406
def test_the_input_tap_starts_once_input_monitoring_is_granted_without_a_restart(tmp_path: Path) -> None:
    tap, changes = _Tap(), []
    trail = SimpleNamespace(record=lambda o: None, publish=lambda k, d: None)
    s = Sense(tmp_path, trail, [], owner_input=tap, on_tap=changes.append)  # type: ignore[arg-type]
    s.tap_recheck_s = 0                                       # each pass of the loop checks again at once

    async def go() -> None:
        await s.start()
        assert not tap.running and s.tap_down == INPUT_MONITORING_OFF
        tap.granted = True                                    # the owner grants it while FERB runs
        assert await asyncio.to_thread(tap.started.wait, 5)  # event-driven: returns as soon as the tap starts
        for _ in range(100):                                  # the loop reports what it just did
            if s.tap_down == "":
                break
            await asyncio.sleep(0)
        await s.stop()
    asyncio.run(go())
    assert changes == [INPUT_MONITORING_OFF, ""]             # down and why, then back up: each reported once


# decision: SENSE-406
def test_without_the_tap_ferb_counts_the_owner_as_using_the_mac(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    real_input = d.mac.input                                  # type: ignore[union-attr]
    assert not d.owner_input.running                         # the test daemon runs no tap: can't tell
    assert real_input.owner_active() is True and real_input.available() is False
    d.owner_input.running = True                             # the tap runs and the owner is quiet
    try:
        assert real_input.owner_active() is False and real_input.available() is True
    finally:
        d.owner_input.running = False


# decision: SENSE-406
# decision: HAND-213
def test_a_tab_sound_change_recorded_without_the_tap_says_the_owners_input_was_unknown(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    sentinel = d.disturbance
    records: list[dict[str, Any]] = []
    sentinel.record = lambda task, kind, data: records.append(data)
    monitored = [False]
    sentinel.owner_input = lambda: {"monitored": monitored[0], "recent": False, "last_event_wall": None}

    async def acting() -> None:
        from ferb.daemon.disturbance import _Observation
        sentinel.active["x"] = _Observation()
        sentinel.call_started("x.worker", "span-1", "browser.use_tab")
        sentinel.sound_changed({"browser": "chrome", "tab_id": 41}, {"audible": {"before": True, "after": False}})
        monitored[0] = True
        sentinel.sound_changed({"browser": "chrome", "tab_id": 41}, {"audible": {"before": False, "after": True}})
    daemon.run(acting())
    assert [r.get("owner_input_monitored") for r in records] == [False, None]
