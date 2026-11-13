"""Fixtures for the Rust port of FERB's time (rust/crates/ferb-clock), made by the Python code itself (ENG-3).

    cd <repo> && uv run python rust/fixtures/clock/make.py

Runs ferb/clock (clock, when, context, timers, wait, tools) and the calendar arithmetic of
ferb/orchestrator/schedules.py on invented data, in a temporary FERB home (never ~/.ferb), with the clock replaced by
scripted fakes the way the Python tests do (TEST-508), and writes next to this file:

- recurrence.json: (a) rules, schedules and plans (rule, zone, now) -> next runs and the text Python composes, across
  DST gaps and overlaps, month ends, leap days, end dates and random windows, in many zones;
- when.json: (b) when.py's resolve / end_at / end_of_day / when_present and their error text;
- timers.json: (c) the timer store's scenarios (add, claim, fire, cancel, crash recovery): returns and rows;
  schedules_store.json: the schedule store's rows;
- waits.json: (d) wait.for with scripted events: wake reason, timing, result, live line;
- context.json: (e) the clock text given to models (header, footer, ages, time block);
- tools.json: time.remind_at / set_timer / sleep_until results;
and regenerates rust/crates/ferb-clock/src/generated.rs (Python 3.12's casefold and printable tables).

Every "now" is what Python's SystemClock gives (`datetime.now().astimezone()`: a fixed offset with the zone's
abbreviation), with TZ set to the case's zone so `astimezone()` and `local_zone()` read it. All data is invented and
the output is the same on every run.
"""

import asyncio
import json
import os
import random
import shutil
import sys
import tempfile
import time
import unicodedata
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any
from zoneinfo import ZoneInfo

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO))
HERE = Path(__file__).parent
CRATE = REPO / "rust" / "crates" / "ferb-clock"

TMP = Path(tempfile.mkdtemp(prefix="ferb-clock-fixture-"))
os.environ["HOME"] = str(TMP / "home")
os.environ["FERB_HOME"] = str(TMP / "home" / ".ferb")
(TMP / "home" / ".ferb").mkdir(parents=True)

from ferb.clock import context as ctxmod  # noqa: E402
from ferb.clock import timers as timers_mod  # noqa: E402
from ferb.clock import tools as tools_mod  # noqa: E402
from ferb.clock import wait as wait_mod  # noqa: E402
from ferb.clock import when as when_mod  # noqa: E402
from ferb.clock.when import When  # noqa: E402
from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.core.errors import Refused  # noqa: E402
from ferb.core.types import Identity, ToolContext  # noqa: E402
from ferb.orchestrator import schedules as sch  # noqa: E402
from ferb.store.db import Database  # noqa: E402

ZONES = ["UTC", "America/New_York", "America/Los_Angeles", "Europe/London", "Europe/Berlin", "Europe/Dublin",
         "Asia/Kolkata", "Asia/Kathmandu", "Australia/Sydney", "Australia/Lord_Howe", "Pacific/Auckland",
         "Pacific/Chatham", "America/St_Johns", "America/Sao_Paulo", "Asia/Tokyo", "America/Santiago"]

# local days near daylight-saving changes (2026, 2027), month ends and leap days
FOCUS = ["2026-03-08", "2026-03-29", "2026-04-05", "2026-09-06", "2026-09-27", "2026-10-04", "2026-10-25",
         "2026-11-01", "2027-03-14", "2027-03-28", "2027-04-04", "2027-10-03", "2027-10-31", "2027-11-07",
         "2026-01-31", "2026-02-28", "2026-04-30", "2026-12-31", "2028-02-28", "2028-02-29", "2027-02-28",
         "2026-06-15", "2026-11-13"]


def set_tz(name: str) -> None:
    os.environ["TZ"] = name
    time.tzset()


def system_now(local: datetime) -> datetime:
    """What SystemClock.now() returns at that instant under the current TZ: a fixed offset named by the zone."""
    return datetime.fromtimestamp(local.timestamp()).astimezone().replace(microsecond=local.microsecond)


def iso(v: datetime | None) -> str | None:
    return v.isoformat() if v is not None else None


def err(e: BaseException) -> dict[str, Any]:
    return {"error": type(e).__name__, "message": str(e)}


def write(name: str, data: Any) -> None:
    (HERE / name).write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n")


def when_json(w: When) -> dict[str, Any]:
    return {"in_minutes": w.in_minutes, "in_days": w.in_days, "day": w.day, "date": w.date, "time": w.time}


# ---------------------------------------------------------------- tables (generated.rs)

def generate_tables() -> None:
    fold, unprintable, word = [], [], []
    start = None
    for cp in range(0x110000):
        c = chr(cp)
        if c.isalnum() or c == "_":
            if word and word[-1][1] == cp - 1:
                word[-1][1] = cp
            else:
                word.append([cp, cp])
        if not 0xD800 <= cp <= 0xDFFF and c.casefold() != c:
            fold.append((cp, c.casefold()))
        bad = not c.isprintable()
        if bad and start is None:
            start = cp
        if not bad and start is not None:
            unprintable.append((start, cp - 1))
            start = None
    if start is not None:
        unprintable.append((start, 0x10FFFF))
    lines = ["//! Generated by rust/fixtures/clock/make.py from Python " + sys.version.split()[0]
             + f" (Unicode {unicodedata.unidata_version}): do not edit.", "",
             "/// `c.casefold()` for every character it changes.",
             "pub(crate) static CASEFOLD: &[(u32, &str)] = &["]
    for cp, s in fold:
        text = "".join("\\u{%04X}" % ord(x) for x in s)
        lines.append(f"    (0x{cp:04X}, \"{text}\"),")
    lines += ["];", "", "/// The ranges `str.isprintable()` is false for (what `repr` escapes).",
              "pub(crate) static UNPRINTABLE: &[(u32, u32)] = &["]
    for a, b in unprintable:
        lines.append(f"    (0x{a:04X}, 0x{b:04X}),")
    lines += ["];", "", "/// What `re`'s `\\w` matches in a `str` pattern (`c.isalnum() or c == \"_\"`).",
              "pub(crate) static WORD: &[(u32, u32)] = &["]
    for a, b in word:
        lines.append(f"    (0x{a:04X}, 0x{b:04X}),")
    lines += ["];", ""]
    (CRATE / "src" / "generated.rs").write_text("\n".join(lines))


# ---------------------------------------------------------------- (a) recurrence

def nows_for(zone: str, rng: random.Random) -> list[datetime]:
    z = ZoneInfo(zone)
    out = []
    for day in FOCUS:
        d = date.fromisoformat(day)
        for hh, mm in ((0, 30), (1, 30), (2, 30), (3, 0), (12, 0), (23, 59), (rng.randrange(24), rng.randrange(60))):
            local = datetime(d.year, d.month, d.day, hh, mm, tzinfo=z)
            out.append(local.astimezone(timezone.utc) + timedelta(seconds=rng.choice([0, 0, 17]),
                                                                  microseconds=rng.choice([0, 0, 0, 250000])))
    return out


def transitions(z: ZoneInfo) -> list[date]:
    """The local days of 2026-2027 on which the zone's UTC offset changes."""
    out, day = [], date(2026, 1, 1)
    while day < date(2028, 1, 1):
        a = datetime.combine(day, datetime.min.time(), z).utcoffset()
        b = datetime.combine(day + timedelta(days=1), datetime.min.time(), z).utcoffset()
        if a != b:
            out.append(day)
        day += timedelta(days=1)
    return out


def schedule_view(s: sch.Schedule, now: datetime) -> dict[str, Any]:
    runs, after = [], now
    for _ in range(6):
        try:
            nxt = s.next_after(after)
        except Exception as e:  # noqa: BLE001
            runs.append(err(e))
            break
        runs.append(iso(nxt))
        if nxt is None:
            break
        after = nxt
    try:
        ends = s.ends_after(now)
    except Exception as e:  # noqa: BLE001
        ends = err(e)
    return {"when_said": s.when_said(), "describe": s.describe(), "runs": runs, "ends_after": ends,
            "run_minutes": spans_of(sorted(s.run_minutes()))}


def spans_of(minutes: list[int]) -> list[list[int]]:
    """Sorted minutes as [first, last] runs of consecutive minutes (keeps the fixture small)."""
    out: list[list[int]] = []
    for m in minutes:
        if out and out[-1][1] == m - 1:
            out[-1][1] = m
        else:
            out.append([m, m])
    return out


def plan_json(p: sch.Plan, now: datetime) -> dict[str, Any]:
    return {"rule": dict(p.rule.__dict__), "first": iso(p.first), "booked": iso(p.booked),
            "return_after": iso(p.return_after), "until": iso(p.until),
            "timing": p.timing.to_json() if p.timing is not None else None, "missing": p.missing,
            "schedule": schedule_view(p.schedule, now)}


def spec_of(d: dict[str, Any]) -> Any:
    until = d.get("until")
    return SimpleNamespace(request=d["request"], when=When(**d["when"]), repeat=d["repeat"], weekday=d.get("weekday"),
                           every_minutes=d.get("every_minutes"), times=list(d.get("times") or []),
                           random=[SimpleNamespace(**w) for w in d.get("random") or []],
                           when_active=d.get("when_active", False),
                           until=SimpleNamespace(when=lambda u=until: When(**u)) if until is not None else None)


