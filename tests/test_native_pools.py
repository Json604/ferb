"""FERB's native threads drain an autorelease pool every turn and every callback (DMN-585, refining DMN-26).

The daemon grew ~100-180 MB an awake hour (2.8-3.5 GB before each restart): the Accessibility watcher's thread listed
the running apps once a second outside any pool, so every NSRunningApplication, its lock and its strings stayed.
Footprints are read in a fresh interpreter, while the native thread is still alive (a thread's leftover pool only goes
when the thread ends)."""
import ast
import gc
import json
import os
import subprocess
import sys
import threading
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
mac_only = pytest.mark.skipif(sys.platform != "darwin", reason="PyObjC and the Objective-C runtime are macOS-only")

_FREED: list[int] = []
_PROBE: list[type] = []


def _autorelease_one() -> None:
    """Autorelease one object whose dealloc FERB can see (an NSArray factory autoreleases what it returns)."""
    import objc
    from Foundation import NSArray, NSObject
    if not _PROBE:
        class FerbNativeLoopProbe(NSObject):                    # type: ignore[misc]
            def dealloc(self) -> None:
                _FREED.append(1)
                objc.super(FerbNativeLoopProbe, self).dealloc()
        _PROBE.append(FerbNativeLoopProbe)
    probe = _PROBE[0].new()
    NSArray.arrayWithObject_(probe)
    del probe


# decision: DMN-585
@mac_only
def test_a_native_loop_turn_drains_what_its_step_autoreleased_while_the_thread_lives() -> None:
    from ferb.device.autorelease import native_loop
    done, second, checked = threading.Event(), threading.Event(), threading.Event()
    turns = [0]

    def step() -> None:
        turns[0] += 1
        if turns[0] == 1:
            _autorelease_one()
        elif turns[0] == 2:
            second.set()
            checked.wait(5)                                       # the thread stays alive while the test looks
            done.set()

    _FREED.clear()
    thread = threading.Thread(target=native_loop, args=(done, 0.0, step), daemon=True)
    thread.start()
    try:
        assert second.wait(5)
        gc.collect()
        assert _FREED == [1]
    finally:
        checked.set()
        thread.join(5)


FOOTPRINT = r"""
import asyncio, ctypes, json, os, sys, threading
import Quartz

class Usage(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [(n, ctypes.c_uint64) for n in (
        "user", "system", "idle", "interrupts", "pageins", "wired", "resident", "footprint", "start", "exit")]
libproc = ctypes.CDLL("/usr/lib/libproc.dylib")

def footprint_mb():
    usage = Usage()
    libproc.proc_pid_rusage(os.getpid(), 0, ctypes.byref(usage))
    return usage.footprint / 1e6

def watcher():
    # The Accessibility watcher's own loop: 1,000 turns of listing the running apps (none attached: all excluded),
    # with the event loop's thread pumping macOS events as the daemon's does, so each listing has fresh app objects.
    from ferb.device import autorelease
    from ferb.sense.watchers import accessibility
    warm, turns, marks, finished = 100, 1000, {}, threading.Event()
    real_loop = accessibility.native_loop

    def counted_loop(done, interval_s, step=None):
        turn = [0]
        def counted():
            turn[0] += 1
            if turn[0] in (warm, warm + turns):
                marks[turn[0]] = footprint_mb()
            if turn[0] >= warm + turns:
                finished.set()
            step()
        real_loop(done, 0.0, counted)
    accessibility.native_loop = counted_loop

    async def watch():
        stop = asyncio.Event()
        async def pump():
            while not stop.is_set():
                with autorelease.pool():
                    Quartz.CFRunLoopRunInMode(Quartz.kCFRunLoopDefaultMode, 0, True)
                await asyncio.sleep(0.001)
        pumping = asyncio.create_task(pump())
        task = asyncio.create_task(accessibility.AccessibilityWatcher(lambda data: data, lambda bundle, name: True)
                                   .run(lambda event: None, stop))
        await asyncio.to_thread(finished.wait, 120)
        stop.set()
        await task
        await pumping
    asyncio.run(watch())
    return marks[warm + turns] - marks[warm]

def tap():
    # The owner-input tap's callback: 50,000 synthetic key presses on one long-lived thread with no pool of its own,
    # each handed to a capture that reads autoreleased native objects.
    from AppKit import NSWorkspace
    from ferb.sense.watchers.owner_input import OwnerInput
    owner_input = OwnerInput(allowed=lambda: True)
    owner_input.on_text = lambda event, marked: len(list(NSWorkspace.sharedWorkspace().runningApplications()))
    callback, grown = owner_input._callback([]), []
    def press(n):
        for _ in range(n):
            callback(None, Quartz.kCGEventKeyDown, Quartz.CGEventCreateKeyboardEvent(None, 0, True), None)
    def presses():
        press(2000)
        before = footprint_mb()
        press(50000)
        grown.append(footprint_mb() - before)
    thread = threading.Thread(target=presses)
    thread.start()
    thread.join()
    return grown[0]

print(json.dumps({"grown_mb": {"watcher": watcher, "tap": tap}[sys.argv[1]]()}))
"""


