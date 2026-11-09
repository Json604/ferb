"""T-1066 (owner 2026-11-07: "build all five. also where is honesty in ferb stating how many schedules are there and
not from jsut its memory???"): several times a day, random times in a window, edits in place, several schedules in one
message, code-checked words, and the owner's schedules listed from FERB's records."""

import json
from datetime import datetime, timedelta, timezone
from types import SimpleNamespace
from typing import Any
from zoneinfo import ZoneInfo

from ferb.clock.when import When
from ferb.orchestrator.schedules import TIMING, Rule, Schedule, Timing, missing_times, plan_edit, plan_new
from tests.conftest import RunningDaemon, chat, cond, install, sup, task
from tests.support.schedules import run

IST = ZoneInfo("Asia/Kolkata")
NOW = datetime(2026, 11, 7, 17, 55, tzinfo=IST)
VISA_WORDS = ("every 6 hours from 6 am, and 2 random checks between 5 and 8 am apart from the 6 am one")


def _spec(request: str, when: When, repeat: str, every: int | None = None, times: tuple[str, ...] = (),
          random: tuple[dict[str, Any], ...] = ()) -> Any:
    return SimpleNamespace(request=request, when=when, repeat=repeat, weekday=None, every_minutes=every,
                           times=list(times), random=[SimpleNamespace(**w) for w in random], when_active=False,
                           until=None)


def _runs(s: Schedule, after: datetime, n: int) -> list[datetime]:
    out = []
    for _ in range(n):
        nxt = s.next_after(after)
        assert nxt is not None
        out.append(nxt.astimezone(IST))
        after = nxt
    return out


# ---------- the rule and its times ----------

# decision: ORCH-550
def test_one_schedule_runs_at_several_fixed_times_a_day() -> None:
    plan = plan_new(_spec("check at 5:20 AM, 6 AM, 7:10 AM, 12 PM, 6 PM and 12 AM", When(time="05:20"), "daily",
                          times=("05:20", "06:00", "07:10", "12:00", "18:00", "00:00")), NOW, "seed")
    s = plan.schedule
    assert s.when_said() == "every day at 00:00, 05:20, 06:00, 07:10, 12:00 and 18:00" and plan.missing == []
    assert plan.booked.astimezone(IST) == datetime(2026, 11, 7, 18, 0, tzinfo=IST)      # today's 18:00 still ahead
    assert [r.strftime("%d %H:%M") for r in _runs(s, NOW, 7)] == [
        "07 18:00", "08 00:00", "08 05:20", "08 06:00", "08 07:10", "08 12:00", "08 18:00"]


# decision: ORCH-550
# decision: ORCH-556
def test_random_times_are_drawn_per_day_from_the_seed_and_keep_off_other_runs() -> None:
    """The visa case: every 6 hours from 06:00 plus 2 random times between 05:00 and 08:00 each day, never on the
    06:00 run's minute. The draw depends only on the seed and the date, so the next run is always known."""
    plan = plan_new(_spec("check every 6 hours from 6:00 AM and twice at random between 5:00 AM and 8:00 AM",
                          When(time="06:00"), "every", 360, random=({"count": 2, "start": "05:00", "end": "08:00"},)),
                    NOW, "seed-a")
    s = plan.schedule
    assert s.when_said() == "every 6 hours, plus 2 at random times between 05:00 and 08:00 each day"
    day = _runs(s, NOW, 6)
    morning = [r for r in day if r.day == 8 and r.hour < 9 and r.minute + r.hour * 60 != 360]
    assert len(morning) == 2 and all(datetime(2026, 11, 8, 5, 0, tzinfo=IST) <= r <= datetime(2026, 11, 8, 8, 0,
                                                                                               tzinfo=IST)
                                     for r in morning)
    assert _runs(s, NOW, 6) == day                                               # asked again: the same times
    assert plan.booked == min(day[0], plan.first)
    timing = s.timing
    assert timing is not None
    days = {repr(timing.picks(NOW.date() + timedelta(days=d), {360})) for d in range(1, 13)}
    assert len(days) > 1                                                         # fresh each day
    assert all(360 not in timing.picks(NOW.date() + timedelta(days=d), {360}) for d in range(1, 60))
    other = plan_new(_spec("same", When(time="06:00"), "every", 360,
                           random=({"count": 2, "start": "05:00", "end": "08:00"},)), NOW, "seed-b").schedule
    assert _runs(other, NOW, 12) != _runs(s, NOW, 12)                            # each schedule its own seed


