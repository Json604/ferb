"""Own ordered resource locks for tabs, accounts, files, apps and recordings. Must not choose resources or authorize
actions.

A capability says which resources one call touches (`locks(args)`); the gateway takes them after policy allowed the
call and releases them when the call returns — held across observe -> act -> verify inside one call, never across
model calls or approval waits. Several locks are always taken in one fixed order (kind order, then name), so two
workers can never hold them crosswise: no deadlock.
"""

import asyncio
from contextlib import asynccontextmanager
from typing import AsyncIterator

KIND_ORDER = {"account": 0, "app": 1, "file": 2, "tab": 3, "recording": 4}   # recording: recording.stop's id


def ordered(keys: list[str]) -> list[str]:
    """'kind:name' keys, deduplicated, in the one global order."""
    uniq = set(keys)
    for k in uniq:
        if k.split(":", 1)[0] not in KIND_ORDER:
            raise ValueError(f"unknown lock kind in {k!r}")
    return sorted(uniq, key=lambda k: (KIND_ORDER[k.split(":", 1)[0]], k))


class ResourceLocks:
    def __init__(self) -> None:
        self._locks: dict[str, asyncio.Lock] = {}
        self._holders: dict[str, str] = {}

    @asynccontextmanager
    async def hold(self, keys: list[str], holder: str) -> AsyncIterator[None]:
        taken: list[str] = []
        try:
            for k in ordered(keys):
                lock = self._locks.setdefault(k, asyncio.Lock())
                await lock.acquire()
                taken.append(k)
                self._holders[k] = holder
            yield
        finally:
            for k in reversed(taken):
                self._holders.pop(k, None)
                self._locks[k].release()

    @asynccontextmanager
    async def try_hold(self, keys: list[str], holder: str) -> AsyncIterator[bool]:
        """Take every lock only if each is free and nobody waits for it, else take none and yield False at once: a
        bystander (the live preview, kb area app) never queues ahead of a worker's next call. Free locks are taken
        on asyncio's fast path, without yielding to the loop, so no worker can slip in between the look and the take.
        (Whether anyone waits is read from asyncio.Lock's private waiter list: a lock just released to a waiter that
        hasn't woken yet looks free otherwise.)"""
        order = ordered(keys)
        locks = [self._locks.setdefault(k, asyncio.Lock()) for k in order]
        if any(lock.locked() or any(not w.cancelled() for w in (getattr(lock, "_waiters", None) or ()))
               for lock in locks):
            yield False
            return
        taken: list[str] = []
        try:
            for k, lock in zip(order, locks):
                await lock.acquire()
                taken.append(k)
                self._holders[k] = holder
            yield True
        finally:
            for k in reversed(taken):
                self._holders.pop(k, None)
                self._locks[k].release()

    def holder(self, key: str) -> str | None:
        return self._holders.get(key)
