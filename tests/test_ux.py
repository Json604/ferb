"""Build step 14a, offline: the owner's names count as targets ("Brave", "Slack"); an app the owner named is no
surprise at the gateway; the irreversible floor (payments, deletion, account/security pages) and the model's flag ask
every time; continuity from the journal across chat sessions and restarts; a closed chat never stops a task (its
answer waits and a notification goes out); Ctrl-C stops it as "stopped by you" (a held send goes too); an installed
but unpaired extension is reported instead of silently falling back; windows on other Spaces are found.
"""

import asyncio
import json
import threading
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.core.types import Identity
from ferb.orchestrator import dag
from ferb.orchestrator.schemas import Plan
from ferb.policy.intent import RequestIntent
from ferb.policy.names import app_named, resolve_apps, site_named
from tests.conftest import Answers, RunningDaemon
from tests.test_gateway import FakeCap, setup
from tests.test_supervisor import STALL, chat, install, plan, sub, sup, writes

APPS = [("com.brave.Browser", "Brave Browser"), ("com.google.Chrome", "Google Chrome"), ("com.apple.Notes", "Notes"),
        ("com.tinyspeck.slackmacgap", "Slack")]


# ---------- names ----------

def test_natural_names_count_as_targets() -> None:
    words = "control my brave browser already opened and scroll to the end of this slack thread"
    assert site_named("slack.com", words) and site_named("app.slack.com", words)
    assert not site_named("evil.com", words)
    assert site_named("mail.google.com", "check my gmail") and site_named("youtu.be", "that youtube video")
    assert app_named("Brave Browser", "open brave") and not app_named("Google Chrome", "open brave")
    found, missing = resolve_apps(["Brave"], APPS, words)
    assert found == {"Brave": "com.brave.Browser"} and missing == []
    assert resolve_apps(["Chrome"], APPS, words)[1] == ["Chrome"]           # not in the owner's words
    p = Plan.model_validate(plan([sub("a", identity="owner", scopes=("mac:act",))], sites=("slack.com",)) |
                            {"intent": {**plan([sub("a")])["intent"], "sites": ["slack.com"], "apps": ["Brave"],
                                        "verbs": ["use_app"]}})
    assert dag.validate(p, words, {"mac:act"}, APPS) == []


def test_an_app_the_owner_named_is_no_surprise(daemon: RunningDaemon) -> None:
    """The owner's session: "in my Brave … slack" asked about com.brave.Browser on every call. Not any more."""
    class MacInput(FakeCap):
        def describe(self, args: Any) -> Any:
            from ferb.core.types import ActionSpec, Target
            return ActionSpec("mac_input", Target(site="com.brave.browser"))
    cap = MacInput("fake.mac", "mac_input", {"mac:act"})
    intent = RequestIntent.from_plan("req-1", {"use_app"}, sites=frozenset({"slack.com", "com.brave.browser"}))
    grant, _ = setup(daemon, {"mac:act"}, intent, [cap])
    asked = Answers(daemon)
    try:
        for _ in range(3):
            assert not daemon.run(daemon.daemon.gateway.call(grant, "fake.mac", {}))["is_error"]
    finally:
        asked.stop()
    assert asked.asked == [] and cap.calls == 3


# ---------- the irreversible floor ----------

def test_account_security_pages_and_model_flagged_kinds_ask_every_time(daemon: RunningDaemon) -> None:
    from ferb.act.browser.tools import security_page
    assert security_page("https://myaccount.google.com/security") and not security_page("https://shop.example/buy")
    submit = FakeCap("fake.submit", "submit", {"browser:act"})
    intent = RequestIntent.from_plan("req-1", {"fill_form"}, irreversible=frozenset({"submit"}))
    grant, _ = setup(daemon, {"browser:act"}, intent, [submit])
    yes = Answers(daemon)
    try:
        for _ in range(2):
            assert not daemon.run(daemon.daemon.gateway.call(grant, "fake.submit", {}))["is_error"]
    finally:
        yes.stop()
    assert len(yes.asked) == 2 and submit.calls == 2                      # flagged: asked each time


# ---------- continuity ----------

def test_ferb_knows_what_it_was_doing_across_chat_sessions(daemon: RunningDaemon) -> None:
    script = install(daemon, [sup("reply", "Sure, I'll look into your storage."), sup("reply", "We talked about it.")])
    chat(daemon, "can you analyse my mac storage?", session="first")
    chat(daemon, "what were you doing before?", session="second")                   # a new chat
    ctx = script.contexts[1][2]
    assert "analyse my mac storage" in ctx and "I'll look into your storage" in ctx


