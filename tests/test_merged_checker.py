"""Step 8b end-to-end runs (TEST-500): the merged checker judges the conditions and chooses what's next (owner
ORCH-302); "done" is about the outcome asked for, and the answer comes first (ORCH-309); a usage limit or a
sign-in failure is written by code and offers another signed-in runtime with one tap (ORCH-315). The real
daemon, gateway and scripted model."""
import json
from typing import Any

from ferb.core.types import RuntimeEvent
from ferb.orchestrator.supervisor import RUNTIME_ASK
from tests.conftest import (RunningDaemon, answer_question, chat, done, install, outcome, report, sup, task, cond,
                            verdict)
from tests.support.schedules import run
from tests.test_execution_runs import Turn

BOOKED = "Booked a table for 4 at 7pm at Nopa. The restaurant confirmed by email."


def _checks(script: Any) -> list[str]:
    return script.calls("checker")


# decision: ORCH-302, ORCH-309
def test_a_report_only_problem_gets_its_answer_fixed_not_a_redo(daemon: RunningDaemon) -> None:
    """31% of rejections failed only the report condition and cost a full worker redo (kb area orchestration). The
    outcome is verified; the checker removes the unproven detail from the answer; the worker runs once."""
    script = install(daemon, [sup("task", "On it.", task("Book a table for 4 at 7pm"))], {
        "worker": done(BOOKED),
        "worker.verify": verdict(True, False, next="fix_answer",
                                 edits=((" The restaurant confirmed by email.", ""),))})
    result = outcome(chat(daemon, "book a table for 4 at 7pm"))
    assert script.calls("worker") == ["worker"] and len(_checks(script)) == 1
    assert result["status"] == "done" and result["verified"] is True
    assert result["answer"] == "Booked a table for 4 at 7pm at Nopa."


# decision: ORCH-309
def test_a_retry_with_every_outcome_verified_fixes_the_answer_instead(daemon: RunningDaemon) -> None:
    """ORCH-309: when every outcome condition is verified, only the answer can be wrong; a retry the checker chose
    anyway becomes fix-only-the-answer (its edits applied, the unproven detail marked), never a worker redo."""
    script = install(daemon, [sup("task", "On it.", task("Book a table for 4 at 7pm"))], {
        "worker": [done(BOOKED), done("Booked again.")],
        "worker.verify": verdict(True, False, next="retry", edits=((" The restaurant confirmed by email.", ""),))})
    result = outcome(chat(daemon, "book a table for 4 at 7pm"))
    assert script.calls("worker") == ["worker"] and len(_checks(script)) == 1
    assert result["status"] == "done" and result["answer"] == "Booked a table for 4 at 7pm at Nopa."


# decision: ORCH-309
def test_an_unverifiable_side_detail_is_marked_inline_and_the_task_is_done(daemon: RunningDaemon) -> None:
    script = install(daemon, [sup("task", "On it.", task("Find the library's address"))], {
        "worker": done("The library is at 100 Larkin St. It opens at 10am."),
        "worker.verify": verdict(True, None, next="fix_answer",
                                 edits=(("It opens at 10am.", "It opens at 10am (I couldn't confirm this)."),))})
    result = outcome(chat(daemon, "where's the library"))
    assert result["status"] == "done" and len(script.calls("worker")) == 1
    assert result["answer"] == "The library is at 100 Larkin St. It opens at 10am (I couldn't confirm this)."


# decision: ORCH-309, PRM-323, PRM-326
def test_a_checker_edit_that_rewrites_marks_its_text_and_one_that_adds_is_dropped(daemon: RunningDaemon) -> None:
    """Freelance task 0218bb01: the checker's edits ADDED sentences ("Preview and Finder came to the front…") to the
    answer. An edit may only remove its text or mark it inline; one that writes more is never applied and is journaled.
    A rewrite marks the text it would have changed (PRM-326: all 12 recorded checker edits were rewrites, so dropping
    them left unconfirmed claims bare); a pure addition leaves its text as it was (it wasn't disputed)."""
    install(daemon, [sup("task", "On it.", task("Find the library's address"))], {
        "worker": done("The library is at 100 Larkin St. It has 3 floors."),
        "worker.verify": verdict(True, None, next="fix_answer", evidence="The floors weren't on the page",
                                 edits=(("It has 3 floors.", "It appears to have several floors."),
                                        ("100 Larkin St.", "100 Larkin St. Preview came to the front once.")))})
    result = outcome(chat(daemon, "where's the library"))
    assert result["status"] == "done"
    assert result["answer"] == "The library is at 100 Larkin St. It has 3 floors (I couldn't confirm this)."
    async def dropped() -> list[Any]:
        return [(e.data["finds"], e.data["marked"]) for e in daemon.daemon.journal.of_kind("task.answer_edit_dropped")]
    assert daemon.run(dropped()) == [(["It has 3 floors.", "100 Larkin St."], ["It has 3 floors."])]