TIMES = ["00:00", "00:30", "01:30", "02:00", "02:30", "03:00", "05:20", "06:00", "09:00", "12:00", "17:45",
         "23:30", "23:59"]
WINDOWS = [(2, "05:00", "08:00"), (3, "01:30", "03:15"), (1, "22:00", "23:59"), (1, "05:00", "05:00"),
           (3, "00:00", "00:02"), (4, "00:00", "00:02"), (0, "06:00", "07:00"), (5, "02:00", "02:59")]
EVERY = [1, 7, 30, 45, 60, 90, 120, 360, 720, 1440, 1441, 2880, 5040, 10080, 43200]
REQUESTS = ["check the visa slots", "remind me at 5:20 AM and 6 am", "ping the build at 18:00 and 7.10pm",
            "water the plants at 6:00", "standup notes at 9:00 a.m.", "post the report at 12 PM",
            "look at 23:59 and 00:00", "every 6 hours from 6 am", "at 7 am, 1:30 pm and 25:00", "check at 3:00",
            "review at 2:30", "at 11.05pm sharp", "5 pm and 17:00", "1:61 is no time; 13:05 is"]


def random_when(rng: random.Random, today: date) -> dict[str, Any]:
    w: dict[str, Any] = {"in_minutes": None, "in_days": None, "day": None, "date": None, "time": None}
    pick = rng.randrange(10)
    if pick == 0:
        w["in_minutes"] = rng.choice([1, 5, 30, 90.5, 0.25, 1440, 60 * 24 * 40])
    elif pick == 1:
        w["in_days"], w["time"] = rng.choice([0, 1, 3, 30]), rng.choice(TIMES)
    elif pick == 2:
        w["day"] = rng.choice(when_mod.DAY_WORDS)
        w["time"] = rng.choice(TIMES)
    elif pick == 3:
        d = today + timedelta(days=rng.randrange(-20, 60))
        w["date"] = rng.choice([f"{d.month:02d}-{d.day:02d}", d.isoformat(), f"{d.month}-{d.day}"])
        w["time"] = rng.choice(TIMES)
    else:
        w["time"] = rng.choice(TIMES)
    return w


def recurrence() -> dict[str, Any]:
    rng = random.Random(20261113)
    rules, plans, edits = [], [], []
    for zone in ZONES:
        set_tz(zone)
        z = ZoneInfo(zone)
        for instant in nows_for(zone, rng)[::4]:
            now = system_now(instant)
            # rules by themselves: daily / weekdays / weekly at a time, every N minutes from an anchor
            for repeat in rng.sample(["daily", "weekdays", "weekly", "every", "every", "none"], 2):
                at = rng.choice(TIMES)
                every = rng.choice(EVERY)
                try:
                    first = when_mod.resolve(When(time=at), now)
                    rule = sch.Rule.build(repeat, first, rng.choice([None, "monday", "Friday", "sunday", "noday"]),
                                          every if repeat == "every" else None, when_active=rng.random() < 0.2)
                except Exception as e:  # noqa: BLE001
                    rules.append({"zone": zone, "now": iso(now), "repeat": repeat, "at": at, "error": err(e)})
                    continue
                anchor = first + timedelta(minutes=rng.choice([0, 0, -90, 300]))
                after = now + timedelta(minutes=rng.choice([0, 0, 1, -5, 600]))
                seq, cur = [], after
                for _ in range(4):
                    try:
                        nxt = rule.next_after(cur, anchor)
                    except Exception as e:  # noqa: BLE001
                        seq.append(err(e))
                        break
                    seq.append(iso(nxt))
                    if nxt is None:
                        break
                    cur = nxt
                rules.append({"zone": zone, "now": iso(now), "rule": dict(rule.__dict__), "anchor": iso(anchor),
                              "after": iso(after), "next": seq, "describe": rule.describe()})
            # plans from the planner's fields
            for _ in range(1):
                today = now.astimezone(z).date()
                repeat = rng.choice(sch.REPEATS)
                spec = {"request": rng.choice(REQUESTS), "when": random_when(rng, today), "repeat": repeat,
                        "weekday": rng.choice([None, None, "tuesday", "Saturday"]),
                        "every_minutes": rng.choice(EVERY) if repeat == "every" else rng.choice([None, None, 15]),
                        "times": rng.sample(TIMES, rng.choice([0, 0, 1, 2, 4])),
                        "random": [dict(zip(("count", "start", "end"), rng.choice(WINDOWS)))
                                   for _ in range(rng.choice([0, 0, 0, 1, 1, 2]))],
                        "when_active": rng.random() < 0.25,
                        "until": rng.choice([None, None, {"in_minutes": None, "in_days": 14, "day": None, "date": None,
                                                          "time": None},
                                             {"in_minutes": None, "in_days": None, "day": "friday", "date": None,
                                              "time": "18:00"},
                                             {"in_minutes": 600, "in_days": None, "day": None, "date": None,
                                              "time": None}])}
                if spec["when_active"] and rng.random() < 0.6:
                    spec["when"]["time"] = None
                    spec["when"]["in_minutes"] = None
                seed = f"seed-{rng.randrange(10**6)}"
                case = {"zone": zone, "now": iso(now), "spec": spec, "seed": seed}
                try:
                    plan = sch.plan_new(spec_of(spec), now, seed)
                    case["plan"] = plan_json(plan, now)
                except Exception as e:  # noqa: BLE001
                    case["plan"] = err(e)
                    plans.append(case)
                    continue
                plans.append(case)
                if rng.random() < 0.5:
                    old = plan.schedule
                    old = sch.Schedule("sc-1", old.request, "words", old.targets, old.rule, None, None,
                                       rng.choice(["active", "paused"]), 0, plan.first, "none", None, plan.until, "")
                    rep = rng.choice(["", "daily", "weekdays", "weekly", "every", "none"])
                    tspec = {"repeat": rep, "times": rng.sample(TIMES, rng.choice([0, 1, 2, 3])),
                             "every_minutes": rng.choice(EVERY) if rep == "every" else 0,
                             "weekday": rng.choice(["", "thursday"]),
                             "random": [{"count": 2, "start": "06:00", "end": "09:00"}] if rng.random() < 0.3 else []}
                    later = now + timedelta(hours=rng.choice([1, 30, 200]))
                    e_case = {"zone": zone, "now": iso(later), "old": {
                        "request": old.request, "targets": old.targets, "rule": dict(old.rule.__dict__),
                        "first": iso(old.first_run), "until": iso(old.until), "state": old.state},
                        "request": rng.choice(["", "check the visa slots twice", "at 9:00 and 21:00"]),
                        "timing": tspec, "seed": "edit-seed"}
                    try:
                        e_case["plan"] = plan_json(sch.plan_edit(old, e_case["request"], SimpleNamespace(
                            repeat=tspec["repeat"], times=tspec["times"], every_minutes=tspec["every_minutes"],
                            weekday=tspec["weekday"], random=[SimpleNamespace(**w) for w in tspec["random"]]),
                            later, "edit-seed"), later)
                    except Exception as e:  # noqa: BLE001
                        e_case["plan"] = err(e)
                    edits.append(e_case)
    # every daylight-saving change of 2026-2027 in each zone, at wall times around it: daily, weekly and
    # hourly rules from the evening before and from the change's own day
    for zone in ZONES:
        set_tz(zone)
        z = ZoneInfo(zone)
        for day in transitions(z):
            for hh, mm in ((0, 30), (1, 30), (2, 0), (2, 30), (2, 45), (3, 0)):
                at = f"{hh:02d}:{mm:02d}"
                for start in (datetime.combine(day - timedelta(days=1), datetime.min.time(), z).replace(hour=20),
                              datetime.combine(day, datetime.min.time(), z)):
                    now = system_now(start.astimezone(timezone.utc))
                    for repeat, every in (("daily", None), ("weekly", None), ("every", 60)):
                        first = when_mod.resolve(When(time=at), now)
                        rule = sch.Rule.build(repeat, first, None, every)
                        seq, cur = [], now
                        for _ in range(3):
                            nxt = rule.next_after(cur, first)
                            seq.append(iso(nxt))
                            if nxt is None:
                                break
                            cur = nxt
                        rules.append({"zone": zone, "now": iso(now), "rule": dict(rule.__dict__),
                                      "anchor": iso(first), "after": iso(now), "next": seq,
                                      "describe": rule.describe(), "dst": True})
    # text helpers
    said = ["check at 5:20 AM, 6 am, 7.10pm and 12 PM", "at 18:00 and 6:00", "12:00 noon, 0:05, 24:00, 13:61",
            "v1.10:30 and 10:300", "at 7 a.m. and 8 p. m. and 9 P.M.", "x5:20pm y 10:15 AM", "ratio 3:45:10",
            "at ١٢:٣٠ and 0٩:15", "at 9:00. then 10.30am", "13 pm, 0 am, 12 am, 12:00 am", "1:5 pm and 01:05",
            "since 2026-11-07 06:00", "06:00:00 or 6:00:", "times: 5.20am/6.40PM", "Meet at 11.59 P.M"]
    said_cases = []
    for text in said:
        said_cases.append({"text": text, "said": [[s, sorted(m)] for s, m in sch.said_times(text)],
                           "missing": sch.missing_times(text, {360, 1080, 720})})
    every = [{"minutes": m, "said": sch.every_said(m)} for m in
             [-5, 0, 1, 2, 45, 59, 60, 61, 90, 120, 360, 1439, 1440, 1441, 1500, 2880, 4320, 5040, 10080, 20160,
              10081, 11520, 43200, 525600]]
    clock = []
    for text in ["06:00", "6:00", " 23:59 ", "24:00", "0:00", "00:5", "12:60", "1٢:30", "٢3:00", "19:0٥", "", "ab",
                 "07:30\n", "\t9:15", "2:3", "123:00", "29:00", "20:00"]:
        try:
            clock.append({"text": text, "minutes": sch.clock_minutes(text)})
        except Exception as e:  # noqa: BLE001
            clock.append({"text": text, **err(e)})
    picks = []
    for i in range(80):
        windows = [sch.Window(rng.choice([1, 2, 5, 30]), rng.choice(["05:00", "00:00", "12:00"]),
                              rng.choice(["05:04", "08:00", "23:59"])) for _ in range(rng.choice([1, 2]))]
        windows = [w for w in windows if sch.clock_minutes(w.end) > sch.clock_minutes(w.start)] or \
            [sch.Window(2, "05:00", "08:00")]
        t = sch.Timing((), tuple(windows), rng.choice(["", "s", "seed-a", "ü-seed", f"x{i}"]), "")
        day = date(2026, 1, 1) + timedelta(days=rng.randrange(1500))
        taken = set(rng.sample(range(1440), rng.choice([0, 1, 5])))
        picks.append({"seed": t.seed, "day": day.isoformat(), "windows": [dict(w.__dict__) for w in windows],
                      "taken": sorted(taken), "picks": t.picks(day, taken)})
    untils = []
    for zone in ("Asia/Kolkata", "America/New_York", "Australia/Lord_Howe"):
        set_tz(zone)
        for instant in ("2026-11-02T18:30:00+00:00", "2026-11-03T05:00:00+00:00", "2026-03-08T07:00:00+00:00",
                        "2026-10-04T13:30:00+00:00", "2026-12-31T23:59:59+00:00"):
            untils.append({"zone": zone, "end": instant, "said": sch.until_said(datetime.fromisoformat(instant))})
    return {"rules": rules, "plans": plans, "edits": edits, "said_times": said_cases, "every_said": every,
            "clock_minutes": clock, "picks": picks, "until_said": untils, "watch_said": sch.WATCH_SAID}


