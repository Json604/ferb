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
from ferb.orchestrator.schemas import SCHEDULE_SCHEMA, WORKER_SCHEMA, ScheduleRequest
from ferb.policy.targets import Targets
from tests.conftest import RunningDaemon, chat, done, install, outcome, sup, task
from tests.support.schedules import fire, make_schedule, own_daemon, results, run, schedule_reply

PROMPTS = Path(__file__).resolve().parents[1] / "ferb" / "orchestrator" / "prompts"
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
    assert WORKER_SCHEMA["properties"]["watch_met"] == {"type": ["boolean", "null"]}
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
        run(daemon, daemon.daemon._facts)                               # the planner's facts say it too


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
    assert notes == [("FERB alert", f"done.txt is in Downloads now.\n🗓 Stopped checking: {FILE_WATCH}")]
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
    assert '"watch_met": true' in still.contexts[0][2] and "The battery is at 12%." in still.contexts[0][2]
    _next_run(daemon, s.id, {"worker": _check(False, "The battery is at 40%.")})
    assert len(notes) == 1 and results(daemon) == []                     # still true, then not: both quiet
    again = _next_run(daemon, s.id, {"worker": _check(True, "The battery is at 10%.")})
    assert '"watch_met": false' in again.contexts[0][2]
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

def _words(name: str) -> str:
    return " ".join((PROMPTS / name).read_text().split())


WATCH_PLANNER = (
    'Watches: a request to be told or alerted when something becomes true ("tell me when the download finishes", '
    '"alert me when the battery is at 10%", "whenever …") is a schedule with repeat "every" whose request names '
    'what to check, what to tell or alert, and whether it ends once that happens (watch "until_met": "when the '
    'download finishes", "once it\'s back in stock") or keeps watching (watch "standing": "whenever the battery '
    'hits 10%"). A standing watch tells the owner once each time the condition becomes true, not again while it stays '
    'true. Never wait for a condition with a loop or sleep inside a task: those die when FERB restarts or updates. '
    '"Turn on Low Power Mode and alert me at 10%" is a task for the work now plus the watch in "schedule", in the '
    'same turn.')
WATCH_RUN = (
    "A watch (watch in the schedule facts) checks its condition afresh in every run: use a task whose answer says what "
    "it observed and whether the condition holds now. When it holds, tell or alert the owner as the request asks; the "
    "run may use its tools for an alert the owner asked for (a sound, speech). In a standing watch whose previous run "
    "already found it holding (previous_run.watch_met true), the owner was told then: stay quiet, with no new alert, "
    "and say it still holds. FERB itself ends an until_met watch once a confirmed run finds the condition met, and "
    "tells the owner it stopped checking.")
WATCH_WORKER = (
    "- watch_met: only when your task checks a condition the owner asked to be told about (a watch): true when your "
    "fresh observation shows it holds now, false when it doesn't; null for any other task.")
NO_LOOP_WORKER = (
    "A request to be told or alerted when something becomes true later is FERB's watch, a schedule that survives "
    "restarts and checks it once per run: check it once when that is your task, do only your own part when your task "
    "is other work from the same request, and never wait for the condition with a loop, sleep or background process "
    "(those die when FERB restarts or updates).")


@pytest.mark.parametrize("name, sentence", [("supervisor.md", WATCH_PLANNER), ("scheduled.md", WATCH_RUN),
                                            ("worker.md", WATCH_WORKER), ("worker.md", NO_LOOP_WORKER)])
def test_the_prompts_carry_the_watch_guidance(name: str, sentence: str) -> None:
    assert " ".join(sentence.split()) in _words(name)


def test_a_schedule_with_work_now_is_allowed_in_the_same_turn() -> None:
    """The planner's schedule entry once said "No task now", which forbade the live case's task plus watch."""
    words = _words("supervisor.md")
    assert "No task now." not in words
    assert ("No task for the scheduled work itself; when the same message also asks for something now, that part is a "
            "task in the same turn.") in words
