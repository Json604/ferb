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
runs plan them as before.

A schedule can run several times a day (T-1066, owner 2026-11-07: "build all five"): fixed clock times beside its
rule's own, and N runs a day at random times in a window, drawn afresh each day from a stored seed and the date, so the
next run is always known (`Timing`). They follow the rule's days (every day for daily and "every N minutes", Monday to
Friday for weekdays, the rule's weekday for weekly, the first run's day for a one-time schedule). The timing is kept in
the schedule's recorded JSON (`TIMING`), never in its rule, so an older FERB still reads the rule and starts. Runs never
overlap (the daemon skips a run while the previous one is under way) and an until_met watch ends the whole schedule.

What a schedule says it does is checked by code (`missing_times`): every clock time its request names must be one of
the times its saved rule runs at (or a window's or its end's), so no request claims runs the rule doesn't make."""

import json
import random
import re
from dataclasses import dataclass, fields
from datetime import date, datetime, time, timedelta, timezone
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


CLOCK = re.compile(r"\s*([01]?\d|2[0-3]):([0-5]\d)\s*")
DAY_MINUTES = 24 * 60
TIMING = "timing"                   # the key of a schedule's several daily times (T-1066) in its recorded JSON


def clock_minutes(text: str) -> int:
    """"HH:MM" (24-hour) as minutes after midnight; ScheduleError for anything else."""
    m = CLOCK.fullmatch(str(text))
    if not m:
        raise ScheduleError(f"a time of day must be HH:MM (24-hour), not {text!r}")
    return int(m[1]) * 60 + int(m[2])


def hhmm(minutes: int) -> str:
    return f"{minutes // 60:02d}:{minutes % 60:02d}"


def said_list(items: list[str]) -> str:
    """"a", "a and b", "a, b and c"."""
    return items[0] if len(items) == 1 else ", ".join(items[:-1]) + " and " + items[-1]


def _field(item: Any, key: str) -> Any:
    return item[key] if isinstance(item, dict) else getattr(item, key)


@dataclass(frozen=True)
class Window:
    """`count` runs a day at random times from `start` to `end` (HH:MM, both included, `start` before `end`)."""
    count: int
    start: str
    end: str


