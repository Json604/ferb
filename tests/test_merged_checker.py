"""Step 8b end-to-end runs (TEST-500): the merged checker judges the conditions and chooses what's next (owner
ORCH-302); "done" is about the outcome asked for, and the answer comes first (ORCH-309); a usage limit or a
sign-in failure is written by code and offers another signed-in runtime with one tap (ORCH-315). The real
daemon, gateway and scripted model."""
import json
from typing import Any

import pytest

from ferb.core.types import RuntimeEvent
from ferb.orchestrator.attempts import CHECK_IN_AFTER
from ferb.core.structured import check_final
from ferb.orchestrator.schemas import CHECKER_SCHEMA, CheckerOutcome
from ferb.orchestrator.supervisor import FOUND_UNCHECKED, RUNTIME_ASK
from tests.conftest import (RunningDaemon, _context_of, answer_question, chat, done, install, outcome, report, sup,
                            task, cond, verdict)
from tests.support.schedules import run
from tests.test_execution_runs import Turn

BOOKED = "Booked a table for 4 at 7pm at Nopa. The restaurant confirmed by email."


def _checks(script: Any) -> list[str]:
    return script.calls("checker")


def _fix(answer: str) -> str:
    return json.dumps({"answer": answer})


def _settled(daemon: RunningDaemon) -> list[Any]:
    async def read() -> list[Any]:
        return [e.data for e in daemon.daemon.journal.of_kind("task.answer_settled")]
    return daemon.run(read())


COURSERA = ("Checked Coursera for the two assignments. Quiz 3 is visible but locked until Monday. The tracker has "
            "both dates.")


# decision: ORCH-545, ORCH-309
def test_a_contradicted_sentence_is_fixed_by_the_worker_once_and_stays_unmarked(daemon: RunningDaemon) -> None:
    """The Coursera case (task 7d870ef5): the answer says a quiz is locked until Monday; the page shows it due Friday.
    The checker quotes the sentence (contradicted) and writes no replacement; the same worker gets one turn to fix only
    that sentence, a short re-check passes, and the corrected answer stands with no mark and no redo of the work."""
    fixed = COURSERA.replace("visible but locked until Monday", "open and due Friday")
    script = install(daemon, [sup("task", "On it.", task("Check the Coursera assignments"))], {
        "worker": done(COURSERA),
        "worker.verify": verdict(True, findings=(("Quiz 3 is visible but locked until Monday.", "contradicted",
                                                  "The course page read at 10:02 shows Quiz 3 open, due Friday."),)),
        "worker.fix": _fix(fixed),
        "worker.recheck": json.dumps({"findings": []})})
    result = outcome(chat(daemon, "check my coursera assignments"))
    assert script.calls("worker") == ["worker", "worker.fix"] and script.calls("rechecker") == ["worker.recheck"]
    assert result["status"] == "done" and result["verified"] is True and result["answer"] == fixed
    assert "couldn't confirm" not in result["answer"].lower()
    fix_context = _context_of(script, "worker.fix")
    assert "Quiz 3 is visible but locked until Monday." in fix_context and "due Friday" in fix_context
    assert [d["how"] for d in _settled(daemon)] == ["fixed"]


# decision: ORCH-545
def test_a_guess_stated_as_fact_is_rewritten_as_a_guess(daemon: RunningDaemon) -> None:
    """The Hermes case: "Hermes is an acquisition funnel." had nothing behind it; the worker's fix turn says it as a
    guess, the re-check passes, and that answer stands."""
    said = "The repo has three services. Hermes is an acquisition funnel."
    guess = "The repo has three services. Hermes looks like an acquisition funnel, but I couldn't see what it does."
    install(daemon, [sup("task", "On it.", task("Summarize the repo's services"))], {
        "worker": done(said),
        "worker.verify": verdict(True, findings=(("Hermes is an acquisition funnel.", "unsupported",
                                                  "No file the worker read says what Hermes does."),)),
        "worker.fix": _fix(guess), "worker.recheck": json.dumps({"findings": []})})
    result = outcome(chat(daemon, "what services does the repo have"))
    assert result["status"] == "done" and result["answer"] == guess


