""""Tell me when …" is a watch: a durable schedule, so it survives restarts and updates (a loop inside a task died on
every `ferb setup --update`: owner live case 2026-10-15, Low Power Mode plus a loud alert at 10% battery). A one-shot
watch ends itself once a confirmed run finds its condition met; a standing one keeps watching and tells the owner once
per crossing; a met condition reaches the owner the way a reminder does (fix wave 6c)."""
import json
import time
from datetime import timedelta
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.orchestrator.schemas import SCHEDULE_SCHEMA, WORKER_WATCH_SCHEMA, ScheduleRequest
from ferb.policy.targets import Targets
from tests.conftest import RunningDaemon, chat, done, install, outcome, sup, task
from tests.support.bus import Subscriber, until
from tests.support.schedules import fire, make_schedule, own_daemon, results, run, schedule_reply

FILE_WORDS = "tell me when ~/Downloads/done.txt exists"
FILE_WATCH = "Check whether ~/Downloads/done.txt exists and tell me once it does."
BATTERY_WORDS = "turn on Low Power Mode and alert me loudly when the battery is at or below 12%"
BATTERY_WATCH = "Check the battery level and alert me loudly each time it is at or below 12%."


def _check(met: bool | None, text: str, status: str = "done") -> Any:
    """A watch run's worker: its fresh observation, and whether the watched condition holds now."""
    def act(token: str) -> str:
        return json.dumps({"status": status, "summary": text, "answer": text, "artifacts": [], "evidence": [],
                           "watch_met": met})
    return act


def _notes(d: RunningDaemon) -> list[tuple[str, str]]:
    notes: list[tuple[str, str]] = []

    async def note(title: str, text: str) -> None:
        notes.append((title, text))
    d.daemon.notify = note
    return notes


def _watch(d: RunningDaemon, request: str, words: str, watch: str) -> Any:
    now = d.daemon.clock.now()
    return make_schedule(d, request, words, Targets.of(), now + timedelta(minutes=15), "every", 15, watch)


def _next_run(d: RunningDaemon, sid: str, workers: dict[str, Any]) -> Any:
    """Fire the schedule's next check with this worker; returns the planner's script."""
    script = install(d, [sup("task", "Checking.", task("check it once"))], workers)
    nxt = run(d, lambda: d.daemon.schedules.get(sid)).next_run
    fire(d, (nxt - d.daemon.clock.now()).total_seconds() + 1)
    return script


def _journal(d: RunningDaemon, kind: str, sid: str) -> list[Any]:
    supervisor = d.daemon.supervisor
    assert supervisor is not None
    return run(d, lambda: supervisor.journal.of_kind(kind, "schedule_id", sid))     # type: ignore[no-any-return]


def _pending_checks(d: RunningDaemon) -> list[Any]:
    return run(d, lambda: d.daemon.timers.pending(("schedule",)))     # type: ignore[no-any-return]


# ---------- the planner marks a watch; it is stored with the schedule ----------

def test_the_planner_marks_a_watch_and_it_is_stored_with_the_schedule(daemon: RunningDaemon) -> None:
    assert SCHEDULE_SCHEMA["properties"]["watch"] == {"type": "string", "enum": ["none", "until_met", "standing"]}
    assert "watch" in SCHEDULE_SCHEMA["required"]                       # strict structured output: always written
    assert WORKER_WATCH_SCHEMA["properties"]["watch_met"] == {"type": ["boolean", "null"]}
    saved = json.loads(schedule_reply(daemon, FILE_WATCH, FILE_WORDS, daemon.daemon.clock.now()))["schedule"]
    del saved["watch"]
    assert ScheduleRequest.model_validate(saved).watch == "none"         # a spec written before watches
    install(daemon, [schedule_reply(daemon, FILE_WATCH, FILE_WORDS, daemon.daemon.clock.now() + timedelta(minutes=15),
                                    "every", every=15, watch="until_met")])
    line = [e["text"] for e in chat(daemon, FILE_WORDS) if e["type"] == "timer"][0]
    assert line.startswith(f"🗓 Scheduled: {FILE_WATCH} — every 15 minutes, first on ")
    assert line.endswith(", until it happens, then it stops")           # how FERB understood it, said once
    [s] = run(daemon, daemon.daemon.schedules.all)
    assert (s.watch, s.rule.describe()) == ("until_met", "every 15 minutes")
    assert _journal(daemon, "schedule.created", s.id)[0].data["watch"] == "until_met"
    c = Client(daemon.sock)
    try:
        assert c.call("schedules.list")["schedules"][0]["watch"] == "until_met"
    finally:
        c.close()
    assert f"- Schedule {s.id} (active): {FILE_WATCH} — every 15 minutes, until it happens, then it stops" in \
        run(daemon, daemon.daemon._live_facts)                               # the planner's facts say it too


