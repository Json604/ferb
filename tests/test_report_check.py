"""Every check also judges the report the owner will see against the recorded evidence (F4 live fixes, task 79bc5bfa):
code adds the condition and hands the checker the report; the checker judges it (rule 6). A done answer is the report
the check judged: the after-cleanup review can change the status, never reword it (P16 V4/V6, D34)."""
import json
from pathlib import Path
from typing import Any

from ferb.orchestrator.supervisor import REPORT_CONDITION
from ferb.trace import compose
from tests.conftest import RunningDaemon, chat, cond, done, install, outcome, sup, task
from tests.support.trace_builder import TraceBuilder

PROMPTS = Path(__file__).resolve().parents[1] / "ferb" / "orchestrator" / "prompts"


def _finish(result: str | None) -> str:
    choice: dict[str, Any] = {"action": "finish", "worker_type": "default", "instructions": "Stop and report.",
                              "question": "", "stop_reason": ""}
    if result is not None:
        choice["result"] = result
    return json.dumps(choice)


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


def test_a_report_that_misstates_who_did_the_work_fails(daemon: RunningDaemon) -> None:
    claim = "Codex Astra produced and validated the submission; I uploaded it."
    verdict = json.dumps({"results": [
        {"index": 0, "met": True, "evidence": "Kaggle shows the submission, public score 0.48067."},
        {"index": 1, "met": False, "evidence": "The records show FERB's worker wrote resnet18_baseline.py with "
                                               "fs.write and ran it; Codex Astra never finished."}]})
    install(daemon, [sup("task", "On it.", task("run the assignment", conds=(cond("the submission was made"),)))],
            {"worker": done(claim), "worker.verify": verdict})
    out = outcome(chat(daemon, "run the assignment"))
    assert out["status"] == "failed" and out["verified"] is False
    # The owner never saw the worker's report: said in their terms, with the checker's evidence (P27)
    assert ("- What I reported didn't match the records: The records show FERB's worker wrote resnet18_baseline.py"
            in out["answer"])
    assert claim not in out["answer"] and REPORT_CONDITION.text not in out["answer"]


def test_an_unanswered_report_condition_is_not_confirmed(daemon: RunningDaemon) -> None:
    verdict = json.dumps({"results": [{"index": 0, "met": True, "evidence": "the file is there"}]})
    script = install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the file exists"),)))],
                     {"worker": done("made it"), "worker.verify": verdict, "worker.dispatch": _finish("unconfirmed")})
    script.fill_report_verdict = False                     # a checker that skipped FERB's report condition
    out = outcome(chat(daemon, "do it"))
    assert out["status"] == "done_unverified" and out["verified"] is False
    assert "- I couldn't confirm my report matches the records: The check gave no answer for this." in out["answer"]
    assert REPORT_CONDITION.text not in out["answer"]


def test_an_unknown_report_says_how_to_check_in_plain_words(daemon: RunningDaemon) -> None:
    verdict = json.dumps({"results": [
        {"index": 0, "met": True, "evidence": "Kaggle lists the submission"},
        {"index": 1, "met": None, "evidence": "the records stop before the model was chosen",
         "how": "Open the notebook's last cell to see which model ran."}]})
    install(daemon, [sup("task", "On it.", task("run the assignment", conds=(cond("the submission was made"),)))],
            {"worker": done("submitted"), "worker.verify": verdict, "worker.dispatch": _finish("unconfirmed")})
    out = outcome(chat(daemon, "run the assignment"))
    assert out["status"] == "done_unverified"
    assert ("- I couldn't confirm my report matches the records: the records stop before the model was chosen\n"
            "  To check: Open the notebook's last cell to see which model ran.") in out["answer"]
    assert REPORT_CONDITION.text not in out["answer"]


def test_a_planner_condition_in_the_same_words_is_its_own_condition(daemon: RunningDaemon) -> None:
    verdict = json.dumps({"results": [{"index": 0, "met": True, "evidence": "the planner's copy holds"},
                                       {"index": 1, "met": None, "evidence": "the Sent folder did not load"},
                                       {"index": 2, "met": False, "evidence": "FERB sent it, not Codex"}]})
    script = install(daemon, [sup("task", "On it.", task("do it", conds=(cond(REPORT_CONDITION.text),
                                                                           cond("the email was sent"))))],
                     {"worker": done("Codex sent it"), "worker.verify": verdict})
    out = outcome(chat(daemon, "do it"))
    ctx = next(c for _, key, c, _ in script.contexts if key == "worker.verify")
    assert f"0. {REPORT_CONDITION.text}\n1. the email was sent\n2. {REPORT_CONDITION.text}\n" in ctx
    assert out["status"] == "failed"
    assert "- What I reported didn't match the records: FERB sent it, not Codex" in out["answer"]   # index 2 only
    assert "Confirmed:\n- the planner's copy holds" in out["answer"]       # the planner's copy is shown, not hidden


def test_a_check_that_crashes_lists_the_report_still_to_check(daemon: RunningDaemon) -> None:
    async def broken(tree: Any, span: str) -> Any:
        raise RuntimeError("checker lost")
    daemon.daemon.supervisor.verify = broken          # type: ignore[method-assign]
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the file exists"),)))],
            {"worker": done("made it")})
    out = outcome(chat(daemon, "do it"))
    assert out["status"] == "failed" and out["verified"] is False
    assert [(v["condition"], v["path"]) for v in out["verification"]] == [("the file exists", "unchecked"),
                                                                          (REPORT_CONDITION.text, "unchecked")]
    assert ("Still to check (the check couldn't run):\n- the file exists\n"
            "- Whether what I reported matches the records\n"
            "Why the check couldn't run: something broke inside FERB (RuntimeError: checker lost). That's a bug in "
            "FERB, not something you asked wrong.") in out["answer"]
    assert REPORT_CONDITION.text not in out["answer"]


