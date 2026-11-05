"""T-1055 (owner ORCH-542, D32): tasks, schedules, reminders and timers carry the planner's short name; every heading
(cards, banners, the RPC rows the app lists) shows it, and the full instruction stays in the rows for the opened
detail. A row made before names shows its instruction's first clause, cut, with no model call and no renaming. The real
daemon, gateway and scripted model."""
import json
import shutil
from datetime import timedelta
from pathlib import Path
from typing import Any

from ferb.cli.client import Client
from ferb.orchestrator.names import NAME_CHARS, shown_name
from tests.conftest import RunningDaemon, chat, done, install, outcome, sup, task
from tests.support.schedules import fire, mins, results, run, schedule_reply

COURSERA = ("Check Coursera in my Chrome “scaler” profile for new BITS assignments. Keep a note of each one you find, "
            "with its due date, and tell me only about new ones since the last run.")


def _call(d: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> Any:
    c = Client(d.sock)
    try:
        return c.call(method, params or {})
    finally:
        c.close()


def _named_schedule(d: RunningDaemon, words: str, name: str) -> str:
    reply = json.loads(schedule_reply(d, COURSERA, words, d.daemon.clock.now() + timedelta(minutes=1), repeat="every",
                                      every=60))
    reply["schedule"]["name"] = name
    return json.dumps(reply)


# decision: ORCH-542, DMN-544
def test_a_schedule_is_headed_by_its_name_and_keeps_its_full_instruction(daemon: RunningDaemon) -> None:
    words = "every hour check coursera for new BITS assignments"
    install(daemon, [_named_schedule(daemon, words, "Coursera BITS assignments")])
    evs = chat(daemon, words)
    [card] = [c for e in evs for c in e.get("cards") or [] if c.get("type") == "event"]
    assert card["kind"] == "schedule" and card["what"] == "Coursera BITS assignments"
    [row] = _call(daemon, "schedules.list")["schedules"]
    assert row["name"] == "Coursera BITS assignments" and row["request"] == COURSERA      # the opened detail's text
    install(daemon, [sup("reply", "No new assignments.")])
    fire(daemon, 61)
    [result] = results(daemon)
    [ran] = [c for c in result["cards"] if c["type"] == "reminder"]
    assert ran["title"] == "Coursera BITS assignments" and ran["kind"] == "schedule"
    assert result["name"] == "Coursera BITS assignments"


# decision: ORCH-542, DMN-544
def test_a_schedule_change_keeps_its_name_unless_it_writes_a_new_one(daemon: RunningDaemon) -> None:
    words = "every hour check coursera for new BITS assignments"
    install(daemon, [_named_schedule(daemon, words, "Coursera BITS assignments")])
    chat(daemon, words)
    sid = _call(daemon, "schedules.list")["schedules"][0]["id"]

    def change(action: str, quote: str, name: str = "") -> str:
        return json.dumps({"kind": "reply", "text": "Done.", "task": None, "schedule": None, "timers": [],
                           "schedule_changes": [{"id": sid, "action": action, "quote": quote, "until": None,
                                                 "name": name}]})
    install(daemon, [change("pause", "pause the coursera check")])
    chat(daemon, "pause the coursera check")
    assert _call(daemon, "schedules.list")["schedules"][0]["name"] == "Coursera BITS assignments"
    install(daemon, [change("resume", "resume it and call it BITS tracker", "BITS tracker")])
    chat(daemon, "resume it and call it BITS tracker")
    assert _call(daemon, "schedules.list")["schedules"][0]["name"] == "BITS tracker"


# decision: ORCH-542, DMN-544
def test_a_timer_says_its_a_timer_and_is_headed_by_its_name(daemon: RunningDaemon) -> None:
    """Timer tm-c2f6c263: a 1-minute timer stored "Your 1-minute timer is up." and its card said "Reminder"."""
    timer = {"when": mins(timedelta(minutes=1)), "message": "Your 1-minute timer is up.", "quote": "1 minute timer",
             "when_active": False, "name": "1-minute timer", "kind": "timer"}
    install(daemon, [json.dumps({"kind": "reply", "text": "Started.", "task": None, "timers": [timer]})])
    evs = chat(daemon, "1 minute timer")
    [card] = [c for e in evs for c in e.get("cards") or [] if c.get("type") == "event"]
    assert card["kind"] == "timer" and card["what"] == "1-minute timer"
    [row] = _call(daemon, "reminders.list")["pending"]
    assert row["kind"] == "timer" and row["name"] == "1-minute timer" and row["message"] == "Your 1-minute timer is up."
    queue = run(daemon, daemon.daemon.posters.open)                  # FERB.app as the poster
    fire(daemon, 61)
    banner = queue.get_nowait()
    assert banner["title"] == "1-minute timer" and banner["body"] == "Your 1-minute timer is up."
    assert banner["card"]["kind"] == "timer" and banner["card"]["title"] == "1-minute timer"


# decision: ORCH-542, DMN-544
def test_a_task_carries_its_name_beside_its_full_goal(daemon: RunningDaemon) -> None:
    spec = task(COURSERA) | {"name": "Coursera BITS assignments"}
    install(daemon, [sup("task", "On it.", spec)], {"worker": done("Two new assignments.")})
    evs = chat(daemon, "check coursera for new BITS assignments")
    [plan] = [e for e in evs if e["type"] == "plan"]
    assert plan["name"] == "Coursera BITS assignments" and plan["goal"] == COURSERA
    assert outcome(evs)["name"] == "Coursera BITS assignments"
    [row] = _call(daemon, "tasks.list")["tasks"]
    assert row["name"] == "Coursera BITS assignments" and row["goal"] == COURSERA
    install(daemon, [sup("task", "On it.", task(COURSERA))], {"worker": done("None.")})
    chat(daemon, "check coursera again")
    unnamed = _call(daemon, "tasks.list")["tasks"][0]                # the planner wrote none: the goal's clause, cut
    assert unnamed["name"] == shown_name("", COURSERA) and len(unnamed["name"]) <= NAME_CHARS


# decision: ORCH-542, STORE-403
def test_schedules_and_reminders_made_before_names_upgrade_and_show_a_cut_heading(tmp_path: Path) -> None:
    """Upgrade (STORE-403): rows from before migration 0060 have no name; they read as unnamed, are headed by their
    instruction's first clause cut at NAME_CHARS, and nothing renames them. 0060 re-runs."""
    from ferb.clock.clock import FakeClock
    from ferb.clock.timers import Timers
    from ferb.core.ids import SequentialIds
    from ferb.orchestrator.schedules import Rule, Schedules
    from ferb.store import db as dbmod
    older = tmp_path / "m"
    older.mkdir()
    for f in dbmod.MIGRATIONS.glob("[0-9][0-9][0-9][0-9]_*.sql"):
        if int(f.name[:4]) <= 59:
            shutil.copy(f, older / f.name)
    db = dbmod.Database(tmp_path / "old.db")
    db.migrate(older)
    clock = FakeClock()
    first = clock.now() + timedelta(minutes=1)
    rule = Rule.build("daily", first)
    db.conn.execute("INSERT INTO schedules (id, request, own_words, intent_json, rule_json, next_run, state, "
                    "created_at) VALUES ('sc-old', ?, 'every day check coursera', '{}', ?, ?, 'active', ?)",
                    (COURSERA, json.dumps(rule.__dict__ | {"first": first.isoformat()}), first.isoformat(),
                     clock.now().isoformat()))
    db.conn.execute("INSERT INTO timers (id, kind, fire_at, message, state, created_at) VALUES ('tm-old', 'reminder', "
                    "?, 'Your 1-minute timer is up.', 'pending', ?)", (first.isoformat(), clock.now().isoformat()))
    assert 60 in db.migrate()
    for statement in dbmod._split_sql((dbmod.MIGRATIONS / "0060_short_names.sql").read_text()):
        db.conn.execute(statement)                                   # re-runnable
    old = Schedules(db, clock, SequentialIds("x")).get("sc-old")
    assert old is not None and old.name == "" and old.request == COURSERA
    assert old.label == shown_name("", COURSERA) and len(old.label) <= NAME_CHARS and old.label.endswith("…")
    reminder = Timers(db, clock, SequentialIds("y")).get("tm-old")
    assert reminder is not None and reminder.name == "" and reminder.label == "Your 1-minute timer is up"
    assert db.conn.execute("SELECT COUNT(*) FROM schedule_names").fetchone()[0] == 0          # nothing renamed
