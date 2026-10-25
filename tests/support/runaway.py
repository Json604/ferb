"""Runaway guards for the test suite (2026-10-25: one test looped for 57 minutes and grew Python to about 110 GB).

A pytest plugin with no dependencies, loaded by tests/conftest.py:

- Per test: before each test's setup, faulthandler arms a timer (FERB_TEST_TIMEOUT_S, 300 s by default) that dumps every
  thread's traceback and ends the process; teardown cancels it. faulthandler's own C thread fires even while a loop
  holds the GIL. `@pytest.mark.ferb_timeout(seconds)` gives one long test its own limit; 0 turns the timer off.
- Per session: a daemon thread reads this process's memory every second. Above FERB_TEST_MAX_RSS_MB (4096 by default)
  it names the running test, dumps every thread's traceback and calls os._exit(3). Memory is the larger of resident
  size and the physical footprint, because macOS compresses and swaps a growing process, so its resident size stays
  low while its footprint keeps growing (the 110 GB was footprint on a 24 GB Mac).

Both write to a copy of the real stderr taken at configure, so pytest's output capture never hides them."""

from __future__ import annotations

import ctypes
import faulthandler
import os
import subprocess
import sys
import threading
import time
from collections.abc import Generator
from typing import Any

import pytest

TIMEOUT_ENV = "FERB_TEST_TIMEOUT_S"
MAX_RSS_ENV = "FERB_TEST_MAX_RSS_MB"
DEFAULT_TIMEOUT_S = 300.0
DEFAULT_MAX_RSS_MB = 4096.0
MEMORY_EXIT_CODE = 3

_stderr_fd: int | None = None
_watcher: threading.Thread | None = None


def _limit(name: str, default: float) -> float:
    raw = os.environ.get(name, "").strip()
    return float(raw) if raw else default


class _RusageInfoV0(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16), ("user_time", ctypes.c_uint64), ("system_time", ctypes.c_uint64),
                ("pkg_idle_wkups", ctypes.c_uint64), ("interrupt_wkups", ctypes.c_uint64),
                ("pageins", ctypes.c_uint64), ("wired_size", ctypes.c_uint64), ("resident_size", ctypes.c_uint64),
                ("phys_footprint", ctypes.c_uint64), ("proc_start_abstime", ctypes.c_uint64),
                ("proc_exit_abstime", ctypes.c_uint64)]


def _darwin_memory(pid: int) -> int | None:
    """max(resident size, physical footprint) in bytes from proc_pid_rusage (libSystem, no subprocess)."""
    try:
        libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
        info = _RusageInfoV0()
        if libc.proc_pid_rusage(pid, 0, ctypes.byref(info)) != 0:          # 0: RUSAGE_INFO_V0
            return None
        return max(int(info.resident_size), int(info.phys_footprint))
    except (OSError, AttributeError):
        return None


def memory_bytes(pid: int | None = None) -> int:
    """This process's memory (or pid's): footprint-aware on macOS, `ps` resident size elsewhere."""
    pid = os.getpid() if pid is None else pid
    if sys.platform == "darwin":
        found = _darwin_memory(pid)
        if found is not None:
            return found
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True, check=False).stdout
    return int(out.strip() or 0) * 1024


def _write(text: str) -> None:
    fd = _stderr_fd if _stderr_fd is not None else 2
    try:
        os.write(fd, text.encode(errors="replace"))
    except OSError:
        pass


def _watch_memory(limit_bytes: int, interval_s: float = 1.0) -> None:
    while True:
        try:
            used = memory_bytes()
        except Exception:                       # never let the guard itself end the run
            used = 0
        if used > limit_bytes:
            test = os.environ.get("PYTEST_CURRENT_TEST", "(no test running)")
            _write(f"\n\nFERB test memory guard: {used >> 20} MB is over the {limit_bytes >> 20} MB limit "
                   f"({MAX_RSS_ENV}) during {test}\nEvery thread's traceback, most recent call first:\n")
            faulthandler.dump_traceback(file=_stderr_fd if _stderr_fd is not None else sys.__stderr__,
                                        all_threads=True)
            os._exit(MEMORY_EXIT_CODE)
        time.sleep(interval_s)


def pytest_configure(config: pytest.Config) -> None:
    global _stderr_fd, _watcher
    config.addinivalue_line("markers", "ferb_timeout(seconds): this test's own runaway limit instead of "
                                       f"{TIMEOUT_ENV} (0: no limit); only for a test that really takes that long")
    if _stderr_fd is None:
        _stderr_fd = os.dup(sys.__stderr__.fileno() if sys.__stderr__ is not None else 2)
    limit_mb = _limit(MAX_RSS_ENV, DEFAULT_MAX_RSS_MB)
    if limit_mb > 0 and _watcher is None:
        _watcher = threading.Thread(target=_watch_memory, args=(int(limit_mb * 1024 * 1024),),
                                    name="ferb-test-memory-guard", daemon=True)
        _watcher.start()


def _timeout_for(item: pytest.Item) -> float:
    marker = item.get_closest_marker("ferb_timeout")
    if marker is not None:
        return float(marker.args[0])
    return _limit(TIMEOUT_ENV, DEFAULT_TIMEOUT_S)


@pytest.hookimpl(wrapper=True, tryfirst=True)
def pytest_runtest_protocol(item: pytest.Item, nextitem: Any) -> Generator[None, object, object]:
    """Covers the test's setup, call and teardown (a hung fixture or daemon shutdown counts too)."""
    timeout = _timeout_for(item)
    if timeout <= 0 or item.config.getoption("usepdb", False):
        return (yield)
    file: Any = _stderr_fd if _stderr_fd is not None else sys.__stderr__
    faulthandler.dump_traceback_later(timeout, exit=True, file=file)
    try:
        return (yield)
    finally:
        faulthandler.cancel_dump_traceback_later()


@pytest.hookimpl(tryfirst=True)
def pytest_enter_pdb() -> None:
    faulthandler.cancel_dump_traceback_later()
