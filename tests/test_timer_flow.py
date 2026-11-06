"""APP-762 (owner, 2026-11-06: Apple's timer flow): a timer's card and its pending row carry when it was set, so the app
draws its ring draining over its duration and the closed notch's live countdown; its done alert's Repeat starts the
same timer again for the same duration, once (DMN-548); every change of a timer reaches every client. The real daemon
and scripted model."""
import asyncio
import json
from datetime import datetime, timedelta
from typing import Any

import pytest

from ferb.cli.client import Client
from tests.conftest import RunningDaemon, chat, install
from ferb.policy.targets import Targets
from tests.support.schedules import fire, make_schedule, mins, run


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


def _changes(d: RunningDaemon) -> Any:
    """The `timer.changed` events from now on, read with `_drain`."""
    return run(d, lambda: d.daemon.bus.subscribe({"timer.changed"}))


def _drain(d: RunningDaemon, sub: Any) -> list[dict[str, Any]]:
    async def read() -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        it = sub.__aiter__()
        while True:
            try:
                ev = await asyncio.wait_for(it.__anext__(), 0.05)
            except asyncio.TimeoutError:
                return out
            out.append({"id": ev.data["timer_id"], "kind": ev.data["kind"], "state": ev.data["state"]}
                       | ({"repeat_of": ev.data["repeat_of"]} if "repeat_of" in ev.data else {}))
    return d.run(read())


def _states(d: RunningDaemon, *ids: str) -> dict[str, Any]:
    return _call(d, "timers.states", {"ids": list(ids)})["timers"]


# decision: DMN-548, APP-762, APP-764
def test_a_timer_cancelled_anywhere_is_told_to_every_client(daemon: RunningDaemon) -> None:
    """Finding 1 of the second review: a timer cancelled in chat or in Settings kept counting down in the closed notch
    and on its card. Every cancel path (the card's and Settings' reminders.cancel, chat, a schedule's change) and every
    firing now publish one `timer.changed`, and `timers.states` gives the row's state on connect."""
    sub = _changes(daemon)
    tea = _tea(daemon)["id"]
    assert _drain(daemon, sub) == [{"id": tea, "kind": "timer", "state": "running"}]
    _call(daemon, "reminders.cancel", {"id": tea})                        # its card's Cancel, or Settings'
    assert _drain(daemon, sub) == [{"id": tea, "kind": "timer", "state": "cancelled"}]
    pasta = _tea(daemon)["id"]
    _drain(daemon, sub)
    install(daemon, [json.dumps({"kind": "reply", "text": "Cancelled.", "task": None,
                                 "reminder_cancels": [{"id": pasta, "quote": "cancel my tea timer"}]})])
    chat(daemon, "cancel my tea timer")                                   # chat
    assert _drain(daemon, sub) == [{"id": pasta, "kind": "timer", "state": "cancelled"}]
    s = make_schedule(daemon, "write report.md", "every day at 9 write report.md", Targets(),
                      daemon.daemon.clock.now() + timedelta(days=1))
    booked = run(daemon, lambda: daemon.daemon.schedules.get(s.id).timer_id)
    _drain(daemon, sub)
    _call(daemon, "schedules.change", {"id": s.id, "action": "cancel"})   # a schedule's change cancels its booked run
    assert _drain(daemon, sub) == [{"id": booked, "kind": "schedule", "state": "cancelled"}]
    rice = _tea(daemon)["id"]
    _drain(daemon, sub)
    fire(daemon, 301)
    assert _drain(daemon, sub) == [{"id": rice, "kind": "timer", "state": "fired"}]
    assert _states(daemon, tea, pasta, rice, booked, "tm-none") == {
        tea: {"kind": "timer", "state": "cancelled"}, pasta: {"kind": "timer", "state": "cancelled"},
        rice: {"kind": "timer", "state": "fired"}}


# decision: DMN-548, DMN-547
def test_a_fired_timer_repeats_at_most_once(daemon: RunningDaemon) -> None:
    """Finding 6: Repeat on the notch's alert and again on the chat's card started two timers. The first Repeat is
    recorded; a second returns the timer it started; a running or cancelled timer is refused."""
    tea = _tea(daemon)["id"]
    with pytest.raises(Exception, match="still running"):
        _call(daemon, "timers.repeat", {"id": tea})
    fire(daemon, 301)
    sub = _changes(daemon)
    again = _call(daemon, "timers.repeat", {"id": tea})
    assert again["state"] == "running" and again["repeat_of"] == tea
    assert _drain(daemon, sub) == [{"id": again["timer"], "kind": "timer", "state": "running", "repeat_of": tea}]
    twice = _call(daemon, "timers.repeat", {"id": tea})
    assert twice["timer"] == again["timer"] and _drain(daemon, sub) == []
    assert [r["id"] for r in _call(daemon, "reminders.list")["pending"]] == [again["timer"]]
    assert _states(daemon, tea)[tea] == {"kind": "timer", "state": "fired", "repeated_by": again["timer"]}
    _call(daemon, "reminders.cancel", {"id": again["timer"]})
    with pytest.raises(Exception, match="cancelled"):
        _call(daemon, "timers.repeat", {"id": again["timer"]})


# decision: DMN-549, APP-769
def test_a_repeats_arrival_names_the_first_timer_of_its_chain(daemon: RunningDaemon) -> None:
    """APP-769 (owner, 2026-11-06: one box per timer): the app draws a timer and the timers its Repeat starts as one
    box. A Repeat's arrival card says the chain's first timer (`repeat_of`); the first timer's arrival says none."""
    tea = _tea(daemon)["id"]
    queue = run(daemon, daemon.daemon.posters.open)
    fire(daemon, 301)
    assert "repeat_of" not in queue.get_nowait()["card"]
    again = _call(daemon, "timers.repeat", {"id": tea})["timer"]
    fire(daemon, 301)
    arrived = queue.get_nowait()["card"]
    assert arrived["timer_id"] == again and arrived["repeat_of"] == tea
    third = _call(daemon, "timers.repeat", {"id": again})["timer"]
    fire(daemon, 301)
    arrived = queue.get_nowait()["card"]
    assert arrived["timer_id"] == third and arrived["repeat_of"] == tea
