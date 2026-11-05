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
"standing" keeps watching. A watch survives restarts and updates as every schedule does.

A schedule can wait for the owner (ORCH-317, "when you're back at your Mac"): `Rule.when_active`. Its runs come due
as its rule says and each fires when the owner is at the Mac then, or when they're back (its timers are booked
`when_active`, ferb/clock/timers.py); the rule alone sets how often (no limit of its own).

A schedule can have an end (ORCH-320): the time code worked out from the end the owner's words gave it ("until Nov
3", "for 2 weeks"), stored beside it (`schedule_ends`, migration 0052). No run is booked after it; a schedule without
one (every schedule made before) runs until stopped.

A schedule records what every run uses (ORCH-313): its folders, whose identity, the worker type, the
owner's browser profile and the account, resolved by the turn that made it and stated then (`run_scope`, kept in its
recorded targets as `run_scope`). Each run's task uses those, never re-guessed; a schedule made before has none, and its
runs plan them as before."""

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


def until_said(end: datetime) -> str:
    """A schedule's end in the owner's words: "until Mon 03 Nov" for the end of a day (local midnight), else "until Mon
    03 Nov 18:00"."""
    local = end.astimezone(local_zone())
    if (local.hour, local.minute, local.second) == (0, 0, 0):
        return f"until {(local - timedelta(seconds=1)):%a %d %b}"
    return f"until {local:%a %d %b %H:%M}"


RUN_SCOPE = "run_scope"             # the key of what every run uses (ORCH-313) in a schedule's recorded targets


def scope_said(scope: dict[str, Any]) -> str:
    """What every run of a schedule uses (ORCH-313), in the owner's words, so they can correct it when it's made."""
    from pathlib import Path
    from ferb.runtimes.catalog import label
    home = str(Path.home())
    who = ("you" + (f", in your browser profile “{scope['profile']}”" if scope.get("profile") else "")
           if scope.get("identity") == "owner" else "FERB's own browser and accounts")
    parts = [f"as {who}" if scope.get("identity") == "owner" else who]
    if scope.get("account"):
        parts.append(f"account {scope['account']}")
    if scope.get("folders"):
        parts.append("in " + ", ".join("~" + f[len(home):] if f == home or f.startswith(home + "/") else f
                                       for f in scope["folders"]))
    if scope.get("worker_type") not in (None, "", "default"):
        parts.append(f"with {label(str(scope['worker_type']))}")
    return "Each run uses: " + "; ".join(parts) + "."


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
    when_active: bool = False    # ORCH-317: each run fires when the owner is at the Mac (a rule saved before: no)

    @classmethod
    def build(cls, repeat: str, first: datetime, weekday: str | None = None, every_minutes: int | None = None,
              tz: ZoneInfo | None = None, when_active: bool = False) -> "Rule":
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
        return cls(repeat, local.strftime("%H:%M"), wd, every_minutes if repeat == "every" else None, zone.key,
                   when_active)

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
        return self._rule() + (", when you're at your Mac" if self.when_active else "")

    def _rule(self) -> str:
        if self.repeat == "none":
            return "once"
        if self.repeat == "every":
            return f"every {self.every_minutes} minutes"
        at = "" if self.when_active and self.at == "00:00" else f" at {self.at}"   # due from the day's start
        if self.repeat == "weekly":
            return f"every {DAYS[self.weekday or 0].capitalize()}{at}"
        return f"{'every day' if self.repeat == 'daily' else 'weekdays'}{at}"


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
    until: datetime | None = None    # ORCH-320: no run after it; None runs until stopped
    name: str = ""                   # T-1055: its short name (`schedule_names`, migration 0060); "" for one made before

    @property
    def label(self) -> str:
        """Its heading (T-1055): its short name, else its request's first clause, cut (`names.shown_name`)."""
        from ferb.orchestrator.names import shown_name
        return shown_name(self.name, self.request)

    @property
    def run_scope(self) -> dict[str, Any] | None:
        """What every run uses (ORCH-313), recorded when it was made; None for a schedule made before."""
        scope = self.targets.get(RUN_SCOPE)
        return dict(scope) if isinstance(scope, dict) else None

    def describe(self) -> str:
        """Its rule, its end and what kind of watch it is (if it is one), in plain words."""
        return ", ".join(part for part in (self.rule.describe(), until_said(self.until) if self.until else "",
                                           WATCH_SAID[self.watch]) if part)

    def next_after(self, after: datetime) -> datetime | None:
        """Its first run strictly after `after` (its rule from its first run), none past its end."""
        nxt = self.rule.next_after(after, self.first_run)
        return None if nxt is None or (self.until is not None and nxt > self.until) else nxt

    def ends_after(self, after: datetime) -> bool:
        """Its end, not its rule, leaves it no run after `after`: it has run its course (ORCH-320)."""
        return self.until is not None and self.rule.next_after(after, self.first_run) is not None and \
            self.next_after(after) is None