# ---------------------------------------------------------------- (b) when.py

def when_cases() -> dict[str, Any]:
    rng = random.Random(7)
    out = []
    odd_dates = ["02-29", "2027-02-29", "2028-02-29", "13-01", "00-10", "1-5", " 12-31 ", "9999-12-31", "0001-01-01",
                 "2026-1-1", "12/31", "٠٢-٢٨", "2026-10-23", "10-23", "01-02", "2026-11-13", "11-13", "it's 5"]
    odd_times = ["9:00", " 09:05 ", "24:00", "23:60", "7", "7pm", "١٧:٣٠", "17:5", "", "x'y", "a\"b'c", "\n"]
    for zone in ZONES:
        set_tz(zone)
        for instant in nows_for(zone, rng)[::20]:
            now = system_now(instant)
            today = now.astimezone(ZoneInfo(zone)).date()
            samples = [random_when(rng, today) for _ in range(3)]
            samples.append({"in_minutes": None, "in_days": None, "day": rng.choice(when_mod.DAY_WORDS + ("FRIDAY",
                            "someday", "Next_Monday")), "date": None, "time": rng.choice(TIMES + odd_times)})
            samples.append({"in_minutes": None, "in_days": None, "day": None, "date": rng.choice(odd_dates),
                            "time": rng.choice([None, "10:00", "00:00"])})
            samples.append({"in_minutes": None, "in_days": rng.choice([None, 0, 2, 400, -3]), "day": None,
                            "date": None, "time": None})
            samples.append({"in_minutes": rng.choice([0, -1, 2.5, None]), "in_days": None,
                            "day": rng.choice([None, "today"]), "date": None, "time": rng.choice([None, "08:00"])})
            for w in samples:
                case: dict[str, Any] = {"zone": zone, "now": iso(now), "when": w}
                for name, fn in (("resolve", lambda w: when_mod.resolve(When(**w), now)),
                                 ("end_at", lambda w: when_mod.end_at(When(**w), now)),
                                 ("end_of_day", lambda w: when_mod.end_of_day(When(**w), now))):
                    try:
                        case[name] = iso(fn(w))
                    except Exception as e:  # noqa: BLE001
                        case[name] = err(e)
                for repeat in ("none", "daily", "every"):
                    every = rng.choice([None, 0, 30, 1440])
                    try:
                        due, ret = when_mod.when_present(When(**w), repeat, now, every)
                        case[f"present_{repeat}"] = {"every": every, "due": iso(due), "return_after": iso(ret)}
                    except Exception as e:  # noqa: BLE001
                        case[f"present_{repeat}"] = {"every": every, **err(e)}
                out.append(case)
    # wall times skipped or repeated by each daylight-saving change, asked for the evening before
    for zone in ZONES:
        set_tz(zone)
        z = ZoneInfo(zone)
        for day in transitions(z):
            now = system_now(datetime.combine(day - timedelta(days=1), datetime.min.time(), z).replace(hour=20)
                             .astimezone(timezone.utc))
            for at in ("01:30", "02:00", "02:30", "02:45", "03:00"):
                for w in ({"time": at}, {"day": "tomorrow", "time": at},
                          {"date": f"{day.month:02d}-{day.day:02d}", "time": at}):
                    full = {"in_minutes": None, "in_days": None, "day": None, "date": None, "time": None} | w
                    case = {"zone": zone, "now": iso(now), "when": full}
                    for name, fn in (("resolve", when_mod.resolve), ("end_at", when_mod.end_at),
                                     ("end_of_day", when_mod.end_of_day)):
                        try:
                            case[name] = iso(fn(When(**full), now))
                        except Exception as e:  # noqa: BLE001
                            case[name] = err(e)
                    out.append(case)
    # overflow and edge spans
    set_tz("UTC")
    now = system_now(datetime(2026, 11, 13, 9, 30, tzinfo=timezone.utc))
    for w in ({"in_days": 10**9}, {"in_days": 3_000_000, "time": "10:00"}, {"in_minutes": 1e15},
              {"in_minutes": 0.0000001}, {"in_minutes": 1.0000005}, {"in_minutes": 2.5e-8}, {"date": "9999-12-31",
                                                                                              "time": "23:59"},
              {"in_minutes": 4_000_000_000}):
        full = {"in_minutes": None, "in_days": None, "day": None, "date": None, "time": None} | w
        case = {"zone": "UTC", "now": iso(now), "when": full}
        for name, fn in (("resolve", when_mod.resolve), ("end_at", when_mod.end_at),
                         ("end_of_day", when_mod.end_of_day)):
            try:
                case[name] = iso(fn(When(**full), now))
            except Exception as e:  # noqa: BLE001
                case[name] = err(e)
        out.append(case)
    return {"cases": out, "schema": when_mod.WHEN_SCHEMA, "help": when_mod.WHEN_HELP,
            "day_words": list(when_mod.DAY_WORDS)}


# ---------------------------------------------------------------- (e) context

def context_cases() -> list[dict[str, Any]]:
    rng = random.Random(3)
    out = []
    spans = [0, 1, 59, 60, 61, 299, 300, 3599, 3600, 3661, 86399, 86400, 90061, 172799, 172800, 10**7]
    for zone in ("UTC", "Asia/Kolkata", "America/New_York", "Australia/Lord_Howe", "Asia/Kathmandu"):
        set_tz(zone)
        for instant in nows_for(zone, rng)[::9]:
            now = system_now(instant)
            for _ in range(3):
                started = now - timedelta(seconds=rng.choice(spans)) if rng.random() < 0.7 else None
                deadline = now + timedelta(seconds=rng.choice(spans) * rng.choice([1, -1])) \
                    if rng.random() < 0.6 else None
                active = now - timedelta(seconds=rng.choice(spans)) if rng.random() < 0.7 else None
                trail = now - timedelta(seconds=rng.choice(spans) + 0.4) if rng.random() < 0.5 else None
                ages = {f"m{i}": (now - timedelta(seconds=rng.choice(spans) * rng.choice([1, 1, -1]))).astimezone(
                    rng.choice([timezone.utc, ZoneInfo("Europe/Berlin"), timezone(timedelta(hours=-3))]))
                    for i in range(rng.choice([0, 1, 3]))}
                since = rng.choice([None, [], ["a task ended", "two mails arrived"]])
                out.append({"zone": zone, "now": iso(now), "now_name": now.tzname(), "task_started": iso(started),
                            "deadline": iso(deadline), "owner_last_active": iso(active),
                            "last_trail_event": iso(trail), "ages": {k: iso(v) for k, v in ages.items()},
                            "since_last_talk": since,
                            "header": ctxmod.header(now, started, deadline, active, trail),
                            "footer": ctxmod.footer(now, started, deadline),
                            "block": ctxmod.time_block(now, started, deadline, active, trail, ages, since)})
    # the nows the tests use: UTC and an unnamed fixed offset
    for now in (datetime(2026, 1, 1, 9, 0, tzinfo=timezone.utc),
                datetime(2026, 5, 2, 4, 5, 6, tzinfo=timezone(timedelta(hours=5, minutes=30))),
                datetime(2026, 5, 2, 23, 5, 6, tzinfo=timezone(timedelta(hours=-3), "BRT"))):
        out.append({"zone": "UTC", "now": iso(now), "now_name": now.tzname(), "task_started": None,
                    "deadline": None, "owner_last_active": None, "last_trail_event": None, "ages": {},
                    "since_last_talk": None, "header": ctxmod.header(now), "footer": ctxmod.footer(now),
                    "block": ctxmod.time_block(now)})
    return out


