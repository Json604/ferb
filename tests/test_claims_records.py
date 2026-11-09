"""What FERB says about its own state and its work comes from its records (owner 2026-11-08: "ferb should not lie";
"where is honesty in ferb stating how many schedules are there and not from jsut its memory???"; claims audit §5,
approved P1, P2, P3). Through the real daemon, gateway and scripted model: an action check reads the task's own calls
and a false one fails the work whatever the checker said; result cards come from calls and files that exist."""
import json
from typing import Any

import pytest

from tests.conftest import FakeCap, RunningDaemon, chat, cond, done, install, outcome, report, sup, task, tool, verdict
from tests.support.confirm import FerbAnswers
from tests.support.trash import use_trash


def _cards(event: dict[str, Any], kind: str) -> list[dict[str, Any]]:
    return [c for c in event.get("cards") or [] if c["type"] == kind]


# decision: ORCH-561
def test_a_send_the_records_dont_show_fails_however_the_checker_judged(daemon: RunningDaemon) -> None:
    """The worker says it sent; no send ran. The checker accepts; code's sent check is false, so the task fails."""
    spec = task("Email Alex the report", conds=(cond("Alex got the report", "sent", expected="alex@work.example"),))
    install(daemon, [sup("task", "Sending.", spec)], {
        "worker": done("Sent the report to alex@work.example."),
        "worker.verify": verdict(True, next="accept")})
    final = outcome(chat(daemon, "email alex the report"))
    assert final["status"] == "failed"
    [result] = final["verification"]
    assert result["met"] is False and result["path"] == "code" and "no send to alex@work.example" in result["evidence"]


