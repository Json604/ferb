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
# ---------- what each banner is ----------

def test_a_reminder_goes_to_ferb_app_with_its_timer(daemon: RunningDaemon, notes: list[tuple[str, str]],
                                                    poster: Poster) -> None:
    d = daemon.daemon
    timer = on_loop(daemon, lambda: d.timers.add("reminder", d.clock.now() + timedelta(minutes=1), "call Mom"))
    daemon.run(d._timer_fired(timer))
    until(lambda: bool(poster.items))
    item = poster.items[0]
    assert (item["id"], item["category"], item["title"], item["body"], item["when_hidden"]) == (
        f"reminder:{timer.id}", "reminder", "FERB reminder", "call Mom", False)
    assert item["target"] == {"kind": "reminder", "timer_id": timer.id}
    assert notes == []


def test_a_finished_task_goes_to_ferb_app_even_when_a_chat_showed_it(daemon: RunningDaemon,
                                                                     notes: list[tuple[str, str]],
                                                                     poster: Poster) -> None:
    d = daemon.daemon
    daemon.run(d._task_result({"task_id": "t9", "status": "done", "answer": "Renamed 12 files."}, shown=True))
    daemon.run(d._task_result({"task_id": "t10", "status": "cancelled", "answer": "Stopped."}, shown=False))
    daemon.run(d._task_result({"task_id": "t11", "status": "done", "answer": "It's $40 now."}, shown=True, alert=True))
    until(lambda: len(poster.items) == 2)
    settle()
    finished, alert = poster.items
    assert (finished["id"], finished["category"], finished["title"], finished["when_hidden"]) == (
        "task:t9", "task_finished", "FERB finished", True)
    assert finished["target"] == {"kind": "task", "task_id": "t9"}
    assert (alert["category"], alert["title"], alert["when_hidden"], alert["target"]) == (
        "alert", "FERB alert", False, {"kind": "task", "task_id": "t11"})
    assert len(poster.items) == 2 and notes == []           # a task the owner stopped is never a banner


def test_without_ferb_app_a_shown_answer_stays_quiet_as_before(daemon: RunningDaemon,
                                                               notes: list[tuple[str, str]]) -> None:
    d = daemon.daemon
    daemon.run(d._task_result({"task_id": "t1", "status": "done", "answer": "Shown in chat."}, shown=True))
    daemon.run(d._task_result({"task_id": "t2", "status": "done", "answer": "Nobody saw it."}, shown=False))
    assert notes == [("FERB finished", "Nobody saw it.")]


def test_a_question_goes_to_ferb_app_even_with_its_chat_attached(daemon: RunningDaemon,
                                                                 notes: list[tuple[str, str]],
                                                                 poster: Poster) -> None:
    d = daemon.daemon
    d._attached_trees.add("t1")
    daemon.call_soon(d._approval_notice, {"approval_id": "a1", "task_id": "t1.w2", "reason": "Send it?",
                                          "summary": "Send the email to Sam?", "capability": "mail.send"})
    daemon.call_soon(d._approval_notice, {"approval_id": "p1", "task_id": None, "reason": "Confirm your profile",
                                          "capability": "setup.persona"})        # never a banner (L3)
    until(lambda: bool(poster.items))
    settle()
    assert poster.items == [{"id": "question:a1", "category": "question", "title": "FERB needs you",
                             "body": "FERB needs your OK: Send the email to Sam?",
                             "target": {"kind": "question", "approval_id": "a1", "task_id": "t1"},
                             "when_hidden": True}]
    assert notes == []


def test_a_loop_notice_goes_to_ferb_app_as_a_loop_banner(daemon: RunningDaemon, notes: list[tuple[str, str]],
                                                         poster: Poster) -> None:
    d = daemon.daemon
    daemon.run(d._loop_notice("tree-1", "keep", "I keep having to redo this: keep the tab playing."))
    until(lambda: bool(poster.items))
    item = poster.items[0]
    assert (item["category"], item["title"], item["when_hidden"]) == ("loop", "FERB loop", True)
    assert item["target"] == {"kind": "task", "task_id": "tree-1"} and item["id"].startswith("loop:")
    assert item["body"] == "I keep having to redo this: keep the tab playing."
    assert notes == []


def test_a_notice_stays_in_the_notch_while_ferb_app_is_connected(daemon: RunningDaemon,
                                                                 notes: list[tuple[str, str]],
                                                                 poster: Poster) -> None:
    daemon.run(daemon.daemon._tell_owner("n-renewed", [], "Your Claude plan renewed.", origin_.FERB))
    settle()
    assert poster.items == [] and notes == []


def test_ferb_apps_own_notification_rows_are_ferbs_and_keep_no_body(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    assert d.perception is not None

    def arrive(external_id: str, bundle: str) -> None:
        d.perception.notification(Observation("notification", {      # type: ignore[union-attr]
            "external_id": external_id, "bundle_id": bundle, "title": "FERB finished", "body": "Renamed 12 files.",
            "conversation": "FERB finished", "conversation_source": "title"}, {"actor": "unknown"}))
    for external_id, bundle in (("nc-app", "com.ferb.FERB"), ("nc-dev", "dev.ferb.notch"),
                                ("nc-other", "net.whatsapp.WhatsApp")):
        daemon.call_soon(arrive, external_id, bundle)
    rows = on_loop(daemon, lambda: {json.loads(r["data_json"])["external_id"]: (r["actor"], json.loads(r["data_json"]))
                                    for r in d.db.conn.execute(          # type: ignore[union-attr]
                                        "SELECT actor, data_json FROM trail WHERE source='notification'")})
    assert rows["nc-app"][0] == "ferb" and "body" not in rows["nc-app"][1]
    assert rows["nc-app"][1]["ferb_banner"] == {"poster": "com.ferb.FERB"}
    assert rows["nc-dev"][0] == "ferb"
    assert rows["nc-other"][0] == "unknown"


def test_chat_status_says_whether_ferb_is_paused_or_stopped(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    original = d.sense
    client = Client(daemon.sock)
    try:
        status = client.call("chat.status", {"peek": True})
        assert status["paused"] is False and status["stopped"] is False
        d.sense = SimpleNamespace(paused=True)                  # type: ignore[assignment]
        assert client.call("chat.status", {"peek": True})["paused"] is True
        d.sense = original
        client.call("stop.engage")
        assert client.call("chat.status", {"peek": True})["stopped"] is True
        client.call("stop.lift")
    finally:
        d.sense = original
        client.close()


def test_chat_status_gives_each_running_task_its_start_for_the_timer(daemon: RunningDaemon,
                                                                       monkeypatch: pytest.MonkeyPatch) -> None:
    """A friend's feedback 2026-10-27: an elapsed timer beside "1 active task". The notch counts from the task's
    start, which survives the app restarting, so chat.status says when each running task started."""
    from tests.browserkit import OWNER, task
    task(daemon, "w", OWNER, "do it")
    monkeypatch.setattr(daemon.daemon.supervisor, "running_trees", lambda watches=False: [("w", "do it")])
    status = daemon.run(daemon.daemon._chat_status(None, {"peek": True}))
    async def created() -> str:
        return str(daemon.daemon.db.conn.execute("SELECT created_at FROM tasks WHERE id = 'w'").fetchone()[0])
    started = daemon.run(created())
    assert status["running"] == [{"task_id": "w", "goal": "do it", "started_at": started}]
