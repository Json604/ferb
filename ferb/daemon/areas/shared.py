"""Definitions more than one daemon area uses (constants, small helpers). Must not hold daemon state."""

import asyncio
import contextlib
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Callable

from ferb.core.interfaces import Clock
from ferb.sanitize.redact import REDACTED


Redact = Callable[[Any], Any]


MACOS_TICK_S = 0.2


TASK_PAGE, TASK_PAGE_MAX = 50, 200                 # the FERB window's Tasks page (L34 Phase E): rows per page, most per call


def _raise_file_limit() -> None:
    """Set the open-file soft limit to the hard limit clamped to kern.maxfilesperproc, so the egress proxy's
    per-connection descriptors (and everything else FERB holds open) don't hit EMFILE, and children still see a finite
    OPEN_MAX. Best effort; a failure never stops the daemon."""
    try:
        import resource
        soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
        target = hard if hard != resource.RLIM_INFINITY else soft
        per_proc = _sysctl_int(b"kern.maxfilesperproc")
        if per_proc:                                          # finite, so children see a finite OPEN_MAX
            target = min(target, per_proc) if hard != resource.RLIM_INFINITY else per_proc
        if target != soft:
            resource.setrlimit(resource.RLIMIT_NOFILE, (target, hard))
    except (OSError, ValueError, ImportError):
        pass


def _sysctl_int(name: bytes) -> int | None:
    import ctypes
    try:
        value, size = ctypes.c_int(0), ctypes.c_size_t(ctypes.sizeof(ctypes.c_int))
        libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
        if libc.sysctlbyname(name, ctypes.byref(value), ctypes.byref(size), None, 0) != 0:
            return None
        return int(value.value) or None
    except OSError:
        return None


# An Edit answer that secret redaction hid whole (rule 14): never saved as the owner's words
EDIT_HIDDEN = ("Those words looked like a secret, so FERB hid them and saved nothing. Type how FERB should remember it, "
               "without the secret.")


def _all_hidden(text: str) -> bool:
    """Words of which redaction left nothing but its marks ("[redacted]", "[redacted] [redacted]")."""
    return bool(text.strip()) and not text.replace(REDACTED, "").strip()


# A request that failed inside FERB (`_record_internal`): its message's first characters and its innermost frames are
# kept, enough to find the line; a deep recursion's thousand frames are not
INTERNAL_MESSAGE_CHARS, INTERNAL_FRAMES = 300, 20
WITHHELD = "(withheld: it came while a hidden answer was handled)"


# A problem that keeps happening (`_Repeats`): its first row at once, then one row per this many seconds with the count
REPEAT_WINDOW_S = 600.0


UNHANDLED = "daemon.unhandled"       # an exception nobody retrieved (the event loop's handler, `_loop_exception`)


@dataclass
class _Window:
    """One repeating problem's open window (`_Repeats`)."""
    kind: str
    until: datetime
    repeats: int = 0
    latest: dict[str, Any] = field(default_factory=dict)


class _Repeats:
    """The journal rows of a problem that keeps happening, bounded (L36 review Q-I2: a failing `chat.status`, asked
    every 5 s, wrote a row each time). The first occurrence of a key is written at once. Repeats in the next
    `REPEAT_WINDOW_S` are counted and written as one row, the latest occurrence's with `repeats: N`, when the window
    ends (or at shutdown: `flush`). A logging detail: no reply changes and no occurrence goes uncounted."""

    def __init__(self, write: Callable[[str, dict[str, Any]], Any], clock: Clock) -> None:
        self._write, self._clock = write, clock
        self._held: dict[tuple[str, ...], _Window] = {}
        self._timers: set[asyncio.Task[None]] = set()

    def record(self, kind: str, key: tuple[str, ...], data: dict[str, Any]) -> None:
        held = self._held.get(key)
        if held is not None and self._clock.now() < held.until:
            held.repeats, held.latest = held.repeats + 1, data
            if held.repeats == 1:                    # the window's count is written when it ends
                timer = asyncio.get_running_loop().create_task(self._at_end(key, held))
                self._timers.add(timer)
                timer.add_done_callback(self._timers.discard)
            return
        self._end(key)
        self._write(kind, data)
        self._held[key] = _Window(kind, self._clock.now() + timedelta(seconds=REPEAT_WINDOW_S))

    async def _at_end(self, key: tuple[str, ...], held: _Window) -> None:
        await self._clock.sleep_until(held.until)
        if self._held.get(key) is held:
            with contextlib.suppress(Exception):     # a recording never raises
                self._end(key)

    def _end(self, key: tuple[str, ...]) -> None:
        held = self._held.pop(key, None)
        if held is not None and held.repeats:
            self._write(held.kind, held.latest | {"repeats": held.repeats})

    def flush(self) -> None:
        """At shutdown: every open window's count is written now."""
        for timer in self._timers:
            timer.cancel()
        for key in list(self._held):
            self._end(key)


def _code_path(filename: str) -> str:
    """A traceback's file as a record shows it: under FERB's package from `ferb/`, else its file name alone (never a
    path through the owner's home)."""
    import ferb
    path = Path(filename)
    try:
        return "ferb/" + path.relative_to(Path(ferb.__file__).parent).as_posix()
    except ValueError:
        return path.name


FRESH_S = 10.0       # how long a kept permission check or browser profile discovery serves before a refresh (`_Fresh`)


class _Fresh:
    """Values read by a slow call (a TCC check, browser profile files), each kept for `ttl_s`. A read after that gets
    the kept value at once and refreshes it in a worker thread; only the first read of a call (nothing kept yet) waits
    for it, and start-up takes those in a thread (`prime`). Keyed by the call itself, so a replaced call is read anew."""

    def __init__(self, ttl_s: float) -> None:
        self.ttl_s = ttl_s
        self._held: dict[Any, tuple[float, Any]] = {}
        self._refreshing: set[Any] = set()

    @staticmethod
    def _key(read: Callable[[], Any], salt: tuple[Any, ...]) -> Any:
        return (getattr(read, "__self__", None) and id(read.__self__), getattr(read, "__func__", read), salt)

    def get(self, read: Callable[[], Any], *salt: Any) -> Any:
        """`salt`: what else the value depends on (a different salt is a different value)."""
        key, now = self._key(read, salt), time.monotonic()
        held = self._held.get(key)
        if held is None:
            value = read()
            self._held[key] = (now, value)
            return value
        if now - held[0] > self.ttl_s and key not in self._refreshing:
            try:
                loop = asyncio.get_running_loop()
            except RuntimeError:                        # a worker thread: read it here
                self._held[key] = (now, read())
                return self._held[key][1]
            self._refreshing.add(key)
            refresh = loop.run_in_executor(None, read)

            def done(f: "asyncio.Future[Any]") -> None:
                self._refreshing.discard(key)
                if not f.cancelled() and f.exception() is None:
                    self._held[key] = (time.monotonic(), f.result())
            refresh.add_done_callback(done)
        return held[1]

    async def prime(self, *reads: Callable[[], Any]) -> None:
        """Take these reads' first values in a worker thread, so no turn waits for them."""
        def first() -> None:
            for read in reads:
                try:
                    self.get(read)
                except Exception:
                    pass
        await asyncio.to_thread(first)


async def _no_notify(title: str, text: str) -> None:
    return None