def test_work_now_and_a_watch_come_from_one_message(daemon: RunningDaemon) -> None:
    """The live case: Low Power Mode now, and the alert as a watch, from the one turn (the schema has both fields)."""
    reply = json.loads(schedule_reply(daemon, BATTERY_WATCH, BATTERY_WORDS,
                                      daemon.daemon.clock.now() + timedelta(minutes=15), "every", every=15,
                                      watch="standing"))
    reply |= {"kind": "task", "text": "Turning on Low Power Mode.", "task": task("Turn on Low Power Mode")}
    script = install(daemon, [json.dumps(reply)], {"worker": done("Low Power Mode is on.")})
    out = outcome(chat(daemon, BATTERY_WORDS))
    assert "worker" in [key for _, key, _, _ in script.contexts]          # the work ran now
    assert out["status"] == "done" and out["answer"].startswith("Low Power Mode is on.")
    assert f"🗓 Scheduled: {BATTERY_WATCH} — every 15 minutes" in out["answer"]
    [s] = run(daemon, daemon.daemon.schedules.all)
    assert s.watch == "standing" and s.state == "active" and len(_pending_checks(daemon)) == 1


# ---------- a one-shot watch ends itself ----------

def test_a_one_shot_watch_ends_itself_once_a_confirmed_run_finds_it_met(daemon: RunningDaemon) -> None:
    notes = _notes(daemon)
    s = _watch(daemon, FILE_WATCH, FILE_WORDS, "until_met")
    first = install(daemon, [sup("task", "Checking.", task("check whether done.txt exists"))],
                    {"worker": _check(False, "done.txt isn't in Downloads yet.")})
    fire(daemon, 15 * 60 + 1)
    assert '"watch": "until_met"' in first.contexts[0][2]                 # the run's planner knows what it runs
    assert run(daemon, lambda: daemon.daemon.schedules.get(s.id)).state == "active"
    assert results(daemon) == [] and notes == []                          # a check that found nothing stays quiet
    _next_run(daemon, s.id, {"worker": _check(True, "done.txt is in Downloads now.")})
    ended = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
    assert (ended.state, ended.timer_id, ended.next_run) == ("done", None, None)
    assert _pending_checks(daemon) == []                                  # the next check was cleared
    [met] = _journal(daemon, "schedule.met", s.id)
    [answer] = results(daemon)
    assert met.data["task_id"] == answer["task_id"]
    assert answer["answer"].startswith("done.txt is in Downloads now.")
    assert answer["answer"].endswith(f"🗓 Stopped checking: {FILE_WATCH}")
    # the alert is the full answer, stop line included (owner, 2026-10-24: no separate short text)
    assert notes == [("FERB alert", f"done.txt is in Downloads now.\n\n🗓 Stopped checking: {FILE_WATCH}")]
    c = Client(daemon.sock)
    try:
        assert c.call("schedules.list")["schedules"] == []
    finally:
        c.close()