# decision: ORCH-545
def test_when_the_fix_turn_fails_code_removes_or_marks_exactly_the_quoted_sentences(daemon: RunningDaemon) -> None:
    said = "Booked a table for 4 at 7pm at Nopa. The restaurant confirmed by email. Parking is free."
    install(daemon, [sup("task", "On it.", task("Book a table for 4 at 7pm"))], {
        "worker": done(said),
        "worker.verify": verdict(True, findings=(
            (" The restaurant confirmed by email.", "contradicted", "No email from Nopa was read."),
            ("Parking is free.", "unsupported", "Nothing about parking was seen."))),
        "worker.fix": "not a report"})
    result = outcome(chat(daemon, "book a table for 4 at 7pm"))
    assert result["status"] == "done"
    assert result["answer"] == "Booked a table for 4 at 7pm at Nopa. Parking is free (I couldn't confirm this)."
    assert [d["how"] for d in _settled(daemon)] == ["marked"]


# decision: ORCH-547, ORCH-309
def test_a_removal_never_leaves_the_answer_empty_and_a_kept_contradiction_is_never_verified(
        daemon: RunningDaemon) -> None:
    """The quoted sentence is the whole answer and the fix turn fails: removing it would leave "done" with nothing
    said, so code marks it instead; and since the evidence contradicts a sentence that still stands, the task is not
    shown as verified (review 2, 8c): done but not confirmed, with code's standard line, never the checker's words."""
    said = "The meeting is on Friday at 3pm."
    install(daemon, [sup("task", "On it.", task("Find when the meeting is"))], {
        "worker": done(said),
        "worker.verify": verdict(True, findings=((said, "contradicted", "The invite says Thursday."),)),
        "worker.fix": "not a report"})
    result = outcome(chat(daemon, "when is the meeting"))
    assert result["status"] == "done_unverified" and result["verified"] is False
    assert result["answer"].startswith("The meeting is on Friday at 3pm (I couldn't confirm this).")
    assert "Couldn't confirm: every detail of this answer against what I saw." in result["answer"]
    assert "Thursday" not in result["answer"]
    assert [d.get("kept_contradicted") for d in _settled(daemon)] == [[said]]


# decision: ORCH-547, ORCH-319
def test_a_plan_steps_kept_contradiction_is_never_verified(daemon: RunningDaemon) -> None:
    """The same in a plan: the answering step's whole answer is contradicted and its fix turn fails; the step and the
    task end done but not confirmed, never with a check mark."""
    from ferb.orchestrator.plans import Plan, Step
    said = "The meeting is on Friday at 3pm."
    spec = task("Find when the meeting is", conds=(cond("The meeting's time is found"),))
    spec["plan"] = Plan(steps=[Step(id="find", kind="do", goal="Find when the meeting is")]).model_dump()
    install(daemon, [sup("task", "On it.", spec)], {
        "step.find": done(said),
        "step.find.verify": verdict(True, findings=((said, "contradicted", "The invite says Thursday."),)),
        "step.find.fix": "not a report"})
    result = outcome(chat(daemon, "when is the meeting"))
    assert result["status"] == "done_unverified" and result["verified"] is False
    assert result["answer"].startswith("The meeting is on Friday at 3pm (I couldn't confirm this).")


# decision: ORCH-547
def test_a_fix_that_changes_other_sentences_is_discarded_for_the_fallback(daemon: RunningDaemon) -> None:
    """The fix turn may change only the quoted sentences: a revision that also rewrites another one (never re-checked)
    is discarded, no re-check runs, and code settles the original."""
    said = "The store opens at 9. It has a café. Parking is free."
    script = install(daemon, [sup("task", "On it.", task("Find the store's hours"))], {
        "worker": done(said),
        "worker.verify": verdict(True, findings=(("It has a café.", "unsupported", "No page mentioned a café."),)),
        "worker.fix": _fix("The store opens at 8. It may have a café. Parking is free.")})
    result = outcome(chat(daemon, "when does the store open"))
    assert script.calls("rechecker") == []
    assert result["answer"].startswith("The store opens at 9. It has a café (I couldn't confirm this). Parking is free.")


