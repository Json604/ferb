"""Owns arithmetic for explicitly typed dates and times. Must not infer ambiguous owner intent or turn reminders into unauthorized actions.

When something should happen, in the owner's terms; code turns it into an absolute time (owner decision,
2026-09-30: the model never computes dates for reminders, schedules or sleeps).

The model fills only what the owner said:
- in_minutes: "in 20 minutes", "in 2 hours" (= 120) — an exact span from now (so it's right across a DST change);
- in_days: "in 3 days";
- day: "today", "tomorrow", "day_after_tomorrow", "end_of_month", a weekday ("friday": the next one — today counts
  while the time is still ahead) or "next_friday" (the Friday of next week, Monday-based);
- date: "MM-DD" or "YYYY-MM-DD" for a calendar date ("on 31 October"); without a year, the next one;
- time: "HH:MM", 24-hour, local ("at 5 pm" = "17:00"). A day requires an explicit time; a time without a day means
  the next one (today if still ahead, else tomorrow).
Which fields a fuzzy phrase maps to ("next Friday", "evening", "this weekend") is not a rule here: the model guesses
from context or follows the owner's stored meaning (a `time:<phrase>` preference); these fields are then exact.
Wall-clock times are local (the Mac's time zone): "tomorrow at 9" stays 09:00 across a daylight-saving change, and a
time that doesn't exist that day (skipped by the change) moves forward by the gap."""

import os
import re
from calendar import monthrange
from datetime import date, datetime, time, timedelta, timezone
from typing import Any
from zoneinfo import ZoneInfo

from pydantic import BaseModel

DAYS = ("monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday")
DAY_WORDS = ("today", "tomorrow", "day_after_tomorrow", "end_of_month", *DAYS, *(f"next_{d}" for d in DAYS))


class WhenError(ValueError):
    pass


class When(BaseModel):
    in_minutes: float | None = None
    in_days: int | None = None
    day: str | None = None
    date: str | None = None
    time: str | None = None


def _nullable(t: str, **extra: Any) -> dict[str, Any]:
    return {"anyOf": [{"type": t, **extra}, {"type": "null"}]}


WHEN_SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False,
    "required": ["in_minutes", "in_days", "day", "date", "time"],
    "properties": {"in_minutes": _nullable("number"), "in_days": _nullable("integer"),
                   "day": _nullable("string", enum=list(DAY_WORDS)), "date": _nullable("string"),
                   "time": _nullable("string")},
}
WHEN_HELP = ("when: only what the owner said — in_minutes (\"in 2 hours\" = 120), in_days, day (today / tomorrow / "
             "day_after_tomorrow / end_of_month / a weekday like \"friday\" / \"next_friday\" for next week's), date "
             "(\"MM-DD\" or \"YYYY-MM-DD\"), time (\"HH:MM\" 24-hour local); unused fields null. Never work out a date "
             "yourself: FERB computes it from the clock.")


def local_zone() -> ZoneInfo:
    import time as _t
    name = os.environ.get("TZ")
    if not name:
        try:
            name = os.readlink("/etc/localtime").split("zoneinfo/", 1)[1]
        except (OSError, IndexError):
            name = None
    try:
        return ZoneInfo(name) if name else ZoneInfo(_t.tzname[0])
    except Exception as exc:
        raise WhenError(f"local time zone unavailable: {name or _t.tzname[0]}") from exc


def _clock(text: str) -> time:
    m = re.fullmatch(r"\s*(\d{1,2}):(\d{2})\s*", text)
    if not m or int(m[1]) > 23 or int(m[2]) > 59:
        raise WhenError(f"time must be HH:MM (24-hour), not {text!r}")
    return time(int(m[1]), int(m[2]))


def _wall(d: date, t: time, zone: ZoneInfo) -> datetime:
    """A local wall time as an absolute instant. fold=0 (PEP 495): a time skipped by a DST change moves forward by the
    gap (02:30 -> 03:30); a time that happens twice is the first one."""
    return datetime.combine(d, t, tzinfo=zone).astimezone(timezone.utc)