UNRESOLVED = json.dumps({"results": [{"index": 0, "met": None, "evidence": "Downloads couldn't be listed."}]})
REFUTED = json.dumps({"results": [{"index": 0, "met": False, "evidence": "Downloads has no done.txt."}]})
FINISH_UNCONFIRMED = json.dumps({"action": "finish", "worker_type": "default", "instructions": "Stop and report.",
                                 "question": "", "stop_reason": "", "result": "unconfirmed"})


@pytest.mark.parametrize("how, workers, status", [
    ("not met", {"worker": _check(False, "done.txt isn't there.")}, "done"),
    ("unconfirmed", {"worker": _check(True, "done.txt is there."), "worker.verify": UNRESOLVED,
                     "worker.dispatch": FINISH_UNCONFIRMED}, "done_unverified"),
    ("failed", {"worker": _check(True, "done.txt is there."), "worker.verify": REFUTED}, "failed"),
])
def test_only_a_confirmed_met_run_ends_a_one_shot_watch(daemon: RunningDaemon, how: str, workers: dict[str, Any],
                                                        status: str) -> None:
    notes = _notes(daemon)
    s = _watch(daemon, FILE_WATCH, FILE_WORDS, "until_met")
    install(daemon, [sup("task", "Checking.", task("check whether done.txt exists"))], workers)
    fire(daemon, 15 * 60 + 1)
    kept = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
    assert kept.state == "active" and [t.id for t in _pending_checks(daemon)] == [kept.timer_id]
    assert _journal(daemon, "schedule.met", s.id) == []
    told = results(daemon)
    assert [r["status"] for r in told] == ([] if how == "not met" else [status])
    assert "Stopped checking" not in json.dumps(told)
    if how == "unconfirmed":           # it may have happened: told now, like a reminder; it keeps checking
        assert [title for title, _ in notes] == ["FERB alert"]
    elif how == "failed":              # a failed check is told as any scheduled run's failure is, never as an alert
        assert [title for title, _ in notes] == ["FERB"]
    else:
        assert notes == []


# ---------- a standing watch keeps watching and tells once per crossing ----------

def test_a_standing_watch_tells_once_per_crossing_and_never_ends_itself(daemon: RunningDaemon) -> None:
    notes = _notes(daemon)
    s = _watch(daemon, BATTERY_WATCH, BATTERY_WORDS, "standing")
    install(daemon, [sup("task", "Checking.", task("check the battery level"))],
            {"worker": _check(True, "The battery is at 12%.")})
    fire(daemon, 15 * 60 + 1)
    assert notes == [("FERB alert", "The battery is at 12%.")]
    assert [r["answer"] for r in results(daemon)] == ["The battery is at 12%."]
    still = _next_run(daemon, s.id, {"worker": _check(True, "The battery is at 11%, still at or below 12%.")})
    # the run sees what its previous run found (the mechanism the Kaggle-rank schedule's runs use, live check 55)
    assert '"last_reported_met": true' in still.contexts[0][2] and "The battery is at 12%." in still.contexts[0][2]
    _next_run(daemon, s.id, {"worker": _check(False, "The battery is at 40%.")})
    assert len(notes) == 1 and results(daemon) == []                     # still true, then not: both quiet
    again = _next_run(daemon, s.id, {"worker": _check(True, "The battery is at 10%.")})
    assert '"last_reported_met": false' in again.contexts[0][2]
    assert notes[1:] == [("FERB alert", "The battery is at 10%.")]       # it crossed again: told again
    current = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
    assert current.state == "active" and current.next_run > daemon.daemon.clock.now()
    assert _journal(daemon, "schedule.met", s.id) == []


# ---------- the alert reaches the owner ----------