# ---------------------------------------------------------------- (c) timers

class ScriptClock:
    """A fixed-offset wall clock that moves only when told (IST, as the Mac's SystemClock gives it)."""

    def __init__(self) -> None:
        self.t = datetime(2026, 11, 13, 9, 0, tzinfo=timezone(timedelta(hours=5, minutes=30), "IST"))
        self.mono = 1000.0

    def now(self) -> datetime:
        return self.t

    def monotonic(self) -> float:
        return self.mono


TIMER_TABLES = ("timers", "timer_when_active", "timer_names", "timer_repeats", "timer_deliveries")
SCHEDULE_TABLES = ("schedules", "schedule_ends", "schedule_names")


def dump(db: Database, tables: tuple[str, ...]) -> dict[str, Any]:
    out = {}
    for t in tables:
        cur = db.conn.execute(f"SELECT * FROM {t} ORDER BY rowid")
        cols = [c[0] for c in cur.description]
        out[t] = [dict(zip(cols, r)) for r in cur.fetchall()]
    return out


def timer_json(t: Any) -> Any:
    if t is None:
        return None
    return {"id": t.id, "kind": t.kind, "fire_at": iso(t.fire_at), "message": t.message, "task_id": t.task_id,
            "request_id": t.request_id, "state": t.state, "overdue_s": t.overdue_s, "name": t.name,
            "created_at": iso(t.created_at)}


def dt(text: str | None) -> datetime | None:
    return datetime.fromisoformat(text) if text is not None else None


def timer_scenarios() -> list[dict[str, Any]]:
    D = "2026-11-13T"
    scenarios = {
        "add_get_pending_cancel": [
            {"op": "add", "kind": "reminder", "fire_at": D + "10:00:00+05:30", "message": "stretch",
             "request_id": "r1", "name": "  Stretch   break "},
            {"op": "add", "kind": "timer", "fire_at": D + "04:00:30.500000+00:00", "message": "tea", "name": ""},
            {"op": "add", "kind": "sleep", "fire_at": D + "04:10:00+00:00", "task_id": "t1"},
            {"op": "add", "kind": "schedule", "fire_at": D + "05:00:00+00:00", "message": "sc-1"},
            {"op": "add", "kind": "reminder", "fire_at": "2026-11-13T10:00:00", "message": "naive"},
            {"op": "add", "kind": "reminder", "fire_at": D + "03:00:00-02:00", "message": "late", "when_active": True,
             "return_after": "2026-11-13T09:00:00"},
            {"op": "pending"}, {"op": "pending", "kinds": ["sleep", "schedule"]},
            {"op": "get", "id": "tm-t-000002"}, {"op": "get", "id": "tm-nope"},
            {"op": "cancel", "id": "tm-t-000001"}, {"op": "cancel", "id": "tm-t-000001"},
            {"op": "cancel", "id": "tm-t-000003"},
            {"op": "add", "kind": "sleep", "fire_at": D + "06:00:00+00:00", "task_id": "t2"},
            {"op": "add", "kind": "sleep", "fire_at": D + "06:00:00+00:00", "task_id": "t1"},
            {"op": "add", "kind": "reminder", "fire_at": D + "06:00:00+00:00", "task_id": "t1"},
            {"op": "cancel_task", "task_id": "t1"}, {"op": "cancel_all_sleeps"},
            {"op": "when_active", "id": "tm-t-000005"}, {"op": "when_active", "id": "tm-t-000002"},
            {"op": "next_due_in"},
        ],
        "claim_fire_ack_and_crash_recovery": [
            {"op": "add", "kind": "reminder", "fire_at": D + "03:30:00+00:00", "message": "a"},
            {"op": "add", "kind": "reminder", "fire_at": D + "03:30:00.250000+00:00", "message": "b"},
            {"op": "add", "kind": "timer", "fire_at": D + "03:31:00+00:00", "message": "c"},
            {"op": "add", "kind": "schedule", "fire_at": D + "03:29:00+00:00", "message": "sc-x"},
            {"op": "next_due_in"},
            {"op": "claim_due"},
            {"op": "advance", "s": 0.25},
            {"op": "claim_due"},
            {"op": "ack", "id": "tm-t-000001"},
            {"op": "advance", "s": 3600},
            {"op": "claim_due"},                                 # claims a crash left: handed over again
            {"op": "mark_handled", "id": "tm-t-000002"}, {"op": "mark_handled", "id": "tm-t-000002"},
            {"op": "delivered", "id": "tm-t-000002"}, {"op": "delivered", "id": "tm-t-000003"},
            {"op": "tick", "fail": ["tm-t-000003"]},
            {"op": "tick", "fail": []},
            {"op": "ack", "id": "tm-t-000001"},
            {"op": "cancel", "id": "tm-t-000002"},
            {"op": "next_due_in"},
            {"op": "add", "kind": "reminder", "fire_at": D + "05:00:00+00:00", "message": "next"},
            {"op": "next_due_in"}, {"op": "advance", "s": 0.4}, {"op": "next_due_in"},
            {"op": "add", "kind": "reminder", "fire_at": D + "04:30:20.100000+00:00", "message": "soon"},
            {"op": "next_due_in"},
        ],
        "when_active_held_and_return": [
            {"op": "presence", "away": True, "since": None},
            {"op": "add", "kind": "reminder", "fire_at": D + "03:00:00+00:00", "message": "held", "when_active": True},
            {"op": "add", "kind": "schedule", "fire_at": D + "03:15:00+00:00", "message": "sc-2", "when_active": True,
             "return_after": D + "03:20:00+00:00"},
            {"op": "add", "kind": "reminder", "fire_at": D + "03:10:00+00:00", "message": "plain"},
            {"op": "next_due_in"},
            {"op": "claim_due"},
            {"op": "advance", "s": 3600},
            {"op": "presence", "away": False, "since": D + "03:10:00+00:00"},
            {"op": "next_due_in"},
            {"op": "claim_due"},
            {"op": "presence", "away": False, "since": D + "04:50:00+00:00"},
            {"op": "claim_due"},
            {"op": "presence", "away": False, "since": None},
            {"op": "claim_due"},
        ],
        "on_change_errors_and_repeats": [
            {"op": "on_change", "fail": True},
            {"op": "add", "kind": "timer", "fire_at": D + "03:35:00+00:00", "message": "pasta", "name": "Pasta"},
            {"op": "on_change", "fail": False},
            {"op": "advance", "s": 400},
            {"op": "tick", "fail": []},
            {"op": "add", "kind": "timer", "fire_at": D + "03:45:00+00:00", "message": "pasta", "name": "Pasta",
             "repeat_of": "tm-t-000001"},
            {"op": "repeated_by", "id": "tm-t-000001"}, {"op": "repeat_of", "id": "tm-t-000002"},
            {"op": "repeated_by", "id": "tm-t-000002"}, {"op": "repeat_of", "id": "tm-t-000001"},
            {"op": "add", "kind": "sleep", "fire_at": D + "03:40:00+00:00", "task_id": "t9"},
            {"op": "advance", "s": 3600},
            {"op": "tick", "fail": []},
            {"op": "tick_error"},
        ],
    }
    out = []
    for name, steps in scenarios.items():
        folder = Path(tempfile.mkdtemp(dir=TMP))
        db = Database(folder / "ferb.db")
        db.migrate()
        clock, tokens = ScriptClock(), iter(f"{n:016x}" for n in range(1, 10**6))
        timers_mod.secrets.token_hex = lambda n=8: next(tokens)  # type: ignore[assignment]
        changes: list[Any] = []
        errors: list[Any] = []
        t = timers_mod.Timers(db, clock, SequentialIds("t"))
        t.on_error = lambda timer, e: errors.append([timer.id if timer is not None else None, str(e)])
        fail_ids: set[str] = set()

        async def consume(timer: Any) -> None:
            if timer.id in fail_ids:
                raise RuntimeError(f"consumer refused {timer.id}")
        t.consume = consume
        record = []
        for step in steps:
            op = step["op"]
            ret: Any = None
            try:
                if op == "add":
                    ret = timer_json(t.add(step["kind"], dt(step["fire_at"]), step.get("message", ""),
                                           step.get("task_id"), step.get("request_id"),
                                           when_active=step.get("when_active", False),
                                           return_after=dt(step.get("return_after")), name=step.get("name", ""),
                                           repeat_of=step.get("repeat_of")))
                elif op == "get":
                    ret = timer_json(t.get(step["id"]))
                elif op == "pending":
                    ret = [timer_json(x) for x in (t.pending(tuple(step["kinds"])) if "kinds" in step else
                                                   t.pending())]
                elif op == "cancel":
                    ret = t.cancel(step["id"])
                elif op == "cancel_task":
                    ret = t.cancel_task(step["task_id"])
                elif op == "cancel_all_sleeps":
                    ret = t.cancel_all_sleeps()
                elif op == "when_active":
                    ret = t.when_active(step["id"])
                elif op == "next_due_in":
                    ret = t.next_due_in()
                elif op == "claim_due":
                    ret = [timer_json(x) for x in t.claim_due()]
                elif op == "ack":
                    t.ack(step["id"])
                elif op == "advance":
                    clock.t += timedelta(seconds=step["s"])
                    clock.mono += step["s"]
                elif op == "mark_handled":
                    ret = timers_mod.mark_handled(db, step["id"], clock.now())
                elif op == "delivered":
                    ret = t.delivered(step["id"])
                elif op == "tick":
                    fail_ids.clear()
                    fail_ids.update(step["fail"])
                    ret = asyncio.run(t.tick())
                elif op == "tick_error":
                    # the daemon's loop: a check that fails is reported and retried (run() catches it)
                    db.conn.execute("ALTER TABLE timer_when_active RENAME TO twa_gone")
                    try:
                        asyncio.run(t.tick())
                    except Exception as e:  # noqa: BLE001
                        t._report(None, e)
                        ret = "reported"
                    db.conn.execute("ALTER TABLE twa_gone RENAME TO timer_when_active")
                elif op == "presence":
                    away, since = step["away"], dt(step["since"])
                    t.owner_away = lambda a=away: a
                    t.present_since = lambda s=since: s
                elif op == "on_change":
                    def told(timer: Any, state: str, fail: bool = step["fail"]) -> None:
                        changes.append([timer.id, timer.kind, state])
                        if fail:
                            raise ValueError("publish failed")
                    t.on_change = told
                elif op == "repeated_by":
                    ret = t.repeated_by(step["id"])
                elif op == "repeat_of":
                    ret = t.repeat_of(step["id"])
                res = {"ret": ret}
            except Exception as e:  # noqa: BLE001
                res = err(e)
            record.append({"step": step, **res, "changes": list(changes), "errors": list(errors),
                           "rows": dump(db, TIMER_TABLES)})
            changes.clear()
            errors.clear()
        db.conn.close()
        out.append({"name": name, "start": iso(ScriptClock().t), "steps": record})
    return out