# decision: ORCH-551
def test_a_request_naming_a_time_its_rule_never_runs_at_is_found() -> None:
    """Live 2026-11-07: the request said six times a day; the rule ran daily at 05:20."""
    words = "at 5:20 AM, 6:00 AM, 7:10 AM, 12:00 PM, 6:00 PM, and 12:00 AM"
    once = plan_new(_spec(words, When(time="05:20"), "daily"), NOW, "s")
    assert once.missing == ["6:00 AM", "7:10 AM", "12:00 PM", "6:00 PM", "12:00 AM"]
    every = plan_new(_spec("every 6 hours from 6:00 AM, i.e. 6 am, 12 pm, 18:00 and midnight 12 AM; window 5-8",
                           When(time="06:00"), "every", 360), NOW, "s")
    assert every.missing == []                                                   # all on the 6-hour sequence
    assert missing_times("no clock here: 3 days, B1/B2, 2 checks", {0}) == []


# decision: ORCH-552
def test_an_edit_changes_timing_and_words_in_place_and_checks_them() -> None:
    plan = plan_new(_spec("check the visa site daily at 5:20 AM", When(time="05:20"), "daily"), NOW, "s")
    old = plan.schedule
    timing = SimpleNamespace(repeat="every", times=["06:00"], every_minutes=360, weekday="",
                             random=[SimpleNamespace(count=2, start="05:00", end="08:00")])
    kept_words = plan_edit(old, "", timing, NOW, "e")
    assert kept_words.missing == ["5:20 AM"]                   # the old words name a time the new timing drops
    edit = plan_edit(old, "check the visa site every 6 hours from 6:00 AM and twice between 5 and 8", timing, NOW,
                     "e")
    assert edit.missing == [] and edit.schedule.when_said().startswith("every 6 hours, plus 2 at random")
    words_only = plan_edit(old, "check the visa site and book any slot, at 5:20 AM", SimpleNamespace(repeat=""),
                           NOW, "e")
    assert words_only.rule == old.rule and words_only.missing == []


# decision: STORE-403
def test_a_schedule_saved_before_keeps_working_and_new_ones_keep_the_old_rule_fields() -> None:
    """An older FERB's rule_json (no timing) reads as before; a new schedule's timing lives in its recorded JSON, so
    its rule_json has exactly the keys an older FERB reads (an older build still starts on the database)."""
    first = datetime(2026, 11, 8, 6, 0, tzinfo=IST)
    rule = Rule.build("daily", first, tz=IST)
    old = Schedule("sc-1", "write report.md", "", {"apps": []}, rule, first, None, "active", 0, first)
    assert old.timing is None and old.when_said() == "every day at 06:00"
    assert [r.hour for r in _runs(old, NOW, 2)] == [6, 6]
    assert set(rule.__dict__) == {"repeat", "at", "weekday", "every_minutes", "tz", "when_active"}
    assert Timing.of({"times": ["nonsense"]}) is None                    # a damaged timing: the rule alone


# ---------- through the daemon ----------

def _schedule(request: str, quote: str, when: dict[str, Any], repeat: str, every: int | None = None,
              times: tuple[str, ...] = (), random: tuple[dict[str, Any], ...] = (), name: str = "") -> dict[str, Any]:
    return {"request": request, "name": name, "when": when, "repeat": repeat, "weekday": None,
            "every_minutes": every, "times": list(times), "random": list(random), "quote": quote,
            "targets": {"apps": [], "sites": [], "paths": [], "recipients": []}, "watch": "none"}


def _at(hhmm: str) -> dict[str, Any]:
    return {"in_minutes": None, "in_days": None, "day": None, "date": None, "time": hhmm}


def _reply(text: str, **fields: Any) -> str:
    return json.dumps({"kind": "reply", "text": text, "task": None, **fields})


def _lines(evs: list[dict[str, Any]]) -> list[str]:
    return [e["text"] for e in evs if e["type"] == "timer"]