def test_a_met_watch_reaches_the_owner_like_a_reminder_with_the_app_open_or_the_owner_away(
        daemon: RunningDaemon) -> None:
    notes = _notes(daemon)
    d = daemon.daemon
    d._client_shows = lambda topic: True                  # type: ignore[method-assign]   # the app shows task results
    d.owner_input.running = True
    d.owner_input.last_owner_wall = time.time() - 3600                    # away for an hour: an alert doesn't wait
    try:
        report = make_schedule(daemon, "write report.md", "every day at 9 write report.md", Targets.of(),
                               d.clock.now() + timedelta(minutes=1))
        install(daemon, [sup("reply", "Nothing to write today.")])
        fire(daemon, 61)
        assert notes == []                                    # an ordinary scheduled answer: the app shows it
        run(daemon, lambda: d._schedule_change(report.id, "cancel"))
        _watch(daemon, BATTERY_WATCH, BATTERY_WORDS, "standing")
        install(daemon, [sup("task", "Checking.", task("check the battery level"))],
                {"worker": _check(True, "The battery is at 12%.")})
        fire(daemon, 15 * 60 + 1)
        assert notes == [("FERB alert", "The battery is at 12%.")] and d._held == []
    finally:
        d.owner_input.running = False


def test_ferb_schedules_says_which_kind_of_watch_each_one_is(daemon: RunningDaemon) -> None:
    import io
    from ferb.cli import admin
    _watch(daemon, FILE_WATCH, FILE_WORDS, "until_met")
    _watch(daemon, BATTERY_WATCH, BATTERY_WORDS, "standing")
    make_schedule(daemon, "write report.md", "every day at 9 write report.md", Targets.of(),
                  daemon.daemon.clock.now() + timedelta(minutes=30))
    out = io.StringIO()
    c = Client(daemon.sock)
    try:
        assert admin.schedules(c, "list", None, out) == 0
    finally:
        c.close()
    ends = {line.rsplit("  ", 1)[-1] for line in out.getvalue().splitlines()}
    assert ends == {f"{FILE_WATCH} (until it happens, then it stops)", f"{BATTERY_WATCH} (each time it happens)",
                    "write report.md"}


# ---------- once per crossing: judged against the last check that reported the condition (review I1) ----------

def _refuted(i: int, why: str) -> str:
    return json.dumps({"results": [{"index": i, "met": False, "evidence": why}]})


def test_a_failed_or_unanswered_check_between_two_met_ones_does_not_tell_again(daemon: RunningDaemon) -> None:
    notes = _notes(daemon)
    s = _watch(daemon, BATTERY_WATCH, BATTERY_WORDS, "standing")
    install(daemon, [sup("task", "Checking.", task("check the battery level"))],
            {"worker": _check(True, "The battery is at 12%.")})
    fire(daemon, 15 * 60 + 1)
    _next_run(daemon, s.id, {"worker": _check(True, "The battery is at 11%."),
                             "worker.verify": _refuted(0, "pmset wasn't read.")})          # this check failed
    install(daemon, [sup("reply", "I'll look again next time.")])                           # an unanswered one
    nxt = run(daemon, lambda: daemon.daemon.schedules.get(s.id)).next_run
    fire(daemon, (nxt - daemon.daemon.clock.now()).total_seconds() + 1)
    again = _next_run(daemon, s.id, {"worker": _check(True, "The battery is at 10%.")})
    assert '"last_reported_met": true' in again.contexts[0][2]        # the last check that reported it found it met
    # told once for the crossing; the failed check and the reply are told as any scheduled run's are
    assert [title for title, _ in notes] == ["FERB alert", "FERB", "FERB finished"]


def test_a_never_confirmed_one_shot_watch_alerts_once(daemon: RunningDaemon) -> None:
    """A page the checker can't confirm: every check ends done but not confirmed, and the watch never ends."""
    notes = _notes(daemon)
    s = _watch(daemon, FILE_WATCH, FILE_WORDS, "until_met")
    unconfirmed = {"worker": _check(True, "done.txt is there."), "worker.verify": UNRESOLVED,
                   "worker.dispatch": FINISH_UNCONFIRMED}
    install(daemon, [sup("task", "Checking.", task("check whether done.txt exists"))], dict(unconfirmed))
    fire(daemon, 15 * 60 + 1)
    for _ in range(2):
        _next_run(daemon, s.id, dict(unconfirmed))
    assert [title for title, _ in notes] == ["FERB alert"]
    assert [r["status"] for r in results(daemon)] == ["done_unverified"]      # the later ones say nothing new
    assert run(daemon, lambda: daemon.daemon.schedules.get(s.id)).state == "active"