@dataclass(frozen=True)
class Timing:
    """A schedule's runs a day beside its rule's own (T-1066): fixed clock times (`times`: every fixed time of the day,
    the rule's own included) and random times in windows, drawn afresh each day from `seed` and the date (never the
    same minute as another run that day), so the next run is always known. `start`: the first local day they count on
    (ISO date). Stored in the schedule's recorded JSON under TIMING, never in its rule."""
    times: tuple[str, ...] = ()
    windows: tuple[Window, ...] = ()
    seed: str = ""
    start: str = ""

    @classmethod
    def build(cls, times: Any, windows: Any, seed: str, start: date, own: int | None) -> "Timing | None":
        """From the planner's fields (`windows`: objects with count, start and end). `own`: the minute its rule itself
        runs at each day (daily, weekdays, weekly, once), kept among the fixed times; None for "every N minutes".
        None when there is nothing beside the rule's own. ScheduleError for a time that isn't HH:MM or a window that
        can't hold its count."""
        fixed = {clock_minutes(t) for t in times or ()}
        made = []
        for w in windows or ():
            count, first, last = int(_field(w, "count")), clock_minutes(_field(w, "start")), \
                clock_minutes(_field(w, "end"))
            if count <= 0:
                raise ScheduleError("a number of random times must be positive")
            if last <= first:
                raise ScheduleError(f"random times need a window that starts before it ends, not "
                                    f"{hhmm(first)}-{hhmm(last)}")
            if count > last - first + 1:
                raise ScheduleError(f"{count} random times don't fit between {hhmm(first)} and {hhmm(last)}")
            made.append(Window(count, hhmm(first), hhmm(last)))
        if not fixed - {own} and not made:
            return None
        kept = sorted(fixed | ({own} if own is not None else set()))
        return cls(tuple(hhmm(m) for m in kept), tuple(made), seed, start.isoformat())

    @classmethod
    def of(cls, data: Any) -> "Timing | None":
        """A stored timing; None for a schedule without one (every schedule made before T-1066)."""
        if not isinstance(data, dict):
            return None
        try:
            return cls(tuple(hhmm(clock_minutes(t)) for t in data.get("times") or ()),
                       tuple(Window(int(w["count"]), str(w["start"]), str(w["end"])) for w in data.get("windows") or ()),
                       str(data.get("seed") or ""), str(data.get("start") or ""))
        except (KeyError, TypeError, ValueError):
            return None

    def to_json(self) -> dict[str, Any]:
        return {"times": list(self.times), "windows": [dict(w.__dict__) for w in self.windows], "seed": self.seed,
                "start": self.start}

    def picks(self, day: date, taken: set[int]) -> list[int]:
        """The random minutes of `day`: per window, drawn from the seed and the date alone (the same every time it is
        asked), never a minute another run of the day has (`taken`, the fixed ones; each pick joins it)."""
        out: list[int] = []
        used = set(taken)
        for i, w in enumerate(self.windows):
            pool = [m for m in range(clock_minutes(w.start), clock_minutes(w.end) + 1) if m not in used]
            chosen = random.Random(f"{self.seed}:{day.isoformat()}:{i}").sample(pool, min(w.count, len(pool)))
            used |= set(chosen)
            out += chosen
        return sorted(out)


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
            return every_said(self.every_minutes or 0)
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

    @property
    def timing(self) -> Timing | None:
        """Its several daily times (T-1066); None for a schedule with only its rule's own."""
        return Timing.of(self.targets.get(TIMING))

    def describe(self) -> str:
        """Its timing, its end and what kind of watch it is (if it is one), in plain words."""
        return ", ".join(part for part in (self.when_said(), until_said(self.until) if self.until else "",
                                           WATCH_SAID[self.watch]) if part)

    def when_said(self) -> str:
        """When it runs, written by code from its saved rule and timing only (T-1066), never from its request."""
        timing, rule = self.timing, self.rule
        if timing is None:
            return rule.describe()
        if rule.repeat == "every":
            said = every_said(rule.every_minutes or 0)
            if timing.times:
                said += f", and at {said_list(list(timing.times))} each day"
        elif rule.repeat == "none":
            said = f"once, at {said_list(list(timing.times or (rule.at,)))}"
        else:
            days = {"daily": "every day", "weekdays": "weekdays"}.get(rule.repeat) or \
                f"every {DAYS[rule.weekday or 0].capitalize()}"
            said = f"{days} at {said_list(list(timing.times or (rule.at,)))}"
        if timing.windows:
            each = {"none": "", "daily": " each day", "every": " each day"}.get(rule.repeat, " each of those days")
            said += ", plus " + said_list([f"{w.count} at random times between {w.start} and {w.end}"
                                      for w in timing.windows]) + each
        return said + (", when you're at your Mac" if rule.when_active else "")

    def next_after(self, after: datetime) -> datetime | None:
        """Its first run strictly after `after` (its rule from its first run, and its timing's times), none past its
        end."""
        nxt = self._next(after)
        return None if nxt is None or (self.until is not None and nxt > self.until) else nxt

    def ends_after(self, after: datetime) -> bool:
        """Its end, not its rule, leaves it no run after `after`: it has run its course (ORCH-320)."""
        return self.until is not None and self._next(after) is not None and self.next_after(after) is None

    def _next(self, after: datetime) -> datetime | None:
        nxt = self.rule.next_after(after, self.first_run)
        extra = self._extra_after(after)
        return extra if extra is not None and (nxt is None or extra < nxt) else nxt

    def _zone(self) -> ZoneInfo:
        return ZoneInfo(self.rule.tz)

    def runs_on(self, day: date) -> bool:
        """Whether its rule runs on this local day: every day (daily, every N minutes), Monday to Friday (weekdays),
        its weekday (weekly), its first run's day (once)."""
        rule = self.rule
        return (rule.repeat in ("daily", "every") or (rule.repeat == "weekdays" and day.weekday() < 5)
                or (rule.repeat == "weekly" and day.weekday() == rule.weekday)
                or (rule.repeat == "none" and day == self.first_run.astimezone(self._zone()).date()))

    def _rule_minutes(self, day: date) -> set[int]:
        """The minutes of `day` its rule itself runs at (local)."""
        zone, rule = self._zone(), self.rule
        if rule.repeat == "every" and rule.every_minutes:
            start = datetime.combine(day, time(0, 0), tzinfo=zone).astimezone(timezone.utc)
            end = datetime.combine(day + timedelta(days=1), time(0, 0), tzinfo=zone).astimezone(timezone.utc)
            step = timedelta(minutes=rule.every_minutes)
            at = self.first_run if start <= self.first_run else \
                self.first_run + -(-(start - self.first_run) // step) * step
            out = set()
            while at < end:
                local = at.astimezone(zone)
                out.add(local.hour * 60 + local.minute)
                at += step
            return out
        if rule.repeat == "none":
            first = self.first_run.astimezone(zone)
            return {first.hour * 60 + first.minute} if first.date() == day else set()
        return {clock_minutes(rule.at)} if self.runs_on(day) else set()

    def _extra_minutes(self, day: date, timing: Timing) -> list[int]:
        """Its timing's minutes of `day`: the fixed times and that day's random picks (never a minute another run of
        the day has)."""
        fixed = {clock_minutes(t) for t in timing.times}
        return sorted(fixed | set(timing.picks(day, fixed | self._rule_minutes(day))))

    def _extra_after(self, after: datetime) -> datetime | None:
        timing = self.timing
        if timing is None:
            return None
        zone = self._zone()
        start = date.fromisoformat(timing.start) if timing.start else self.first_run.astimezone(zone).date()
        day = max(after.astimezone(zone).date(), start)
        for _ in range(9):
            if self.runs_on(day):
                for m in self._extra_minutes(day, timing):
                    at = datetime.combine(day, time(m // 60, m % 60), tzinfo=zone).astimezone(timezone.utc)
                    if at > after:
                        return at
            day += timedelta(days=1)
        return None

    def run_minutes(self) -> set[int]:
        """Every minute of the day it can run at, or that bounds a run (a window's start and end, its end's time): what
        a clock time in its request may name (`missing_times`)."""
        zone = self._zone()
        day = self.first_run.astimezone(zone).date()
        out = {m for i in range(8) for m in self._rule_minutes(day + timedelta(days=i))}
        if self.rule.repeat not in ("every", "none"):
            out.add(clock_minutes(self.rule.at))
        timing = self.timing
        if timing is not None:
            out |= {clock_minutes(t) for t in timing.times}
            out |= {clock_minutes(x) for w in timing.windows for x in (w.start, w.end)}
        if self.until is not None:
            end = self.until.astimezone(zone)
            out.add(end.hour * 60 + end.minute)
        return out


# A clock time as written: "5:20 AM", "6 am", "7.10pm", "12 PM" (12-hour, with a.m. or p.m.) or "18:00", "6:00" (a
# 24-hour HH:MM; one of 1-11 may also be its p.m. time). Shape only: which number is a time, never what it's about
_CLOCK_SAID = re.compile(r"(?<![\w:.])(\d{1,2})(?:[:.](\d{2}))?\s*([ap])(?:\.\s*m\.|\s*m\b)"
                         r"|(?<![\w:.])([01]?\d|2[0-3]):([0-5]\d)(?![\d:])", re.IGNORECASE)


def said_times(text: str) -> list[tuple[str, set[int]]]:
    """Each clock time written in `text`, with the minutes of the day it can be."""
    out = []
    for m in _CLOCK_SAID.finditer(text):
        if m[3]:
            hour, minute = int(m[1]), int(m[2] or 0)
            if not 1 <= hour <= 12 or minute > 59:
                continue
            out.append((m[0].strip(), {(hour % 12 + (12 if m[3].lower() == "p" else 0)) * 60 + minute}))
        else:
            hour, minute = int(m[4]), int(m[5])
            out.append((m[0], {hour * 60 + minute} | ({(hour + 12) * 60 + minute} if 1 <= hour <= 11 else set())))
    return out


def missing_times(request: str, runs: set[int]) -> list[str]:
    """The clock times `request` names that its schedule never runs at (T-1066, owner: "where is honesty"): a request
    may say only times its saved rule and timing make."""
    return list(dict.fromkeys(said for said, can_be in said_times(request) if not can_be & runs))


@dataclass(frozen=True)
class Plan:
    """A schedule as code worked it out from the planner's fields (T-1066), before anything is saved."""
    rule: Rule
    first: datetime                  # its rule's first run (the anchor of "every N minutes")
    booked: datetime                 # its first run: the earlier of that and its timing's first time
    return_after: datetime | None    # ORCH-317: a return to the Mac must come after this
    until: datetime | None
    timing: Timing | None
    schedule: Schedule               # as it would stand (its words and its times)
    missing: list[str]               # clock times its request names that it never runs at


def every_said(minutes: int) -> str:
    """An "every N minutes" repeat as a person says it: "every 6 hours", "every 3 days and 12 hours", "every week",
    "every 45 minutes" (owner 2026-11-09, a schedule card read "Every 5040 minutes")."""
    if minutes <= 0:
        return f"every {minutes} minutes"
    if minutes % (7 * DAY_MINUTES) == 0:
        weeks = minutes // (7 * DAY_MINUTES)
        return "every week" if weeks == 1 else f"every {weeks} weeks"
    days, rest = divmod(minutes, DAY_MINUTES)
    hours, mins = divmod(rest, 60)
    parts = [f"{n} {unit}{'' if n == 1 else 's'}" for n, unit in ((days, "day"), (hours, "hour"), (mins, "minute")) if n]
    if len(parts) == 1 and parts[0].startswith("1 "):
        return "every " + parts[0][2:]                  # every day, every hour, every minute
    return "every " + (parts[0] if len(parts) == 1 else ", ".join(parts[:-1]) + " and " + parts[-1])


def _own_minute(rule: Rule) -> int | None:
    return None if rule.repeat == "every" else clock_minutes(rule.at)


def _on_sequence(rule: Rule, first: datetime, times: list[str]) -> list[str]:
    """`times` without those "every N minutes" from its first run already makes every day."""
    n = rule.every_minutes or 0
    if rule.repeat != "every" or not n or DAY_MINUTES % n:
        return list(times)
    local = first.astimezone(ZoneInfo(rule.tz))
    anchor = local.hour * 60 + local.minute
    return [t for t in times if (clock_minutes(t) - anchor) % n]


def _planned(request: str, rule: Rule, first: datetime, until: datetime | None, timing: Timing | None,
             now: datetime, old: Schedule | None = None, return_after: datetime | None = None) -> Plan:
    targets = {k: v for k, v in (old.targets if old is not None else {}).items() if k != TIMING} | \
        ({TIMING: timing.to_json()} if timing is not None else {})
    s = Schedule(old.id if old is not None else "", request, old.own_words if old is not None else "", targets, rule,
                 None, None, old.state if old is not None else "active", 0, first,
                 old.watch if old is not None else "none", None, until, old.name if old is not None else "")
    extra = s._extra_after(now)
    booked = extra if extra is not None and extra < first else first
    return Plan(rule, first, booked, return_after, until, timing, s, missing_times(request, s.run_minutes()))


def plan_new(spec: Any, now: datetime, seed: str) -> Plan:
    """A new schedule from the planner's fields: code computes its first run, its end and its timing (the times of
    day beside `when`'s, the random windows); WhenError or ScheduleError (ValueErrors) say what doesn't work."""
    from ferb.clock.when import end_at, resolve, when_present
    present = bool(getattr(spec, "when_active", False))                 # ORCH-317: when they're at the Mac
    return_after = None
    if present:
        first, return_after = when_present(spec.when, spec.repeat, now, spec.every_minutes)
    else:
        first = resolve(spec.when, now)                                 # code computes the date, not the model
    ends = getattr(spec, "until", None)
    until = end_at(ends.when(), now) if ends is not None else None      # and the end (ORCH-320)
    rule = Rule.build(spec.repeat, first, spec.weekday, spec.every_minutes, when_active=present)
    if present and spec.repeat in ("daily", "weekdays", "weekly") and spec.when.time is None:
        # due from the start of a day its rule names: the first such day from the one `when` gave
        first = rule.next_after(first - timedelta(seconds=1), first) or first
    if first <= now:                    # "when I'm back at my Mac", said now: due now, fires at their return
        first = now + timedelta(seconds=1)
    zone = ZoneInfo(rule.tz)
    # its other times count from the day `when` named, else from today
    named = any(v is not None for v in (spec.when.day, spec.when.date, spec.when.in_days))
    start = (first if named else now).astimezone(zone).date()
    timing = Timing.build(_on_sequence(rule, first, list(getattr(spec, "times", []) or [])),
                          getattr(spec, "random", []) or [], seed, start, _own_minute(rule))
    return _planned(spec.request, rule, first, until, timing, now, return_after=return_after)


def plan_edit(old: Schedule, request: str, timing_spec: Any, now: datetime, seed: str) -> Plan:
    """A schedule changed in place (T-1066): its new instructions (`request`, "" keeps them) and its new timing
    (`timing_spec.repeat` "" keeps it). A new timing starts from now: "every N minutes" from its first time (times[0])
    or from now, the other rules at their times; its end and its watch stay."""
    words = request.strip() or old.request
    repeat = str(getattr(timing_spec, "repeat", "") or "")
    if not repeat:
        return _planned(words, old.rule, old.first_run, old.until, old.timing, now, old)
    zone = ZoneInfo(old.rule.tz)
    local_now = now.astimezone(zone)
    times = [hhmm(clock_minutes(t)) for t in getattr(timing_spec, "times", []) or []]

    def next_at(t: str) -> datetime:
        m = clock_minutes(t)
        at = datetime.combine(local_now.date(), time(m // 60, m % 60), tzinfo=zone)
        return (at if at > local_now else at + timedelta(days=1)).astimezone(timezone.utc)
    every = int(getattr(timing_spec, "every_minutes", 0) or 0)
    weekday = str(getattr(timing_spec, "weekday", "") or "") or None
    if repeat == "every":
        first = next_at(times[0]) if times else now + timedelta(minutes=every)
        rule = Rule.build(repeat, first, None, every or None, zone, old.rule.when_active)
    else:
        if not times:
            raise ScheduleError("say the time of day it runs")
        probe = Schedule("", words, "", {}, Rule.build(repeat, min(next_at(t) for t in times), weekday, None, zone),
                         None, None, "active", 0, min(next_at(t) for t in times))
        # the soonest of its times on a day its rule runs (a one-time change: the soonest of its times)
        first = min(at for i in range(8) for day in [local_now.date() + timedelta(days=i)]
                    if repeat == "none" or probe.runs_on(day) for t in times
                    for at in [datetime.combine(day, time(clock_minutes(t) // 60, clock_minutes(t) % 60), tzinfo=zone)
                               .astimezone(timezone.utc)] if at > now)
        rule = Rule.build(repeat, first, weekday, None, zone, old.rule.when_active)
    old_timing = old.timing
    timing = Timing.build(_on_sequence(rule, first, times), getattr(timing_spec, "random", []) or [],
                          old_timing.seed if old_timing is not None and old_timing.seed else seed,
                          local_now.date(), _own_minute(rule))
    return _planned(words, rule, first, old.until, timing, now, old)


class Schedules:
    def __init__(self, db: Database, clock: Clock, ids: IdSource) -> None:
        self.db, self.clock, self.ids = db, clock, ids

    def _row(self, r: Any) -> Schedule:
        rule = json.loads(r["rule_json"])
        first = datetime.fromisoformat(rule.pop("first"))
        known = {f.name for f in fields(Rule)}        # a key a newer FERB added is left for it
        return Schedule(r["id"], r["request"], r["own_words"], json.loads(r["intent_json"]),
                        Rule(**{k: v for k, v in rule.items() if k in known}),
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

    def edit(self, sid: str, request: str | None = None, rule: Rule | None = None, first: datetime | None = None,
             targets: dict[str, Any] | None = None, own_words: str = "") -> None:
        """Changes a schedule in place (T-1066): same id, runs and history. `request`: its new instructions; `rule`
        with `first` (its new first run, its anchor) and `targets` (its recorded JSON with the new timing): its new
        timing; `own_words`: the owner's words for the change, added to the words it rests on."""
        sets, args = [], []
        if request is not None:
            sets.append("request = ?")
            args.append(request)
        if rule is not None and first is not None:
            sets.append("rule_json = ?")
            args.append(json.dumps(rule.__dict__ | {"first": first.astimezone(timezone.utc).isoformat()}))
        if targets is not None:
            sets.append("intent_json = ?")
            args.append(json.dumps(targets))
        if own_words.strip():
            sets.append("own_words = own_words || ?")
            args.append("\n" + own_words)
        if sets:
            self.db.conn.execute(f"UPDATE schedules SET {', '.join(sets)} WHERE id = ?", (*args, sid))

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
