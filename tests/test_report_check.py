"""Every check also judges the report the owner will see against the recorded evidence (F4 live fixes, task 79bc5bfa):
code adds the condition and hands the checker the report; the checker judges it (rule 6). A done answer is the report
the check judged, with only the checker's exact fixes (step 8b: no after-cleanup review; owner decision 9)."""
import json
from pathlib import Path
from typing import Any

from ferb.orchestrator.supervisor import REPORT_CONDITION
from ferb.trace import compose
from tests.conftest import RunningDaemon, chat, cond, done, install, outcome, sup, task, verdict
from tests.support.trace_builder import TraceBuilder

PROMPTS = Path(__file__).resolve().parents[1] / "ferb" / "orchestrator" / "prompts"


def test_every_check_judges_the_report_the_owner_will_see(daemon: RunningDaemon) -> None:
    report = "Codex Astra trained the model and I submitted its submission.csv."
    script = install(daemon, [sup("task", "On it.", task("run the assignment", conds=(cond("the submission was made"),)))],
                     {"worker": done(report)})
    out = outcome(chat(daemon, "run the assignment"))
    ctx = next(c for _, key, c, _ in script.contexts if key == "worker.verify")
    assert f"0. the submission was made\n1. {REPORT_CONDITION.text}\n" in ctx       # numbered last
    assert report in ctx                                                          # the report itself, as data
    assert '\nCondition layers: {"0": "outcome", "1": "outcome"}' in ctx           # not "planner judgments" now
    assert out["status"] == "done" and out["verified"] is True and out["answer"] == report
    assert [v["condition"] for v in out["verification"]] == ["the submission was made", REPORT_CONDITION.text]


def test_a_report_that_misstates_who_did_the_work_gets_its_answer_fixed(daemon: RunningDaemon) -> None:
    """Owner decision 9: the outcome is verified, so the task is done; the claim the records contradict is fixed in the
    answer by the checker's exact edit, with no redo."""
    claim = "Codex Astra produced and validated the submission; I uploaded it."
    v = verdict(True, False, next="fix_answer", edits=((claim, "I wrote and ran a ResNet18 baseline myself (Codex "
                                                              "Astra never finished) and uploaded it."),))
    script = install(daemon, [sup("task", "On it.", task("run the assignment", conds=(cond("the submission was made"),)))],
                     {"worker": done(claim), "worker.verify": v})
    out = outcome(chat(daemon, "run the assignment"))
    assert out["status"] == "done" and out["verified"] is True and len(script.calls("worker")) == 1
    assert claim not in out["answer"] and "Codex Astra never finished" in out["answer"]
    assert REPORT_CONDITION.text not in out["answer"]


def test_a_contradicted_detail_the_checker_left_unfixed_is_said_as_a_correction(daemon: RunningDaemon) -> None:
    claim = "Codex Astra produced and validated the submission; I uploaded it."
    v = verdict(True, False, next="accept", evidence="The records show FERB wrote the model; Codex never finished")
    install(daemon, [sup("task", "On it.", task("run the assignment", conds=(cond("the submission was made"),)))],
            {"worker": done(claim), "worker.verify": v})
    out = outcome(chat(daemon, "run the assignment"))
    assert out["status"] == "done"
    assert out["answer"] == claim + "\n\nCorrection: The records show FERB wrote the model; Codex never finished."


def test_an_unanswered_report_condition_is_not_confirmed(daemon: RunningDaemon) -> None:
    v = json.dumps({"results": [{"index": 0, "met": True, "evidence": "the file is there"}], "next": "accept"})
    script = install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the file exists"),)))],
                     {"worker": done("made it"), "worker.verify": v})
    script.fill_report_verdict = False                     # a checker that skipped FERB's report condition
    out = outcome(chat(daemon, "do it"))
    assert out["status"] == "done" and out["verified"] is True        # the outcome asked for is verified
    assert out["answer"] == "made it\n\nCouldn't confirm: The check gave no answer for this."
    assert REPORT_CONDITION.text not in out["answer"]


def test_a_planner_condition_in_the_same_words_is_its_own_condition(daemon: RunningDaemon) -> None:
    v = json.dumps({"results": [{"index": 0, "met": True, "evidence": "the planner's copy holds"},
                                {"index": 1, "met": None, "evidence": "the Sent folder did not load"},
                                {"index": 2, "met": False, "evidence": "FERB sent it, not Codex"}],
                    "next": "accept_unconfirmed"})
    script = install(daemon, [sup("task", "On it.", task("do it", conds=(cond(REPORT_CONDITION.text),
                                                                           cond("the email was sent"))))],
                     {"worker": done("Codex sent it"), "worker.verify": v})
    out = outcome(chat(daemon, "do it"))
    ctx = next(c for _, key, c, _ in script.contexts if key == "worker.verify")
    assert f"0. {REPORT_CONDITION.text}\n1. the email was sent\n2. {REPORT_CONDITION.text}\n" in ctx
    assert out["status"] == "done_unverified"
    assert out["answer"] == ("Codex sent it\n\nCouldn't confirm: the email was sent. "
                             "Correction: FERB sent it, not Codex.")                        # index 2 only