def test_a_one_shot_told_while_unconfirmed_ends_with_only_the_stop_line(daemon: RunningDaemon) -> None:
    notes = _notes(daemon)
    s = _watch(daemon, FILE_WATCH, FILE_WORDS, "until_met")
    install(daemon, [sup("task", "Checking.", task("check whether done.txt exists"))],
            {"worker": _check(True, "done.txt is there."), "worker.verify": UNRESOLVED,
             "worker.dispatch": FINISH_UNCONFIRMED})
    fire(daemon, 15 * 60 + 1)
    _next_run(daemon, s.id, {"worker": _check(True, "done.txt is in Downloads now.")})
    assert run(daemon, lambda: daemon.daemon.schedules.get(s.id)).state == "done"
    assert [title for title, _ in notes] == ["FERB alert", "FERB finished"]     # no second alert for one crossing
    ending = results(daemon)[-1]
    assert ending["answer"].endswith(f"🗓 Stopped checking: {FILE_WATCH}")      # but the stop line reaches the owner


# ---------- the checker judges the reported condition (review I2) ----------

def test_the_checker_judges_the_reported_condition_and_a_contradicting_flag_never_ends_a_watch(
        daemon: RunningDaemon) -> None:
    notes = _notes(daemon)
    s = _watch(daemon, FILE_WATCH, FILE_WORDS, "until_met")
    script = install(daemon, [sup("task", "Checking.", task("check whether done.txt exists"))],
                     {"worker": _check(True, "done.txt isn't in Downloads yet."),
                      "worker.verify": json.dumps({"results": [
                          {"index": 0, "met": True, "evidence": "Downloads was listed."},
                          {"index": 1, "met": False, "evidence": "Downloads has no done.txt."}]})})
    fire(daemon, 15 * 60 + 1)
    checker = next(c for _, key, c, _ in script.contexts if key == "worker.verify")
    assert "1. The condition this watch checks holds now, as my report says (watch_met true)\n" in checker
    assert run(daemon, lambda: daemon.daemon.schedules.get(s.id)).state == "active"
    assert _journal(daemon, "schedule.met", s.id) == [] and [title for title, _ in notes] == ["FERB"]
    [failed] = results(daemon)
    assert failed["status"] == "failed"
    assert "whether it has happened: Downloads has no done.txt." in failed["answer"]


def test_an_ordinary_task_gets_no_watch_condition(daemon: RunningDaemon) -> None:
    script = install(daemon, [sup("task", "On it.", task("write report.md"))], {"worker": done("wrote it")})
    chat(daemon, "write report.md")
    checker = next(c for _, key, c, _ in script.contexts if key == "worker.verify")
    assert "watch" not in checker.split("Conditions:", 1)[1].split("Condition layers", 1)[0]


# ---------- a quiet check reaches no client (review I3) ----------

def test_a_quiet_check_is_recorded_but_never_reaches_a_client(daemon: RunningDaemon) -> None:
    s = _watch(daemon, BATTERY_WATCH, BATTERY_WORDS, "standing")
    seen: dict[str, Any] = {}

    def check(token: str) -> str:                           # while the check runs, as the app's 5 s refresh does
        c = Client(daemon.sock)
        try:
            seen["running"] = c.call("chat.status", {"peek": True})["running"]
        finally:
            c.close()
        return _check(False, "The battery is at 64%.")(token)
    client = Subscriber(daemon)
    try:
        install(daemon, [sup("task", "Checking.", task("check the battery level"))], {"worker": check})
        fire(daemon, 15 * 60 + 1)
        quiet = run(daemon, lambda: daemon.daemon.db.conn.execute(
            "SELECT task_id, shown, answer FROM task_results").fetchall())
        assert [(r["shown"], r["answer"]) for r in quiet] == [(1, "The battery is at 64%.")]     # recorded
        assert seen["running"] == []                    # never listed as running, so no client adopts it
        assert client.topic("task.created") == [] and client.topic("task.result") == []
        _next_run(daemon, s.id, {"worker": _check(True, "The battery is at 12%.")})
        told = until(lambda: client.topic("task.result"), "the alert's result")
        assert [r["answer"] for r in told] == ["The battery is at 12%."] and told[0]["goal"] == BATTERY_WATCH
        assert client.topic("task.created") == []       # it arrives as work started elsewhere, as clients expect
    finally:
        client.close()