# decision: ORCH-561
def test_a_send_that_ran_meets_its_check_and_is_a_card(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:new_recipient")
    daemon.daemon.registry.capabilities["probe.send"] = FakeCap("probe.send", "send", {"probe"})

    def sends(token: str) -> str:
        assert not tool(daemon.sock, token, "probe.send", to="Alex@Work.example", site="mail.google.com")["is_error"]
        return done("Sent it.")
    spec = task("Email Alex the report", conds=(cond("Alex got the report", "sent", expected="alex@work.example"),))
    install(daemon, [sup("task", "Sending.", spec)], {"worker": sends, "worker.verify": verdict(True)})
    final = outcome(chat(daemon, "email alex the report"))
    assert final["status"] == "done"
    assert _cards(final, "send")[0]["recipient"] == "Alex@Work.example"


# decision: ORCH-561
def test_trash_and_files_are_cards_only_from_calls_and_paths_that_exist(daemon: RunningDaemon,
                                                                        monkeypatch: pytest.MonkeyPatch) -> None:
    """A trashed file is a trash card from the fs.trash call; a file the worker lists that doesn't exist gets no card;
    a trashed check on another path is false."""
    bin_ = daemon.daemon.home.parent / ".Trash"
    bin_.mkdir(parents=True, exist_ok=True)
    use_trash(monkeypatch, bin_)
    folder = daemon.daemon.home / "workspace"
    folder.mkdir(parents=True, exist_ok=True)
    old, kept = folder / "old.txt", folder / "kept.md"
    old.write_text("x")
    kept.write_text("y")

    def tidies(token: str) -> str:
        r = tool(daemon.sock, token, "fs.trash", path=str(old))
        assert not r["is_error"], r
        return json.dumps(report("done", "Trashed old.txt and wrote two files.",
                                 artifacts=(str(kept), str(folder / "never-made.md"))))
    spec = task("Tidy the folder", paths=(str(folder),), conds=(
        cond("old.txt is in the Trash", "trashed", path=str(old)),
        cond("kept.md is in the Trash", "trashed", path=str(kept))))
    install(daemon, [sup("task", "Tidying.", spec)], {"worker": tidies, "worker.verify": verdict(True, True)})
    final = outcome(chat(daemon, "tidy the folder"))
    met = final["verification"][1]                          # kept.md was never trashed: code's false stands
    assert met["met"] is False and met["path"] == "code" and final["status"] == "failed"
    assert [c["path"] for c in _cards(final, "trash")] == [str(old)]
    assert [c["path"] for c in _cards(final, "file")] == [str(kept)]


def _reply(text: str, **fields: Any) -> str:
    return json.dumps({"kind": "reply", "text": text, "task": None, **fields})


def _reply_of(evs: list[dict[str, Any]]) -> dict[str, Any]:
    return next(e for e in evs if e["type"] == "reply")


# decision: ORCH-563, ORCH-565
def test_each_kind_of_ferbs_state_is_a_card_from_its_records(daemon: RunningDaemon) -> None:
    """The planner names what the owner asked about; code draws each from FERB's rows after the turn's own changes
    (a reminder set in the same turn is in the list), and the planner's live facts count the reminders as the truth."""
    from tests.support.schedules import mins
    from datetime import timedelta
    script = install(daemon, [_reply("Here is what I have.", timers=[
        {"when": mins(timedelta(minutes=30)), "message": "stretch", "quote": "remind me in 30 minutes to stretch",
         "kind": "reminder", "name": "Stretch", "when_active": False}],
        show=["reminders", "tasks", "memory", "permissions", "logins", "about", "reminders"])])
    reply = _reply_of(chat(daemon, "remind me in 30 minutes to stretch, and what do you have set up for me?"))
    kinds = [c["type"] for c in reply["cards"] if c["type"] not in ("event",)]
    assert kinds == ["reminders", "tasks", "memory", "permissions", "logins", "about"]       # each once, in order
    reminders = _cards(reply, "reminders")[0]
    assert reminders["count"] == 1 and reminders["reminders"][0]["name"] == "Stretch"
    assert all(c["line"] in reply["text"] for c in reply["cards"] if c["type"] != "event")
    assert "routine questions on" in _cards(reply, "permissions")[0]["line"]
    script = install(daemon, [_reply("One reminder.")])
    chat(daemon, "how many reminders?")
    planner = next(ctx for mode, key, ctx, _ in script.contexts)
    assert "pending reminders and timers, from FERB's records (the truth" in planner and ": 1 (1 reminder)" in planner


# decision: ORCH-564
def test_a_reply_about_a_task_shows_its_records_beside_its_words(ferb_yes: FerbAnswers,
                                                               daemon: RunningDaemon) -> None:
    """"Did you do it or not?" (journal 1066): the planner names the task its reply is about; code shows that task's
    status and its send card from its records. An id the planner may not name is told back once, then shows nothing."""
    ferb_yes.expect("floor:new_recipient")
    daemon.daemon.registry.capabilities["probe.send"] = FakeCap("probe.send", "send", {"probe"})

    def sends(token: str) -> str:
        assert not tool(daemon.sock, token, "probe.send", to="alex@work.example", site="mail.google.com")["is_error"]
        return done("Sent it.")
    install(daemon, [sup("task", "Sending.", task("Email Alex the report"))], {"worker": sends})
    first = outcome(chat(daemon, "email alex the report"))
    script = install(daemon, [_reply("Yes, I sent it.", about_tasks=[first["task_id"], "nope"]),
                              _reply("Yes, I sent it.", about_tasks=[first["task_id"], "nope"])])
    reply = _reply_of(chat(daemon, "did you send it or not?"))
    [card] = _cards(reply, "task")
    assert card["task_id"] == first["task_id"] and card["status"] == first["status"] and card["line"] in reply["text"]
    assert card["cards"][0]["recipient"] == "alex@work.example" and _cards(reply, "send")
    assert any("about_tasks: 'nope'" in ctx for _, _, ctx, _ in script.contexts)        # told back once


# decision: ORCH-566
def test_the_history_tells_ferbs_unchecked_words_from_codes_records(daemon: RunningDaemon) -> None:
    """The six-checks case (journal 1128-1136): code's "Scheduled" line and the planner's wrong prose were one reply,
    and the next turn read both as fact. Now code's lines are kept apart and the history labels each."""
    from datetime import timedelta
    from tests.support.schedules import mins
    install(daemon, [_reply("It checks six times a day.", timers=[
        {"when": mins(timedelta(minutes=30)), "message": "stretch", "quote": "remind me in 30 minutes to stretch",
         "kind": "reminder", "name": "Stretch", "when_active": False}])])
    reply = _reply_of(chat(daemon, "remind me in 30 minutes to stretch"))
    code_line = reply["text"].split("\n", 1)[1]
    script = install(daemon, [_reply("Okay.")])
    chat(daemon, "and how often does it check?")
    planner = next(ctx for mode, key, ctx, _ in script.contexts)
    assert "FERB (reply, model-written, unchecked): It checks six times a day." in planner
    assert f"FERB (code record): {code_line}" in planner


# decision: ORCH-567
def test_forgets_are_on_record_without_their_contents(daemon: RunningDaemon) -> None:
    """A forgotten-items lookup: memory.read forgotten says when FERB forgot and how many items; never what they were."""
    from tests.mackit import gateway_call as call, grant_task as granted_task
    from tests.support.schedules import run
    m = daemon.daemon.memory
    item = run(daemon, lambda: m.save("fact", "owner", "locker", "My locker code is 4512", "owner_stated",
                                      [("journal", "j1")], 1.0).item)
    run(daemon, lambda: daemon.daemon._forget_memory(item.id))
    from ferb.core.types import Identity
    g = granted_task(daemon, "q", Identity.OWNER, {"task:read"}, {"research"})
    r = call(daemon, g, "memory.read", forgotten=True)
    assert not r["is_error"] and [f["items"] for f in r["content"]["forgotten"]] == [1]
    assert "4512" not in json.dumps(r["content"])


# decision: ORCH-567
def test_a_task_card_carries_the_questions_the_owner_answered(daemon: RunningDaemon) -> None:
    """The tasks card's rows hold each question a task asked and the owner's answer, from its owner.ask records."""
    import asyncio
    import time
    from tests.conftest import gateway_request, gateway_setup
    grant, _ = gateway_setup(daemon, set(), gateway_request("deploy the site"), [])
    daemon.daemon.gateway.route_question = None              # straight to the owner (no orchestrator in this test)
    asked = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.call(grant, "owner.ask", {
        "question": "Which branch should I deploy?", "options": []}), daemon.loop)
    for _ in range(400):
        pending = [ap for ap in daemon.daemon.state.approvals.pending() if ap.capability == "owner.ask"]
        if pending:
            break
        time.sleep(0.05)
    daemon.run(daemon.daemon._chat_answer(None, {"approval_id": pending[0].id, "text": "main"}))
    asked.result(timeout=10)
    from tests.support.schedules import run
    got = run(daemon, lambda: daemon.daemon.supervisor._answered_questions(grant.task_id.split(".", 1)[0]))
    assert got == [{"question": "Which branch should I deploy?", "answer": "main"}]
