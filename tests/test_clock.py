"""Clock (section 13): FakeClock sleepers wake only when time passes; monotonic ignores wall-clock changes."""

import asyncio
from datetime import timedelta

from ferb.clock.clock import FakeClock, SystemClock


def test_fake_clock_sleeper_wakes_only_after_deadline() -> None:
    async def scenario() -> list[str]:
        clock = FakeClock()
        log: list[str] = []

        async def sleeper() -> None:
            await clock.sleep_until(clock.now() + timedelta(seconds=60))
            log.append("woke")

        task = asyncio.create_task(sleeper())
        await asyncio.sleep(0)
        clock.advance(59)
        await asyncio.sleep(0)
        log.append("after 59s")
        clock.advance(1)
        await asyncio.wait_for(task, 1)
        return log

    assert asyncio.run(scenario()) == ["after 59s", "woke"]


def test_monotonic_is_independent_of_wall_clock_changes() -> None:
    async def scenario() -> tuple[float, float]:
        clock = FakeClock()
        m0 = clock.monotonic()
        clock.set_wall(clock.now() - timedelta(hours=5))    # e.g. timezone change / NTP jump backwards
        clock.advance(2)
        return m0, clock.monotonic()

    m0, m1 = asyncio.run(scenario())
    assert m1 - m0 == 2


def test_system_clock_is_timezone_aware() -> None:
    assert SystemClock().now().tzinfo is not None