# ---------- the watch text leaves FERB's own work alone (review I4) ----------


# ---------- ending a watch, overlapping runs, pauses (review M1, M3, M6, M7) ----------

def test_an_ended_watch_withdraws_its_recorded_scope_as_a_cancel_does(daemon: RunningDaemon) -> None:
    """A check overlapping the ending one (or a plan resumed after a restart) holds the watch's recorded scope under
    `schedule:<id>`; ending the watch withdraws it from that run as a cancel would (review M1, re-review N2)."""
    from ferb.orchestrator.plans import SendScope
    from ferb.policy.standing import SCHEDULE_GRANT
    scope = SendScope(capability="mail.send", identity="owner", recipients=["me@owner.example"])
    s = make_schedule(daemon, FILE_WATCH, FILE_WORDS + " and email me@owner.example", Targets.of(),
                      daemon.daemon.clock.now() + timedelta(minutes=15), "every", 15, "until_met",
                      standing=[scope.model_dump()])
    supervisor = daemon.daemon.supervisor
    assert supervisor is not None

    def ending(token: str) -> str:                          # another check of it holds the recorded scope meanwhile
        run(daemon, lambda: supervisor.standing.grant("tree-overlap", [scope], SCHEDULE_GRANT + s.id))
        return _check(True, "done.txt is in Downloads now.")(token)
    install(daemon, [sup("task", "Checking.", task("check whether done.txt exists"))], {"worker": ending})
    fire(daemon, 15 * 60 + 1)
    assert run(daemon, lambda: daemon.daemon.schedules.get(s.id)).state == "done"
    rows = run(daemon, lambda: [(r["kind"], json.loads(r["data_json"])) for r in daemon.daemon.db.conn.execute(
        "SELECT kind, data_json FROM journal WHERE kind IN ('schedule.met', 'standing.revoked') ORDER BY seq")])
    assert [kind for kind, _ in rows] == ["schedule.met", "standing.revoked"]      # journaled, then withdrawn
    assert rows[1][1] == {"approval_id": SCHEDULE_GRANT + s.id, "task_ids": ["tree-overlap"], "reason": "schedule met"}
    assert run(daemon, lambda: supervisor.standing.held(SCHEDULE_GRANT + s.id)) is False
    assert run(daemon, lambda: daemon.daemon._schedule_live(s.id)) is False
    assert run(daemon, lambda: supervisor._grant_in_force(SCHEDULE_GRANT + s.id)) is False


def test_a_check_overlapping_the_one_that_ended_the_watch_tells_nothing(daemon: RunningDaemon) -> None:
    notes = _notes(daemon)
    s = _watch(daemon, FILE_WATCH, FILE_WORDS, "until_met")

    def overlapped(token: str) -> str:                      # another check of it ends the watch meanwhile
        run(daemon, lambda: daemon.daemon._end_watch(s.id, "tree-other"))
        return _check(True, "done.txt is in Downloads now.")(token)
    install(daemon, [sup("task", "Checking.", task("check whether done.txt exists"))], {"worker": overlapped})
    fire(daemon, 15 * 60 + 1)
    assert notes == [] and results(daemon) == []
    assert [e.data["task_id"] for e in _journal(daemon, "schedule.met", s.id)] == ["tree-other"]


