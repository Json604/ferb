"""L34 D: FERB.app posts FERB's banners while it holds `notifications.subscribe`; osascript is the fallback; the owner's
category switches; nothing secret in a banner; each banner's category and click target."""
import json
import threading
import time
from datetime import timedelta
from types import SimpleNamespace
from typing import Any, Callable, Iterator

import pytest

from ferb.cli.client import Client, RpcError
from ferb.core import origin as origin_
from ferb.daemon.banners import Banner
from ferb.sense.trail import Observation
from tests.conftest import RunningDaemon


class Poster:
    """FERB.app's subscription, faked: a thread reads the stream into `items`."""

    def __init__(self, daemon: RunningDaemon, authorized: bool = True) -> None:
        self.client = Client(daemon.sock)
        self.items: list[dict[str, Any]] = []
        self.error: Exception | None = None
        self.thread = threading.Thread(target=self._read, args=(authorized,), daemon=True)
        self.thread.start()

    def _read(self, authorized: bool) -> None:
        try:
            for item in self.client.stream("notifications.subscribe", {"authorized": authorized}):
                self.items.append(item)
        except Exception as e:                    # the daemon's refusal, or this side closing
            self.error = e

    def close(self) -> None:
        self.client.interrupt()                   # wakes the reader; the daemon sees the connection end
        self.thread.join(2)
        self.client.close()


def until(check: Callable[[], bool], seconds: float = 5) -> None:
    end = time.monotonic() + seconds
    while not check():
        if time.monotonic() > end:
            raise AssertionError("never happened")
        time.sleep(0.05)


def settle() -> None:
    time.sleep(0.3)                               # long enough for a banner that would arrive to arrive


def on_loop(daemon: RunningDaemon, fn: Callable[[], Any]) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


@pytest.fixture
def notes(daemon: RunningDaemon) -> list[tuple[str, str]]:
    """osascript, faked: what it would have posted."""
    seen: list[tuple[str, str]] = []

    async def note(title: str, text: str) -> None:
        seen.append((title, text))
    daemon.daemon.notify = note
    return seen


@pytest.fixture
def poster(daemon: RunningDaemon) -> Iterator[Poster]:
    p = Poster(daemon)
    until(lambda: daemon.daemon.posters.connected)
    yield p
    p.close()


# ---------- routing ----------

def test_with_ferb_app_connected_a_banner_goes_to_it_and_not_to_osascript(
        daemon: RunningDaemon, notes: list[tuple[str, str]], poster: Poster) -> None:
    daemon.run(daemon.daemon._notify("FERB reminder", "call Mom", urgent=True, category="reminder",
                                     target={"kind": "reminder", "timer_id": "t1"}, ident="reminder:t1"))
    until(lambda: bool(poster.items))
    assert poster.items == [{"id": "reminder:t1", "category": "reminder", "title": "FERB reminder", "body": "call Mom",
                             "target": {"kind": "reminder", "timer_id": "t1"}, "when_hidden": False}]
    assert notes == []


def test_osascript_is_the_fallback_once_ferb_app_leaves(daemon: RunningDaemon, notes: list[tuple[str, str]],
                                                        poster: Poster) -> None:
    poster.close()
    until(lambda: not daemon.daemon.posters.connected)
    daemon.run(daemon.daemon._notify("FERB reminder", "call Mom", urgent=True, category="reminder"))
    assert notes == [("FERB reminder", "call Mom")]


def test_an_app_macos_doesnt_let_notify_is_refused_and_osascript_stays(daemon: RunningDaemon,
                                                                      notes: list[tuple[str, str]]) -> None:
    refused = Poster(daemon, authorized=False)
    until(lambda: refused.error is not None)
    assert isinstance(refused.error, RpcError) and not daemon.daemon.posters.connected
    daemon.run(daemon.daemon._notify("FERB alert", "Price dropped", urgent=True, category="alert"))
    assert notes == [("FERB alert", "Price dropped")]
    refused.close()


def test_a_switched_off_category_posts_nothing_anywhere(daemon: RunningDaemon, notes: list[tuple[str, str]]) -> None:
    client = Client(daemon.sock)
    try:
        assert client.call("notifications.categories")["categories"] == {
            "question": True, "reminder": True, "alert": True, "task_finished": True, "loop": True}
        assert client.call("notifications.categories", {"set": {"reminder": False}})["categories"]["reminder"] is False
        with pytest.raises(RpcError, match="unknown banner categories"):
            client.call("notifications.categories", {"set": {"weather": True}})
    finally:
        client.close()
    d = daemon.daemon
    daemon.run(d._notify("FERB reminder", "call Mom", urgent=True, category="reminder"))
    daemon.run(d._notify("FERB noticed", "Your Claude plan renewed"))          # no switch governs a notice
    assert notes == [("FERB noticed", "Your Claude plan renewed")]
    assert d.config.get("banners", "reminder") is False
    entries = on_loop(daemon, lambda: d._journal().of_kind("banners.switched"))
    assert [e.data for e in entries] == [{"set": {"reminder": False}}]


def test_a_banner_never_carries_a_secret(daemon: RunningDaemon, poster: Poster) -> None:
    d = daemon.daemon
    daemon.call_soon(d._add_secret_value, "hunter2-correct-horse")
    daemon.run(d._notify("FERB finished", "Logged in with hunter2-correct-horse", category="task_finished",
                         target={"kind": "task", "task_id": "t1"}))
    until(lambda: bool(poster.items))
    assert "hunter2-correct-horse" not in json.dumps(poster.items)
    assert poster.items[0]["id"].startswith("banner:")


def test_a_banner_held_while_the_owner_was_away_goes_to_ferb_app_when_they_are_back(daemon: RunningDaemon,
                                                                                   poster: Poster) -> None:
    d = daemon.daemon
    d.owner_input.running = True
    d.owner_input.last_owner_wall = time.time() - 3600                          # away for an hour
    try:
        daemon.run(d._notify("FERB finished", "done", category="task_finished"))
        assert poster.items == [] and d._held == [Banner("FERB finished", "done", "task_finished")]
        d.owner_input.last_owner_wall = time.time()                             # back
        until(lambda: bool(poster.items), seconds=15)                           # the held queue is read every 10 s
        assert poster.items[0]["body"] == "done"
    finally:
        d.owner_input.running = False