def _reply_of(evs: list[dict[str, Any]]) -> dict[str, Any]:
    return next(e for e in evs if e["type"] == "reply")


# decision: ORCH-550
def test_the_visa_schedule_end_to_end(daemon: RunningDaemon) -> None:
    visa = _schedule("Check the visa site every 6 hours from 06:00, and twice at random between 05:00 and 08:00",
                     VISA_WORDS, _at("06:00"), "every", 360, random=({"count": 2, "start": "05:00", "end": "08:00"},),
                     name="Visa checks")
    install(daemon, [_reply("Setting that up.", schedules=[visa])])
    evs = chat(daemon, VISA_WORDS)
    line = _lines(evs)[0]
    assert line.startswith("🗓 Scheduled: Visa checks — every 6 hours, plus 2 at random times between 05:00 and "
                           "08:00 each day, first on ")
    card = next(c for c in _reply_of(evs)["cards"] if c["type"] == "event")
    assert card["repeat"].startswith("every 6 hours, plus 2 at random")
    s = run(daemon, daemon.daemon.schedules.all)[0]
    assert s.timing is not None and s.next_run == s.next_after(daemon.daemon.clock.now())
    stored = run(daemon, lambda: daemon.daemon.db.conn.execute("SELECT rule_json, intent_json FROM schedules")
                 .fetchone())
    assert TIMING not in json.loads(stored[0]) and TIMING in json.loads(stored[1])


# decision: ORCH-551
# decision: ORCH-555
def test_a_schedule_whose_words_name_times_it_doesnt_run_at_is_sent_back_then_kept_and_said_plainly(
        daemon: RunningDaemon) -> None:
    """Owner 2026-11-08: "Keep it, say it plainly". Told back to the planner once; still mismatched, the schedule is
    kept and code's line says when it runs and which times in its instructions aren't run times."""
    words = "check the visa site daily at 5:20 AM, 6:00 AM and 7:10 AM"
    dishonest = _schedule("Check the visa site at 5:20 AM, 6:00 AM and 7:10 AM", words, _at("05:20"), "daily",
                          name="Visa check")
    script = install(daemon, [_reply("Done.", schedules=[dishonest]), _reply("Done.", schedules=[dishonest])])
    evs = chat(daemon, words)
    repair = [ctx for mode, key, ctx, _ in script.contexts if "6:00 AM, 7:10 AM" in ctx and "would run" in ctx]
    assert repair                                                  # told back once, naming the times
    [line] = _lines(evs)
    assert line.startswith("🗓 Scheduled: Visa check — every day at 05:20, first on ")
    assert "It runs every day at 05:20; 6:00 AM and 7:10 AM in its instructions are not a time it runs." in \
        _reply_of(evs)["text"]
    assert len(run(daemon, daemon.daemon.schedules.all)) == 1


# decision: ORCH-550
def test_several_schedules_in_one_message(daemon: RunningDaemon) -> None:
    words = "every day at 9 water the plants note, and every friday at 17:00 send me the week's summary"
    install(daemon, [_reply("Both set.", schedules=[
        _schedule("Remind me to water the plants", "every day at 9 water the plants note", _at("09:00"), "daily"),
        _schedule("Write the week's summary", "every friday at 17:00 send me the week's summary", _at("17:00"),
                  "weekly")])])
    evs = chat(daemon, words)
    assert [line.split(" — ")[0] for line in _lines(evs)] == ["🗓 Scheduled: Remind me to water the plants",
                                                             "🗓 Scheduled: Write the week's summary"]
    assert len([c for c in _reply_of(evs)["cards"] if c["type"] == "event"]) == 2
    assert len(run(daemon, daemon.daemon.schedules.all)) == 2