@pytest.mark.parametrize("change", ["pause", "cancel"])
def test_a_watch_paused_or_cancelled_during_its_ending_run_stays_as_the_owner_set_it(
        daemon: RunningDaemon, change: str) -> None:
    notes = _notes(daemon)
    s = _watch(daemon, FILE_WATCH, FILE_WORDS, "until_met")

    def meanwhile(token: str) -> str:
        run(daemon, lambda: daemon.daemon._schedule_change(s.id, change))
        return _check(True, "done.txt is in Downloads now.")(token)
    install(daemon, [sup("task", "Checking.", task("check whether done.txt exists"))], {"worker": meanwhile})
    fire(daemon, 15 * 60 + 1)
    kept = run(daemon, lambda: daemon.daemon.schedules.get(s.id))
    assert kept.state == {"pause": "paused", "cancel": "cancelled"}[change]
    assert _journal(daemon, "schedule.met", s.id) == [] and _pending_checks(daemon) == []
    assert notes == [("FERB alert", "done.txt is in Downloads now.")]       # told, without a stop line


def test_the_stop_line_is_in_the_answer_a_client_restores(daemon: RunningDaemon) -> None:
    s = _watch(daemon, FILE_WATCH, FILE_WORDS, "until_met")
    install(daemon, [sup("task", "Checking.", task("check whether done.txt exists"))],
            {"worker": _check(True, "done.txt is in Downloads now.")})
    fire(daemon, 15 * 60 + 1)
    assert run(daemon, lambda: daemon.daemon.schedules.get(s.id)).state == "done"
    c = Client(daemon.sock)
    try:
        [restored] = c.call("chat.status", {"peek": True})["results"]         # the app's restore path (M3)
    finally:
        c.close()
    assert restored["answer"].endswith(f"🗓 Stopped checking: {FILE_WATCH}") and "display_text" not in restored
    # ... and in the task's record, which later planner turns read (review M3)
    [row] = run(daemon, lambda: daemon.daemon.tasks.rows(restored["task_id"]))[:1]
    assert json.loads(row["outcome_json"])["answer"].endswith(f"🗓 Stopped checking: {FILE_WATCH}")


def test_a_failed_attempt_does_not_keep_an_earlier_attempts_report(daemon: RunningDaemon) -> None:
    retry = json.dumps({"action": "retry", "worker_type": "default", "instructions": "Look again.", "question": "",
                        "stop_reason": ""})
    finish = json.dumps({"action": "finish", "worker_type": "default", "instructions": "Stop and report.",
                         "question": "", "stop_reason": ""})
    s = _watch(daemon, FILE_WATCH, FILE_WORDS, "until_met")
    install(daemon, [sup("task", "Checking.", task("check whether done.txt exists"))],
            {"worker": [_check(True, "done.txt is there.")(""), json.dumps({"status": "done"})],   # invalid
             "worker.verify": _refuted(1, "Downloads has no done.txt."), "worker.dispatch": [retry, finish]})
    fire(daemon, 15 * 60 + 1)
    supervisor = daemon.daemon.supervisor
    assert supervisor is not None
    [answer] = run(daemon, lambda: [e for e in supervisor.journal.entries_of("task.answer")])
    assert answer.data["status"] == "failed" and "watch_met" not in answer.data
    assert run(daemon, lambda: daemon.daemon.schedules.get(s.id)).state == "active"


# ---------- the closing round: stored finding, check conditions, checked flags only (re-review N1, N3, M2) ----------

def test_a_watchs_last_reported_finding_is_stored_with_it(daemon: RunningDaemon) -> None:
    s = _watch(daemon, BATTERY_WATCH, BATTERY_WORDS, "standing")
    found = lambda: run(daemon, lambda: daemon.daemon.schedules.get(s.id)).last_reported_met   # noqa: E731
    assert found() is None                                              # no check has reported it yet
    install(daemon, [sup("task", "Checking.", task("check the battery level"))],
            {"worker": _check(True, "The battery is at 12%.")})
    fire(daemon, 15 * 60 + 1)
    assert found() is True
    _next_run(daemon, s.id, {"worker": _check(False, "The battery is at 40%."),
                             "worker.verify": _refuted(0, "pmset wasn't read.")})            # a failed check
    assert found() is True                                              # reports nothing the owner was told
    _next_run(daemon, s.id, {"worker": _check(False, "The battery is at 40%.")})
    assert found() is False