def schedule_store_scenarios() -> list[dict[str, Any]]:
    folder = Path(tempfile.mkdtemp(dir=TMP))
    db = Database(folder / "ferb.db")
    db.migrate()
    clock = ScriptClock()
    store = sch.Schedules(db, clock, SequentialIds("s"))
    set_tz("Asia/Kolkata")
    rule = sch.Rule.build("daily", datetime(2026, 11, 14, 3, 30, tzinfo=timezone.utc), tz=ZoneInfo("Asia/Kolkata"))
    every = sch.Rule.build("every", datetime(2026, 11, 14, 3, 30, tzinfo=timezone.utc), every_minutes=90,
                           tz=ZoneInfo("Asia/Kolkata"), when_active=True)
    steps: list[dict[str, Any]] = [
        {"op": "add", "request": "check the “visa” slots", "own_words": "check the visa slots daily at 9",
         "targets": {"apps": [], "sites": ["https://example.com/ü"], "timing": {"times": ["09:00"], "windows": [],
                                                                              "seed": "s", "start": "2026-11-14"}},
         "rule": dict(rule.__dict__), "first": "2026-11-14T03:30:00+00:00", "watch": "none",
         "until": "2026-11-30T18:30:00+00:00", "name": "  Visa   slots "},
        {"op": "add", "request": "x", "own_words": "x", "targets": {}, "rule": dict(every.__dict__),
         "first": "2026-11-13T03:00:00+00:00", "watch": "none", "until": None, "name": ""},
        {"op": "add", "request": "x", "own_words": "x", "targets": {}, "rule": dict(every.__dict__),
         "first": "2026-11-14T03:00:00+00:00", "watch": "sometimes", "until": None, "name": ""},
        {"op": "add", "request": "x", "own_words": "x", "targets": {}, "rule": dict(every.__dict__),
         "first": "2026-11-14T03:00:00+00:00", "watch": "standing", "until": "2026-11-14T02:00:00+00:00",
         "name": ""},
        {"op": "add", "request": "watch the price", "own_words": "tell me when", "targets": {"n": 1.5},
         "rule": dict(every.__dict__), "first": "2026-11-14T03:00:00.500000+05:30", "watch": "until_met",
         "until": None, "name": "Price"},
        {"op": "set_timer", "id": "sc-s-000001", "timer_id": "tm-1", "next_run": "2026-11-14T09:00:00+05:30"},
        {"op": "set_state", "id": "sc-s-000005", "state": "paused"},
        {"op": "reported", "id": "sc-s-000005", "met": True},
        {"op": "ran", "id": "sc-s-000001", "ok": False}, {"op": "ran", "id": "sc-s-000001", "ok": None},
        {"op": "ran", "id": "sc-s-000001", "ok": False}, {"op": "ran", "id": "sc-s-000001", "ok": True},
        {"op": "edit", "id": "sc-s-000001", "request": "check the slots twice", "rule": dict(every.__dict__),
         "first": "2026-11-15T01:00:00+00:00", "targets": {"apps": ["Mail"]}, "own_words": "make it twice"},
        {"op": "edit", "id": "sc-s-000001", "own_words": "   "},
        {"op": "set_until", "id": "sc-s-000001", "until": "2026-12-01T00:00:00+05:30"},
        {"op": "set_name", "id": "sc-s-000005", "name": " New\tname "},
        {"op": "set_timer", "id": "sc-s-000001", "timer_id": None, "next_run": None},
        {"op": "get", "id": "sc-s-000001"}, {"op": "get", "id": "sc-none"},
        {"op": "all"}, {"op": "all", "states": ["paused", "done"]},
    ]
    record = []
    for step in steps:
        op = step["op"]
        try:
            ret: Any = None
            if op == "add":
                r = sch.Rule(**step["rule"])
                s = store.add(step["request"], step["own_words"], step["targets"], r, dt(step["first"]), step["watch"],
                              dt(step["until"]), step["name"])
                ret = s.id
            elif op == "set_timer":
                store.set_timer(step["id"], step["timer_id"], dt(step["next_run"]))
            elif op == "set_state":
                store.set_state(step["id"], step["state"])
            elif op == "reported":
                store.reported(step["id"], step["met"])
            elif op == "ran":
                ret = store.ran(step["id"], step["ok"])
            elif op == "edit":
                store.edit(step["id"], step.get("request"), sch.Rule(**step["rule"]) if "rule" in step else None,
                           dt(step.get("first")), step.get("targets"), step.get("own_words", ""))
            elif op == "set_until":
                store.set_until(step["id"], dt(step["until"]))
            elif op == "set_name":
                store.set_name(step["id"], step["name"])
            elif op == "get":
                s = store.get(step["id"])
                ret = None if s is None else sched_json(s)
            elif op == "all":
                ret = [sched_json(s) for s in (store.all(tuple(step["states"])) if "states" in step else store.all())]
            res = {"ret": ret}
        except Exception as e:  # noqa: BLE001
            res = err(e)
        record.append({"step": step, **res, "rows": dump(db, SCHEDULE_TABLES)})
        clock.t += timedelta(seconds=61.5)
    db.conn.close()
    return [{"name": "store", "steps": record}]


def sched_json(s: sch.Schedule) -> dict[str, Any]:
    return {"id": s.id, "request": s.request, "own_words": s.own_words, "targets": s.targets,
            "rule": dict(s.rule.__dict__), "next_run": iso(s.next_run), "timer_id": s.timer_id, "state": s.state,
            "failures": s.failures, "first_run": iso(s.first_run), "watch": s.watch,
            "last_reported_met": s.last_reported_met, "until": iso(s.until), "name": s.name,
            "when_said": s.when_said()}


# ---------------------------------------------------------------- tools