# decision: ORCH-548, ORCH-333
def test_what_the_fix_turn_and_the_recheck_open_is_released_and_their_rows_end_done(daemon: RunningDaemon) -> None:
    """The fix turn may look again (observe only): a tab it opens in FERB's browser closes at the task's end like the
    worker's and the checker's, and the fix and re-check rows end done, not "cancelled"."""
    tabs: dict[str, str] = {}
    released: list[str] = []
    router = daemon.daemon.router
    assert router is not None

    async def release(owner: str) -> list[dict[str, Any]]:
        released.append(owner)
        tab = tabs.pop(owner, None)
        return [{"resource": "tab", "tab": tab, "closed": True}] if tab else []
    router.release = release                                 # type: ignore[method-assign]

    def fix(token: str) -> str:
        tabs[daemon.daemon.supervisor.tokens.validate(token).task_id] = "t-fix"     # it opened a page to look again
        return _fix(COURSERA.replace("visible but locked until Monday", "open and due Friday"))
    install(daemon, [sup("task", "On it.", task("Check the Coursera assignments"))], {
        "worker": done(COURSERA),
        "worker.verify": verdict(True, findings=(("Quiz 3 is visible but locked until Monday.", "contradicted",
                                                  "The page shows it open."),)),
        "worker.fix": fix, "worker.recheck": json.dumps({"findings": []})})
    result = outcome(chat(daemon, "check my coursera assignments"))
    assert result["status"] == "done"
    tree = next(o for o in released if o.endswith(".worker")).rsplit(".", 1)[0]
    assert tree + ".worker.fix" in released and tree + ".worker.recheck" in released
    assert tabs == {}, "the fix turn's tab was left open"
    rows = dict(run(daemon, lambda: [tuple(r) for r in daemon.daemon.db.conn.execute(
        "SELECT id, status FROM tasks WHERE id LIKE ?", (tree + ".worker.%",)).fetchall()]))
    assert rows[tree + ".worker.fix"] == "done" and rows[tree + ".worker.recheck"] == "done"


# decision: ORCH-548, ORCH-309
def test_a_contradicted_sentence_never_stands_in_an_unconfirmed_answer(daemon: RunningDaemon) -> None:
    """accept_unconfirmed takes the checker's findings as accept does: the quoted sentence goes to the fix round."""
    said = "The library opens at 10am. It has a rooftop garden."
    install(daemon, [sup("task", "On it.", task("Find the library's hours"))], {
        "worker": done(said),
        "worker.verify": verdict(None, next="accept_unconfirmed", how="Check the library's site.", findings=(
            ("It has a rooftop garden.", "contradicted", "The page lists no garden."),)),
        "worker.fix": "not a report"})
    result = outcome(chat(daemon, "library hours?"))
    assert "rooftop garden" not in result["answer"] and result["answer"].startswith("The library opens at 10am.")


def test_only_the_quoted_sentences_may_change() -> None:
    from ferb.orchestrator.answer_marks import only_quoted_changed
    said = "A is 1. B is 2. C is 3."
    assert only_quoted_changed(said, "A is 1. B is two, I think. C is 3.", ["B is 2."])
    assert only_quoted_changed(said, "A is 1. C is 3.", ["B is 2."])
    assert only_quoted_changed(said, "A is 1. B is 4. C is 3.", ["B is 2.", "Z."])
    assert not only_quoted_changed(said, "A is 5. B is 4. C is 3.", ["B is 2."])
    assert not only_quoted_changed(said, "A is 1. B is 4. C is 3. D is 9.", ["B is 2."])
    assert not only_quoted_changed(said, "New. A is 1. B is 4. C is 3.", ["B is 2."])
    assert only_quoted_changed(said, "A is 1. B is 4. C is 5.", ["B is 2.", "C is 3."])
    assert not only_quoted_changed(said, "A is 1. B is 4.", ["Z."])
    # ORCH-547, review 2 (8b): no insertion anywhere, a new sentence beside a fixed one included
    assert not only_quoted_changed(said, "A is 1. B is 4. Also D is 9. C is 3.", ["B is 2."])
    assert not only_quoted_changed(said, "A is 1. B is 4.\nD is 9. C is 3.", ["B is 2."])
    assert not only_quoted_changed(said, "A is 1. B is 4. C is 5. D is 9.", ["B is 2.", "C is 3."])
    assert not only_quoted_changed(said, "A is one. B is 4. C is 3.", ["B is 2."])
    assert only_quoted_changed(said, "A is 1. B is 4. C is 5.", ["C is 3.", "B is 2."])   # adjacent: one place, two
    assert only_quoted_changed(said, "A is 1.\nB is 4.\nC is 3.", ["B is 2."])           # only the joins changed