def test_a_check_that_crashes_leaves_finished_work_unconfirmed(daemon: RunningDaemon) -> None:
    async def broken(tree: Any, span: str) -> Any:
        raise RuntimeError("checker lost")
    daemon.daemon.supervisor.verify = broken          # type: ignore[method-assign]
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the file exists"),)))],
            {"worker": done("made it")})
    out = outcome(chat(daemon, "do it"))
    assert out["status"] == "done_unverified" and out["verified"] is False
    assert [(v["condition"], v["path"]) for v in out["verification"]] == [("the file exists", "unchecked"),
                                                                          (REPORT_CONDITION.text, "unchecked")]
    assert out["answer"] == ("made it\n\nCouldn't confirm: the check couldn't run: something broke inside FERB "
                             "(RuntimeError: checker lost). That's a bug in FERB, not something you asked wrong.")


def test_a_check_with_no_verdict_leaves_finished_work_unconfirmed(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the file exists"),)))],
            {"worker": done("made it"), "worker.verify": "not a verdict"})
    out = outcome(chat(daemon, "do it"))
    assert out["status"] == "done_unverified" and out["verified"] is False
    assert [(v["condition"], v["path"]) for v in out["verification"]] == [("the file exists", "unchecked"),
                                                                          (REPORT_CONDITION.text, "unchecked")]
    assert all(v["evidence"].startswith("The check couldn't finish:") for v in out["verification"])
    assert out["answer"].startswith("made it\n\nCouldn't confirm: the check couldn't run")


def test_a_confirmed_report_is_not_listed_among_the_outcomes(daemon: RunningDaemon) -> None:
    v = json.dumps({"results": [{"index": 0, "met": None, "evidence": "the Sent folder did not load"},
                                {"index": 1, "met": True, "evidence": "the report matches the records"}],
                    "next": "accept_unconfirmed"})
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"), "worker.verify": v})
    out = outcome(chat(daemon, "do it"))
    assert out["status"] == "done_unverified" and REPORT_CONDITION.text not in out["answer"]
    assert out["answer"] == "sent it\n\nCouldn't confirm: the email was sent."


def test_unfinished_work_never_shows_the_report_condition(daemon: RunningDaemon) -> None:
    failed = json.dumps({"status": "failed", "summary": "couldn't find it", "artifacts": [], "evidence": []})
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the file exists"),)))], {"worker": failed})
    out = outcome(chat(daemon, "do it"))
    assert out["status"] == "failed" and REPORT_CONDITION.text not in out["answer"]
    assert out["answer"] == "I couldn't finish this task. couldn't find it."


def test_the_checker_is_told_about_the_report() -> None:
    verifier = " ".join((PROMPTS / "verifier.md").read_text().split())
    assert ('FERB adds one condition to every check, numbered last, starting "My report to you says truthfully what '
            'was done".') in verifier
    assert ("Give `met=false` when the report states or implies something the records contradict, or leaves out "
            "something that changes the result") in verifier
    assert "Use this, not retry, when only FERB's report condition isn't met." in verifier


def test_the_trace_counts_the_report_condition_when_a_check_checked_nothing() -> None:
    b = TraceBuilder(conditions=["the file exists"])
    b.worker(5, "done", "made it")
    b.checker(8, [None, None], ["the folder did not load", "the folder did not load"])
    trace = b.finish(status="done_unverified", verified=False)
    trace["outcome"]["outcome_records"][1]["verification"] = [
        {"index": 0, "condition": "the file exists", "met": None, "path": "verifier", "evidence": "did not load"},
        {"index": 1, "condition": REPORT_CONDITION.text, "met": None, "path": "verifier", "evidence": "did not load"}]
    steps = compose.compose(trace)["steps"]
    assert "checked nothing — 2 conditions unknown" in steps[1]["text"]
    planner_only = TraceBuilder(conditions=["the file exists"])      # no verification rows: the planner's count
    planner_only.worker(5, "done", "made it")
    planner_only.checker(8, [None], ["did not load"])
    trace = planner_only.finish(status="done_unverified", verified=False)
    trace["outcome"]["outcome_records"][1]["verification"] = []
    assert "checked nothing — 1 condition unknown" in compose.compose(trace)["steps"][1]["text"]
