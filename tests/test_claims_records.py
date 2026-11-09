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