# decision: ORCH-547
def test_a_removal_that_would_leave_only_a_marker_or_a_label_marks_instead() -> None:
    """Review 2 (8a): "never empty" means a letter or a digit is left, not just a list marker or emphasis."""
    from ferb.orchestrator.answer_marks import settle
    for answer, quote in (("- Friday.", "Friday."), ("* **Friday.**", "Friday.")):
        done = settle(answer, [(quote, "contradicted")])
        assert done.removed == [] and done.kept_contradicted == [quote] and "(I couldn't confirm this)" in done.answer
    kept = settle("- Friday.\n- Saturday.", [("Friday.", "contradicted")])
    assert kept.removed == ["Friday."] and kept.kept_contradicted == []


# decision: ORCH-545
def test_a_sentence_the_recheck_still_finds_is_marked_in_the_revised_answer(daemon: RunningDaemon) -> None:
    said = "The store opens at 9. It has a café."
    revised = "The store opens at 9. It has a café on the second floor."
    install(daemon, [sup("task", "On it.", task("Find the store's hours"))], {
        "worker": done(said),
        "worker.verify": verdict(True, findings=(("It has a café.", "unsupported", "No page mentioned a café."),)),
        "worker.fix": _fix(revised),
        "worker.recheck": json.dumps({"findings": [{"quote": "It has a café on the second floor.",
                                                    "kind": "unsupported", "evidence": "Still nothing about a café."}]})})
    result = outcome(chat(daemon, "when does the store open"))
    assert result["answer"] == "The store opens at 9. It has a café on the second floor (I couldn't confirm this)."


# decision: ORCH-545
def test_wording_alone_is_no_finding_and_costs_no_turn(daemon: RunningDaemon) -> None:
    """An unnamed source or passive voice is not the checker's to judge: with no finding there is no fix turn, no
    re-check and no mark; the answer stands as the worker wrote it."""
    said = "The invoices were collected and saved to Desktop/invoices."
    script = install(daemon, [sup("task", "On it.", task("Collect the invoices"))], {
        "worker": done(said), "worker.verify": verdict(True)})
    result = outcome(chat(daemon, "collect the invoices"))
    assert script.calls("worker") == ["worker"] and script.calls("rechecker") == []
    assert result["status"] == "done" and result["answer"] == said


# decision: ORCH-309, ORCH-545
def test_a_retry_with_every_outcome_verified_is_an_accept_with_its_findings_fixed(daemon: RunningDaemon) -> None:
    """ORCH-309: when every outcome is verified only the answer can be wrong; a retry the checker chose anyway never
    redoes the work: its findings go to the fix turn."""
    script = install(daemon, [sup("task", "On it.", task("Book a table for 4 at 7pm"))], {
        "worker": [done(BOOKED), done("Booked again.")],
        "worker.verify": verdict(True, next="retry", instructions="Say it differently.", findings=(
            (" The restaurant confirmed by email.", "contradicted", "No email was read."),)),
        "worker.fix": _fix("Booked a table for 4 at 7pm at Nopa."), "worker.recheck": json.dumps({"findings": []})})
    result = outcome(chat(daemon, "book a table for 4 at 7pm"))
    assert script.calls("worker") == ["worker", "worker.fix"]
    assert result["status"] == "done" and result["answer"] == "Booked a table for 4 at 7pm at Nopa."


# decision: ORCH-545, ORCH-307
def test_an_unattended_run_gets_the_same_single_round(daemon: RunningDaemon, unattended: None) -> None:
    script = install(daemon, [sup("task", "On it.", task("Find the library's hours"))], {
        "worker": done("The library opens at 10am. It closes at 6pm."),
        "worker.verify": verdict(True, findings=(("It closes at 6pm.", "unsupported", "Closing time wasn't shown."),)),
        "worker.fix": [_fix("The library opens at 10am. It closes at 6pm, I think.")],
        "worker.recheck": [json.dumps({"findings": [{"quote": "It closes at 6pm, I think.", "kind": "unsupported",
                                                     "evidence": "Still stated."}]})]})
    result = outcome(chat(daemon, "library hours?"))
    assert script.calls("worker") == ["worker", "worker.fix"] and script.calls("rechecker") == ["worker.recheck"]
    assert result["answer"] == "The library opens at 10am. It closes at 6pm, I think (I couldn't confirm this)."


