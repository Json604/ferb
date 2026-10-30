"""Regression contracts from the failed search and login attempts, using synthetic inputs only."""
import asyncio
import pytest

from ferb.core.types import ComputerAction
from tests.mackit import Ax, ids, background_mac as setup


def test_ax_set_value_success_without_value_change_is_not_reported_as_success(monkeypatch) -> None:
    ax = Ax(takes_background=False)
    ax.set_value = lambda *args: True
    d, _, _ = setup(monkeypatch, ax)
    e = ids(d)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("set_value", e["Message"], "test")))
    assert r.ok is False
    assert d.profiles.status("net.whatsapp.WhatsApp", "ax_value") == "failed"


def test_search_semantics_do_not_make_an_ax_button_a_writable_text_field(monkeypatch) -> None:
    from ferb.act.mac.accessibility import AxNode, AxTree
    node = AxNode("x", "searchbox", "Search", None, (0, 0, 10, 10), False, True, ax_role="AXButton")
    ax = Ax()
    ax.read_tree = lambda *args: AxTree([node])
    ax.post_text = lambda *args: pytest.fail("a search label does not authorize a keyboard fallback")
    d, apps, posted = setup(monkeypatch, ax)
    ref = ids(d)["Search"]
    result = asyncio.run(d.act("WhatsApp", ComputerAction("set_value", ref, "query")))
    assert not result.ok and result.facts["accepted"] is False
    assert not posted and not apps.activated


def test_learned_input_failures_are_observations_not_permissions() -> None:
    from ferb.act.mac.profile import Profiles
    p = Profiles()
    for _ in range(4):
        p.record("example.app", "pid_keys", False)
    assert p.status("example.app", "pid_keys") == "failed"
    p.record("example.app", "pid_keys", True)
    assert p.status("example.app", "pid_keys") == "landed"


def test_followup_keeps_the_original_owner_words_and_browser_choice(daemon, monkeypatch) -> None:
    import json
    from tests.conftest import chat, install, sup, task, outcome
    # needing the owner pauses an attended task in place; here each run ends at once with its question (unattended)
    monkeypatch.setattr(type(daemon.daemon.supervisor), "_unattended", lambda self, task: True)
    first = task("use Work profile to order lunch on shop.example", "owner", sites=("shop.example",))
    install(daemon, [sup("task", "Starting.", first)], {"worker": json.dumps({"status": "needs_user",
        "summary": "Login needed", "artifacts": [], "evidence": []})})
    previous = outcome(chat(daemon, "order lunch on shop.example in my Work profile"))
    followup = json.loads(sup("task", "Retrying.", task("retry login", "owner")))
    followup["control"] = {"task_id": previous["task_id"], "action": "retry", "standing_sends": "unchanged", "supersedes": []}
    seen = []

    def worker(token):
        grant = daemon.daemon.tokens.validate(token)
        rec = daemon.daemon.tasks.get(grant.task_id)
        seen.append((rec.task.goal, daemon.daemon.state.requests.get(rec.task.request_id).words))
        return json.dumps({"status": "needs_user", "summary": "Waiting for login", "artifacts": [], "evidence": []})

    install(daemon, [json.dumps(followup)], {"worker": worker})
    chat(daemon, "retry login")
    assert "Work profile" in seen[0][0] and "shop.example" in seen[0][1]


@pytest.mark.parametrize("phase", ["starting", "worker", "verifier"])
def test_chat_stop_cancels_the_running_attempt_instead_of_just_saying_so(daemon, phase, monkeypatch) -> None:
    import json
    import threading
    from tests.conftest import STALL, FakeRuntime, chat, install, sup, task, cond
    started = threading.Event()
    root = []

    def run():
        from ferb.cli.client import Client
        c = Client(daemon.sock, timeout=30)
        try:
            for ev in c.stream("chat.send", {"session_id": "s", "text": "do a long task"}):
                if ev["type"] == "plan":
                    root.append(ev["task_id"])
                    started.set()
        finally:
            c.close()

    if phase == "starting":
        original_start = FakeRuntime.start

        async def slow_start(runtime, task, request, token):
            handle = await original_start(runtime, task, request, token)
            if handle.state["key"] == "worker":
                tree = daemon.daemon.supervisor._tree_of[task.id]
                async with asyncio.timeout(5):
                    while not tree.stop:
                        await asyncio.sleep(.01)
            return handle

        monkeypatch.setattr(FakeRuntime, "start", slow_start)
    steps = {"worker": STALL} if phase != "verifier" else {"worker": json.dumps(
        {"status": "done", "summary": "finished", "artifacts": [], "evidence": []}), "worker.verify": STALL}
    script = install(daemon, [sup("task", "Starting", task("long task", conds=(cond("goal achieved"),)))], steps)
    thread = threading.Thread(target=run)
    thread.start()
    assert started.wait(5)
    if phase == "verifier":
        import time
        deadline = time.monotonic() + 5
        while not any(key == "worker.verify" for _, key, _, _ in script.contexts) and time.monotonic() < deadline:
            time.sleep(.01)
        assert any(key == "worker.verify" for _, key, _, _ in script.contexts)
    response = json.loads(sup("reply", "Stopped"))
    response["control"] = {"task_id": root[0], "action": "stop", "standing_sends": "unchanged", "supersedes": []}
    script.sup.append(json.dumps(response))
    evs = chat(daemon, "stop", session="s")
    thread.join(5)
    assert not thread.is_alive()
    assert not daemon.daemon.supervisor.running_trees()
    assert any(e.get("text") == "Stopped, as you asked." for e in evs)
