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


def _waited_for(lock: asyncio.Lock) -> bool:
    """Someone waits for this lock (asyncio.Lock's private waiter list: a lock just released to a waiter that hasn't
    woken yet looks free otherwise)."""
    return any(not w.cancelled() for w in (getattr(lock, "_waiters", None) or ()))


class ResourceLocks:
    def __init__(self) -> None:
        self._locks: dict[str, asyncio.Lock] = {}
        self._holders: dict[str, str] = {}
        self._reading: set[str] = set()      # keys whose holder only reads (a call marked `observation`)

    @asynccontextmanager
    async def hold(self, keys: list[str], holder: str, reading: bool = False) -> AsyncIterator[None]:
        """`reading`: the call only reads what these keys name (it declares `observation`); a bystander may look at
        it meanwhile (`try_hold` with `during_reads`). It is still exclusive between calls."""
        taken: list[str] = []
        try:
            for k in ordered(keys):
                lock = self._locks.setdefault(k, asyncio.Lock())
                await lock.acquire()
                taken.append(k)
                self._holders[k] = holder
                if reading:
                    self._reading.add(k)
            yield
        finally:
            for k in reversed(taken):
                self._holders.pop(k, None)
                self._reading.discard(k)
                self._locks[k].release()

    @asynccontextmanager
    async def try_hold(self, keys: list[str], holder: str, during_reads: bool = False) -> AsyncIterator[bool]:
        """Take every lock only if each is free and nobody waits for it, else take none and yield False at once: a
        bystander (the live preview, kb area app) never queues ahead of a worker's next call. Free locks are taken
        on asyncio's fast path, without yielding to the loop, so no worker can slip in between the look and the take.
        `during_reads` (DMN-406): a key held by a call that only reads, with nobody waiting for it, also counts as
        available: the bystander looks alongside that read without taking the key, so it still never delays the
        worker (a call queued behind the read makes the key busy again)."""
        order = ordered(keys)
        locks = [self._locks.setdefault(k, asyncio.Lock()) for k in order]
        if any(_waited_for(lock) or (lock.locked() and not (during_reads and k in self._reading))
               for k, lock in zip(order, locks)):
            yield False
            return
        taken: list[str] = []
        try:
            for k, lock in zip(order, locks):
                if lock.locked():                    # a read holds it (during_reads): looked at alongside, not taken
                    continue
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