# decision: ORCH-552
def test_an_edit_keeps_the_schedule_and_shows_old_and_new(daemon: RunningDaemon) -> None:
    install(daemon, [_reply("Set.", schedules=[_schedule("Check the visa site at 5:20 AM", "daily at 5:20 check the "
                                                         "visa site", _at("05:20"), "daily", name="Visa check")])])
    chat(daemon, "daily at 5:20 check the visa site")
    s = run(daemon, daemon.daemon.schedules.all)[0]
    change = {"id": s.id, "action": "edit", "quote": "make it every 6 hours from 6 am instead",
              "until": None, "name": "", "request": "Check the visa site every 6 hours from 6:00 AM",
              "timing": {"repeat": "every", "times": ["06:00"], "every_minutes": 360, "weekday": "", "random": []}}
    install(daemon, [_reply("Changing it.", schedule_changes=[change])])
    evs = chat(daemon, "make it every 6 hours from 6 am instead")
    assert _lines(evs)[0].startswith("🗓 Changed: Visa check — now every 6 hours (was every day at 05:20); next "
                                     "run ")
    card = next(c for c in _reply_of(evs)["cards"] if c["type"] == "event")
    assert card["changed"] and card["was_repeat"] == "every day at 05:20" and card["repeat"] == "every 6 hours"
    after = run(daemon, daemon.daemon.schedules.all)
    assert [x.id for x in after] == [s.id] and after[0].request == "Check the visa site every 6 hours from 6:00 AM"
    assert after[0].next_run is not None and after[0].next_run.astimezone().strftime("%H:%M") in (
        "00:00", "06:00", "12:00", "18:00")
    edited = run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.edited", "schedule_id", s.id))
    assert edited and edited[0].data["before"]["rule"] == "every day at 05:20"


# decision: ORCH-553
def test_schedule_work_handed_to_a_task_is_sent_back(daemon: RunningDaemon) -> None:
    """Live 2026-11-07: a part said this turn's schedule entry answers it, but no entry did, and the worker (with no
    schedule tool) was given the timing instead."""
    words = "log in to the visa site, and add 2 random checks between 5 and 8 am"
    handed = task("Log in to the visa site and add two random morning checks", "owner", sites=("visa.example",),
                  conds=(cond("logged in"),)) | {"parts": [
        {"id": "login", "quote": "log in to the visa site", "label": "Log in", "direct": None,
         "answer_step": "worker", "conditions": [0]},
        {"id": "random", "quote": "add 2 random checks between 5 and 8 am", "label": "Random checks", "direct": "",
         "answer_step": None, "conditions": []}]}
    script = install(daemon, [sup("task", "On it.", handed), _reply("I'll ask again.")])
    chat(daemon, words)
    assert any("part random" in ctx and "a worker can't make or change a schedule" in ctx
               for _, _, ctx, _ in script.contexts)


# decision: ORCH-554
# decision: DMN-550
def test_the_planner_sees_the_schedule_list_and_the_owner_gets_it_as_a_card(daemon: RunningDaemon) -> None:
    install(daemon, [_reply("Set.", schedules=[_schedule("Check the visa site at 5:20 AM", "daily at 5:20 check the "
                                                         "visa site", _at("05:20"), "daily", name="Visa check")])])
    chat(daemon, "daily at 5:20 check the visa site")
    script = install(daemon, [_reply("You have one schedule.", show=["schedules"])])
    evs = chat(daemon, "how many schedules do we have??")
    planner = next(ctx for mode, key, ctx, _ in script.contexts)
    assert "The owner's schedules, from FERB's records (the truth" in planner and ": 1 (1 active)" in planner
    assert "“Visa check” (active): runs every day at 05:20; next run " in planner
    card = next(c for c in _reply_of(evs)["cards"] if c["type"] == "schedules")
    assert card["count"] == 1 and card["schedules"][0]["name"] == "Visa check"
    assert card["schedules"][0]["repeat"] == "every day at 05:20" and card["schedules"][0]["next_run"]
    assert card["line"] in _reply_of(evs)["text"]


# decision: ORCH-334
def test_a_long_interval_is_said_in_days_and_hours_never_in_thousands_of_minutes() -> None:
    """Owner 2026-11-09: a friend's Coursera schedule (Tuesdays 10:37 AM and Fridays 10:37 PM, saved as every 5,040
    minutes from Tuesday 10:37) showed "Every 5040 minutes" on its card."""
    plan = plan_new(_spec("check Coursera for due assignments on Tuesdays at 10:37 AM and Fridays at 10:37 PM",
                          When(time="10:37"), "every", 5040), NOW, "seed")
    said = plan.schedule.when_said()
    assert "5040" not in said and "3 days and 12 hours" in said
