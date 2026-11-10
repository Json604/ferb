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
    assert final["answer"].split("\n\n")[0] == "I couldn't finish this task. No form on usvisascheduling.com was submitted."     # ORCH-609


# decision: ORCH-600, ORCH-603, ORCH-605, ORCH-609, ORCH-568
def test_a_refuted_claim_fails_in_codes_words_without_the_checkers(daemon: RunningDaemon) -> None:
    """Owner 2026-11-11 ("why it seems not like a ferb response but some error its justification"): a run whose claimed
    booking a code check proved false keeps that claim out (ORCH-568) and says why in code's sentence: no checker's
    reason or evidence of what it confirmed, no "What the check confirmed" list. A worker that reported not done is
    told in one text only (ORCH-605, "One text only"): its answer when it wrote one."""
    install(daemon, [sup("task", "Checking.", _visa_task())],
            {"worker": done(BOOKED), "worker.verify": _checked(True)})
    final = outcome(chat(daemon, "check visa slots and book the earliest if any"))
    assert final["status"] == "failed" and BOOKED not in final["answer"]
    assert final["answer"].split("\n\n")[0] == "I couldn't finish this task. No form on usvisascheduling.com was submitted."   # code's finding, no goal text
    assert "All five posts were read." not in final["answer"] and "check confirmed" not in final["answer"]
    blocked = json.dumps({"status": "not_done", "reason": "The site signed me out at Kolkata.", "question": None,
                          "answer": "Chennai and Hyderabad had no slots. The site signed me out at Kolkata.",
                          "artifacts": [], "how": ""})
    install(daemon, [sup("task", "Checking.", _visa_task())],
            {"worker": blocked, "worker.verify": json.dumps({"results": [
                {"index": 0, "met": True, "evidence": "Chennai and Hyderabad showed No Slots Available.", "how": None},
                {"index": 1, "met": None, "evidence": "Not every post was read.", "how": "a sign-in"}],
                "next": "fail", "findings": [], "missing": "", "instructions": "", "worker_type": None,
                "question": "", "reason": "The run could not read every post."})})
    final = outcome(chat(daemon, "check visa slots and book the earliest if any", session="s2"))
    assert final["answer"].split("\n\n")[:2] == [
        "I couldn't finish this task.", "Chennai and Hyderabad had no slots. The site signed me out at Kolkata."]
    assert final["answer"].count("signed me out") == 1 and "What I found" not in final["answer"]
    assert "could not read every post" not in final["answer"] and "No Slots Available" not in final["answer"]


BLOCKED = ("The visa-site check was blocked, so nothing was booked.\n\nThe page showed: \u201cYou have exceeded the "
           "limit for viewing this page.\u201d I stopped to avoid spamming the site.")


# decision: ORCH-600, ORCH-601, ORCH-602, ORCH-606, ORCH-607, ORCH-587
def test_a_blocked_check_left_unconfirmed_is_ferbs_answer_whole(daemon: RunningDaemon) -> None:
    """Live 2026-11-11 (task 2bb55837): the checker left the booking condition unconfirmed (met null, applies true)
    and chose accept_unconfirmed; the literal submitted check turned it false and the run failed with the checker's
    reason and a cut-off evidence line. Now the worker's answer stands, whole, done-unconfirmed, with the one fixed
    line (ORCH-606); which condition, and the checker's reason, evidence and how, stay in the record."""
    spec = _visa_task()
    spec["success_conditions"][1]["owner_text"] = "If slots are available, your earliest slot is booked"
    long_evidence = "Fresh snapshots showed the page-view limit after one refresh and one new tab. " * 6
    install(daemon, [sup("task", "Checking.", spec)], {"worker": done(BLOCKED), "worker.verify": json.dumps({
        "results": [{"index": 0, "met": True, "applies": True, "evidence": long_evidence, "how": None},
                    {"index": 1, "met": None, "applies": True, "evidence": "Availability could not be seen.",
                     "how": "Open the visa site after the limit clears."}],
        "next": "accept_unconfirmed", "findings": [], "missing": "", "instructions": "", "worker_type": None,
        "question": "", "reason": "The barrier made current availability impossible to verify."})})
    final = outcome(chat(daemon, "check visa slots and book the earliest if any"))
    assert final["status"] == "done_unverified"
    assert final["answer"].split("\n\nSites contacted")[0] == (
        BLOCKED + "\n\nI couldn't confirm part of this.")                    # ORCH-606: the one fixed line
    assert final["unconfirmed"] == "If slots are available, your earliest slot is booked"
    assert final["verification"][1]["met"] is None and final["verification"][1]["path"] == "verifier"
    for checker_text in ("impossible to verify", "page-view limit after", "after the limit clears", "\u2026"):
        assert checker_text not in final["answer"]