# decision: ORCH-545, STORE-403
def test_an_older_checker_reply_reads_as_findings_and_never_marks_a_rewrite() -> None:
    """A checker reply in the shape before ORCH-545 (saved or in flight across the upgrade): fix_answer is an accept;
    an edit that removed or marked its text is that text as a finding; a rewrite or an addition is dropped, unmarked
    (the recorded voice rewrites of task 7d870ef5 and the inserted source of task 4be19c0f)."""
    old = CheckerOutcome.model_validate({"results": [{"index": 0, "met": True, "evidence": "seen"}],
                                         "next": "fix_answer", "answer_edits": [
        {"find": "Checked Coursera", "replace": "I checked Coursera"},
        {"find": "Saved the summary to `a.txt`.", "replace": "Saved the summary of `b.txt` to `a.txt`."},
        {"find": " It rained.", "replace": ""},
        {"find": "It opens at 10am.", "replace": "It opens at 10am (I couldn't confirm this)."}],
        "missing": "", "instructions": "", "worker_type": None, "question": "", "reason": ""})
    assert old.next == "accept"
    assert [(f.quote, f.kind) for f in old.findings] == [(" It rained.", "contradicted"),
                                                         ("It opens at 10am.", "unsupported")]
    assert old.dropped_edits == ["Checked Coursera", "Saved the summary to `a.txt`."]
    with pytest.raises(ValueError):           # the schema the checker answers in has no replacement text
        check_final(json.dumps({"results": [], "next": "accept", "findings": [], "answer_edits": [], "missing": "",
                                "instructions": "", "worker_type": None, "question": "", "reason": ""}),
                    CHECKER_SCHEMA)


# decision: ORCH-309
def test_an_unverifiable_outcome_is_done_unconfirmed_with_the_answer_first(daemon: RunningDaemon) -> None:
    """Task 810e9973: the answer was buried under "Why work stopped". Now the answer comes first, then one line."""
    spec = task("Send the invite to Sam", conds=(cond("Sam received the invite"),))
    spec["success_conditions"][0]["owner_text"] = "Sam received the invite"
    install(daemon, [sup("task", "On it.", spec)], {
        "worker": done("I sent the invite to Sam from your Gmail."),
        "worker.verify": verdict(None, True, next="accept_unconfirmed", reason="Delivery can't be seen from here",
                                 how="Check your Gmail Sent folder")})
    result = outcome(chat(daemon, "send the invite to sam"))
    assert result["status"] == "done_unverified" and result["verified"] is False
    assert result["answer"] == ("I sent the invite to Sam from your Gmail.\n\n"
                                "Couldn't confirm: Sam received the invite (Check your Gmail Sent folder).")
    assert "Why work stopped" not in result["answer"] and "Send the invite to Sam" not in result["answer"]



SIGNED_OUT = "The session had expired and the saved password could not be filled during this unattended run."
FOUND = "The visa site's sign-in page loaded; no appointment was booked."


# decision: ORCH-568, ORCH-585
def test_a_failure_the_worker_explained_is_told_once(daemon: RunningDaemon) -> None:
    """Owner 2026-11-09, the 7:10 visa run: "why is this giving two replies????". The worker's reason, its answer and
    the checker's reason all said the same failure. The owner reads it once: the worker's own reason (what stopped
    it), then its answer as what it found (PRM-344: a not-done answer holds only that); the checker's stays the
    record's."""
    not_done = json.dumps({"status": "not_done", "question": None, "artifacts": [], "how": "",
                           "reason": SIGNED_OUT, "answer": FOUND})
    install(daemon, [sup("task", "On it.", task("Check visa slots"))], {
        "worker": not_done,
        "worker.verify": verdict(False, True, next="fail",
                                 reason="The 7:10 AM check could not reach availability: the session had expired.")})
    result = outcome(chat(daemon, "check visa slots"))
    assert result["status"] == "failed"
    assert result["answer"].split("\n\n")[:2] == [f"I couldn't finish this task. {SIGNED_OUT}",
                                                   f"{FOUND_UNCHECKED} {FOUND}"]
    assert result["answer"].count(SIGNED_OUT) == 1 and "could not reach availability" not in result["answer"]
    assert "(not checked)" not in result["answer"]                    # ORCH-585: no bracket on the findings line
    assert "could not reach availability" in result["summary"]          # the checker's reason stays in the record


