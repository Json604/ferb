"""Drain Objective-C autorelease pools on FERB's own threads. Must not change what the wrapped work does.

A thread that calls into Cocoa through PyObjC without an autorelease pool keeps every object autoreleased in it until
the thread ends; the daemon's worker threads and native run-loop threads live as long as the daemon, so those objects
only pile up (measured 2026-10-31: 270-450 bytes per call for NSRunningApplication and Accessibility window reads,
none with a pool). Every place FERB runs work on a long-lived thread drains one pool per unit of work: each job of the
event loop's default executor (`asyncio.to_thread`), each watcher poll, each turn of a native thread's loop (its own
step and the run-loop slice that dispatches its callbacks, together: `native_loop`), each native callback.

Measured 2026-11-12: the Accessibility watcher's thread listed the running apps once a second outside any pool (only
the run-loop slice had one), keeping ~110 NSRunningApplication objects, a lock each and their strings a second, until
the daemon grew from ~200 MB to over 3 GB in a day."""

import concurrent.futures
import contextlib
import functools
import threading
import time
from typing import Any, Callable, ContextManager, TypeVar

T = TypeVar("T")


def pool() -> ContextManager[Any]:
    """An autorelease pool for one unit of work (nothing to drain where PyObjC isn't available)."""
    try:
        import objc
    except ImportError:                      # pragma: no cover - FERB always runs with PyObjC on macOS
        return contextlib.nullcontext()
    make = getattr(objc, "autorelease_pool", None)       # absent from the stand-in modules some tests inject
    return make() if make is not None else contextlib.nullcontext()


def drained(fn: Callable[..., T]) -> Callable[..., T]:
    """`fn` with one autorelease pool around each call."""
    @functools.wraps(fn)
    def run(*args: Any, **kwargs: Any) -> T:
        with pool():
            return fn(*args, **kwargs)
    return run


def native_loop(done: threading.Event, interval_s: float, step: Callable[[], Any] | None = None) -> None:
    """The loop of a FERB thread that services its own run loop, until `done` is set: each turn runs `step`, then
    pumps the run loop for `interval_s` (where its sources' callbacks run), both inside one autorelease pool, and
    waits out what is left of the turn when the run loop had nothing to wait on (it returns at once without sources).
    The one way FERB runs a native thread's loop: what the step and the callbacks autorelease goes with the turn."""
    import Quartz
    while not done.is_set():
        started = time.monotonic()
        with pool():
            if step is not None:
                step()
            if done.is_set():
                break
            Quartz.CFRunLoopRunInMode(Quartz.kCFRunLoopDefaultMode, interval_s, False)
        remaining = interval_s - (time.monotonic() - started)
        if remaining > 0:
            done.wait(remaining)


class DrainingExecutor(concurrent.futures.ThreadPoolExecutor):
    """The daemon loop's default executor: a thread pool whose every job runs inside its own autorelease pool."""

    def submit(self, fn: Callable[..., T], /, *args: Any, **kwargs: Any) -> "concurrent.futures.Future[T]":
        return super().submit(drained(fn), *args, **kwargs)
