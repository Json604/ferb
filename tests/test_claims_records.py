"""What FERB says about its own state and its work comes from its records (owner 2026-11-08: "ferb should not lie";
"where is honesty in ferb stating how many schedules are there and not from jsut its memory???"; claims audit §5,
approved P1, P2, P3). Through the real daemon, gateway and scripted model: an action check reads the task's own calls
and a false one fails the work whatever the checker said; a file card comes only from a file that exists. Owner
2026-11-10 (APP-823): the record cards of 2026-11-09 are gone; what FERB says still rests on its records (facts,
lookups), never on cards."""
import json
from typing import Any

import pytest

from tests.conftest import FakeCap, RunningDaemon, chat, cond, done, install, outcome, report, sup, task, tool, verdict
from tests.support.confirm import FerbAnswers
from tests.support.trash import use_trash


def _cards(event: dict[str, Any], kind: str) -> list[dict[str, Any]]:
    return [c for c in event.get("cards") or [] if c["type"] == kind]


# decision: ORCH-576
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


# decision: ORCH-576
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


# decision: ORCH-576
def test_trashed_checks_read_the_calls_and_files_are_cards_only_when_they_exist(daemon: RunningDaemon,
                                                                              monkeypatch: pytest.MonkeyPatch) -> None:
    """A trashed check on a path no fs.trash call named is false; the trash itself is no card (APP-823);
    a file the worker lists that doesn't exist gets no card."""
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
    assert not [c for c in final.get("cards") or [] if c["type"] in ("submit", "trash", "delete", "download")]
    assert [c["path"] for c in _cards(final, "file")] == [str(kept)]


def _reply(text: str, **fields: Any) -> str:
    return json.dumps({"kind": "reply", "text": text, "task": None, **fields})


def _reply_of(evs: list[dict[str, Any]]) -> dict[str, Any]:
    return next(e for e in evs if e["type"] == "reply")


# decision: ORCH-577
def test_the_planner_counts_reminders_from_the_records_and_shows_no_record_cards(daemon: RunningDaemon) -> None:
    """The planner's live facts count the reminders as FERB's records hold them, marked as the truth. An answer in the
    2026-11-09 form (show, about_tasks) reads as before them: its schedules list is shown, nothing else is a card."""
    from datetime import timedelta
    from tests.support.schedules import mins
    install(daemon, [_reply("Set.", timers=[
        {"when": mins(timedelta(minutes=30)), "message": "stretch", "quote": "remind me in 30 minutes to stretch",
         "kind": "reminder", "name": "Stretch", "when_active": False}])])
    chat(daemon, "remind me in 30 minutes to stretch")
    script = install(daemon, [_reply("One reminder, no schedules.", show=["schedules", "reminders", "tasks", "memory",
                                                                          "permissions", "logins", "about"],
                                     about_tasks=["nope"])])
    reply = _reply_of(chat(daemon, "how many reminders and schedules do I have?"))
    planner = next(ctx for mode, key, ctx, _ in script.contexts)
    assert "pending reminders and timers, from FERB's records (the truth" in planner and ": 1 (1 reminder)" in planner
    assert [c["type"] for c in reply.get("cards") or []] == ["schedules"]
    assert len(script.contexts) == 1                         # no repair round for the old about_tasks


# decision: ORCH-578
def test_the_self_description_names_permissions_and_logins_without_values(daemon: RunningDaemon) -> None:
    """about.read (one of the planner's lookups) carries the permissions in force and the saved logins by name."""
    from ferb.orchestrator.supervisor import PLANNER_TOOLS
    doc = daemon.run(daemon.daemon._about_document())
    held = doc["permissions_in_force"]
    assert set(held) == {"standing_sends", "known_destinations", "routine_questions_off_until"}
    assert set(doc["saved_logins"]) == {"owner_saved", "ferb_own"} and "about.read" in PLANNER_TOOLS


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


# decision: ORCH-578
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