# decision: ORCH-562
def test_a_failures_own_explanation_gets_the_same_findings_round(daemon: RunningDaemon) -> None:
    """Task 7b86c4ad (claims audit A1): a failure said "FERB cannot operate its own app", a capability claim drawn from
    one task's limits that nothing checked. Now the checker's findings on a not-done report's words get the worker's
    one fix turn, and the failed answer says the fixed words; the checker is given what FERB can use now."""
    claim = "FERB cannot operate its own app."
    said = f"I couldn't change the schedule. {claim}"
    fixed = "I couldn't change the schedule. The edit didn't save."
    script = install(daemon, [sup("task", "On it.", task("Change the visa schedule"))], {
        "worker": json.dumps(report("not_done", said, reason=f"The edit didn't save. {claim}")),
        "worker.verify": verdict(False, next="fail", findings=((claim, "contradicted",
                                                                "FERB's facts list Mac control as available."),)),
        "worker.fix": _fix(fixed), "worker.recheck": json.dumps({"findings": []})})
    result = outcome(chat(daemon, "change my visa schedule"))
    assert result["status"] == "failed" and script.calls("worker") == ["worker", "worker.fix"]
    assert result["answer"].split("\n\n")[:2] == ["I couldn't finish this task. The edit didn't save.",
                                                   f"{FOUND_UNCHECKED} {fixed}"]
    assert claim not in result["answer"] and claim not in result["summary"]      # the reason's quote, removed by code
    assert "What FERB can use now" in _context_of(script, "worker.verify")

# decision: ORCH-309
def test_a_failure_says_plainly_what_happened(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "On it.", task("Book a table"))], {
        "worker": done(BOOKED),
        "worker.verify": verdict(False, True, next="fail", reason="Nopa's page shows no booking for tonight.")})
    result = outcome(chat(daemon, "book a table"))
    assert result["status"] == "failed"
    assert result["answer"] == "I couldn't finish this task. Nopa's page shows no booking for tonight."
    assert BOOKED not in result["answer"]            # a claim the check refuted is never stated


# decision: ORCH-302
def test_conditions_confirmed_in_an_earlier_attempt_carry_over(daemon: RunningDaemon) -> None:
    spec = task("Rename and move the file", conds=(cond("The file is renamed"), cond("The file is in Archive")))
    script = install(daemon, [sup("task", "On it.", spec)], {
        "worker": [done("Renamed it."), done("Renamed it and moved it to Archive.")],
        "worker.verify": [verdict(True, None, True, next="retry", missing="It isn't in Archive yet.",
                                  instructions="Move it to Archive."),
                          verdict(None, True, True, next="accept")]})
    result = outcome(chat(daemon, "rename and archive the file"))
    assert result["status"] == "done" and result["verified"] is True
    assert result["verification"][0]["evidence"].startswith("Confirmed in an earlier attempt")
    second = [c for _, k, c, _ in script.contexts if k == "worker.verify"][1]
    assert "Confirmed in an earlier attempt of this task" in second
    first_worker, second_worker = [c for _, k, c, _ in script.contexts if k == "worker"]
    assert "Instructions for this attempt: Move it to Archive." in second_worker
    assert "It isn't in Archive yet." in second_worker


# decision: ORCH-509
def test_a_report_that_fails_validation_keeps_nothing_of_an_earlier_attempt(daemon: RunningDaemon) -> None:
    """Fix list (stale fields): an attempt whose report is incomplete reports nothing, not the previous answer."""
    install(daemon, [sup("task", "On it.", task("Check the price"))], {
        "worker": [done("It's $40."), json.dumps({"status": "done", "answer": ["not text"]})],
        "worker.verify": [verdict(False, True, next="retry", instructions="Read the price again."),
                          verdict(None, None, next="fail", reason="")]})
    result = outcome(chat(daemon, "check the price"))
    assert result["status"] == "failed" and "$40" not in result["answer"]
    assert "The report from this attempt was incomplete." in result["answer"]


