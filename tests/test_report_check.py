"""Every check also judges the report the owner will see against the recorded evidence (F4 live fixes, task 79bc5bfa):
code adds the condition and hands the checker the report; the checker judges it (rule 6). A done answer is the report
the check judged: the after-cleanup review can change the status, never reword it (P16 V4/V6, D34)."""
import json
from pathlib import Path
from typing import Any

from ferb.orchestrator.supervisor import REPORT_CONDITION
from tests.conftest import RunningDaemon, chat, cond, done, install, outcome, sup, task

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
    assert (f"- {REPORT_CONDITION.text} — failed: The records show FERB's worker wrote resnet18_baseline.py"
            in out["answer"])
    assert claim not in out["answer"]


def test_an_unanswered_report_condition_is_not_confirmed(daemon: RunningDaemon) -> None:
    verdict = json.dumps({"results": [{"index": 0, "met": True, "evidence": "the file is there"}]})
    script = install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the file exists"),)))],
                     {"worker": done("made it"), "worker.verify": verdict, "worker.dispatch": _finish("unconfirmed")})
    script.fill_report_verdict = False                     # a checker that skipped FERB's report condition
    out = outcome(chat(daemon, "do it"))
    assert out["status"] == "done_unverified" and out["verified"] is False
    assert f"- {REPORT_CONDITION.text} — unverified: The check gave no answer for this." in out["answer"]


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


def test_the_checker_and_the_reviewer_are_told_about_the_report() -> None:
    verifier = " ".join((PROMPTS / "verifier.md").read_text().split())
    resources = " ".join((PROMPTS / "resources.md").read_text().split())
    assert ('FERB adds one condition to every check, numbered last, starting "My report to you says truthfully what '
            'was done".') in verifier
    assert ("Give `met=false` when the report states or implies something the records contradict, or leaves out "
            "something that changes the result") in verifier
    assert ("When you return done, FERB keeps the report the check judged as the owner's answer and records your "
            "summary in the trace: write the lifecycle facts there and never change who did what.") in resources