# decision: ORCH-309, PRM-326, ORCH-540
def test_a_report_condition_the_checker_found_unmet_never_adds_a_correction_line(daemon: RunningDaemon) -> None:
    """Live task 794e3bd8 (owner 2026-11-05: "what correction message is this?? who added it??"): the checker failed
    FERB's report condition because the answer didn't say FERB did the research, tried a pure addition ("I researched
    this. " + the sentence), which code dropped, and its evidence then showed as "Correction: …". The addition is
    dropped and the sentence left bare (it was confirmed); the evidence stays in the record only."""
    found = "Three of the five tools support offline mode."
    evidence = "The answer does not state that FERB performed the research"
    install(daemon, [sup("task", "On it.", task("Research offline support in the five tools"))], {
        "worker": done(found),
        "worker.verify": verdict(True, False, next="fix_answer", evidence=evidence,
                                 edits=((found, "I researched the five tools myself. " + found),))})
    result = outcome(chat(daemon, "which of the five tools work offline?"))
    assert result["answer"] == found and "correction" not in result
    assert "Correction" not in result["answer"] and "couldn't confirm" not in result["answer"]
    async def recorded() -> list[Any]:
        return [e.data["evidence"] for e in daemon.daemon.journal.of_kind("task.report_condition_unmet")]
    assert daemon.run(recorded()) == [evidence]


# decision: ORCH-309, PRM-323
def test_an_answer_that_leaves_out_a_result_goes_back_as_a_retry(daemon: RunningDaemon) -> None:
    """P4's trade-off (owner 2026-11-05): with every outcome verified, a report condition found false with no edit
    (the answer leaves out a result that matters, which an edit can't add) is a new attempt, not a checker rewrite."""
    script = install(daemon, [sup("task", "On it.", task("Book a table for 4 at 7pm"))], {
        "worker": [done("Booked."), done(BOOKED)],
        "worker.verify": [verdict(True, False, next="retry", instructions="Say where and when the table is."),
                          verdict(True, True)]})
    result = outcome(chat(daemon, "book a table for 4 at 7pm"))
    assert len(script.calls("worker")) == 2 and result["status"] == "done" and result["answer"] == BOOKED


# decision: ORCH-309
def test_an_unconfirmed_detail_the_checker_left_unfixed_gets_the_one_line(daemon: RunningDaemon) -> None:
    """The checker accepted while FERB's report condition stayed unknown: the outcome is verified (done), and the
    unproven detail is marked by the one "Couldn't confirm" line, never by a redo."""
    install(daemon, [sup("task", "On it.", task("Find the library's address"))], {
        "worker": done("The library is at 100 Larkin St."),
        "worker.verify": verdict(True, None, next="accept", evidence="The opening hours weren't on the page")})
    result = outcome(chat(daemon, "where's the library"))
    assert result["status"] == "done"
    assert result["answer"] == ("The library is at 100 Larkin St.\n\n"
                                "Couldn't confirm: The opening hours weren't on the page.")


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
    flags = ("inputs", "proposals", "conversation", "watch", "loop", "parts", "scheduled")
    schemas = [worker_schema(**dict(zip(flags, on))) for on in product((False, True), repeat=len(flags))]
    for schema in [*schemas, CHECKER_SCHEMA]:
        unions, optional = complexity(for_anthropic(schema).wire)
        assert unions <= ANTHROPIC_MAX_UNIONS and optional <= ANTHROPIC_MAX_OPTIONAL
        for_gemini(schema)
    assert set(WORKER_SCHEMA["required"]) == {"status", "reason", "question", "answer", "artifacts", "how"}