def test_a_check_with_no_verdict_lists_the_report_still_to_check(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the file exists"),)))],
            {"worker": done("made it"), "worker.verify": "not a verdict"})
    out = outcome(chat(daemon, "do it"))
    assert out["status"] == "failed" and out["verified"] is False
    assert [(v["condition"], v["path"]) for v in out["verification"]] == [("the file exists", "unchecked"),
                                                                          (REPORT_CONDITION.text, "unchecked")]
    assert all(v["evidence"].startswith("The check couldn't finish:") for v in out["verification"])
    assert "\n- the file exists\n- Whether what I reported matches the records" in out["answer"]


def test_a_confirmed_report_is_not_listed_among_the_outcomes(daemon: RunningDaemon) -> None:
    verdict = json.dumps({"results": [{"index": 0, "met": None, "evidence": "the Sent folder did not load"},
                                       {"index": 1, "met": True, "evidence": "the report matches the records"}]})
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"), "worker.verify": verdict, "worker.dispatch": _finish("unconfirmed")})
    out = outcome(chat(daemon, "do it"))
    assert out["status"] == "done_unverified" and REPORT_CONDITION.text not in out["answer"]


def test_work_that_stops_before_a_check_does_not_list_the_report_condition(daemon: RunningDaemon) -> None:
    failed = json.dumps({"status": "failed", "summary": "couldn't find it", "artifacts": [], "evidence": []})
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the file exists"),)))], {"worker": failed})
    out = outcome(chat(daemon, "do it"))
    assert out["status"] == "failed" and REPORT_CONDITION.text not in out["answer"]
    assert [v["condition"] for v in out["verification"]] == ["the file exists"]


def test_the_after_cleanup_review_cannot_reword_a_checked_report(daemon: RunningDaemon) -> None:
    released = False

    async def cleanup(task_id: str) -> list[dict[str, Any]]:
        nonlocal released
        if task_id.endswith(".worker") and not released:
            released = True
            return [{"tab": "t1", "closed": True, "released": True}]
        return []
    daemon.daemon.supervisor.on_task_end = cleanup
    checked = ("I started Codex Astra; it stalled loading its model, so I wrote and ran a ResNet18 fallback myself and "
               "submitted that. Public score 0.48067.")
    reworded = "I produced and validated the submission artifact and read back the score, 0.48067."
    install(daemon, [sup("task", "On it.", task("run the assignment", conds=(cond("the score was read"),)))],
            {"worker": done(checked), "worker.resources": done(reworded)})
    out = outcome(chat(daemon, "run the assignment"))
    assert out["status"] == "done" and out["verified"] is True
    assert out["answer"] == checked and reworded not in out["answer"]

    async def reviews() -> list[str]:
        return [r[0] for r in daemon.daemon.db.conn.execute("SELECT output_json FROM spans WHERE kind = 'model.resources'")]
    assert any(reworded in r for r in daemon.run(reviews()))          # the review's words stay in the trace


def test_a_second_after_cleanup_review_cannot_write_new_done_wording(daemon: RunningDaemon) -> None:
    """Review of a6494d6: the in-loop review lowers the checked report to failed, the planner finishes, then the
    end-of-task release review (run_tree) returns done in its own words. The answer is still the checked report."""
    releases = 0

    async def cleanup(task_id: str) -> list[dict[str, Any]]:
        nonlocal releases
        if task_id.endswith(".worker") and releases < 2:      # a driver release that keeps reporting an error
            releases += 1
            return [{"tab": "t1", "released": False, "error": "release acknowledgement lost"}]
        return []
    daemon.daemon.supervisor.on_task_end = cleanup
    checked = "I saved the notes to notes.md."
    failed = json.dumps({"status": "failed", "summary": "The tab may still be open after cleanup.",
                         "artifacts": [], "evidence": []})
    install(daemon, [sup("task", "On it.", task("save the notes", conds=(cond("the notes were saved"),)))],
            {"worker": done(checked), "worker.resources": [failed, done("REWORDED BY REVIEW")]})
    out = outcome(chat(daemon, "save the notes"))
    assert releases == 2
    assert out["status"] == "done" and out["verified"] is True
    assert out["answer"] == checked and out["summary"] == checked

    async def reviews() -> list[str]:
        return [r[0] for r in daemon.daemon.db.conn.execute("SELECT output_json FROM spans WHERE kind = 'model.resources'")]
    assert any("REWORDED BY REVIEW" in r for r in daemon.run(reviews()))


def test_the_checker_and_the_reviewer_are_told_about_the_report() -> None:
    verifier = " ".join((PROMPTS / "verifier.md").read_text().split())
    resources = " ".join((PROMPTS / "resources.md").read_text().split())
    assert ('FERB adds one condition to every check, numbered last, starting "My report to you says truthfully what '
            'was done".') in verifier
    assert ("Give `met=false` when the report states or implies something the records contradict, or leaves out "
            "something that changes the result") in verifier
    assert ("When you return done, FERB keeps the report the check judged as the owner's answer and records your "
            "summary in the trace: write the lifecycle facts there and never change who did what.") in resources


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