# decision: ORCH-509
def test_a_correction_during_the_check_keeps_the_done_report(daemon: RunningDaemon) -> None:
    """Fix list: a steer delivered while the check ran started a fresh attempt and lost the finished report. Now the
    report stays in the attempts the next worker reads, and the next attempt starts from the correction."""
    def steer_then_judge(token: str) -> str:
        tree = next(t for t in daemon.daemon.supervisor._tree_of.values() if t.worker.id.endswith(".worker"))
        tree.steering.messages.append("Make it for 5 people instead.")
        return verdict(True, True)
    script = install(daemon, [sup("task", "On it.", task("Book a table for 4"))], {
        "worker": [done("Booked a table for 4."), done("Changed the booking to 5.")],
        "worker.verify": [steer_then_judge, verdict(True, True)]})
    result = outcome(chat(daemon, "book a table for 4"))
    assert result["status"] == "done" and result["answer"] == "Changed the booking to 5."
    second = [c for _, k, c, _ in script.contexts if k == "worker"][1]
    assert "Booked a table for 4." in second and "Finished before the latest owner correction" in second


# decision: ORCH-309
def test_the_checker_gets_code_facts_about_how_the_work_was_done(daemon: RunningDaemon) -> None:
    script = install(daemon, [sup("task", "On it.", task("Say hi"))], {"worker": done("Hi.")})
    outcome(chat(daemon, "say hi"))
    ctx = [c for _, k, c, _ in script.contexts if k == "worker.verify"][0]
    assert "FERB's code facts about how this work was done" in ctx
    assert "screen_changes_during_ferb_calls" in ctx and "foreground_calls" in ctx
    assert "Worker types available for a retry" in ctx


# decision: ORCH-302, ORCH-333
def test_no_separate_next_attempt_or_resource_review_runs(daemon: RunningDaemon) -> None:
    """ORCH-302 and ORCH-333: one role judges and chooses; the dispatcher and the resource reviewer
    are gone."""
    script = install(daemon, [sup("task", "On it.", task("Say hi"))], {"worker": done("Hi.")})
    outcome(chat(daemon, "say hi"))
    assert {k for _, k, _, _ in script.contexts} == {"supervisor", "worker", "worker.verify"}
    kinds = run(daemon, lambda: {r[0] for r in daemon.daemon.db.conn.execute("SELECT kind FROM spans")})
    assert not kinds & {"model.dispatch", "model.resources", "resources.skipped"}


def test_the_how_note_is_kept_on_the_episode(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "On it.", task("Turn on dark mode"))], {
        "worker": json.dumps(report("done", "Dark mode is on.", how="System Settings > Appearance > Dark."))})
    result = outcome(chat(daemon, "turn on dark mode"))
    from ferb.orchestrator import episodes
    ep = run(daemon, lambda: episodes.get(daemon.daemon.db, result["task_id"]))
    assert ep["how"] == "System Settings > Appearance > Dark."


# ---------- ORCH-315: usage limit or sign-in failure ----------

LIMIT = RuntimeEvent("error", {"message": "You've hit your usage limit.", "account": "usage_limit"})


def _signed_in(daemon: RunningDaemon, *others: str) -> None:
    daemon.daemon.supervisor.worker_types = lambda: ["default", *others]


# decision: ORCH-315
def test_a_usage_limit_is_written_by_code_and_offers_another_runtime(daemon: RunningDaemon) -> None:
    _signed_in(daemon, "claude")
    roles: list[str] = []
    script = install(daemon, [sup("task", "On it.", task("Tidy the desktop"))],
                     {"worker": [LIMIT, done("Tidied the desktop.")]})
    make = daemon.daemon.supervisor.make_runtime

    def recording(role: str, **kw: Any) -> Any:
        roles.append(role)
        return make(role, **kw)
    daemon.daemon.supervisor.make_runtime = recording
    turn = Turn(daemon, "tidy my desktop")
    asked = answer_question(daemon, RUNTIME_ASK, choice="Use Claude")
    assert asked.reason.startswith("fake has reached its usage limit")
    assert asked.options == ("Use Claude", "Stop")
    result = turn.result()
    assert result["status"] == "done" and result["answer"] == "Tidied the desktop."
    assert len(script.calls("checker")) == 1        # no model judged the usage limit
    assert "worker:claude" in roles and "checker:claude" in roles
    switched = run(daemon, lambda: [e.data for e in daemon.daemon.journal.entries_of("runtime.switched")])
    assert switched == [{"task_id": result["task_id"], "runtime": "claude", "role": "worker"}]