def _day(w: When, today: date) -> date | None:
    if w.date:
        m = re.fullmatch(r"\s*(?:(\d{4})-)?(\d{1,2})-(\d{1,2})\s*", w.date)
        if not m:
            raise WhenError(f"date must be MM-DD or YYYY-MM-DD, not {w.date!r}")
        y, mo, dd = int(m[1]) if m[1] else today.year, int(m[2]), int(m[3])
        try:
            d = date(y, mo, dd)
            if not m[1] and d < today:
                d = date(y + 1, mo, dd)
        except ValueError:
            raise WhenError(f"there's no {w.date}") from None
        return d
    if w.in_days is not None:
        return today + timedelta(days=w.in_days)
    day = (w.day or "").lower()
    if not day:
        return None
    if day == "today":
        return today
    if day == "tomorrow":
        return today + timedelta(days=1)
    if day == "day_after_tomorrow":
        return today + timedelta(days=2)
    if day == "end_of_month":
        return today.replace(day=monthrange(today.year, today.month)[1])
    if day.startswith("next_") and day[5:] in DAYS:
        monday_next = today + timedelta(days=7 - today.weekday())
        return monday_next + timedelta(days=DAYS.index(day[5:]))
    if day in DAYS:
        return today + timedelta(days=(DAYS.index(day) - today.weekday()) % 7)
    raise WhenError(f"unknown day {w.day!r}")


def end_of_day(w: When, now: datetime, zone: ZoneInfo | None = None) -> datetime:
    """The end of the calendar day `w` names, its next local midnight (UTC), strictly after `now` (L36 §6: a remembered
    item's last day). Only one of day, date or in_days; never a time. A "MM-DD" already behind this year is over, not
    next year's (`_day` rolls it forward, which suits a reminder; scan m13)."""
    zone = zone or local_zone()
    if w.in_minutes is not None or w.time:
        raise WhenError("an end names a day, not a time")
    if sum(value is not None for value in (w.day, w.date, w.in_days)) != 1:
        raise WhenError("give exactly one calendar day source: day, date or in_days")
    today = now.astimezone(zone).date()
    d = _day(w, today)
    assert d is not None
    if w.date and not re.match(r"\s*\d{4}-", w.date) and d.year > today.year:
        d = d.replace(year=today.year)                            # the day this year, already behind: over
    end = _wall(d + timedelta(days=1), time(0, 0), zone)
    if end <= now:
        raise WhenError(f"that day is already over ({d:%a %d %b})")
    return end


def resolve(w: When, now: datetime, zone: ZoneInfo | None = None) -> datetime:
    """The absolute time (UTC) for `w`, strictly after `now`."""
    zone = zone or local_zone()
    if sum(value is not None for value in (w.day, w.date, w.in_days)) > 1:
        raise WhenError("give only one calendar day source: day, date or in_days")
    if w.in_minutes is not None:
        if w.in_minutes <= 0:
            raise WhenError("in_minutes must be more than 0")
        at = now.astimezone(timezone.utc) + timedelta(minutes=w.in_minutes)   # a real span, not wall time
        if w.day or w.date or w.in_days is not None or w.time:
            raise WhenError("give in_minutes on its own")
    else:
        local_now = now.astimezone(zone)
        d = _day(w, local_now.date())
        t = _clock(w.time) if w.time else None
        if d is None and t is None:
            raise WhenError("say when: a time, a day, a date or how long from now")
        if t is None:
            raise WhenError("a calendar day needs an explicit local time; resolve it from owner context")
        at = _wall(d or local_now.date(), t, zone)
        if at <= now:
            if d is None:                                         # a time alone: the next one
                at = _wall(local_now.date() + timedelta(days=1), t, zone)
            elif (w.day or "").lower() in DAYS:                   # "friday" at a time already gone: next week's
                at = _wall(d + timedelta(days=7), t, zone)
            elif (w.day or "").lower() == "end_of_month":
                nxt = (d + timedelta(days=1))
                at = _wall(nxt.replace(day=monthrange(nxt.year, nxt.month)[1]), t, zone)
    if at <= now:
        raise WhenError(f"that's in the past ({at.astimezone(zone):%a %d %b %H:%M})")
    return at.astimezone(timezone.utc)
