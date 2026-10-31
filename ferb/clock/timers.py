"""Owns durable timer delivery. Must not infer ambiguous owner intent or turn reminders into unauthorized actions.

Durable timers: SQLite-backed; survive restarts and sleep; delivered exactly once.

- Due times are checked against the wall clock at least every POLL_S: the monotonic clock stops while the Mac sleeps,
  so a timer due during sleep (or while FERB was down) fires on the next check, with `overdue_s`.
- Exactly once: a due timer is claimed (pending -> claimed, with a token), handed to its consumer,
  then acknowledged (-> delivered). A crash between claim and acknowledgement leaves it claimed; on start it's
  handed over again, and the consumer skips a timer id it already acted on (`timer_deliveries`, written in the same
  transaction as the consumer's own record).
- When the owner is at the Mac (orchestration decision 17): a timer added `when_active` (a row in
  `timer_when_active`) is not claimed while the Mac is out of use (`owner_away`: the Mac's presence signal, asleep,
  displays asleep, locked or switched away; never input idleness); it fires at
  its time when they're active then, else when they're back (`wake()` on their return). One with `return_after` waits
  for an active stretch that began after that time ("when I'm back at my Mac", said while at it). Its lateness counts
  from when it could fire: waiting for the owner isn't late. Nothing else about it changes (pending, cancel, exactly
  once)."""

import asyncio
import secrets
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any, Awaitable, Callable

from ferb.core.interfaces import Clock, IdSource
from ferb.store.db import Database

POLL_S = 30.0


def recheck_in(due: datetime, now: datetime) -> float:
    """Seconds to wait before looking at the wall clock again for something due at `due`. Never longer than POLL_S:
    asyncio's timeouts run on the monotonic clock, which stops while the Mac sleeps."""
    return max(0.0, min(POLL_S, (due - now).total_seconds()))


@dataclass(frozen=True)
class Timer:
    id: str
    kind: str
    fire_at: datetime
    message: str
    task_id: str | None
    request_id: str | None
    state: str
    overdue_s: float | None = None


def _row(r: Any) -> Timer:
    return Timer(r["id"], r["kind"], datetime.fromisoformat(r["fire_at"]), r["message"], r["task_id"], r["request_id"],
                 r["state"], r["overdue_s"])