# decision: DMN-585
@mac_only
@pytest.mark.ferb_timeout(240)
@pytest.mark.parametrize("part", ["watcher", "tap"])
def test_the_watchers_loop_and_the_tap_callback_keep_the_footprint_flat(tmp_path: Path, part: str) -> None:
    """Measured 2026-11-12 with the pools taken out: the watcher's turns kept ~33 MB a thousand and the key presses
    ~16 MB; with them, both stay under 0.1 MB."""
    env = {**os.environ, "FERB_HOME": str(tmp_path / "home"), "HOME": str(tmp_path)}
    run = subprocess.run([sys.executable, "-c", FOOTPRINT, part], capture_output=True, text=True, timeout=200,
                         env=env, cwd=ROOT)
    assert run.returncode == 0, run.stderr[-2000:]
    grown = json.loads(run.stdout.strip().splitlines()[-1])["grown_mb"]
    assert grown < 5, grown


NATIVE_LOOPS = {"CFRunLoopRun", "CFRunLoopRunInMode"}
HELPER = Path("ferb/device/autorelease.py")


def _loops_outside_the_helper(tree: ast.AST, helper: bool) -> list[int]:
    """Lines that service a run loop other than inside `native_loop` itself: allowed only as one zero-wait turn inside
    a `with pool():` block (the event loop's thread giving macOS a turn), never a wait or a CFRunLoopRun."""
    found: list[int] = []

    def visit(node: ast.AST, pooled: bool) -> None:
        if helper and isinstance(node, ast.FunctionDef) and node.name == "native_loop":
            return
        if isinstance(node, ast.With) and any(isinstance(item.context_expr, ast.Call)
                                              and getattr(item.context_expr.func, "id", "") == "pool"
                                              for item in node.items):
            pooled = True
        if isinstance(node, ast.Call) and getattr(node.func, "attr", getattr(node.func, "id", "")) in NATIVE_LOOPS:
            zero_wait = (len(node.args) >= 2 and isinstance(node.args[1], ast.Constant) and node.args[1].value == 0)
            if not (pooled and zero_wait):
                found.append(node.lineno)
        for child in ast.iter_child_nodes(node):
            visit(child, pooled)
    visit(tree, False)
    return found


# decision: DMN-585
def test_every_native_threads_loop_goes_through_native_loop() -> None:
    """A thread that waits in its own run loop does it through ferb/device/autorelease.py's native_loop, the step and
    the slice in one pool, so a new watcher (or a helper pooling only the slice, as the leaking one did) can't bring
    the leak back. The event loop's thread may take a single zero-wait turn inside `with pool():`."""
    unpooled = {}
    for path in sorted((ROOT / "ferb").rglob("*.py")):
        relative = path.relative_to(ROOT)
        lines = _loops_outside_the_helper(ast.parse(path.read_text(), str(path)), relative == HELPER)
        if lines:
            unpooled[str(relative)] = lines
    assert unpooled == {}
