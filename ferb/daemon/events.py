"""Own the daemon's published events: each one is redacted once, then handed to its consumers in a fixed order.
Must not change an event's data beyond redaction, or let a consumer see it unredacted.

The consumers (plan waits, today's activity, delegated conversations, attention, forgets waiting on a task, the bus,
the approval banner) used to redact the same data one by one, up to five times per event."""

from typing import Any, Callable

Redact = Callable[[Any], Any]
Consumer = Callable[[str, dict[str, Any]], None]


class Events:
    def __init__(self, redact: Callable[[], Redact]) -> None:
        self._redact = redact                   # read per event: the daemon may still swap its redactor at start
        self._consumers: list[tuple[frozenset[str] | None, Consumer]] = []

    def on(self, topics: set[str] | None, consumer: Consumer) -> None:
        """`consumer(topic, redacted data)` for these topics (None: every topic), after the ones added before it."""
        self._consumers.append((None if topics is None else frozenset(topics), consumer))

    def publish(self, topic: str, data: dict[str, Any]) -> None:
        redacted = self._redact()(data)
        for topics, consumer in self._consumers:
            if topics is None or topic in topics:
                consumer(topic, dict(redacted))     # a shallow copy each: one consumer adding a key can't leak to the next