class Timers:
    def __init__(self, db: Database, clock: Clock, ids: IdSource,
                 consume: Callable[[Timer], Awaitable[None]] | None = None,
                 on_error: Callable[[Timer | None, Exception], None] | None = None) -> None:
        self.db, self.clock, self.ids = db, clock, ids
        self.consume = consume
        # told of a consumer that raised (the timer) or a check that failed (None); the daemon journals it
        self.on_error = on_error
        # decision 17: whether the Mac is out of use, and when its current stretch of use began (None: not known); the
        # daemon sets both from its presence signal (MacPresence), and calls `wake()` when the Mac is back in use
        self.owner_away: Callable[[], bool] = lambda: False
        self.present_since: Callable[[], datetime | None] = lambda: None   # None: FERB can't tell (unknown = present)
        self._wake = asyncio.Event()

    # ---------- writes ----------

    def add(self, kind: str, fire_at: datetime, message: str = "", task_id: str | None = None,
            request_id: str | None = None, *, when_active: bool = False,
            return_after: datetime | None = None) -> Timer:
        """`when_active` (decision 17): it fires only while the owner is at the Mac; `return_after`: only in an active
        stretch that began after that time."""
        now = self.clock.now()
        if fire_at.tzinfo is None or (return_after is not None and return_after.tzinfo is None):
            raise ValueError("a timer needs a time zone")
        tid = "tm-" + self.ids.new_id()
        self.db.conn.execute(
            "INSERT INTO timers (id, kind, fire_at, message, task_id, request_id, state, created_at) "
            "VALUES (?, ?, ?, ?, ?, ?, 'pending', ?)",
            (tid, kind, fire_at.astimezone(timezone.utc).isoformat(), message, task_id, request_id,
             now.isoformat()))
        if when_active or return_after is not None:
            self.db.conn.execute("INSERT INTO timer_when_active (timer_id, return_after) VALUES (?, ?)",
                                 (tid, return_after.astimezone(timezone.utc).isoformat() if return_after else None))
        self._wake.set()
        t = self.get(tid)
        assert t is not None
        return t

    def get(self, tid: str) -> Timer | None:
        r = self.db.conn.execute("SELECT * FROM timers WHERE id = ?", (tid,)).fetchone()
        return _row(r) if r else None

    def when_active(self, tid: str) -> bool:
        """It fires only while the owner is at the Mac (decision 17)."""
        return self.db.conn.execute("SELECT 1 FROM timer_when_active WHERE timer_id = ?", (tid,)).fetchone() is not None

    def wake(self) -> None:
        """Look at the timers again now (the owner is back at the Mac: a timer held for them may fire)."""
        self._wake.set()

    def _held(self) -> tuple[str, dict[str, Any]]:
        """The SQL condition (on `timers`) for a pending timer held for the owner now, and its arguments: when-active
        while they're away, or one waiting for a return that hasn't come (decision 17)."""
        since = self.present_since()
        return ("id IN (SELECT timer_id FROM timer_when_active WHERE :away OR (return_after IS NOT NULL AND "
                ":since IS NOT NULL AND julianday(return_after) >= julianday(:since)))",
                {"away": 1 if self.owner_away() else 0,
                 "since": since.astimezone(timezone.utc).isoformat() if since is not None else None})

    def delivered(self, tid: str) -> bool:
        """A delivery of this timer is recorded (its consumer handled it)."""
        return self.db.conn.execute("SELECT 1 FROM timer_deliveries WHERE timer_id = ?", (tid,)).fetchone() is not None

    def cancel(self, tid: str) -> bool:
        return self.db.conn.execute("UPDATE timers SET state = 'cancelled' WHERE id = ? AND state IN ('pending', "
                                    "'claimed')", (tid,)).rowcount > 0

    def cancel_task(self, task_id: str) -> int:
        """A task ended: its sleeps go (reminders it set for the owner stay)."""
        return self.db.conn.execute("UPDATE timers SET state = 'cancelled' WHERE task_id = ? AND kind = 'sleep' AND "
                                    "state IN ('pending', 'claimed')", (task_id,)).rowcount

    def cancel_all_sleeps(self) -> int:
        """After a restart: every sleeping worker's task was interrupted."""
        return self.db.conn.execute("UPDATE timers SET state = 'cancelled' WHERE kind = 'sleep' AND state IN "
                                    "('pending', 'claimed')").rowcount

    def pending(self, kinds: tuple[str, ...] = ("reminder", "timer")) -> list[Timer]:
        q = ",".join("?" * len(kinds))
        return [_row(r) for r in self.db.conn.execute(
            f"SELECT * FROM timers WHERE state = 'pending' AND kind IN ({q}) ORDER BY fire_at", kinds)]

    # ---------- firing ----------

    def claim_due(self) -> list[Timer]:
        """Due pending timers plus claims a crash left unacknowledged, each under a fresh token."""
        now = self.clock.now()
        token = secrets.token_hex(8)
        held, args = self._held()
        self.db.conn.execute(
            "UPDATE timers SET state = 'claimed', claim_token = :token, claimed_at = :at WHERE (state = 'pending' AND "
            f"fire_at <= :now AND NOT {held}) OR state = 'claimed'",
            {"token": token, "at": now.isoformat(), "now": now.astimezone(timezone.utc).isoformat()} | args)
        rows = self.db.conn.execute("SELECT * FROM timers WHERE claim_token = ? AND state = 'claimed' ORDER BY fire_at",
                                    (token,)).fetchall()
        since = self.present_since()
        out = []
        for r in rows:
            due = datetime.fromisoformat(r["fire_at"])
            if since is not None and since > due and self.when_active(r["id"]):
                due = since                                # it waited for the owner: late only from their return
            overdue = max(0.0, (now - due).total_seconds())
            self.db.conn.execute("UPDATE timers SET overdue_s = ? WHERE id = ?", (overdue, r["id"]))
            out.append(_row(r) if overdue == 0 else Timer(**{**_row(r).__dict__, "overdue_s": overdue}))
        return out

    def ack(self, tid: str) -> None:
        self.db.conn.execute("UPDATE timers SET state = 'delivered', delivered_at = ? WHERE id = ? AND state = 'claimed'",
                             (self.clock.now().isoformat(), tid))

    async def tick(self) -> int:
        """Hand each due timer to the consumer and acknowledge it. A consumer that raises leaves that timer claimed (it
        is handed over again at the next check) and is reported; the timers after it are still delivered."""
        n = 0
        for t in self.claim_due():
            if self.consume is not None:
                try:
                    await self.consume(t)
                except asyncio.CancelledError:
                    raise
                except Exception as e:
                    self._report(t, e)
                    continue
            self.ack(t.id)
            n += 1
        return n

    def _report(self, timer: Timer | None, error: Exception) -> None:
        if self.on_error is not None:
            try:
                self.on_error(timer, error)
            except Exception:
                pass                                              # reporting never stops delivery

    def next_due_in(self) -> float:
        held, args = self._held()                          # held for the owner: `wake()` on their return
        r = self.db.conn.execute(f"SELECT MIN(fire_at) AS f FROM timers WHERE state = 'pending' AND NOT {held}",
                                 args).fetchone()
        if not r or not r["f"]:
            return POLL_S
        return recheck_in(datetime.fromisoformat(r["f"]), self.clock.now())

    async def run(self) -> None:
        """The daemon's loop: fire what's due (and what a crash left claimed), then wait for the next one."""
        while True:
            try:
                await self.tick()
            except asyncio.CancelledError:
                raise
            except Exception as e:
                self._report(None, e)                             # retried on the next check; claims stay claimed
            self._wake.clear()
            try:
                await asyncio.wait_for(self._wake.wait(), self.next_due_in() + 0.05)
            except asyncio.TimeoutError:
                pass


def mark_handled(db: Database, timer_id: str, at: datetime) -> bool:
    """The consumer's idempotency record: False if this timer was already handled (a redelivery after a crash)."""
    return db.conn.execute("INSERT OR IGNORE INTO timer_deliveries (timer_id, at) VALUES (?, ?)",
                           (timer_id, at.isoformat())).rowcount == 1