def tool_cases() -> list[dict[str, Any]]:
    out = []
    set_tz("Asia/Kolkata")
    folder = Path(tempfile.mkdtemp(dir=TMP))
    db = Database(folder / "ferb.db")
    db.migrate()
    clock = ScriptClock()
    timers = timers_mod.Timers(db, clock, SequentialIds("t"))
    ctx = ToolContext(task_id="t1", worker_id="w1", request_id="r1", identity=Identity.FERB,
                      scopes=frozenset({"time"}), device_id="d", span_id="sp1", workspace=str(folder),
                      owner_roots=(), untrusted_roots=())
    limit = {"value": clock.t + timedelta(hours=4)}
    slept: list[Any] = []

    async def sleep(worker: str, task: str, at: datetime, span: str | None) -> None:
        slept.append([worker, task, iso(at), span])
        clock.t = at
    remind = tools_mod.TimeRemindAt(lambda: timers, clock.now)
    timer = tools_mod.TimeSetTimer(lambda: timers, clock.now)
    sleeper = tools_mod.TimeSleepUntil(lambda: timers, clock.now, lambda: sleep, lambda task: limit["value"])
    none_timers = tools_mod.TimeRemindAt(lambda: None, clock.now)
    calls = [
        ("time.remind_at", remind, {"time": "18:30", "message": "call mom"}),
        ("time.remind_at", remind, {"day": "tomorrow", "message": "no time"}),
        ("time.remind_at", remind, {"in_minutes": 90, "message": "stretch"}),
        ("time.remind_at", remind, {"date": "02-30", "time": "09:00", "message": "bad"}),
        ("time.remind_at", none_timers, {"time": "18:30", "message": "x"}),
        ("time.set_timer", timer, {"minutes": 12.5, "message": "tea"}),
        ("time.set_timer", timer, {"minutes": 0.0001, "message": "blink"}),
        ("time.sleep_until", sleeper, {"in_minutes": 30}),
        ("time.sleep_until", sleeper, {"in_minutes": 600}),
        ("time.sleep_until", sleeper, {"time": "25:00"}),
    ]
    for name, t, args in calls:
        model = t.Input.model_validate(args)
        before = iso(clock.now())
        res = asyncio.run(t.run(model, ctx))
        out.append({"tool": name, "args": args, "now": before, "content": res.content, "is_error": res.is_error,
                    "slept": list(slept), "rows": dump(db, ("timers",))})
        slept.clear()
    # describe() and the gateway-facing facts
    out.append({"descriptions": {"time.remind_at": tools_mod.TimeRemindAt.description,
                                 "time.set_timer": tools_mod.TimeSetTimer.description,
                                 "time.sleep_until": tools_mod.TimeSleepUntil.description}})
    db.conn.close()
    return out


# ---------------------------------------------------------------- (d) wait.for

from ferb.act.shell import guard as shell_guard  # noqa: E402
from ferb.act.shell.manager import ProcHandle  # noqa: E402

T0 = datetime(2026, 11, 12, 16, 0, tzinfo=timezone.utc)


def fake_working_directory(cwd: str | None, ctx: ToolContext) -> str:
    if cwd and cwd.startswith("/forbidden"):
        raise Refused(f"{cwd} is outside this task's folders")
    return cwd or ctx.workspace


shell_guard.working_directory = fake_working_directory  # type: ignore[assignment]


@dataclass
class Info:
    command: str
    running: bool = True
    exit_code: int | None = None
    stopped: str | None = None
    id: str = ""
    owner_task: str = "t1"


class Shells:
    """The tests' FakeShells (tests/test_wait_for.py) plus scripted check-script runs."""

    def __init__(self, scripts: list[dict[str, Any]]) -> None:
        self.procs: dict[str, Info] = {}
        self.out: dict[str, str] = {}
        self.exits: dict[str, asyncio.Event] = {}
        self.told_ids: set[str] = set()
        self.scripts = list(scripts)     # each run: {code, output, stopped?, hosts?}
        self.runs = 0
        self.released: list[str] = []
        self.killed: list[str] = []
        self.started: list[list[Any]] = []

    def add(self, pid: str, command: str, task: str = "t1") -> None:
        self.procs[pid], self.out[pid], self.exits[pid] = Info(command, id=pid, owner_task=task), "", asyncio.Event()

    def finish(self, pid: str, code: int, stopped: str | None = None) -> None:
        self.procs[pid].running, self.procs[pid].exit_code, self.procs[pid].stopped = False, code, stopped
        self.exits[pid].set()

    def handle(self, pid: str, task: str) -> ProcHandle:
        return ProcHandle(pid, task)

    def info(self, h: ProcHandle) -> Info:
        if h.id not in self.procs or h.owner_task != self.procs[h.id].owner_task:
            raise Refused(f"no process {h.id} for this task")
        return self.procs[h.id]

    def list(self) -> list[Info]:
        return list(self.procs.values())

    def exit_told(self, h: ProcHandle) -> None:
        self.told_ids.add(h.id)

    def told(self, pid: str) -> bool:
        return pid in self.told_ids

    async def read(self, h: ProcHandle, since: int = 0, limit: int | None = None) -> tuple[str, int, int]:
        text = self.out[h.id]
        return text[since:], len(text), len(text)

    async def tail(self, h: ProcHandle, limit: int) -> tuple[str, int, int]:
        text = self.out[h.id]
        return text[-limit:], len(text), len(text)

    async def wait(self, h: ProcHandle, timeout: float) -> int | None:
        if h.id.startswith("s-"):                      # a script run: it ends as scripted (None: ran too long)
            return self.procs[h.id].exit_code
        await self.exits[h.id].wait()
        return self.procs[h.id].exit_code

    async def start(self, command: str, cwd: str, task: str, background: bool, folders: Any = ()) -> ProcHandle:
        run = self.scripts[min(self.runs, len(self.scripts) - 1)]
        self.runs += 1
        pid = f"s-{self.runs}"
        self.started.append([command, cwd, task])
        self.procs[pid] = Info(command, running=run["code"] is None, exit_code=run["code"],
                               stopped=run.get("stopped"), id=pid, owner_task=task)
        self.out[pid] = run.get("output", "")
        self.exits[pid] = asyncio.Event()
        self._hosts = run.get("hosts", [])
        return ProcHandle(pid, task)

    async def kill(self, h: ProcHandle) -> None:
        self.killed.append(h.id)
        self.procs[h.id].running = False

    def network(self, h: ProcHandle) -> dict[str, Any]:
        return {"destinations": [{"host": x} for x in self._hosts]}

    async def release(self, h: ProcHandle) -> None:
        self.released.append(h.id)
        self.procs.pop(h.id, None)


@dataclass
class Snap:
    url: str
    title: str
    text: str


@dataclass
class Browser:
    url: str = "https://www.example.in/dp/B0X"
    pages: list[str] = field(default_factory=list)
    reads: int = 0
    reloads: int = 0
    downloads: list[list[dict[str, Any]]] = field(default_factory=list)
    fail_reads: list[int] = field(default_factory=list)      # read numbers that raise (a hung page)
    no_downloads: bool = False

    def owned_by(self, tab: str, owner: str) -> str:
        if tab != "t1" or owner != "t1":
            raise Refused(f"no tab {tab} open for this task; its open tabs: t1")
        return tab

    def tab_url(self, tab: str) -> str:
        return self.url

    async def snapshot(self, ref: str) -> Snap:
        self.reads += 1
        if self.reads in self.fail_reads:
            raise TimeoutError(f"read {self.reads} timed out")
        text = self.pages[min(self.reads, len(self.pages)) - 1]
        return Snap(self.url, "Product\u0007 page", text)

    async def navigate(self, ref: str, url: str) -> None:
        self.reloads += 1

    async def downloads_for(self, ref: str, wait: float = 0.0) -> list[dict[str, Any]]:
        return self.downloads.pop(0) if self.downloads else []


class NoDownloads(Browser):
    def __getattribute__(self, name: str) -> Any:
        if name == "downloads_for":
            raise AttributeError(name)
        return super().__getattribute__(name)