def test_a_closed_chat_never_stops_a_task_and_its_answer_waits(daemon: RunningDaemon) -> None:
    notes: list[tuple[str, str]] = []

    async def note(title: str, text: str) -> None:
        notes.append((title, text))
    daemon.daemon.notify = note
    gate = threading.Event()

    def slow(token: str) -> str:
        gate.wait(10)
        return writes(daemon.sock, "todo.md", "milk")(token)
    install(daemon, [sup("plan", "On it.", plan([sub("a")], paths=("todo.md",)))],
            {"a": slow, "answer": json.dumps({"text": "I made todo.md with milk."})})
    c = Client(daemon.sock, timeout=30)
    stream = c.stream("chat.send", {"session_id": "s", "text": "create todo.md with milk"})
    first = next(stream)
    assert first["type"] == "plan"
    c.close()                                                                       # the owner closes chat
    time.sleep(0.3)
    gate.set()
    for _ in range(100):
        if notes:
            break
        time.sleep(0.05)
    assert notes and "todo.md" in notes[0][1]                                       # notified
    c2 = Client(daemon.sock)
    try:
        st = c2.call("chat.status")
        assert [r["answer"] for r in st["results"]] == ["I made todo.md with milk."]
        assert c2.call("chat.status")["results"] == []                              # shown once
    finally:
        c2.close()
    assert (daemon.daemon.home / "workspace" / "todo.md").read_text() == "milk"


def test_ctrl_c_stops_the_task_as_stopped_by_you(daemon: RunningDaemon) -> None:
    install(daemon, [sup("plan", "On it.", plan([sub("a")], paths=("todo.md",)))], {"a": STALL})
    c = Client(daemon.sock, timeout=30)
    stream = c.stream("chat.send", {"session_id": "s", "text": "create todo.md with milk"})
    tid = next(stream)["task_id"]
    c2 = Client(daemon.sock)
    try:
        assert c2.call("task.stop", {"task_ids": [tid]})["stopped"] == [tid]
    finally:
        c2.close()
    evs = [e for e in stream]
    c.close()
    out = next(e for e in evs if e["type"] == "outcome")
    assert out["status"] == "cancelled" and out["answer"] == "Stopped, as you asked."
    ctx = daemon.run(_async(daemon.daemon.supervisor.recent))
    assert "stopped by the owner" in ctx


def test_a_restart_tells_the_owner_what_was_interrupted() -> None:
    import shutil
    import tempfile
    base = Path(tempfile.mkdtemp(prefix="fx-", dir="/tmp"))                 # a unix socket path must stay short
    home = base / "h"
    try:
        _restart(home)
    finally:
        shutil.rmtree(base, ignore_errors=True)


def _restart(home: Path) -> None:
    d = RunningDaemon(home)
    try:
        async def seed() -> None:
            d.daemon.db.conn.execute("INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, "  # type: ignore[union-attr]
                                     "scopes_json, status, created_at, updated_at) VALUES ('t9', 't9', NULL, 'r', "
                                     "'research the video and the journalist', 'owner', 'mixed', '[]', 'running', "
                                     "'2026-01-01T09:00:00+00:00', '2026-01-01T09:00:00+00:00')")
        d.run(seed())
    finally:
        d.stop()
    again = RunningDaemon(home)
    try:
        c = Client(again.sock)
        r = c.call("chat.status")["results"]
        c.close()
        assert r and r[0]["status"] == "interrupted" and "research the video" in r[0]["answer"]
    finally:
        again.stop()


# ---------- unpaired extension, Spaces ----------

def test_an_installed_but_unpaired_extension_is_reported(daemon: RunningDaemon) -> None:
    from tests.fake_extension import FakeExtension
    ext = FakeExtension(daemon.daemon.home, browser="brave")
    ext.connect()
    try:
        for _ in range(50):
            if "brave" in daemon.daemon.hub.unpaired.values():                   # type: ignore[union-attr]
                break
            time.sleep(0.05)
        note = daemon.run(_async(lambda: daemon.daemon._unpaired_note("brave")))
        assert note and "installed in Brave but not paired" in note and "safety tests" in note
        assert "not paired" in daemon.run(_async(daemon.daemon._facts))
        daemon.daemon.config.set("gates", "owner_profiles", True)
        assert "ferb browsers pair brave" in daemon.run(_async(lambda: daemon.daemon._unpaired_note("brave")))
    finally:
        ext.kill()


async def _async(fn: Any) -> Any:
    return fn()


def test_a_window_on_another_space_is_still_found(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.mac import screenshot as shots
    here = [{"kCGWindowNumber": 1, "kCGWindowOwnerPID": 9, "kCGWindowLayer": 0, "kCGWindowAlpha": 1,
             "kCGWindowBounds": {"X": 0, "Y": 0, "Width": 40, "Height": 20}, "kCGWindowName": ""}]   # a helper strip
    everywhere = here + [{"kCGWindowNumber": 2, "kCGWindowOwnerPID": 9, "kCGWindowLayer": 0, "kCGWindowAlpha": 1,
                          "kCGWindowBounds": {"X": 0, "Y": 0, "Width": 1470, "Height": 956},
                          "kCGWindowName": "Slack thread"}]                      # full screen, another Space
    monkeypatch.setattr(shots, "_info_windows", lambda all_=False: everywhere if all_ else here)
    w = shots.front_window(9)
    assert w is not None and w.id == 2 and w.title == "Slack thread"
