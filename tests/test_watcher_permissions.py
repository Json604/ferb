"""Live 2026-10-26/27 (friends' first installs): macOS showed "FERB would like to receive keystrokes" on setup's first
screen, because the keyboard-activity tap started with the daemon; and on a newer macOS "Data Access Blocked: FERB
tried to access your data from other apps", because the notification watcher read macOS's notification database
before Full Disk Access was on, after which it stopped for good ("watcher notification: stopped: OperationalError:
unable to open database file"). The tap starts only once Input Monitoring is granted; the notification watcher reads
nothing until Full Disk Access is on; a watcher that fails tries again later instead of stopping until a restart."""

import asyncio
from pathlib import Path
from typing import Any

from ferb.sense.watchers.notifications import NotificationWatcher
from ferb.sense.watchers.owner_input import OwnerInput


def test_the_keyboard_tap_waits_for_input_monitoring() -> None:
    oi = OwnerInput(allowed=lambda: False)
    assert oi.start() is False and not oi.running and oi._tap is None       # no tap: macOS never prompts


class _Reader:
    def __init__(self) -> None:
        self.reads = 0

    def read(self) -> list[Any]:
        self.reads += 1
        return []


def test_the_notification_watcher_reads_nothing_until_full_disk_access() -> None:
    reader = _Reader()
    granted = [False]
    w = NotificationWatcher(reader, lambda e: False, lambda o: None, interval_s=0.01,  # type: ignore[arg-type]
                            allowed=lambda: granted[0], recheck_s=0.01)

    async def go() -> int:
        stop = asyncio.Event()
        task = asyncio.create_task(w.run(lambda o: None, stop))
        await asyncio.sleep(0.05)
        before = reader.reads
        granted[0] = True
        await asyncio.sleep(0.05)
        stop.set()
        await task
        return before
    assert asyncio.run(go()) == 0 and reader.reads > 0


def test_a_failed_watcher_tries_again_instead_of_stopping(tmp_path: Path) -> None:
    from ferb.sense import manager

    class Flaky:
        source = "notification"

        def __init__(self) -> None:
            self.runs = 0

        async def run(self, emit: Any, stop: asyncio.Event) -> None:
            self.runs += 1
            if self.runs == 1:
                raise RuntimeError("unable to open database file")
            await stop.wait()

    flaky = Flaky()
    failures: list[str] = []
    trail = type("T", (), {"record": lambda self, o: None, "publish": lambda self, k, d: None})()
    s = manager.Sense(tmp_path, trail, [flaky], on_fail=lambda src, why: failures.append(why))  # type: ignore[arg-type]
    s.retry_s = 0.01

    async def go() -> None:
        await s.start()
        await asyncio.sleep(0.1)
        await s.stop()
    asyncio.run(go())
    assert flaky.runs == 2 and failures == ["RuntimeError: unable to open database file"]