WAIT_SCENARIOS: list[dict[str, Any]] = [
    {"name": "process_exit_named", "procs": [["p-1", "codex exec -s danger-full-access -C ~/.ferb/workspace 'fix'"]],
     "out": {"p-1": "…\ntests: 214 passed\n"}, "events": [{"at": 1184, "do": "finish", "id": "p-1", "code": 0}],
     "args": {"any_of": [{"kind": "process", "id": "p-1"}], "limit_minutes": 40}},
    {"name": "process_output_match", "procs": [["p-1", "npm run dev"]],
     "events": [{"at": 30, "do": "output", "id": "p-1", "text": "compiling\n  ➜  Local: http://localhost:5173/ ready\n"}],
     "args": {"any_of": [{"kind": "process", "id": "p-1", "match": "READY"}]}},
    {"name": "process_regex_since", "procs": [["p-1", "make test\nsecond line"]],
     "out": {"p-1": "old: FAIL 1\n"},
     "events": [{"at": 7, "do": "output", "id": "p-1", "text": "old: FAIL 1\nnew: FAIL 22\nok\n"}],
     "args": {"any_of": [{"kind": "process", "id": "p-1", "match": r"FAIL \d{2}", "regex": True, "since": 3}]}},
    {"name": "process_time_limit", "procs": [["p-2", "make build"]],
     "args": {"any_of": [{"kind": "process", "id": "p-2"}], "limit_minutes": 5}},
    {"name": "process_not_this_task", "args": {"any_of": [{"kind": "process", "id": "p-9"}]}},
    {"name": "process_stopped_by_ferb", "procs": [["p-1", "sleep 100"]],
     "events": [{"at": 3, "do": "finish", "id": "p-1", "code": -15, "stopped": "the owner stopped the task"}],
     "args": {"any_of": [{"kind": "process", "id": "p-1"}]}},
    {"name": "file_exists", "files": [], "dirs": ["Downloads"],
     "events": [{"at": 60, "do": "write", "path": "Downloads/report.pdf", "bytes": 40, "mtime": 1800000000}],
     "args": {"any_of": [{"kind": "file", "path": "Downloads/report.pdf", "state": "exists"}]}},
    {"name": "file_settled", "dirs": ["Downloads"],
     "events": [{"at": 1, "do": "append", "path": "Downloads/report.pdf", "bytes": 1000, "mtime": 1800000001},
                {"at": 2, "do": "append", "path": "Downloads/report.pdf", "bytes": 1000, "mtime": 1800000002},
                {"at": 3, "do": "append", "path": "Downloads/report.pdf", "bytes": 1000, "mtime": 1800000003},
                {"at": 5, "do": "append", "path": "Downloads/report.pdf", "bytes": 1000, "mtime": 1800000005}],
     "args": {"any_of": [{"kind": "file", "path": "Downloads/report.pdf", "state": "settled", "settle_s": 10}]}},
    {"name": "file_gone", "files": [["out/old.txt", 5, 1800000000]],
     "events": [{"at": 20, "do": "delete", "path": "out/old.txt"}],
     "args": {"any_of": [{"kind": "file", "path": "./out//old.txt", "state": "gone"}]}},
    {"name": "file_home_and_time_limit", "args": {"any_of": [{"kind": "file", "path": "~/export.csv",
                                                               "state": "exists"}], "limit_minutes": 1}},
    {"name": "file_not_a_directory", "files": [["plain.txt", 3, 1800000000]],
     "args": {"any_of": [{"kind": "file", "path": "plain.txt/inner", "state": "exists"}]}},
    {"name": "file_slash_name", "args": {"any_of": [{"kind": "file", "path": "/", "state": "gone"}],
                                          "limit_minutes": 0.05}},
    {"name": "page_reload_match", "pages": ["Currently unavailable."] * 4 + ["In stock.\nAdd to Cart"],
     "args": {"any_of": [{"kind": "page", "tab": "t1", "match": "in stock", "reload": True, "every_s": 60}]}},
    {"name": "page_default_interval_limit", "pages": ["Currently unavailable."],
     "args": {"any_of": [{"kind": "page", "tab": "t1", "match": "In stock"}], "limit_minutes": 10}},
    {"name": "page_floor_refused", "args": {"any_of": [{"kind": "page", "tab": "t1", "match": "x", "every_s": 10}]}},
    {"name": "page_not_this_tab", "args": {"any_of": [{"kind": "page", "tab": "t2", "match": "x"}]}},
    {"name": "page_change", "pages": ["Order #1: preparing", "Order #1: preparing",
                                      "Order #1: out for delivery\n\n  Order #1: preparing  "],
     "args": {"any_of": [{"kind": "page", "tab": "t1", "every_s": 30}]}},
    {"name": "page_errors_three", "pages": ["a"], "fail_reads": [1, 2, 3],
     "args": {"any_of": [{"kind": "page", "tab": "t1", "match": "b", "every_s": 31.5}]}},
    {"name": "page_error_recovers", "pages": ["x", "y", "STRASSE ist da"], "fail_reads": [1],
     "args": {"any_of": [{"kind": "page", "tab": "t1", "match": "straße", "every_s": 3600}],
              "limit_minutes": 200}},
    {"name": "url_match", "events": [{"at": 8.5, "do": "url", "url": "https://www.example.in/thankyou"}],
     "args": {"any_of": [{"kind": "url", "tab": "t1", "match": "/thankyou"}]}},
    {"name": "url_change", "events": [{"at": 4, "do": "url", "url": "https://shop.example.com/cart"}],
     "args": {"any_of": [{"kind": "url", "tab": "t1"}]}},
    {"name": "mail_new_line", "url": "https://mail.example.com/mail/u/0/#inbox",
     "pages": ["Inbox\nAlice Rao — Lunch? — 9:02", "Inbox\nAlice Rao — Lunch? — 9:02",
               "Inbox\nAlice Rao — Invoice 2231 — 16:01\nAlice Rao — Lunch? — 9:02"],
     "args": {"any_of": [{"kind": "mail", "tab": "t1", "sender": "alice rao"}]}},
    {"name": "mail_subject_and_text", "url": "https://mail.example.com/",
     "pages": ["Bob — Invoice — x", "Bob — Invoice — x\nBob — Invoice — x\nCarol — invoice paid"],
     "args": {"any_of": [{"kind": "mail", "tab": "t1", "subject": "INVOICE", "text": "x", "every_s": 45}]}},
    {"name": "download", "downloads": [[], [], [{"path": "/x/statement.pdf", "bytes": 912}]],
     "args": {"any_of": [{"kind": "download", "tab": "t1"}]}},
    {"name": "download_no_method", "no_downloads": True,
     "args": {"any_of": [{"kind": "download", "tab": "t1"}], "limit_minutes": 0.1}},
    {"name": "time_in_minutes", "args": {"any_of": [{"kind": "time", "in_minutes": 12}]}},
    {"name": "time_at", "args": {"any_of": [{"kind": "time", "at": "21:47"}], "limit_minutes": 60}},
    {"name": "limit_longer", "args": {"any_of": [{"kind": "file", "path": "never.txt", "state": "exists"}],
                                      "limit_minutes": 90}},
    {"name": "limit_past_task", "args": {"any_of": [{"kind": "time", "in_minutes": 5}], "limit_minutes": 300}},
    {"name": "time_after_default", "args": {"any_of": [{"kind": "time", "in_minutes": 45}]}},
    {"name": "task_limit_sooner", "task_limit_min": 10,
     "args": {"any_of": [{"kind": "file", "path": "never.txt", "state": "exists"}]}},
    {"name": "no_task_limit", "task_limit_min": None,
     "args": {"any_of": [{"kind": "url", "tab": "t1", "match": "never"}], "limit_minutes": 0.02}},
    {"name": "any_of_two", "procs": [["p-1", "codex exec 'fix the tests'"]],
     "events": [{"at": 50, "do": "write", "path": "out.json", "bytes": 2, "mtime": 1800000000}],
     "args": {"any_of": [{"kind": "process", "id": "p-1"}, {"kind": "file", "path": "out.json", "state": "exists"}]}},
    {"name": "correction", "procs": [["p-1", "codex exec 'fix the tests'"]],
     "events": [{"at": 100, "do": "steer"}], "args": {"any_of": [{"kind": "process", "id": "p-1"}]}},
    {"name": "implicit_command_ends", "procs": [["p-1", "blender -b scene.blend --python render.py"],
                                                ["p-9", "make all", "t2"]],
     "out": {"p-1": "Read blend: scene.blend\nError: render.py: No such file or directory\n"},
     "events": [{"at": 1, "do": "finish", "id": "p-9", "code": 0}, {"at": 3, "do": "finish", "id": "p-1", "code": 1}],
     "args": {"any_of": [{"kind": "file", "path": "report2.txt", "state": "exists"}], "limit_minutes": 2},
     "again": {"any_of": [{"kind": "file", "path": "report2.txt", "state": "exists"}], "limit_minutes": 1}},
    {"name": "implicit_ended_before", "procs": [["p-1", "python3 build.py"]],
     "out": {"p-1": "Traceback (most recent call last):\nModuleNotFoundError: No module named 'bpy'\n"},
     "finished": [["p-1", 1]], "args": {"any_of": [{"kind": "time", "in_minutes": 5}]}},
    {"name": "implicit_already_told", "procs": [["p-1", "python3 build.py"]], "told": ["p-1"],
     "finished": [["p-1", 1]], "args": {"any_of": [{"kind": "time", "in_minutes": 5}]}},
    {"name": "implicit_without_shell_scope", "procs": [["p-1", "python3 build.py"]], "finished": [["p-1", 1]],
     "scopes": ["time", "fs:read"], "args": {"any_of": [{"kind": "time", "in_minutes": 1}]}},
    {"name": "named_process_second", "procs": [["p-1", "npm run build"]],
     "events": [{"at": 5, "do": "finish", "id": "p-1", "code": 2}],
     "args": {"any_of": [{"kind": "file", "path": "dist/x", "state": "exists"}, {"kind": "process", "id": "p-1"}]}},
    {"name": "action_failed", "failed": {"tool": "mac.act", "error": "mac.act failed: PointerInterrupted"},
     "args": {"any_of": [{"kind": "file", "path": "report2.txt", "state": "exists"}], "limit_minutes": 2},
     "again": {"any_of": [{"kind": "file", "path": "report2.txt", "state": "exists"}], "limit_minutes": 2}},
    {"name": "action_failed_no_tool", "failed": {"exit_code": 3},
     "args": {"any_of": [{"kind": "time", "in_minutes": 1}]}},
    {"name": "scope_missing", "scopes": ["time"], "args": {"any_of": [{"kind": "page", "tab": "t1"}]}},
    {"name": "scope_owner_browser", "identity": "owner", "scopes": ["time", "browser:read"], "pages": ["hi"],
     "args": {"any_of": [{"kind": "page", "tab": "t1", "match": "HI"}]}},
    {"name": "script_ok", "scripts": [{"code": 1, "output": "not yet"}, {"code": 0, "output": "{\"ok\": true}",
                                                                          "hosts": ["api.example.com"]}],
     "args": {"any_of": [{"kind": "script", "command": "test -s out.json && cat out.json", "every_s": 5}]}},
    {"name": "script_not_found", "scripts": [{"code": 127, "output": "sh: no-such-tool: command not found"}],
     "args": {"any_of": [{"kind": "script", "command": "no-such-tool --check"}]}},
    {"name": "script_timeout_then_limit", "scripts": [{"code": None, "output": "slow"}],
     "args": {"any_of": [{"kind": "script", "command": "sleep 999", "every_s": 120}], "limit_minutes": 3}},
    {"name": "script_stopped", "scripts": [{"code": -9, "output": "", "stopped": "FERB stopped it"}],
     "args": {"any_of": [{"kind": "script", "command": "curl x", "cwd": "sub"}]}},
    {"name": "script_floor", "args": {"any_of": [{"kind": "script", "command": "true", "every_s": 1}]}},
    {"name": "script_cwd_refused", "args": {"any_of": [{"kind": "script", "command": "true", "cwd": "/forbidden/x"}]}},
    {"name": "script_long_every", "scripts": [{"code": 1, "output": ""}],
     "args": {"any_of": [{"kind": "script", "command": "check", "every_s": 90}], "limit_minutes": 4}},
    {"name": "many_conditions", "procs": [["p-1", "   \n  build   all\t`x`  \n"]],
     "args": {"any_of": [{"kind": "time", "in_minutes": 1}, {"kind": "process", "id": "p-1", "match": "“q”"},
                         {"kind": "url", "tab": "t1", "match": "a" * 60}], "limit_minutes": 2}},
]


