"""APP-762 (owner, 2026-11-06: Apple's timer flow): a timer's card and its pending row carry when it was set, so the app
draws its ring draining over its duration and the closed notch's live countdown; its done alert's Repeat starts the
same timer again for the same duration. The real daemon and scripted model."""
import json
from datetime import datetime, timedelta
from typing import Any

import pytest

from ferb.cli.client import Client
from tests.conftest import RunningDaemon, chat, install
from tests.support.schedules import fire, mins, run


def _call(d: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> Any:
    c = Client(d.sock)
    try:
        return c.call(method, params or {})
    finally:
        c.close()


def _tea(d: RunningDaemon) -> dict[str, Any]:
    timer = {"when": mins(timedelta(minutes=5)), "message": "Your tea is ready.", "quote": "5 minute tea timer",
             "when_active": False, "name": "Tea", "kind": "timer"}
    install(d, [json.dumps({"kind": "reply", "text": "Started.", "task": None, "timers": [timer]})])
    [card] = [c for e in chat(d, "5 minute tea timer") for c in e.get("cards") or [] if c.get("type") == "event"]
    return card


# decision: APP-762, DMN-547, ORCH-329
def test_a_timer_carries_its_start_on_its_card_its_row_and_its_arrival(daemon: RunningDaemon) -> None:
    card = _tea(daemon)
    started, ends = datetime.fromisoformat(card["started"]), datetime.fromisoformat(card["when"])
    assert card["kind"] == "timer" and timedelta(minutes=4) < ends - started <= timedelta(minutes=5, seconds=1)
    [row] = _call(daemon, "reminders.list")["pending"]
    assert row["started"] == card["started"] and row["id"] == card["id"]
    queue = run(daemon, daemon.daemon.posters.open)
    fire(daemon, 301)
    arrived = queue.get_nowait()["card"]
    assert arrived["kind"] == "timer" and arrived["started"] == card["started"] and arrived["timer_id"] == card["id"]


# decision: APP-762, DMN-547
def test_repeat_starts_the_same_timer_again_for_the_same_duration(daemon: RunningDaemon) -> None:
    card = _tea(daemon)
    duration = datetime.fromisoformat(card["when"]) - datetime.fromisoformat(card["started"])
    fire(daemon, 301)
    again = _call(daemon, "timers.repeat", {"id": card["id"]})
    assert again["timer"] != card["id"] and again["name"] == "Tea" and again["message"] == "Your tea is ready."
    assert datetime.fromisoformat(again["at"]) - datetime.fromisoformat(again["started"]) == duration
    [row] = _call(daemon, "reminders.list")["pending"]
    assert row["id"] == again["timer"] and row["kind"] == "timer"
    reminder = {"when": mins(timedelta(minutes=5)), "message": "Call the bank.", "quote": "remind me to call the bank",
                "when_active": False, "name": "Call the bank", "kind": "reminder"}
    install(daemon, [json.dumps({"kind": "reply", "text": "Set.", "task": None, "timers": [reminder]})])
    [made] = [c for e in chat(daemon, "remind me to call the bank in 5 minutes") for c in e.get("cards") or []
              if c.get("type") == "event"]
    assert "started" not in made                                   # only a timer has a duration to repeat
    with pytest.raises(Exception):
        _call(daemon, "timers.repeat", {"id": made["id"]})
