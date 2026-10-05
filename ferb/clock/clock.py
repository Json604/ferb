"""Owns wall and monotonic clocks. Must not infer ambiguous owner intent or turn reminders into unauthorized actions.

Clocks: wall time for timestamps/schedules, monotonic time for elapsed time and timeouts."""

import asyncio
import time
from datetime import datetime, timedelta, timezone


class SystemClock:
    def now(self) -> datetime:
        return datetime.now().astimezone()

    def monotonic(self) -> float:
        return time.monotonic()

    async def sleep_until(self, t: datetime) -> None:
        # Re-check the wall clock in short slices so sleep/wake and clock changes are noticed.
        while (delay := (t - self.now()).total_seconds()) > 0:
            await asyncio.sleep(min(delay, 30.0))


class FakeClock:
    """Test clock: time moves only via advance(); sleepers wake when time passes their deadline."""

    def __init__(self, start: datetime | None = None) -> None:
        self._now = start or datetime(2026, 1, 1, 9, 0, tzinfo=timezone.utc)
        self._mono = 1000.0
        self._changed = asyncio.Event()

    def now(self) -> datetime:
        return self._now

    def monotonic(self) -> float:
        return self._mono

    def advance(self, seconds: float) -> None:
        self._now += timedelta(seconds=seconds)
        self._mono += seconds
        self._changed.set()
        self._changed = asyncio.Event()

    def set_wall(self, t: datetime) -> None:
        """Simulate a wall-clock change (e.g. timezone or NTP jump); monotonic time is unaffected."""
        self._now = t
        self._changed.set()
        self._changed = asyncio.Event()

    async def sleep_until(self, t: datetime) -> None:
        while self._now < t:
            await self._changed.wait()