def wait_scenarios() -> list[dict[str, Any]]:
    set_tz("Asia/Kolkata")
    out = []
    for sc in WAIT_SCENARIOS:
        ws = Path(tempfile.mkdtemp(dir=TMP))
        work = ws / "work"
        work.mkdir()
        for d in sc.get("dirs", []):
            (work / d).mkdir(parents=True, exist_ok=True)
        for rel, size, mtime in sc.get("files", []):
            p = work / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(b"x" * size)
            os.utime(p, (mtime, mtime))
        steps = SimpleNamespace(wall=T0, mono=0.0, rests=0)
        shells = Shells(sc.get("scripts", [{"code": 0}]))
        for p in sc.get("procs", []):
            shells.add(p[0], p[1], *(p[2:]))
        for pid, text in sc.get("out", {}).items():
            shells.out[pid] = text
        for pid in sc.get("told", []):
            shells.told_ids.add(pid)
        for pid, code in sc.get("finished", []):
            shells.finish(pid, code)
        browser = (NoDownloads if sc.get("no_downloads") else Browser)()
        if "url" in sc:
            browser.url = sc["url"]
        browser.pages = list(sc.get("pages", []))
        browser.downloads = [list(x) for x in sc.get("downloads", [])]
        browser.fail_reads = list(sc.get("fail_reads", []))
        lines: list[Any] = []
        steer, ending, ended = [0], [None], []
        failed = {"w1": sc["failed"]} if "failed" in sc else {}
        task_limit = sc.get("task_limit_min", 240)
        events = [dict(e) for e in sc.get("events", [])]

        async def pause(seconds: float, wake: asyncio.Event) -> None:
            steps.rests += 1
            steps.wall += timedelta(seconds=seconds)
            steps.mono += seconds
            for e in events:
                if e.get("done") or steps.mono < e["at"]:
                    continue
                e["done"] = True
                do = e["do"]
                if do == "finish":
                    shells.finish(e["id"], e["code"], e.get("stopped"))
                elif do == "output":
                    shells.out[e["id"]] = e["text"]
                elif do == "steer":
                    steer[0] += 1
                elif do == "url":
                    browser.url = e["url"]
                elif do in ("write", "append"):
                    p = work / e["path"]
                    with p.open("ab" if do == "append" else "wb") as f:
                        f.write(b"x" * e["bytes"])
                    os.utime(p, (e["mtime"], e["mtime"]))
                elif do == "delete":
                    (work / e["path"]).unlink()
            await asyncio.sleep(0)

        tool = wait_mod.WaitFor(wait_mod.WaitDeps(
            now=lambda: steps.wall,
            limit=lambda t: T0 + timedelta(minutes=task_limit) if task_limit is not None else None,
            shells=lambda: shells, browsers=lambda identity: browser,
            status=lambda w, t, until, line: lines.append([iso(until), line]),
            steered=lambda t: steer[0], ending=lambda: ending[0], ended=lambda t, data: ended.append(data),
            monotonic=lambda: steps.mono, pause=pause, failed_act=lambda worker: failed.pop(worker, None)))
        identity = Identity.OWNER if sc.get("identity") == "owner" else Identity.FERB
        scopes = frozenset(sc.get("scopes", ["time", "shell", "fs:read", "identity:browse", "browser:read"]))
        ctx = ToolContext(task_id="t1", worker_id="w1", request_id="r1", identity=identity, scopes=scopes,
                          device_id="d", span_id=None, workspace=str(work), owner_roots=(), untrusted_roots=(),
                          folders=())
        runs = []
        for args in [sc["args"]] + ([sc["again"]] if "again" in sc else []):
            try:
                model = wait_mod.WaitInput.model_validate(args)
                res = asyncio.run(tool.run(model, ctx))
                result = {"content": res.content, "is_error": res.is_error,
                          "read_sources": [list(x) for x in res.read_sources]}
            except Exception as e:  # noqa: BLE001
                result = err(e)
            runs.append({"args": args, "result": result, "lines": list(lines), "rests": steps.rests,
                         "mono": steps.mono, "wall": iso(steps.wall), "reads": browser.reads,
                         "reloads": browser.reloads, "released": list(shells.released),
                         "killed": list(shells.killed), "started": list(shells.started),
                         "told": sorted(shells.told_ids)})
            lines.clear()
        text = json.dumps(runs, ensure_ascii=False).replace(str(work), "<WORK>").replace(os.environ["HOME"], "<HOME>")
        out.append({k: v for k, v in sc.items()} | {"runs": json.loads(text)})
    # the stop / restart shapes: a cancelled wait journals wait.ended only on a restart
    for ending_value in (None, "restart"):
        steps = SimpleNamespace(wall=T0, mono=0.0)
        shells = Shells([{"code": 0}])
        shells.add("p-1", "codex exec 'fix the tests'")
        lines, ended = [], []

        async def pause2(seconds: float, wake: asyncio.Event) -> None:
            steps.wall += timedelta(seconds=seconds)
            steps.mono += seconds
            await asyncio.sleep(0)

        tool = wait_mod.WaitFor(wait_mod.WaitDeps(
            now=lambda: steps.wall, limit=lambda t: None, shells=lambda: shells, browsers=lambda identity: None,
            status=lambda w, t, until, line: lines.append([iso(until), line]), steered=lambda t: 0,
            ending=lambda e=ending_value: e, ended=lambda t, data: ended.append([t, data]),
            monotonic=lambda: steps.mono, pause=pause2))
        ctx = ToolContext(task_id="t1", worker_id="w1", request_id="r1", identity=Identity.FERB,
                          scopes=frozenset({"time", "shell"}), device_id="d", span_id=None, workspace="/w",
                          owner_roots=(), untrusted_roots=())

        async def go() -> None:
            job = asyncio.ensure_future(tool.run(wait_mod.WaitInput.model_validate(
                {"any_of": [{"kind": "process", "id": "p-1"}]}), ctx))
            for _ in range(20):
                await asyncio.sleep(0)
            job.cancel()
            try:
                await job
            except asyncio.CancelledError:
                pass
        asyncio.run(go())
        out.append({"name": f"cancelled_{ending_value}", "lines": lines, "ended": ended})
    # input checks (pydantic's model validators) and describe()
    checks = []
    for args in [{"any_of": [{"kind": "process"}]}, {"any_of": [{"kind": "file", "path": "x"}]},
                 {"any_of": [{"kind": "mail", "tab": "t1"}]}, {"any_of": [{"kind": "time"}]},
                 {"any_of": [{"kind": "script", "command": "   "}]}, {"any_of": [{"kind": "page"}]},
                 {"any_of": [{"kind": "url"}]}, {"any_of": [{"kind": "download"}]},
                 {"any_of": [{"kind": "script", "command": "a"}, {"kind": "script", "command": "b"}]},
                 {"any_of": [{"kind": "page", "tab": "t1", "match": "(", "regex": True}]},
                 {"any_of": [{"kind": "script", "command": "test -s x", "cwd": "sub"}]},
                 {"any_of": [{"kind": "file", "path": "a", "state": "exists"}, {"kind": "file", "path": "/b",
                                                                                   "state": "gone"}]}]:
        try:
            model = wait_mod.WaitInput.model_validate(args)
            spec = wait_mod.WaitFor(None).describe(model)  # type: ignore[arg-type]
            checks.append({"args": args, "kind": str(spec.kind), "path": spec.target.path,
                           "content": spec.content, "file_paths": wait_mod.WaitFor(None).file_paths(model)})
        except Exception as e:  # noqa: BLE001
            errors = getattr(e, "errors", None)
            checks.append({"args": args, "error": type(e).__name__,
                           "messages": [x["msg"] for x in errors()] if errors else [str(e)]})
    return [{"scenarios": out, "checks": checks, "description": wait_mod.DESCRIPTION,
             "start": iso(T0)}]


# ---------------------------------------------------------------- main

def main() -> None:
    generate_tables()
    write("recurrence.json", recurrence())
    write("when.json", when_cases())
    write("context.json", context_cases())
    write("timers.json", timer_scenarios())
    write("schedules_store.json", schedule_store_scenarios())
    write("tools.json", tool_cases())
    write("waits.json", wait_scenarios()[0])
    print("wrote", ", ".join(p.name for p in sorted(HERE.glob("*.json"))))
    shutil.rmtree(TMP, ignore_errors=True)


if __name__ == "__main__":
    main()
