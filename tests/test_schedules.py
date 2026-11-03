"""Schedules preserve owner instructions and timing/failure facts for agent decisions; owner controls recurrence."""

import asyncio
import json
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo

import pytest

from ferb.cli.client import Client
from ferb.orchestrator.schedules import Rule, ScheduleError, until_said
from ferb.policy.targets import Request, Targets
from tests.conftest import Answers, RunningDaemon
from tests.conftest import chat, install, sup, task, writes
from tests.support.schedules import fire, make_schedule, results, run, schedule_reply

NY = ZoneInfo("America/New_York")


# ---------- rules ----------

# decision: ORCH-334
def test_recurrence_keeps_local_time_across_daylight_saving() -> None:
    first = datetime(2026, 10, 30, 9, 0, tzinfo=NY)
    daily = Rule.build("daily", first, tz=NY)
    after_dst = daily.next_after(datetime(2026, 10, 31, 14, 0, tzinfo=timezone.utc), first)
    assert after_dst is not None and after_dst.astimezone(NY).hour == 9           # 09:00 local, EDT -> EST
    weekdays = Rule.build("weekdays", datetime(2026, 10, 2, 9, 0, tzinfo=NY), tz=NY)
    nxt = weekdays.next_after(datetime(2026, 10, 2, 14, 0, tzinfo=timezone.utc), first)
    assert nxt is not None and nxt.astimezone(NY).weekday() == 0                  # Friday -> Monday
    weekly = Rule.build("weekly", first, "monday", tz=NY)
    assert weekly.next_after(first, first).astimezone(NY).weekday() == 0          # type: ignore[union-attr]
    with pytest.raises(ScheduleError, match="positive"):
        Rule.build("every", first, every_minutes=0, tz=NY)
    assert Rule.build("none", first, tz=NY).next_after(first, first) is None


# ---------- schedules from chat ----------