# the recorded reason of the scheduled run eea265b9's last attempt (2026-11-10 18:10), as the model returned it
STRAY = ("After one submission attempt for the Hyderabad OFC slot on 12/01/2026 at 14:30, usvisascheduling.com again "
         "showed \"The system is processing too many requests. Please try again.\" I stopped without retrying. Linked "
         "consular availability was not reached and nothing was booked. The session was still logged in, and no "
         "Cloudflare check appeared.</CLARIFICATION>\n")


# decision: ORCH-604, ORCH-605
def test_a_closing_tag_the_model_left_alone_never_reaches_the_owner(daemon: RunningDaemon) -> None:
    """Live 2026-11-10 18:12: a worker's reason ended "…appeared.</CLARIFICATION>" and the failed reply showed it.
    A closing tag with no opening tag in the same field is removed before the report is used; code and FERB's own
    tags stay."""
    report = json.dumps({"status": "not_done", "reason": STRAY, "question": None, "answer": "", "artifacts": [],
                         "how": ""})
    install(daemon, [sup("task", "Checking.", _visa_task())], {"worker": report, "worker.verify": json.dumps({
        "results": [{"index": 0, "met": False, "evidence": "Blocked.", "how": None},
                    {"index": 1, "met": False, "evidence": "Nothing booked.", "how": None}],
        "next": "fail", "findings": [], "missing": "", "instructions": "", "worker_type": None, "question": "",
        "reason": "Blocked."})})
    final = outcome(chat(daemon, "check visa slots and book the earliest if any"))
    assert final["answer"].split("\n\n")[:2] == ["I couldn't finish this task.", STRAY.split("</")[0]]
    assert "CLARIFICATION" not in json.dumps(final)
    from ferb.orchestrator.answer_marks import without_stray_closing_tags as clean
    kept = 'Run `</div>` in <b>bold</b>, and <untrusted-data id="ab">x</untrusted-data id="ab">.'
    assert clean(kept) == kept and clean("a </x> b") == "a b"


# decision: ORCH-610, ORCH-608, ORCH-606
def test_a_claim_only_the_checker_doubted_stands_unconfirmed_with_just_the_line(daemon: RunningDaemon) -> None:
    """Task 2357c470 (2026-11-08): the checker judged the outcome not met with no code proof and chose
    accept_unconfirmed; the run failed and hid the answer. Now it stands done-unconfirmed with the worker's answer and
    the one line; a sentence the checker found unsupported, left by a fix turn that failed, carries no inline mark
    since the line ends the reply (a contradicted one is still removed)."""
    said = "Unsubscribed you from ChatGPT, Slack and Cloudways. Cloudways was never opened. Mailsuite failed."
    install(daemon, [sup("task", "On it.", task("Unsubscribe me", conds=(cond("Three senders unsubscribed"),)))], {
        "worker": done(said), "worker.fix": "not a report",
        "worker.verify": json.dumps({"results": [{"index": 0, "met": False, "evidence": "Calls 28, 31.", "how": None}],
                                     "next": "accept_unconfirmed", "missing": "", "instructions": "",
                                     "worker_type": None, "question": "", "reason": "Confirmation pages unseen.",
                                     "findings": [{"quote": "Cloudways was never opened.", "kind": "unsupported",
                                                   "evidence": "One was read."},
                                                  {"quote": "Mailsuite failed.", "kind": "contradicted",
                                                   "evidence": "Cancelled."}]})})
    final = outcome(chat(daemon, "unsubscribe me"))
    assert final["status"] == "done_unverified"
    assert final["answer"].split("\n\nSites contacted")[0] == (
        "Unsubscribed you from ChatGPT, Slack and Cloudways. Cloudways was never opened.\n\n"
        "I couldn't confirm part of this.")