class Schedules:
    def __init__(self, db: Database, clock: Clock, ids: IdSource) -> None:
        self.db, self.clock, self.ids = db, clock, ids

    def _row(self, r: Any) -> Schedule:
        rule = json.loads(r["rule_json"])
        first = datetime.fromisoformat(rule.pop("first"))
        return Schedule(r["id"], r["request"], r["own_words"], json.loads(r["intent_json"]), Rule(**rule),
                        datetime.fromisoformat(r["next_run"]) if r["next_run"] else None, r["timer_id"], r["state"],
                        int(r["failures"]), first, r["watch"],
                        None if r["last_reported_met"] is None else bool(r["last_reported_met"]),
                        datetime.fromisoformat(r["ends_at"]) if r["ends_at"] else None, r["name"] or "")

    def add(self, request: str, own_words: str, targets: dict[str, Any], rule: Rule, first: datetime,
            watch: str = "none", until: datetime | None = None, name: str = "") -> Schedule:
        if first <= self.clock.now():
            raise ScheduleError("the first run must be in the future")
        if watch not in WATCHES:
            raise ScheduleError(f"unknown watch {watch!r}")
        if until is not None and until < first:
            raise ScheduleError(f"it would end ({until_said(until)}) before its first run")
        sid = "sc-" + self.ids.new_id()[:10]
        self.db.conn.execute(
            "INSERT INTO schedules (id, request, own_words, intent_json, rule_json, next_run, state, created_at, "
            "watch) VALUES (?, ?, ?, ?, ?, ?, 'active', ?, ?)",
            (sid, request, own_words, json.dumps(targets), json.dumps(rule.__dict__ | {
                "first": first.astimezone(timezone.utc).isoformat()}),
             first.astimezone(timezone.utc).isoformat(), self.clock.now().isoformat(), watch))
        if until is not None:
            self.set_until(sid, until)
        if name.strip():
            self.set_name(sid, name)
        s = self.get(sid)
        assert s is not None
        return s

    def set_until(self, sid: str, until: datetime) -> None:
        """Its end (ORCH-320), replacing any earlier one."""
        self.db.conn.execute("INSERT INTO schedule_ends (schedule_id, ends_at) VALUES (?, ?) ON CONFLICT(schedule_id) "
                             "DO UPDATE SET ends_at = excluded.ends_at",
                             (sid, until.astimezone(timezone.utc).isoformat()))

    def set_name(self, sid: str, name: str) -> None:
        """Its short name (T-1055), replacing any earlier one: only a change the planner wrote a new name in."""
        self.db.conn.execute("INSERT INTO schedule_names (schedule_id, name) VALUES (?, ?) ON CONFLICT(schedule_id) "
                             "DO UPDATE SET name = excluded.name", (sid, " ".join(name.split())))

    _SELECT = ("SELECT s.*, e.ends_at, n.name FROM schedules s LEFT JOIN schedule_ends e ON e.schedule_id = s.id "
               "LEFT JOIN schedule_names n ON n.schedule_id = s.id")

    def get(self, sid: str) -> Schedule | None:
        r = self.db.conn.execute(f"{self._SELECT} WHERE s.id = ?", (sid,)).fetchone()
        return self._row(r) if r else None

    def all(self, states: tuple[str, ...] = ("active", "paused")) -> list[Schedule]:
        q = ",".join("?" * len(states))
        return [self._row(r) for r in self.db.conn.execute(
            f"{self._SELECT} WHERE s.state IN ({q}) ORDER BY s.next_run", states)]

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