# decision: ORCH-324
def test_a_schedule_needs_the_owners_words_and_a_future_first_run(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    unsaid = schedule_reply(daemon, "wire money", "send money", now + timedelta(days=1))
    install(daemon, [schedule_reply(daemon, "write report.md", "tomorrow at 9 write report.md",
                                    now + timedelta(days=1)),
                     schedule_reply(daemon, "back up notes.md", "back up notes.md", now + timedelta(days=1)),
                     unsaid, unsaid,               # words the owner never wrote: told back once (ORCH-520), refused
                     schedule_reply(daemon, "write report.md", "write report.md now", now - timedelta(hours=1))])
    first = [e["text"] for e in chat(daemon, "tomorrow at 9 write report.md") if e["type"] == "timer"]
    assert first and first[0].startswith("🗓 Scheduled: ")              # plain words, no id (L11)
    pasted = [e["text"] for e in chat(daemon, 'tomorrow ```back up notes.md```') if e["type"] == "timer"]
    assert pasted[0].startswith("🗓 Scheduled: ")                      # a paste is the owner's words (ORCH-518)
    refused = [e["text"] for e in chat(daemon, "do this now") if e["type"] == "timer"]
    assert refused[0].startswith("Blocked by a safety check: reminders and schedules are set only from words you "
                                 "wrote in this chat.") and "“wire money” isn't scheduled" in refused[0]   # ORCH-519
    past = [e["text"] for e in chat(daemon, "write report.md now") if e["type"] == "timer"]
    assert "couldn't schedule" in past[0]
    assert len(run(daemon, daemon.daemon.schedules.all)) == 2


# decision: ORCH-512
def test_remembered_address_does_not_rewrite_schedule_recipient(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    ask = schedule_reply(daemon, "email me the report", "every morning at 9 email me the report",
                         now + timedelta(hours=10), "daily", recipients=("me",), paths=())
    install(daemon, [sup("clarify", "What email address should I use for you?"), json.dumps({"kind": "reply", "text": "Got it.", "task": None, "timers": [],
                                      "schedule": None, "schedule_changes": [], "remember": [
                                          {"kind": "fact", "subject": "owner", "key": "email",
                                           "content": "The owner's email is me@owner.example",
                                           "quote": "my email is me@owner.example"}]}), ask])
    evs = chat(daemon, "every morning at 9 email me the report")
    assert "What email address" in [e["text"] for e in evs if e["type"] == "clarify"][0]  # the planner asks
    chat(daemon, "my email is me@owner.example")
    evs = chat(daemon, "every morning at 9 email me the report")
    assert [e["text"] for e in evs if e["type"] == "timer"][0].startswith("🗓 Scheduled: ")
    s = run(daemon, daemon.daemon.schedules.all)[0]
    assert s.targets["recipients"] == ["me"] and "every day at" in s.rule.describe()


# ---------- runs ----------


# decision: ORCH-323
def test_a_scheduled_run_never_asks_the_owner_a_question(daemon: RunningDaemon) -> None:
    """Nobody is at chat during a scheduled run: owner_ask is declined at once, with a note for the worker."""
    now = daemon.daemon.clock.now()
    from tests.conftest import tool
    seen: dict[str, Any] = {}

    def worker(token: str) -> str:
        seen["r"] = tool(daemon.sock, token, "owner.ask", question="Which report?", options=["a", "b"])
        return json.dumps({"status": "done", "summary": "wrote it", "artifacts": [], "evidence": []})
    make_schedule(daemon, "write report.md", "every day at 9 write report.md",
                  Targets.of(paths=[str(daemon.daemon.home / "workspace" / "report.md")]), now + timedelta(minutes=1))
    install(daemon, [sup("task", "x", task("write report.md", paths=("report.md",)))], {"worker": worker})
    fire(daemon, 61)
    assert seen["r"]["content"] == {"declined": True, "reason": seen["r"]["content"]["reason"]}
    assert "scheduled run" in seen["r"]["content"]["reason"]


# decision: ORCH-314
def test_repeated_failure_does_not_silently_pause_a_recurring_task(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    scheduled = make_schedule(daemon, 'Check source', 'Check source every day', Targets(),
                              now + timedelta(minutes=1))
    for _ in range(4):
        run(daemon, lambda: daemon.daemon.schedules.ran(scheduled.id, False))
    script = install(daemon, [sup('reply', 'Source still unavailable.')])
    fire(daemon, 61)
    current = run(daemon, lambda: daemon.daemon.schedules.get(scheduled.id))
    assert current.state == 'active' and current.next_run > daemon.daemon.clock.now()
    assert '"consecutive_failures": 4' in script.contexts[0][2]


# ---------- progressive setup: folders ----------


# ---------- time awareness ----------


def test_non_urgent_notifications_wait_while_the_mac_is_out_of_use(daemon: RunningDaemon) -> None:
    """APP-301: a banner is for when the notch can't show; while the displays sleep it waits for them to wake.
    The owner not touching input (a film) holds nothing."""
    from ferb.daemon.banners import Banner
    from tests.support.schedules import mac_away, mac_back, watching
    notes: list[tuple[str, str]] = []

    async def note(title: str, text: str) -> None:
        notes.append((title, text))
    d = daemon.daemon
    d.notify = note
    watching(daemon)
    d.owner_input.running = True
    d.owner_input.last_owner_wall = d.clock.now().timestamp() - 3600               # no input for an hour
    daemon.run(d._notify("FERB note", "a film is on"))
    assert notes == [("FERB note", "a film is on")] and d._held == []
    notes.clear()
    mac_away(daemon, "displays_asleep")
    daemon.run(d._notify("FERB finished", "done"))
    daemon.run(d._notify("FERB reminder", "call Mom", urgent=True))
    assert notes == [("FERB reminder", "call Mom")] and d._held == [Banner("FERB finished", "done")]
    mac_back(daemon, "displays_asleep")                                            # the screens wake
    for _ in range(150):
        if len(notes) == 2:
            break
        time.sleep(0.1)
    d.owner_input.running = False
    d.presence.stop()
    assert notes[-1] == ("FERB finished", "done")


# ---------- final review M4: a run nobody started from chat leaves no per-request context behind ----------

@pytest.mark.parametrize("decision", ["task", "reply"])
def test_a_scheduled_run_clears_its_request_context_when_it_ends(daemon: RunningDaemon, decision: str) -> None:
    """Each run's planner blocks and context exposures (since L28, the full text of every planner block) were kept in
    memory until the daemon restarted: 96 entries a day for an every-15-minutes schedule."""
    now = daemon.daemon.clock.now()
    make_schedule(daemon, "write report.md", "every day at 9 write report.md",
                  Targets.of(paths=[str(daemon.daemon.home / "workspace" / "report.md")]), now + timedelta(minutes=1))
    planned = sup("task", "Writing it.", task("write report.md", paths=("report.md",))) if decision == "task" else \
        sup("reply", "Nothing to do today.")
    install(daemon, [planned], {"worker": writes(daemon.sock, "report.md", "daily")})
    supervisor = daemon.daemon.supervisor
    assert supervisor is not None
    before = set(run(daemon, lambda: set(supervisor._turns)))
    fire(daemon, 61)
    assert results(daemon)                                            # the run happened
    assert run(daemon, lambda: set(supervisor._turns)) == before


# ---------- a recurring run compares with the one before (live: sc-933b051c3c, the daily Contest rank) ----------

def _rank_worker(text: str) -> Any:
    def act(token: str) -> str:
        return json.dumps({"status": "done", "summary": text, "artifacts": [], "evidence": []})
    return act


# decision: ORCH-512
def test_a_previous_runs_answer_keeps_its_own_origins(daemon: RunningDaemon) -> None:
    """MP2: what an earlier run read from outside stays outside when the next run is shown its answer."""
    from ferb.core import origin
    now = daemon.daemon.clock.now()
    request = "check my ORBIT Contest rank and notify me if it falls or improves"
    s = make_schedule(daemon, request, "every day at 9 " + request, Targets.of(), now + timedelta(minutes=1))
    supervisor = daemon.daemon.supervisor
    assert supervisor is not None
    outside = origin.external("web", "contest.example")

    def earlier() -> None:                     # a run before this change: no conversation, only its journal rows
        supervisor.journal.append("schedule.run", {"schedule_id": s.id, "request_id": "req-earlier", "late": False})
        supervisor.journal.append("task.answer", {"task_id": "tree-earlier", "request_id": "req-earlier",
                                                  "status": "done", "text": "Rank 146, score 0.957", "display_text": ""},
                                  origin=origin.join({origin.INFERRED, outside}))
    run(daemon, earlier)
    second = install(daemon, [sup("task", "Checking.", task("check the ORBIT Contest rank"))],
                     {"worker": _rank_worker("Rank 205 (was 146)")})
    fire(daemon, 61)
    assert "Rank 146, score 0.957" in second.contexts[0][2]
    tree = results(daemon)[0]["task_id"]
    assert outside in run(daemon, lambda: supervisor.spans.task_origins(tree))


# ---------- step 8a: schedule bookkeeping (kb area storage findings 4, 5; kb area orchestration) ----------

# decision: ORCH-511
def test_an_every_schedule_resumed_before_its_first_run_keeps_that_first_run() -> None:
    """Resumed less than one interval before its first run, `next_after(first_run - 1s)` skipped to the second."""
    first = datetime(2026, 10, 30, 9, 0, tzinfo=timezone.utc)
    rule = Rule.build("every", first, every_minutes=60, tz=ZoneInfo("UTC"))
    assert rule.next_after(first - timedelta(seconds=1), first) == first
    assert rule.next_after(first - timedelta(minutes=59), first) == first
    assert rule.next_after(first, first) == first + timedelta(minutes=60)
    assert rule.next_after(first + timedelta(minutes=61), first) == first + timedelta(minutes=120)


# decision: ORCH-511
def test_a_schedule_whose_timer_was_handled_but_not_rebooked_is_reconciled_at_start(daemon: RunningDaemon) -> None:
    """A crash between marking a schedule's timer handled and booking its next run stranded it: nothing fired again."""
    from ferb.clock.timers import mark_handled
    first = daemon.daemon.clock.now() + timedelta(minutes=1)
    s = make_schedule(daemon, "check the build", "every day check the build", Targets.of(), first)
    run(daemon, lambda: mark_handled(daemon.daemon.db, s.timer_id, daemon.daemon.clock.now()))    # then the crash
    run(daemon, daemon.daemon._reconcile_schedules)
    after = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
    assert after.timer_id != s.timer_id and after.next_run == first
    assert run(daemon, lambda: daemon.daemon.timers.get(after.timer_id)).state == "pending"
    run(daemon, daemon.daemon._reconcile_schedules)                                  # booked once, not again
    assert run(daemon, lambda: daemon.daemon.schedules.get(s.id)).timer_id == after.timer_id


# decision: ORCH-511
def test_a_scheduled_run_never_overlaps_its_previous_run(daemon: RunningDaemon) -> None:
    first = daemon.daemon.clock.now() + timedelta(minutes=1)
    s = make_schedule(daemon, "check the build", "every 5 minutes check the build", Targets.of(), first, "every", 5)
    started: list[str] = []

    async def slow(sched: Any, **_: Any) -> dict[str, Any]:
        started.append(sched.id)
        await asyncio.sleep(0.5)
        return {"task_id": "t", "status": "done", "answer": "ok"}
    daemon.daemon.supervisor.run_scheduled = slow                     # type: ignore[method-assign]

    async def twice() -> None:
        await asyncio.gather(daemon.daemon._run_schedule(s.id, 0), daemon.daemon._run_schedule(s.id, 0))
    daemon.run(twice())
    assert started == [s.id]
    skipped = run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.skipped", "schedule_id", s.id))
    assert [e.data["why"] for e in skipped] == ["the previous run is still under way"]


def test_a_scheduled_run_respects_the_global_stop(daemon: RunningDaemon) -> None:
    first = daemon.daemon.clock.now() + timedelta(minutes=1)
    s = make_schedule(daemon, "check the build", "every day check the build", Targets.of(), first)
    install(daemon, [], {})                                           # no planner turn may run
    daemon.daemon.state.stopped = True
    try:
        daemon.run(daemon.daemon._run_schedule(s.id, 0))
    finally:
        daemon.daemon.state.stopped = False
    skipped = run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.skipped", "schedule_id", s.id))
    assert [e.data["why"] for e in skipped] == ["FERB was stopped"]


# ---------- ORCH-320: a schedule's end from the owner's words ----------

def _until(quote: str, **said: Any) -> dict[str, Any]:
    """The planner's `until` (unused fields "" / 0, as the schema has them)."""
    return {"quote": quote, "day": "", "date": "", "in_days": 0, "time": "", "in_minutes": 0} | said


def _ending(d: RunningDaemon, request: str, said: str, until: dict[str, Any], every: int = 60) -> str:
    reply = json.loads(schedule_reply(d, request, said, d.daemon.clock.now() + timedelta(minutes=1), repeat="every",
                                      paths=(), every=every))
    reply["schedule"]["until"] = until
    return json.dumps(reply)


# decision: ORCH-320
def test_a_schedule_ends_on_the_date_the_owners_words_give(daemon: RunningDaemon) -> None:
    """The planner states the date in a typed field; code works out the end (the end of that local day) and stores it;
    the scheduling line, the event card, the planner's facts and the schedule list show it."""
    from ferb.clock.when import When, end_of_day, local_zone
    now = daemon.daemon.clock.now()
    day = (now.astimezone(local_zone()) + timedelta(days=2)).date()
    words = f"until {day:%b} {day.day}"
    said = f"every 3 hours remind me to apply to YC while applications are open {words}"
    install(daemon, [_ending(daemon, "remind me to apply to YC", said, _until(words, date=f"{day:%m-%d}"), 180)])
    reply = next(e for e in chat(daemon, said) if e["type"] == "reply")
    [s] = run(daemon, daemon.daemon.schedules.all)
    assert s.until == end_of_day(When(date=f"{day:%m-%d}"), now)
    [card] = [c for c in reply["cards"] if c["type"] == "event"]
    assert card["until"] == s.until.isoformat()
    assert f"until {day:%a %d %b}" in card["line"] and card["line"] in reply["text"].split("\n")
    assert f"until {day:%a %d %b}" in run(daemon, daemon.daemon._live_facts)
    c = Client(daemon.sock)
    try:
        assert c.call("schedules.list")["schedules"][0]["until"] == s.until.isoformat()
    finally:
        c.close()


# decision: ORCH-320
def test_no_run_is_booked_after_a_schedules_end_and_its_last_run_says_it_ended(daemon: RunningDaemon) -> None:
    """"for the next 3 hours", every hour from a minute on: runs at +1, +61 and +121 minutes; the one at +181 would be
    past the end, so the run at +121 is the last, says so, and nothing is booked after it."""
    said = "check the build every hour for the next 3 hours"
    install(daemon, [_ending(daemon, "check the build", said, _until("for the next 3 hours", in_minutes=180))])
    chat(daemon, said)
    [s] = run(daemon, daemon.daemon.schedules.all)
    assert s.until == daemon.daemon.clock.now() + timedelta(minutes=180)
    answers = []
    for wait in (61, 3600, 3600):
        install(daemon, [sup("reply", "The build is green.")])
        fire(daemon, wait)
        answers += [r["answer"] for r in results(daemon)]
    assert len(answers) == 3
    assert not any("Schedule ended" in a for a in answers[:2])
    assert answers[2].endswith(f"🗓 Schedule ended: check the build (it ran {until_said(s.until)})")
    after = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
    assert after.state == "done" and after.next_run is None and after.timer_id is None
    install(daemon, [], {})                                           # no planner turn may run again
    fire(daemon, 3600 * 5)
    assert results(daemon) == []
    assert len(run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.ended", "schedule_id", s.id))) == 1


# decision: ORCH-518, ORCH-520
def test_a_reminder_set_again_quotes_the_owners_earlier_message_or_their_paste(daemon: RunningDaemon) -> None:
    """Live 2026-11-04: "set the reminder again" re-created a reminder asked for earlier in the chat, and code refused
    it because the quote was only in the earlier message. The owner's recent messages in this chat count (ORCH-520),
    and so does what they paste, leaked terminal paste markers removed (ORCH-518)."""
    asked = "remind me every 2 hours until 2pm tomorrow to finish the grant form"
    again = _ending(daemon, "finish the grant form", asked, _until("until 2pm tomorrow", day="tomorrow", time="14:00"),
                    every=120)
    stretch = schedule_reply(daemon, "stretch", "remind me every 2 hours to stretch",
                             daemon.daemon.clock.now() + timedelta(hours=2), repeat="every", paths=(), every=120)
    install(daemon, [sup("reply", "Noted."), again, stretch])
    chat(daemon, asked)
    lines = [e["text"] for e in chat(daemon, "set the reminder again") if e["type"] == "timer"]
    assert lines[0].startswith("🗓 Scheduled: finish the grant form")
    pasted = "\x1b[200~[200~remind me every 2 hours to stretch[201~\x1b[201~"
    lines = [e["text"] for e in chat(daemon, pasted) if e["type"] == "timer"]
    assert lines[0].startswith("🗓 Scheduled: stretch")
    assert sorted(s.request for s in run(daemon, daemon.daemon.schedules.all)) == ["finish the grant form", "stretch"]
    assert run(daemon, lambda: daemon.daemon.journal.of_kind("task.repair")) == []


# decision: ORCH-519, ORCH-520, ORCH-522
def test_a_reply_never_claims_what_the_quote_check_refuses(daemon: RunningDaemon) -> None:
    """Live 2026-11-04: FERB's reply said it was restoring a reminder, then code refused it. The planner is told the
    refusal before its reply stands (ORCH-520); a refusal that still happens is said as a safety check (ORCH-519)."""
    claim = json.loads(schedule_reply(daemon, "water the plants", "remind me daily to water the plants",
                                      daemon.daemon.clock.now() + timedelta(days=1), repeat="daily", paths=()))
    claim["text"] = "Restoring your plant reminder."
    honest = sup("reply", "I can't find where you asked for a plant reminder. Tell me when you'd like it and I'll set "
                          "it.")
    install(daemon, [json.dumps(claim), honest, json.dumps(claim), json.dumps(claim)])
    [reply] = [e["text"] for e in chat(daemon, "restore the old one") if e["type"] == "reply"]
    assert "Restoring" not in reply and reply.startswith("I can't find where you asked")
    [problems] = [e.data["problems"] for e in run(daemon, lambda: daemon.daemon.journal.of_kind("task.repair"))]
    assert any("remind me daily to water the plants" in p for p in problems)
    [reply] = [e["text"] for e in chat(daemon, "restore the old one please") if e["type"] == "reply"]
    assert reply.endswith("Blocked by a safety check: reminders and schedules are set only from words you wrote in "
                          "this chat. The words I quoted from you aren't in this message or your recent messages in "
                          "this chat, so “water the plants” isn't scheduled. Type what you want scheduled in this "
                          "chat and I'll set it up.")
    assert run(daemon, daemon.daemon.schedules.all) == []


# decision: ORCH-320, ORCH-324
def test_end_date_words_the_owner_did_not_type_refuse_the_schedule(daemon: RunningDaemon) -> None:
    said = "every hour check the build"
    install(daemon, [_ending(daemon, "check the build", said, _until("until Friday", day="friday"))] * 2)
    lines = [e["text"] for e in chat(daemon, said) if e["type"] == "timer"]
    assert lines and lines[0].startswith("Blocked by a safety check: an end date comes only from words you wrote")
    assert run(daemon, daemon.daemon.schedules.all) == []


# decision: ORCH-320
def test_a_schedule_change_gives_an_existing_schedule_an_end(daemon: RunningDaemon) -> None:
    first = daemon.daemon.clock.now() + timedelta(minutes=1)
    s = make_schedule(daemon, "check the build", "every hour check the build", Targets.of(), first, "every", 60)
    said = "keep checking the build for 2 more hours"
    install(daemon, [json.dumps({"kind": "reply", "text": "Okay.", "schedule_changes": [
        {"id": s.id, "action": "set_end", "quote": "keep checking the build",
         "until": _until("for 2 more hours", in_minutes=120)}]})])
    lines = [e["text"] for e in chat(daemon, said) if e["type"] == "timer"]
    after = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
    assert after.until == daemon.daemon.clock.now() + timedelta(minutes=120) and after.state == "active"
    assert lines == [f"🗓 check the build now runs {until_said(after.until)}."]


def test_a_schedule_made_before_ends_existed_runs_on_unchanged(tmp_path: Path, daemon: RunningDaemon) -> None:
    """Upgrade (STORE-403): a schedule row from before migration 0052 has no end; it reads as one
    without, its runs keep being booked, and its run card counts down to the next run as before. 0052 re-runs."""
    import shutil
    from ferb.clock.clock import FakeClock
    from ferb.core.ids import SequentialIds
    from ferb.orchestrator.schedules import Schedules
    from ferb.store import db as dbmod
    older = tmp_path / "m"
    older.mkdir()
    for f in dbmod.MIGRATIONS.glob("[0-9][0-9][0-9][0-9]_*.sql"):
        if int(f.name[:4]) <= 51:
            shutil.copy(f, older / f.name)
    db = dbmod.Database(tmp_path / "old.db")
    db.migrate(older)
    clock = FakeClock()
    first = clock.now() + timedelta(minutes=1)
    rule = Rule.build("every", first, every_minutes=60)
    db.conn.execute("INSERT INTO schedules (id, request, own_words, intent_json, rule_json, next_run, state, "
                    "created_at) VALUES ('sc-old', 'check the build', 'every hour check the build', '{}', ?, ?, "
                    "'active', ?)", (json.dumps(rule.__dict__ | {"first": first.isoformat()}), first.isoformat(),
                                     clock.now().isoformat()))
    assert db.migrate()[0] == 52                                    # and any later ones
    for statement in dbmod._split_sql((dbmod.MIGRATIONS / "0052_schedule_ends.sql").read_text()):
        db.conn.execute(statement)                                   # re-runnable
    old = Schedules(db, clock, SequentialIds("x")).get("sc-old")
    assert old is not None and old.until is None and old.describe() == "every 60 minutes"
    assert old.next_after(first + timedelta(days=30)) is not None and not old.ends_after(first + timedelta(days=30))
    s = make_schedule(daemon, "check the build", "every hour check the build", Targets.of(),
                      daemon.daemon.clock.now() + timedelta(minutes=1), "every", 60)
    install(daemon, [sup("reply", "The build is green.")])
    fire(daemon, 61)
    [result] = results(daemon)
    [card] = [c for c in result["cards"] if c["type"] == "reminder"]
    after = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
    assert "until" not in card and card["next_run"] == after.next_run.isoformat() and after.state == "active"
    assert "Schedule ended" not in result["answer"]


# ---------- ORCH-313: what every run uses is recorded when the schedule is made ----------

def _scoped_schedule(d: RunningDaemon, words: str, folders: list[str], identity: str, profile: str) -> str:
    reply = json.loads(schedule_reply(d, "tidy my downloads", words, d.daemon.clock.now() + timedelta(minutes=1),
                                      repeat="daily", paths=()))
    reply["schedule"] |= {"until": None, "when_active": False, "folders": folders, "identity": identity,
                          "worker_type": "default", "profile": profile, "account": ""}
    return json.dumps(reply)


# decision: ORCH-313
def test_a_scheduled_run_uses_the_folders_and_identity_recorded_when_it_was_made(daemon: RunningDaemon) -> None:
    """ORCH-313 (owner, 2026-10-30): folders, profile/identity and worker type are fixed when the schedule is made
    and stated then; an unattended run never re-guesses them. The run's plan proposes other folders and FERB's own
    identity: the run works in the recorded folder as the owner, and its worker is given the recorded scope."""
    words = "every day tidy my downloads folder in my Work profile"
    sv = daemon.daemon.supervisor
    seen: dict[str, Any] = {}

    def worker(token: str) -> str:
        grant = daemon.daemon.tokens.validate(token)
        seen["folders"] = sv.gateway.task_folders.get(grant.task_id.split(".", 1)[0], ())
        seen["identity"] = grant.identity
        return json.dumps({"status": "done", "summary": "Tidied.", "artifacts": [], "evidence": []})
    proposed = task("Tidy the documents") | {"folders": ["~/Documents"], "identity": "ferb"}
    script = install(daemon, [_scoped_schedule(daemon, words, ["~/Downloads"], "owner", "Work"),
                              sup("task", "Tidying.", proposed)], {"worker": worker})
    told = "\n".join(str(e.get("text")) for e in chat(daemon, words))
    assert "~/Downloads" in told and "Work" in told                    # stated, so the owner can correct it
    [s] = run(daemon, daemon.daemon.schedules.all)
    downloads = str(Path.home() / "Downloads")
    assert s.run_scope == {"folders": [downloads], "identity": "owner", "worker_type": "default", "profile": "Work",
                           "account": ""}
    fire(daemon, 61)
    assert seen["folders"] == (downloads,) and getattr(seen["identity"], "value", seen["identity"]) == "owner"
    run_planner = [ctx for _, key, ctx, _ in script.contexts if key == "supervisor"][-1]
    assert "recorded_scope" in run_planner                              # the run's planner sees it as a fact
    worker_ctx = [ctx for _, key, ctx, _ in script.contexts if key == "worker"][-1]
    assert "Schedule facts" in worker_ctx and "recorded_scope" in worker_ctx and "Work" in worker_ctx
    applied = run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.scope_applied", "schedule_id", s.id))
    assert applied and applied[-1].data["proposed"]["identity"] == "ferb"


def test_a_schedule_made_before_decision_14_plans_each_run_as_before(daemon: RunningDaemon) -> None:
    """An older schedule (no recorded scope) keeps working: its run's task keeps the folders its plan resolved."""
    sv = daemon.daemon.supervisor
    seen: dict[str, Any] = {}

    def worker(token: str) -> str:
        grant = daemon.daemon.tokens.validate(token)
        seen["folders"] = sv.gateway.task_folders.get(grant.task_id.split(".", 1)[0], ())
        return json.dumps({"status": "done", "summary": "Tidied.", "artifacts": [], "evidence": []})
    s = make_schedule(daemon, "tidy my downloads", "every day tidy my downloads", Targets.of(),
                      daemon.daemon.clock.now() + timedelta(minutes=1))
    assert s.run_scope is None
    install(daemon, [sup("task", "Tidying.", task("Tidy the downloads") | {"folders": ["~/Downloads"]})],
            {"worker": worker})
    fire(daemon, 61)
    assert seen["folders"] == (str(Path.home() / "Downloads"),)
