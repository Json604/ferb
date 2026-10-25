"""Owns the facts a tool's code measures while one call runs, kept on that call's tool span (D31 fix round 4: which
check decided each click's point). Must not decide anything: code adds facts, the gateway records them, nobody reads
them back to act.

The gateway opens a record around each tool run (`recording`); code under it, in the event loop or a worker thread
(`asyncio.to_thread` carries the record), adds a fact with `note`. With no record open, `note` does nothing."""

from collections.abc import Iterator
from contextlib import contextmanager
from contextvars import ContextVar

_HELD: ContextVar[dict[str, list[str]] | None] = ContextVar("ferb_call_facts", default=None)


def note(key: str, value: str) -> None:
    """Add one fact under `key` to the running call's record, in order."""
    held = _HELD.get()
    if held is not None:
        held.setdefault(key, []).append(value)


@contextmanager
def recording() -> Iterator[dict[str, list[str]]]:
    """A fresh record for one call: what `note` adds while it is open."""
    held: dict[str, list[str]] = {}
    token = _HELD.set(held)
    try:
        yield held
    finally:
        _HELD.reset(token)
