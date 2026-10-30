"""Own event subscription and overflow signaling. Must not turn event loss into a successful task outcome.

Classes: critical events are never dropped — a subscriber whose queue is full of them is disconnected with an
explicit overflow; telemetry may be dropped (oldest first) and the subscriber is told how many."""

import asyncio
from collections import deque
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Any, AsyncIterator

from ferb.core.interfaces import Clock


class EventClass(Enum):
    CRITICAL = "critical"
    TELEMETRY = "telemetry"


CRITICAL = {"task.created", "task.started", "task.finished", "task.retried", "task.steered", "approval.requested", "approval.resolved",
            "device.changed", "browser.profile_paired", "timer.fired", "memory.committed",
            "memory.retired", "memory.expires",   # L36 §6: an item stops holding now, or gets the end its words give
            "stop.engaged", "stop.lifted", "sense.paused", "sense.resumed", "sense.watcher_failed",
            "browser.debugger_detached",
            "task.result", "task.resumed_event", "conversation.paused", "attention.notice",
            "loop.status",       # a loop's state change (L31): the notch's standing work must never miss its last
            "memory.confirmed_derived", "memory.forgot_derived", "memory.rollback",
            "memory.forgot"}   # a forget the owner said Yes to from the notch's buttons (I2)
TELEMETRY = {"tool.called", "tool.finished", "trail.event", "action.observed", "browser.connected", "browser.disconnected", "browser.profiles_changed",
             "memory.proposed", "browser.dialog", "browser.file_chooser", "browser.download", "browser.cleanup_failed",
             "disturbance.observed", "memory.derived", "memory.digest",
             "memory.embedding_unavailable", "perception.attention", "perception.batch", "security.alignment",
             "cursor.point"}   # L33: where an action is about to land, for the notch overlay (geometry only)
STABLE_TOPICS = CRITICAL | TELEMETRY
DROPPED = "bus.dropped"   # synthetic notice to a subscriber: {"count": n} telemetry events were dropped


@dataclass(frozen=True)
class Event:
    topic: str
    at: datetime
    data: dict[str, Any] = field(default_factory=dict)

    @property
    def cls(self) -> EventClass:
        return EventClass.CRITICAL if self.topic in CRITICAL else EventClass.TELEMETRY


class Overflow(Exception):
    """This subscriber fell so far behind that a critical event couldn't be queued; resubscribe and re-read."""


class Subscription:
    def __init__(self, bus: "Bus", topics: set[str] | None, maxsize: int) -> None:
        self._bus, self.topics, self.maxsize = bus, topics, maxsize
        self._queue: deque[Event] = deque()
        self._wakeup = asyncio.Event()
        self.dropped = 0
        self._reported_dropped = 0
        self.overflowed = False
        self.closed = False

    def wants(self, topic: str) -> bool:
        return self.topics is None or any(topic == t or (t.endswith(".*") and topic.startswith(t[:-1]))
                                          for t in self.topics)

    def _offer(self, ev: Event) -> None:
        if len(self._queue) >= self.maxsize:
            victim = next((e for e in self._queue if e.cls is EventClass.TELEMETRY), None)
            if victim is not None:
                self._queue.remove(victim)          # drop the oldest telemetry
                self.dropped += 1
            elif ev.cls is EventClass.TELEMETRY:
                self.dropped += 1                   # queue is all critical: drop the new telemetry
                self._wakeup.set()
                return
            else:
                self.overflowed = True              # never drop critical silently
                self._bus.unsubscribe(self)
                self._wakeup.set()
                return
        self._queue.append(ev)
        self._wakeup.set()

    def __aiter__(self) -> AsyncIterator[Event]:
        return self._iter()

    async def _iter(self) -> AsyncIterator[Event]:
        while True:
            if self.dropped > self._reported_dropped:
                n, self._reported_dropped = self.dropped - self._reported_dropped, self.dropped
                yield Event(DROPPED, self._bus.clock.now(), {"count": n})
                continue
            if self._queue:
                yield self._queue.popleft()
                continue
            if self.overflowed:
                raise Overflow("subscriber overflowed with critical events")
            if self.closed:
                return
            self._wakeup.clear()
            await self._wakeup.wait()

    def close(self) -> None:
        self.closed = True
        self._bus.unsubscribe(self)
        self._wakeup.set()


class Bus:
    def __init__(self, clock: Clock) -> None:
        self.clock = clock
        self._subs: list[Subscription] = []

    def publish(self, topic: str, data: dict[str, Any] | None = None) -> Event:
        if topic not in STABLE_TOPICS:
            raise ValueError(f"unknown bus topic {topic!r}; declare it in CRITICAL or TELEMETRY first")
        ev = Event(topic, self.clock.now(), data or {})
        for sub in list(self._subs):
            if sub.wants(topic):
                sub._offer(ev)
        return ev

    def subscribe(self, topics: set[str] | None = None, maxsize: int = 1000) -> Subscription:
        sub = Subscription(self, topics, maxsize)
        self._subs.append(sub)
        return sub

    def unsubscribe(self, sub: Subscription) -> None:
        if sub in self._subs:
            self._subs.remove(sub)
