"""ORCH-587: a literal action check (ORCH-576) makes a claimed send, payment or submit real; it never turns an outcome
that didn't arise into a failure. Live 2026-11-10 (the scheduled visa check): every post showed no slots, nothing was
booked, and the `submitted` check of "if slots are available, book the earliest" failed the run and hid its answer.
Through the real daemon, gateway and scripted model."""
import json
from typing import Any

from tests.conftest import FakeCap, RunningDaemon, chat, cond, done, install, outcome, sup, task, tool

SLOTS = ("Visa slots are checked", "If slots are available, the earliest slot is booked")
NO_SLOTS = "No B1/B2 slots at any of the 5 posts (Chennai, Hyderabad, Kolkata, Mumbai, New Delhi)."
BOOKED = "Booked the 12 Dec slot in Chennai."


def _visa_task() -> dict[str, Any]:
    return task("Check visa slots and book the earliest if any", conds=(
        cond(SLOTS[0]), cond(SLOTS[1], "submitted", site="usvisascheduling.com")))


def _checked(applies: bool, met: bool | None = True) -> str:
    return json.dumps({"results": [
        {"index": 0, "met": True, "evidence": "All five posts were read.", "how": None},
        {"index": 1, "met": met, "applies": applies, "evidence": "No slot opened at any post, so nothing to book.",
         "how": None}], "next": "accept", "findings": [], "missing": "", "instructions": "", "worker_type": None,
        "question": "", "reason": ""})


# decision: ORCH-587, ORCH-576
def test_a_conditional_outcome_that_didnt_arise_leaves_a_correct_run_done(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Checking.", _visa_task())],
            {"worker": done(NO_SLOTS), "worker.verify": _checked(applies=False, met=None)})
    final = outcome(chat(daemon, "check visa slots and book the earliest if any"))
    assert final["status"] == "done" and NO_SLOTS in final["answer"]
    assert final["verification"][1]["met"] is True and final["verification"][1]["path"] == "verifier"


# decision: ORCH-587, ORCH-576, ORCH-583
def test_a_conditional_outcome_that_arose_is_met_only_by_a_recorded_submit(daemon: RunningDaemon) -> None:
    """A slot opened, so booking applies: a run whose submit is recorded is done; one that only claims it fails with
    code's plain sentence, whatever the checker said."""
    daemon.daemon.registry.capabilities["probe.submit"] = FakeCap("probe.submit", "submit", {"probe"})

    def books(token: str) -> str:
        assert not tool(daemon.sock, token, "probe.submit", to="", site="usvisascheduling.com")["is_error"]
        return done(BOOKED)
    install(daemon, [sup("task", "Checking.", _visa_task())], {"worker": books, "worker.verify": _checked(True)})
    assert outcome(chat(daemon, "check visa slots and book the earliest if any"))["status"] == "done"
    install(daemon, [sup("task", "Checking.", _visa_task())],
            {"worker": done(BOOKED), "worker.verify": _checked(True)})
    final = outcome(chat(daemon, "check visa slots and book the earliest if any", session="s2"))
    assert final["status"] == "failed" and final["verification"][1]["path"] == "code"
    assert final["answer"].startswith("I couldn't finish this task. Not done: ")
    assert "usvisascheduling.com" in final["answer"]


# decision: ORCH-594, ORCH-568
def test_a_failed_run_still_tells_what_the_check_confirmed(daemon: RunningDaemon) -> None:
    """Owner 2026-11-10 ("never hide what a run found"): a run that failed because a code check proved its claimed
    booking false keeps that claim out (ORCH-568) but tells, after code's sentence, each condition the check judged met
    with the checker's evidence; so does a run whose worker reported not done."""
    install(daemon, [sup("task", "Checking.", _visa_task())],
            {"worker": done(BOOKED), "worker.verify": _checked(True)})
    final = outcome(chat(daemon, "check visa slots and book the earliest if any"))
    assert final["status"] == "failed" and BOOKED not in final["answer"]
    reason, confirmed = final["answer"].split("\n\nWhat the check confirmed:\n", 1)
    assert reason.startswith("I couldn't finish this task. Not done: ")
    assert confirmed.startswith(f"- {SLOTS[0]}: All five posts were read.")
    blocked = json.dumps({"status": "not_done", "reason": "The site signed me out at Kolkata.", "question": None,
                          "answer": "Chennai and Hyderabad had no slots.", "artifacts": [], "how": ""})
    install(daemon, [sup("task", "Checking.", _visa_task())],
            {"worker": blocked, "worker.verify": json.dumps({"results": [
                {"index": 0, "met": True, "evidence": "Chennai and Hyderabad showed No Slots Available.", "how": None},
                {"index": 1, "met": None, "evidence": "Not every post was read.", "how": "a sign-in"}],
                "next": "fail", "findings": [], "missing": "", "instructions": "", "worker_type": None,
                "question": "", "reason": ""})})
    answer = outcome(chat(daemon, "check visa slots and book the earliest if any", session="s2"))["answer"]
    assert answer.index("The site signed me out at Kolkata.") < answer.index(
        f"What the check confirmed:\n- {SLOTS[0]}: Chennai and Hyderabad showed No Slots Available.") < \
        answer.index("What I found: Chennai and Hyderabad had no slots.")
