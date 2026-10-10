"""Bus (section 6): topic filters, bounded queues that never block the publisher, telemetry dropped with a
notice, critical never silently dropped (overflow disconnects the subscriber instead)."""

import asyncio

import pytest

from ferb.bus import DROPPED, Bus, EventClass, Overflow
from ferb.clock.clock import FakeClock


async def drain(sub, n: int) -> list:
    out = []
    async for ev in sub:
        out.append(ev)
        if len(out) == n:
            break
    return out


def test_topic_filter_and_wildcards() -> None:
    async def scenario() -> list[str]:
        bus = Bus(FakeClock())
        sub = bus.subscribe({"task.*", "trail.event"})
        bus.publish("task.created", {"id": "t1"})
        bus.publish("tool.called", {})
        bus.publish("trail.event", {})
        bus.publish("task.finished", {"id": "t1"})
        return [e.topic for e in await drain(sub, 3)]

    assert asyncio.run(scenario()) == ["task.created", "trail.event", "task.finished"]


def test_unknown_topic_is_refused() -> None:
    with pytest.raises(ValueError):
        Bus(FakeClock()).publish("made.up", {})


def test_classes() -> None:
    bus = Bus(FakeClock())
    assert bus.publish("approval.requested").cls is EventClass.CRITICAL
    assert bus.publish("trail.event").cls is EventClass.TELEMETRY


def test_slow_subscriber_loses_oldest_telemetry_with_a_notice_and_keeps_every_critical() -> None:
    async def scenario() -> list:
        bus = Bus(FakeClock())
        sub = bus.subscribe(maxsize=3)
        bus.publish("trail.event", {"n": 1})
        bus.publish("task.created", {"id": "a"})
        bus.publish("trail.event", {"n": 2})
        bus.publish("trail.event", {"n": 3})      # full: drops trail n=1
        bus.publish("task.finished", {"id": "a"})  # full: drops trail n=2
        return await drain(sub, 4)

    evs = asyncio.run(scenario())
    assert evs[0].topic == DROPPED and evs[0].data == {"count": 2}
    assert [(e.topic, e.data) for e in evs[1:]] == [
        ("task.created", {"id": "a"}), ("trail.event", {"n": 3}), ("task.finished", {"id": "a"})]


def test_queue_full_of_critical_disconnects_subscriber_with_overflow_never_silent_loss() -> None:
    async def scenario() -> tuple[list, bool]:
        bus = Bus(FakeClock())
        sub = bus.subscribe(maxsize=2)
        bus.publish("task.created", {"id": "1"})
        bus.publish("task.created", {"id": "2"})
        bus.publish("trail.event", {})             # telemetry into an all-critical queue: dropped, counted
        bus.publish("task.created", {"id": "3"})   # critical can't fit: overflow
        got, overflowed = [], False
        try:
            async for ev in sub:
                got.append(ev)
        except Overflow:
            overflowed = True
        return got, overflowed

    got, overflowed = asyncio.run(scenario())
    assert overflowed
    assert [e.data.get("id") for e in got if e.topic == "task.created"] == ["1", "2"]
    assert any(e.topic == DROPPED for e in got)


def test_publisher_never_blocks_and_closed_subscriber_stops_receiving() -> None:
    async def scenario() -> int:
        bus = Bus(FakeClock())
        sub = bus.subscribe(maxsize=10)
        for i in range(10_000):                    # far more than the queue; must return immediately
            bus.publish("trail.event", {"i": i})
        sub.close()
        bus.publish("task.created", {})
        return len(bus._subs)

    assert asyncio.run(scenario()) == 0


def test_every_literal_production_publication_has_a_declared_topic():
    import ast
    from pathlib import Path
    from ferb.bus import STABLE_TOPICS
    root = Path(__file__).resolve().parents[1] / 'ferb'
    undeclared = []
    for path in root.rglob('*.py'):
        for call in ast.walk(ast.parse(path.read_text())):
            if (isinstance(call, ast.Call) and isinstance(call.func, ast.Attribute)
                    and call.func.attr in ('publish', '_publish') and call.args
                    and isinstance(call.args[0], ast.Constant) and isinstance(call.args[0].value, str)
                    and call.args[0].value not in STABLE_TOPICS):
                undeclared.append((str(path.relative_to(root)), call.lineno, call.args[0].value))
    assert not undeclared
