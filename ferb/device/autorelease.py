"""Drain Objective-C autorelease pools on FERB's own threads. Must not change what the wrapped work does.

A thread that calls into Cocoa through PyObjC without an autorelease pool keeps every object autoreleased in it until
the thread ends; the daemon's worker threads and native run-loop threads live as long as the daemon, so those objects
only pile up (measured 2026-10-31: 270-450 bytes per call for NSRunningApplication and Accessibility window reads,
none with a pool). Every place FERB runs work on a long-lived thread drains one pool per unit of work: each job of the
event loop's default executor (`asyncio.to_thread`), each watcher poll and run-loop slice, each native callback."""

import concurrent.futures
import contextlib
import functools
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


class DrainingExecutor(concurrent.futures.ThreadPoolExecutor):
    """The daemon loop's default executor: a thread pool whose every job runs inside its own autorelease pool."""

    def submit(self, fn: Callable[..., T], /, *args: Any, **kwargs: Any) -> "concurrent.futures.Future[T]":
        return super().submit(drained(fn), *args, **kwargs)
