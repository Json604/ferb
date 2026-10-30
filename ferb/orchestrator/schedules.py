"""Owns durable schedules and calendar arithmetic. Must not infer intent, choose a target or claim semantic success from transport facts.

Scheduled and recurring tasks.

A schedule keeps the owner's full words and the targets recorded at scheduling time; the agent decides each run
afresh using that context. Rules: once, daily at HH:MM, weekdays at HH:MM, weekly on a day at
HH:MM, or every positive N minutes, in the owner's local time zone (so a daily 09:00 stays 09:00 across daylight-
saving changes). Each next run is a durable timer (exactly once).

Missed runs are presented to the orchestrator with lateness and prior failures. Those observations do not cancel
an occurrence or pause the owner's schedule automatically.

A schedule can be a watch ("tell me when …"), marked by the planner when the owner's turn made it: "until_met" ends
once a confirmed run finds its condition met (code applying the owner's stored words, never a run widening anything);
"standing" keeps watching. A watch survives restarts and updates as every schedule does."""

import json
from dataclasses import dataclass
from datetime import datetime, time, timedelta, timezone
from typing import Any
from zoneinfo import ZoneInfo

from ferb.clock.when import local_zone
from ferb.core.interfaces import Clock, IdSource
from ferb.store.db import Database

REPEATS = ("none", "daily", "weekdays", "weekly", "every")
DAYS = ("monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday")
WATCHES = ("none", "until_met", "standing")
# How a watch is said to the owner and the planner (its scheduling line, `ferb schedules`, the planner's facts)
WATCH_SAID = {"none": "", "until_met": "until it happens, then it stops", "standing": "each time it happens"}
FINISHED = ("done", "done_unverified")      # a run that finished: confirmed, or done but not confirmed (V4)


def watch_met(status: Any, met: Any) -> bool:
    """Whether a watch run found its condition met: it finished and its outcome says so (`watch_met`: the worker's
    report of its fresh observation, given beside the answer the check judged). A failed run never did."""
    return met is True and status in FINISHED


class ScheduleError(ValueError):
    pass


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
            if every_minutes is None or every_minutes <= 0:
                raise ScheduleError("a repeating interval must be positive")
        wd = None
        if repeat == "weekly":
            wd = DAYS.index(weekday.lower()) if weekday and weekday.lower() in DAYS else local.weekday()
        return cls(repeat, local.strftime("%H:%M"), wd, every_minutes if repeat == "every" else None, zone.key)

    def next_after(self, after: datetime, anchor: datetime) -> datetime | None:
        """The first run strictly after `after` (anchor: the schedule's first run)."""
        if self.repeat == "none":
            return None
        if self.repeat == "every":
            if self.every_minutes is None or self.every_minutes <= 0:
                raise ScheduleError("a repeating interval must be positive")
            if after < anchor:              # before the first run (a schedule resumed less than one interval
                return anchor               # before it): the first run itself, never the one after it
            step = timedelta(minutes=self.every_minutes)
            return anchor + (int((after - anchor) / step) + 1) * step
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
    targets: dict[str, Any]          # recorded from the owner's words at scheduling time (stored as intent_json)
    rule: Rule
    next_run: datetime | None
    timer_id: str | None
    state: str
    failures: int
    first_run: datetime
    watch: str = "none"              # WATCHES
    last_reported_met: bool | None = None   # a watch: what its latest finished check that reported it found

    def describe(self) -> str:
        """Its rule, and what kind of watch it is (if it is one), in plain words."""
        return ", ".join(part for part in (self.rule.describe(), WATCH_SAID[self.watch]) if part)


class Schedules:
    def __init__(self, db: Database, clock: Clock, ids: IdSource) -> None:
        self.db, self.clock, self.ids = db, clock, ids

    def _row(self, r: Any) -> Schedule:
        rule = json.loads(r["rule_json"])
        first = datetime.fromisoformat(rule.pop("first"))
        return Schedule(r["id"], r["request"], r["own_words"], json.loads(r["intent_json"]), Rule(**rule),
                        datetime.fromisoformat(r["next_run"]) if r["next_run"] else None, r["timer_id"], r["state"],
                        int(r["failures"]), first, r["watch"],
                        None if r["last_reported_met"] is None else bool(r["last_reported_met"]))

    def add(self, request: str, own_words: str, targets: dict[str, Any], rule: Rule, first: datetime,
            watch: str = "none") -> Schedule:
        if first <= self.clock.now():
            raise ScheduleError("the first run must be in the future")
        if watch not in WATCHES:
            raise ScheduleError(f"unknown watch {watch!r}")
        sid = "sc-" + self.ids.new_id()[:10]
        self.db.conn.execute(
            "INSERT INTO schedules (id, request, own_words, intent_json, rule_json, next_run, state, created_at, "
            "watch) VALUES (?, ?, ?, ?, ?, ?, 'active', ?, ?)",
            (sid, request, own_words, json.dumps(targets), json.dumps(rule.__dict__ | {
                "first": first.astimezone(timezone.utc).isoformat()}),
             first.astimezone(timezone.utc).isoformat(), self.clock.now().isoformat(), watch))
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

    def reported(self, sid: str, met: bool) -> None:
        """A watch's finished check reported its condition: what the next check compares with (once per crossing)."""
        self.db.conn.execute("UPDATE schedules SET last_reported_met = ? WHERE id = ?", (int(met), sid))

    def ran(self, sid: str, ok: bool | None) -> int:
        """Record a run; returns the failures in a row. ok=None is a run that finished but a check couldn't confirm
        (done_unverified, V4): neither a confirmed success that clears the count nor a failure that adds to it."""
        if ok is None:
            self.db.conn.execute("UPDATE schedules SET last_run = ? WHERE id = ?", (self.clock.now().isoformat(), sid))
            return int(self.db.conn.execute("SELECT failures FROM schedules WHERE id = ?", (sid,)).fetchone()[0])
        if ok:
            self.db.conn.execute("UPDATE schedules SET failures = 0, last_run = ? WHERE id = ?",
                                 (self.clock.now().isoformat(), sid))
            return 0
        self.db.conn.execute("UPDATE schedules SET failures = failures + 1, last_run = ? WHERE id = ?",
                             (self.clock.now().isoformat(), sid))
        return int(self.db.conn.execute("SELECT failures FROM schedules WHERE id = ?", (sid,)).fetchone()[0])
