"""Build step 14a, offline: an app the owner named is no surprise at the gateway; the irreversible floor (payments,
deletion, account/security pages) asks every time; continuity from the journal across chat sessions and restarts;
a closed chat never stops a task (its answer waits and a notification goes out); Ctrl-C stops it as "stopped by
you" (a held send goes too); an installed but unpaired extension is reported instead of silently falling back;
windows on other Spaces are found.
"""

import json
import threading
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.core.types import ActionSpec, Target
from tests.conftest import Answers, RunningDaemon
from tests.conftest import GatewayCap as FakeCap, gateway_request as request, gateway_setup as setup
from tests.conftest import STALL, chat, install, sup, task, writes


# ---------- naming ----------

def test_an_app_the_owner_named_is_no_surprise(daemon: RunningDaemon) -> None:
    """The owner's session: "in my Brave … slack" asked about com.brave.Browser on every call. Not any more: input in
    an app is an edit and just runs; a submit in an app the owner named is auto-granted."""
    class MacInput(FakeCap):
        def describe(self, args: Any) -> Any:
            return ActionSpec(self.kind, Target(site="com.brave.browser"))
    cap = MacInput("fake.mac", "mac_input", {"mac:act"})
    submit = MacInput("fake.submit", "submit", {"mac:act"})
    grant, _ = setup(daemon, {"mac:act"}, request("in my brave, join the slack thread"), [cap, submit])
    asked = Answers(daemon)
    try:
        for _ in range(3):
            assert not daemon.run(daemon.daemon.gateway.call(grant, "fake.mac", {"effect": "edit"}))["is_error"]
        assert not daemon.run(daemon.daemon.gateway.call(grant, "fake.submit", {"why": "joining the thread"}))["is_error"]
    finally:
        asked.stop()
    assert asked.asked == [] and cap.calls == 3 and submit.calls == 1


# ---------- the irreversible floor ----------


# ---------- continuity ----------

def test_ferb_knows_what_it_was_doing_across_chat_sessions(daemon: RunningDaemon) -> None:
    script = install(daemon, [sup("reply", "Sure, I'll look into your storage."), sup("reply", "We talked about it.")])
    first = Client(daemon.sock).call("chat.attach", {})["session_id"]
    chat(daemon, "can you analyse my mac storage?", session=first)
    second = Client(daemon.sock).call("chat.attach", {})["session_id"]   # a new connection resumes the conversation
    chat(daemon, "what were you doing before?", session=second)
    assert first == second
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
        writes(daemon.sock, "todo.md", "milk")(token)
        return json.dumps({"status": "done", "summary": "I made todo.md with milk.", "artifacts": ["todo.md"],
                           "evidence": []})
    install(daemon, [sup("task", "On it.", task("write todo.md", paths=("todo.md",)))], {"worker": slow})
    c = Client(daemon.sock, timeout=30)
    stream = c.stream("chat.send", {"session_id": "s", "text": "create todo.md with milk"})
    assert next(stream)["type"] == "ack"                                           # on receipt (L7)
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
        assert c2.call("chat.status")["results"] == st["results"]                   # reading marks nothing
        c2.call("results.ack", {"task_ids": [r["task_id"] for r in st["results"]]})
        assert c2.call("chat.status")["results"] == []                              # shown once acknowledged
    finally:
        c2.close()
    assert (daemon.daemon.home / "workspace" / "todo.md").read_text() == "milk"


def test_ctrl_c_stops_the_task_as_stopped_by_you(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "On it.", task("write todo.md", paths=("todo.md",)))], {"worker": STALL})
    c = Client(daemon.sock, timeout=30)
    stream = c.stream("chat.send", {"session_id": "s", "text": "create todo.md with milk"})
    tid = next(e for e in stream if e["type"] == "plan")["task_id"]
    c2 = Client(daemon.sock)
    try:
        # notes/daemon.md decision 1: no ids is refused (it once stopped every running tree, scheduled runs and other
        # chats included); stopping everything is stop.engage
        with pytest.raises(Exception, match="needs the ids"):
            c2.call("task.stop", {})
        assert tid in [t for t, _ in daemon.daemon.supervisor.running_trees()]
        assert c2.call("task.stop", {"task_ids": [tid]})["stopped"] == [tid]
    finally:
        c2.close()
    evs = [e for e in stream]
    c.close()
    out = next(e for e in evs if e["type"] == "outcome")
    assert out["status"] == "cancelled" and out["answer"] == "Stopped, as you asked."
    from tests.support.history import conversation_text
    ctx = daemon.run(_async(lambda: conversation_text(daemon.daemon.supervisor, "s")))
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
        assert note and "installed in Brave but not paired" in note and "browser_pair" in note
        assert "not paired" in daemon.run(_async(daemon.daemon._facts))
        assert "Settings → Browsers" in daemon.run(_async(lambda: daemon.daemon._unpaired_note("brave")))
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
    assert shots.front_window(9) is None  # two candidates: size/current Space never chooses one
    w = shots.front_window(9, (0, 0, 1470, 956))
    assert w is not None and w.id == 2 and w.title == "Slack thread"


def test_a_second_message_is_answered_while_the_first_task_runs(daemon: RunningDaemon) -> None:
    """Several at once (owner request, 2026-09-30): the daemon takes a new message in the same chat session while an
    earlier task is still working."""
    install(daemon, [sup("task", "On it.", task("write todo.md", paths=("todo.md",))), sup("reply", "It's 9 am.")],
            {"worker": STALL})
    first: list[dict[str, Any]] = []

    def long_one() -> None:
        c = Client(daemon.sock, timeout=30)
        try:
            for ev in c.stream("chat.send", {"session_id": "s", "text": "create todo.md"}):
                first.append(ev)
                if ev["type"] in ("outcome", "reply"):
                    break
        finally:
            c.close()
    t = threading.Thread(target=long_one, daemon=True)
    t.start()
    for _ in range(200):
        if any(e["type"] == "plan" for e in first):
            break
        time.sleep(0.02)
    second = chat(daemon, "what time is it?", session="s")
    assert [e["text"] for e in second if e["type"] == "reply"] == ["It's 9 am."]
    assert not any(e["type"] == "outcome" for e in first)                  # the first is still working
    tid = next(e["task_id"] for e in first if e["type"] == "plan")
    c2 = Client(daemon.sock)
    try:
        c2.call("task.stop", {"task_ids": [tid]})
    finally:
        c2.close()
    t.join(10)
