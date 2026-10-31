"""Whether the Mac is in use: asleep, its displays asleep, its screen locked or its login session switched away, or not.
Must not read input, windows or content, and must not treat input idleness as being away.

FERB's one presence signal (orchestration decision 17, clarified by the owner 2026-10-31): "on my Mac" means the Mac
coming back into use: the lid opened or the Mac woken from sleep, the screen unlocked, the owner logging in, or a
restart (the daemon starting in a new login session, `Daemon._mac_started`). Watching a film without touching anything
is still being at the Mac, so no keyboard or mouse quiet time counts as away.

macOS's own notifications tell it, on one thread of FERB's with its own run loop: NSWorkspace's will-sleep / did-wake,
screens-did-sleep / screens-did-wake and session resign / become active, and the distributed `com.apple.screenIsLocked`
/ `com.apple.screenIsUnlocked`. Every POLL_S the same thread also reads the session's lock and console state and the
main display's sleep (CoreGraphics, read-only), so a missed notification is caught within seconds. The Mac is back the
moment none of those holds any more after one did: `on_return(signal)` is told once (on that thread). Without the
watcher (sensing paused with Pause FERB, or not started) FERB can't tell: `away()` is False (unknown counts as present)
and no return comes."""

import threading
import time
from typing import Any, Callable

from ferb.sense.watchers.base import run_loop_slice

POLL_S = 5.0
# what holds the Mac out of use, and the notification that sets or clears each (name, flag, value)
WORKSPACE = (("NSWorkspaceWillSleepNotification", "asleep", True), ("NSWorkspaceDidWakeNotification", "asleep", False),
             ("NSWorkspaceScreensDidSleepNotification", "displays_asleep", True),
             ("NSWorkspaceScreensDidWakeNotification", "displays_asleep", False),
             ("NSWorkspaceSessionDidResignActiveNotification", "switched_away", True),
             ("NSWorkspaceSessionDidBecomeActiveNotification", "switched_away", False))
DISTRIBUTED = (("com.apple.screenIsLocked", "locked", True), ("com.apple.screenIsUnlocked", "locked", False))
SIGNALS = {"asleep": "wake", "displays_asleep": "screens_wake", "locked": "unlock", "switched_away": "session_active"}


def session() -> dict[str, Any]:
    """The login session as CoreGraphics reports it (read-only): its unique id, whether its screen is locked and
    whether it is the one on the console. Empty when it can't be read."""
    try:
        import Quartz
        d = Quartz.CGSessionCopyCurrentDictionary()
    except Exception:
        return {}
    if d is None:
        return {}
    d = dict(d)
    return {"id": str(d.get("CGSSessionUniqueSessionUUID") or "") or None,
            "locked": bool(d.get("CGSSessionScreenIsLocked", False)),
            "on_console": bool(d.get("kCGSSessionOnConsoleKey", True))}


def displays_asleep() -> bool | None:
    """The main display is asleep (read-only); None when it can't be read."""
    try:
        import Quartz
        return bool(Quartz.CGDisplayIsAsleep(Quartz.CGMainDisplayID()))
    except Exception:
        return None


class MacPresence:
    def __init__(self, wall: Callable[[], float] = time.time,
                 read_session: Callable[[], dict[str, Any]] = session,
                 read_displays: Callable[[], bool | None] = displays_asleep) -> None:
        """`wall`: the daemon's clock as a POSIX time."""
        self.wall, self.read_session, self.read_displays = wall, read_session, read_displays
        self.running = False
        self.present_since: float | None = None          # wall-clock start of the current stretch in use (None: unknown)
        self.on_return: Callable[[str], None] | None = None
        self._held = {flag: False for flag in SIGNALS}
        self._lock = threading.Lock()
        self._done: threading.Event | None = None
        self._thread: threading.Thread | None = None

    def away(self) -> bool:
        """The Mac is out of use now (asleep, displays asleep, locked or switched away). Not running: False."""
        with self._lock:
            return self.running and any(self._held.values())

    def changed(self, flag: str, value: bool) -> None:
        """One fact changed (a notification, or a read that differs). Back in use when nothing holds it out any more:
        `returned` with the signal that cleared the last one."""
        with self._lock:
            if not self.running or flag not in self._held or self._held[flag] == value:
                return
            was = any(self._held.values())
            self._held[flag] = value
            back = was and not any(self._held.values())
        if back:
            self.returned(SIGNALS[flag])

    def returned(self, signal: str) -> None:
        """The Mac came back into use (or the daemon started in a new login session: `signal` "login")."""
        self.present_since = self.wall()
        if self.on_return is not None:
            try:
                self.on_return(signal)
            except Exception:
                pass                                       # telling FERB never stops the watcher

    def read(self) -> None:
        """Catch up with what can be read now: the lock, the console and the main display's sleep."""
        s = self.read_session()
        if "locked" in s:
            self.changed("locked", bool(s["locked"]))
        if "on_console" in s:
            self.changed("switched_away", not bool(s["on_console"]))
        asleep = self.read_displays()
        if asleep is not None:
            self.changed("displays_asleep", asleep)

    def start(self, native: bool = True) -> bool:
        """Start watching: the state read now (a locked screen at start is away), then the notifications. `native`
        False (tests): no thread, the caller tells `changed`."""
        if self.running:
            return True
        with self._lock:
            self.running = True
            self._held = {flag: False for flag in SIGNALS}
        self.read()
        if native:
            self._done = threading.Event()
            self._thread = threading.Thread(target=self._watch, args=(self._done,), name="ferb-presence", daemon=True)
            self._thread.start()
        return True

    def stop(self) -> None:
        with self._lock:
            self.running = False
        if self._done is not None:
            self._done.set()
        self._done = self._thread = None

    def _watch(self, done: threading.Event) -> None:
        tokens: list[tuple[Any, Any]] = []
        try:
            import AppKit
            from Foundation import NSDistributedNotificationCenter
            workspace = AppKit.NSWorkspace.sharedWorkspace().notificationCenter()
            distributed = NSDistributedNotificationCenter.defaultCenter()

            def observer(flag: str, value: bool) -> Callable[[Any], None]:
                return lambda note: self.changed(flag, value)
            for name, flag, value in WORKSPACE:
                tokens.append((workspace, workspace.addObserverForName_object_queue_usingBlock_(
                    getattr(AppKit, name), None, None, observer(flag, value))))
            for name, flag, value in DISTRIBUTED:
                tokens.append((distributed, distributed.addObserverForName_object_queue_usingBlock_(
                    name, None, None, observer(flag, value))))
        except Exception:
            pass                                           # notifications unavailable: the reads below still tell
        try:
            while not done.is_set():
                run_loop_slice(done, POLL_S)
                if not done.is_set():
                    try:
                        self.read()
                    except Exception:
                        pass
        finally:
            for center, token in tokens:
                center.removeObserver_(token)
