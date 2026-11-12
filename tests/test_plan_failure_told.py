"""ORCH-627: a one-ask plan that stops at a step is told as a single task's end is (ORCH-600, ORCH-605, ORCH-609).
Live 2026-11-12 16:30 (task f68317e9): "I couldn't finish this task. I stopped at “In Brave “Work,” fill every
required …”: Not done: <the condition's text>. FERB didn't submit it; …\\n\\nDone before that:\\n- <two step reports>".
Through the real daemon and the scripted model."""
import json

from ferb.orchestrator.plans import Plan, Step
from tests.conftest import RunningDaemon, chat, cond, done, install, outcome, report, sup, task, verdict

SITE = "hack2skill.example"
FILL_GOAL = "In Brave Work, fill every required field of the Buildathon form, verify the attachment and all fields"
CONDITION = "Your corrected deck is attached, every required field is filled again, and Hack2skill records the submission"


def _spec() -> dict:
    spec = task("Attach the corrected deck and submit the form", conds=(cond(CONDITION, "submitted", site=SITE),))
    spec["plan"] = Plan(steps=[Step(id="deck", kind="do", label="Correct the deck", goal="Correct the deck."),
                               Step(id="fill", kind="do", label="Fill and submit the form", goal=FILL_GOAL,
                                    depends_on=["deck"])]).model_dump()
    return spec


# decision: ORCH-627, ORCH-609, ORCH-605
def test_a_plan_whose_answer_step_claim_code_refuted_is_told_in_codes_one_sentence(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Here's the plan.", _spec())],
            {"step.deck": done("The deck now uses the official template and has 13 slides, all checked one by one."),
             "step.fill": done("Submitted the form."), "step.fill.verify": verdict(True)})
    final = outcome(chat(daemon, "attach the corrected deck and submit the form"))
    assert final["status"] == "failed"
    assert final["answer"].split("\n\n")[0] == f"I couldn't finish this task. No form on {SITE} was submitted."
    for said in ("I stopped at", "Done before that", FILL_GOAL[:30], CONDITION[:30], "13 slides", "Submitted the form"):
        assert said not in final["answer"], said


# decision: ORCH-627, ORCH-605
def test_a_plan_whose_step_reported_not_done_says_that_steps_one_text(daemon: RunningDaemon) -> None:
    answer = "The form page asks for a team name I don't have, so I left it unsubmitted."
    install(daemon, [sup("task", "Here's the plan.", _spec())],
            {"step.deck": done("Deck corrected."),
             "step.fill": json.dumps(report("not_done", answer, reason="No team name."))})
    final = outcome(chat(daemon, "attach the corrected deck and submit the form"))
    assert final["status"] == "failed"
    assert final["answer"].split("\n\n")[:2] == ["I couldn't finish this task.", answer]
    assert "Deck corrected." not in final["answer"] and "I stopped at" not in final["answer"]
