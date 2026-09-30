"""Scheduled and recurring tasks (section 18 step 15).

A schedule keeps the owner's words and the intent recorded from them at scheduling time; each run plans afresh (pages
and inboxes change) but is held to that intent. Rules: once, daily at HH:MM, weekdays at HH:MM, weekly on a day at
HH:MM, or every N minutes (N >= 15), in the owner's local time zone (so a daily 09:00 stays 09:00 across daylight-
saving changes). Each next run is a durable timer (exactly once, step 14).

Missed runs (Mac asleep or off): one run when it wakes, marked late; skipped if later than half the repeat interval
(at most 12 hours); a one-off runs up to 12 hours late. Three failures in a row pause the schedule.
"""

import json
from dataclasses import dataclass
from datetime import datetime, time, timedelta, timezone
from typing import Any
from zoneinfo import ZoneInfo

from ferb.core.interfaces import Clock, IdSource
from ferb.store.db import Database

REPEATS = ("none", "daily", "weekdays", "weekly", "every")
MIN_EVERY_MIN = 15
MAX_LATE_S = 12 * 3600
FAILURES_TO_PAUSE = 3
DAYS = ("monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday")


class ScheduleError(ValueError):
    pass


def local_zone() -> ZoneInfo:
    import os
    import time as _t
    name = os.environ.get("TZ")
    if not name:
        try:
            link = os.readlink("/etc/localtime")
            name = link.split("zoneinfo/", 1)[1]
        except (OSError, IndexError):
            name = None
    try:
        return ZoneInfo(name) if name else ZoneInfo(_t.tzname[0])
    except Exception:
        return ZoneInfo("UTC")


@dataclass(frozen=True)
class Rule:
    repeat: str
    at: str = ""                 # "HH:MM" local (daily / weekdays / weekly)
    weekday: int | None = None   # 0 = Monday (weekly)
    every_minutes: int | None = None
    tz: str = "UTC"

    @classmethod
    def build(cls, repeat: str, first: datetime, weekday: str | None = None, every_minutes: int | None = None,
              tz: ZoneInfo | None = None) -> "Rule":
        if repeat not in REPEATS:
            raise ScheduleError(f"unknown repeat {repeat!r}")
        zone = tz or local_zone()
        local = first.astimezone(zone)
        if repeat == "every":
            if not every_minutes or every_minutes < MIN_EVERY_MIN:
                raise ScheduleError(f"a repeating task runs at most every {MIN_EVERY_MIN} minutes")
        wd = None
        if repeat == "weekly":
            wd = DAYS.index(weekday.lower()) if weekday and weekday.lower() in DAYS else local.weekday()
        return cls(repeat, local.strftime("%H:%M"), wd, every_minutes if repeat == "every" else None, zone.key)

    def interval_s(self) -> float | None:
        return {"none": None, "daily": 86400.0, "weekdays": 86400.0, "weekly": 7 * 86400.0}.get(
            self.repeat, (self.every_minutes or MIN_EVERY_MIN) * 60.0)

    def late_limit_s(self) -> float:
        iv = self.interval_s()
        return MAX_LATE_S if iv is None else min(iv / 2, MAX_LATE_S)

    def next_after(self, after: datetime, anchor: datetime) -> datetime | None:
        """The first run strictly after `after` (anchor: the schedule's first run)."""
        if self.repeat == "none":
            return None
        if self.repeat == "every":
            step = timedelta(minutes=self.every_minutes or MIN_EVERY_MIN)
            n = max(0, int((after - anchor) / step) + 1)
            return anchor + n * step
        zone = ZoneInfo(self.tz)
        hh, mm = (int(x) for x in self.at.split(":"))
        day = after.astimezone(zone).date()
        for _ in range(9):
            cand = datetime.combine(day, time(hh, mm), tzinfo=zone)       # local wall time: DST-safe
            ok = (self.repeat == "daily" or (self.repeat == "weekdays" and cand.weekday() < 5)
                  or (self.repeat == "weekly" and cand.weekday() == self.weekday))
            if ok and cand > after:
                return cand.astimezone(timezone.utc)
            day += timedelta(days=1)
        return None

    def describe(self) -> str:
        if self.repeat == "none":
            return "once"
        if self.repeat == "every":
            return f"every {self.every_minutes} minutes"
        if self.repeat == "weekly":
            return f"every {DAYS[self.weekday or 0].capitalize()} at {self.at}"
        return f"{'every day' if self.repeat == 'daily' else 'weekdays'} at {self.at}"


@dataclass(frozen=True)
class Schedule:
    id: str
    request: str
    own_words: str
    intent: dict[str, Any]
    rule: Rule
    next_run: datetime | None
    timer_id: str | None
    state: str
    failures: int
    first_run: datetime


class Schedules:
    def __init__(self, db: Database, clock: Clock, ids: IdSource) -> None:
        self.db, self.clock, self.ids = db, clock, ids

    def _row(self, r: Any) -> Schedule:
        rule = json.loads(r["rule_json"])
        first = datetime.fromisoformat(rule.pop("first"))
        return Schedule(r["id"], r["request"], r["own_words"], json.loads(r["intent_json"]), Rule(**rule),
                        datetime.fromisoformat(r["next_run"]) if r["next_run"] else None, r["timer_id"], r["state"],
                        int(r["failures"]), first)

    def add(self, request: str, own_words: str, intent: dict[str, Any], rule: Rule, first: datetime) -> Schedule:
        if first <= self.clock.now():
            raise ScheduleError("the first run must be in the future")
        sid = "sc-" + self.ids.new_id()[:10]
        self.db.conn.execute(
            "INSERT INTO schedules (id, request, own_words, intent_json, rule_json, next_run, state, created_at) "
            "VALUES (?, ?, ?, ?, ?, ?, 'active', ?)",
            (sid, request[:1000], own_words[:4000], json.dumps(intent), json.dumps(rule.__dict__ | {
                "first": first.astimezone(timezone.utc).isoformat()}),
             first.astimezone(timezone.utc).isoformat(), self.clock.now().isoformat()))
        s = self.get(sid)
        assert s is not None
        return s

    def get(self, sid: str) -> Schedule | None:
        r = self.db.conn.execute("SELECT * FROM schedules WHERE id = ?", (sid,)).fetchone()
        return self._row(r) if r else None

    def all(self, states: tuple[str, ...] = ("active", "paused")) -> list[Schedule]:
        q = ",".join("?" * len(states))
        return [self._row(r) for r in self.db.conn.execute(
            f"SELECT * FROM schedules WHERE state IN ({q}) ORDER BY next_run", states)]

    def set_timer(self, sid: str, timer_id: str | None, next_run: datetime | None) -> None:
        self.db.conn.execute("UPDATE schedules SET timer_id = ?, next_run = ? WHERE id = ?",
                             (timer_id, next_run.astimezone(timezone.utc).isoformat() if next_run else None, sid))

    def set_state(self, sid: str, state: str) -> None:
        self.db.conn.execute("UPDATE schedules SET state = ? WHERE id = ?", (state, sid))

    def ran(self, sid: str, ok: bool) -> int:
        """Record a run; returns the failures in a row."""
        if ok:
            self.db.conn.execute("UPDATE schedules SET failures = 0, last_run = ? WHERE id = ?",
                                 (self.clock.now().isoformat(), sid))
            return 0
        self.db.conn.execute("UPDATE schedules SET failures = failures + 1, last_run = ? WHERE id = ?",
                             (self.clock.now().isoformat(), sid))
        return int(self.db.conn.execute("SELECT failures FROM schedules WHERE id = ?", (sid,)).fetchone()[0])