# decision: ORCH-315
def test_the_app_switches_a_task_with_one_rpc(daemon: RunningDaemon) -> None:
    from ferb.cli.client import Client
    _signed_in(daemon, "claude", "openai")
    install(daemon, [sup("task", "On it.", task("Tidy the desktop"))], {"worker": [LIMIT, done("Tidied.")]})
    turn = Turn(daemon, "tidy my desktop")

    def waiting() -> Any:
        return next((a for a in daemon.daemon.state.approvals.pending() if a.capability == RUNTIME_ASK), None)
    import time
    for _ in range(500):
        if run(daemon, waiting) is not None:
            break
        time.sleep(0.02)
    ap = run(daemon, waiting)
    assert ap is not None
    c = Client(daemon.sock)
    try:
        got = c.call("runtimes.switch_task", {"task_id": ap.task_id, "runtime": "openai"})
    finally:
        c.close()
    assert got["runtime"] == "openai" and got["scope"] == "task" and got["role"] == "worker"
    assert turn.result()["status"] == "done"


# decision: ORCH-315
def test_with_no_other_runtime_signed_in_the_failure_says_so(daemon: RunningDaemon) -> None:
    _signed_in(daemon)
    install(daemon, [sup("task", "On it.", task("Tidy the desktop"))], {"worker": LIMIT})
    result = outcome(chat(daemon, "tidy my desktop"))
    assert result["status"] == "failed"
    assert "fake has reached its usage limit" in result["answer"]
    assert "No other AI service is signed in for FERB" in result["answer"]
    assert result["runtime_switch"] == {"runtime": "fake", "problem": "usage_limit", "role": "worker", "choices": []}
    assert not daemon.daemon.state.approvals.pending()


# decision: ORCH-315
def test_a_planner_turn_on_a_usage_limit_offers_the_switch_for_its_role(daemon: RunningDaemon) -> None:
    _signed_in(daemon, "claude")
    install(daemon, [LIMIT], {})
    reply = next(e for e in chat(daemon, "hello") if e["type"] == "reply")
    assert reply["status"] == "failed" and "fake has reached its usage limit" in reply["text"]
    assert reply["runtime_switch"] == {"runtime": "fake", "problem": "usage_limit", "role": "orchestrator",
                                       "choices": ["claude"]}


# ---------- per-role schemas (step 8b) fit every provider ----------

# decision: RUN-400, RUN-408
def test_every_per_role_report_schema_fits_the_providers_limits() -> None:
    """Each combination `worker_schema` can build, and the checker's, passes the Anthropic limits and Gemini's
    keywords (ferb/runtimes/schema_shape.py); the shared report holds only the role-free fields."""
    from itertools import product
    from ferb.orchestrator.schemas import CHECKER_SCHEMA, WORKER_SCHEMA, worker_schema
    from ferb.runtimes.schema_shape import (ANTHROPIC_MAX_OPTIONAL, ANTHROPIC_MAX_UNIONS, complexity, for_anthropic,
                                            for_gemini)
    flags = ("inputs", "proposals", "conversation", "watch", "loop", "parts")
    schemas = [worker_schema(**dict(zip(flags, on))) for on in product((False, True), repeat=len(flags))]
    for schema in [*schemas, CHECKER_SCHEMA]:
        unions, optional = complexity(for_anthropic(schema).wire)
        assert unions <= ANTHROPIC_MAX_UNIONS and optional <= ANTHROPIC_MAX_OPTIONAL
        for_gemini(schema)
    assert set(WORKER_SCHEMA["required"]) == {"status", "reason", "question", "answer", "artifacts", "how"}