def test_a_check_reads_the_last_finding_without_reading_the_watchs_history(daemon: RunningDaemon) -> None:
    """The reviewer's case: 600 failed checks since the last one that reported. The lookup used to read every one of
    them (three journal scans each, on the event loop): 1.4 s per check here before, and growing (re-review N1)."""
    notes = _notes(daemon)
    s = _watch(daemon, BATTERY_WATCH, BATTERY_WORDS, "standing")
    install(daemon, [sup("task", "Checking.", task("check the battery level"))],
            {"worker": _check(True, "The battery is at 12%.")})
    fire(daemon, 15 * 60 + 1)
    supervisor = daemon.daemon.supervisor
    assert supervisor is not None

    def failed_checks() -> None:
        for i in range(600):
            supervisor.journal.append("schedule.run", {"schedule_id": s.id, "request_id": f"r-{i}", "late": False})
            supervisor.journal.append("task.answer", {"task_id": f"tree-{i}", "request_id": f"r-{i}",
                                                      "status": "failed", "text": "pmset wasn't read."})
    run(daemon, failed_checks)
    read: list[str] = []
    real = supervisor.journal.of_kind

    def counted(kind: str, field: str = "", equals: str = "", missing: str | None = None) -> Any:
        if field == "request_id" and str(equals).startswith("r-"):
            read.append(str(equals))
        return real(kind, field, equals, missing)
    supervisor.journal.of_kind = counted                    # type: ignore[method-assign]
    still = _next_run(daemon, s.id, {"worker": _check(True, "The battery is at 11%.")})
    assert set(read) == {"r-599"}                           # only the latest run, for its answer (live check 55)
    assert '"last_reported_met": true' in still.contexts[0][2]
    assert len(notes) == 1                                  # still at or below: told once



def test_only_a_watchs_check_records_watch_met(daemon: RunningDaemon) -> None:
    """An ordinary task's stray flag was never judged, so it is never recorded (re-review M2)."""
    install(daemon, [sup("task", "On it.", task("write report.md"))], {"worker": _check(True, "Wrote report.md.")})
    out = outcome(chat(daemon, "write report.md"))
    supervisor = daemon.daemon.supervisor
    assert supervisor is not None
    [answer] = run(daemon, lambda: supervisor.journal.entries_of("task.answer"))
    assert "watch_met" not in out and "watch_met" not in answer.data


# ---------- restarts and updates ----------

def test_a_watch_and_its_next_check_survive_a_restart(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("ferb.sanitize.images.recognize_text", lambda _image: [])
    first = own_daemon(home, "a")
    try:
        install(first, [schedule_reply(first, FILE_WATCH, FILE_WORDS, first.daemon.clock.now() + timedelta(minutes=15),
                                       "every", every=15, watch="until_met")])
        chat(first, FILE_WORDS)
        [s] = run(first, first.daemon.schedules.all)
    finally:
        first.stop()                                          # `ferb setup --update` restarts the daemon
    second = own_daemon(home, "b")
    try:
        kept = run(second, lambda: second.daemon.schedules.get(s.id))
        assert (kept.state, kept.watch, kept.timer_id) == ("active", "until_met", s.timer_id)
        assert [t.id for t in _pending_checks(second)] == [s.timer_id]
        install(second, [sup("task", "Checking.", task("check whether done.txt exists"))],
                {"worker": _check(True, "done.txt is in Downloads now.")})
        fire(second, 15 * 60 + 1)
        assert run(second, lambda: second.daemon.schedules.get(s.id)).state == "done"
        assert results(second)[0]["answer"].endswith(f"🗓 Stopped checking: {FILE_WATCH}")
    finally:
        second.stop()


# ---------- the prompts carry the guidance ----------
