"""Spike 14: owner-input detection and GUI yield. Only harmless synthetic events (mouse-moved to the pointer's
current position). Needs Accessibility (or Input Monitoring) for the event tap."""

import random
import statistics
import threading
import time

import pytest

import owner_input as O


@pytest.fixture(scope="module")
def watcher():
    try:
        w = O.OwnerInputWatcher(quiet_ms=1500)
    except PermissionError as e:
        pytest.skip(str(e))
    time.sleep(0.3)
    yield w
    w.stop()


def wait_quiet(w, timeout=10):
    """Real owner activity during the run would disturb the timing tests: wait until the owner is quiet."""
    end = time.monotonic() + timeout
    while w.owner_active():
        if time.monotonic() > end:
            pytest.skip("owner is actively using the Mac; rerun when idle")
        time.sleep(0.1)


def test_ferb_events_are_never_mistaken_for_the_owner(watcher):
    wait_quiet(watcher)
    before_owner, before_ferb = len(watcher.owner_events), watcher.ferb_events
    for _ in range(50):
        O.post_still_mouse_move(ferb=True)
        time.sleep(0.005)
    time.sleep(0.2)
    assert watcher.ferb_events - before_ferb == 50
    assert len(watcher.owner_events) == before_owner
    assert not watcher.owner_active()


def test_hid_idle_timer_cannot_tell_ferb_from_owner(watcher):
    """Why the tap + marker is needed: FERB's own synthetic input resets the system idle timer."""
    wait_quiet(watcher)
    time.sleep(1.0)
    assert O.hid_idle_seconds() > 0.9
    O.post_still_mouse_move(ferb=True)
    time.sleep(0.05)
    assert O.hid_idle_seconds() < 0.5


def test_owner_detection_latency(watcher):
    wait_quiet(watcher)
    lat = []
    for _ in range(40):
        k = len(watcher.owner_events)
        t = O.post_still_mouse_move(ferb=False)
        end = time.monotonic() + 1
        while len(watcher.owner_events) == k and time.monotonic() < end:
            time.sleep(0.0005)
        assert len(watcher.owner_events) > k, "owner event not detected within 1 s"
        lat.append((watcher.owner_events[-1][2] - t) / 1e6)
        time.sleep(0.03)
    print(f"\ndetection ms: median {statistics.median(lat):.2f} p90 {sorted(lat)[35]:.2f} max {max(lat):.2f}")
    assert statistics.median(lat) < 10 and max(lat) < 50


@pytest.mark.parametrize("step_ms", [20, 100])
def test_gui_action_yields_within_one_step(watcher, step_ms):
    yields = []
    for _ in range(5):
        wait_quiet(watcher, timeout=5)
        log: list = []
        t_owner = {}

        def owner():
            time.sleep(random.uniform(0.15, 0.4))
            t_owner["t"] = O.post_still_mouse_move(ferb=False)

        th = threading.Thread(target=owner)
        th.start()
        outcome = O.gui_action(watcher, steps=1000, step_ms=step_ms, log=log)
        th.join()
        assert outcome == "yielded"
        stop_ns = log[-1][2]
        steps_after = [e for e in log if e[0] == "step" and e[2] > t_owner["t"]]
        yields.append((stop_ns - t_owner["t"]) / 1e6)
        assert len(steps_after) <= 1            # at most the step already in flight
        time.sleep(1.6)                         # let the quiet period pass before the next round
    print(f"\nstep {step_ms} ms -> yield ms: {[round(y, 1) for y in yields]}")
    assert max(yields) <= step_ms + 15


def test_resumes_only_after_quiet_period(watcher):
    wait_quiet(watcher)
    O.post_still_mouse_move(ferb=False)
    time.sleep(0.05)
    assert watcher.owner_active()
    time.sleep(1.0)
    assert watcher.owner_active()               # still within 1.5 s quiet period
    time.sleep(0.6)
    assert not watcher.owner_active()
