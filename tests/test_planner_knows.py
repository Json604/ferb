"""ORCH-589 (owner 2026-11-10: "the perception that a single ferb agent does eveyrhign"): the planner knows what FERB
did. Through the real daemon, gateway and scripted model: the schedules' last runs in Layer 2 (ORCH-592)."""
import json
from datetime import timedelta

from ferb.policy.targets import Targets
from ferb.cli.client import Client
from tests.conftest import RunningDaemon, chat, cond, install, report, sup, task, verdict
from tests.support.schedules import fire, make_schedule, run
from tests.test_answers_in_steps import Turn, _until

NO_SLOTS = "No slots at any of the five posts."
BLOCKED = "The visa site signed me out before any post was read."


def _planner(script: object, n: int = -1) -> str:
    return [c for _, k, c, _ in script.contexts if k == "supervisor"][n]   # type: ignore[attr-defined]


def _visa_schedule(d: RunningDaemon, minutes: int = 1) -> object:
    return make_schedule(d, "Check visa slots at the five posts", "check the visa slots every day", Targets(),
                         d.daemon.clock.now() + timedelta(minutes=minutes))


# decision: ORCH-592
def test_each_schedule_line_carries_its_last_run(daemon: RunningDaemon) -> None:
    _visa_schedule(daemon)
    install(daemon, [sup("task", "Checking.", task("Check visa slots"))],
            {"worker": json.dumps(report("not_done", reason=BLOCKED))})
    fire(daemon, 61)
    assert run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.posted"))
    script = install(daemon, [sup("reply", "It failed once.")])
    chat(daemon, "how is the visa check doing?", session="owner")
    layer2 = _planner(script).split("Layer 2 — right now", 1)[1]
    line = next(x for x in layer2.splitlines() if "Check visa slots at the five posts" in x)
    assert ", failed, 1 failed in a row, its answer's first line (model-written, data): \"I couldn't finish" in line
    assert BLOCKED in line


# decision: ORCH-591, DMN-583
def test_the_planner_is_told_what_ferb_did_since_the_owners_last_message(daemon: RunningDaemon) -> None:
    """A card answered in a task, a scheduled run that failed (its unmet condition with the checker's evidence) and a
    reminder that arrived, all after the owner's last message, reach the next turn as code's record, after the
    conversation."""
    install(daemon, [sup("task", "Booking.", task("Book a table at Toit tonight"))],
            {"worker": [json.dumps(report("not_done", reason="Two times are free.", question="Which time: 8 or 9:30?")),
                        json.dumps(report("done", "Booked 8:00 pm."))]})
    turn = Turn(daemon, "book a table at toit tonight")
    _until(lambda: any(a.capability == "plan.ask" for a in run(daemon, daemon.daemon.state.approvals.pending)))
    [asked] = [a for a in run(daemon, daemon.daemon.state.approvals.pending) if a.capability == "plan.ask"]
    client = Client(daemon.sock)
    try:                                        # the card's own box, as FERB.app answers it
        client.call("chat.answer", {"approval_id": asked.id, "text": "8 pm please"})
    finally:
        client.close()
    turn.done()
    _visa_schedule(daemon)
    install(daemon, [sup("task", "Checking.", task("Check visa slots", conds=(cond("A slot is booked"),)))],
            {"worker": json.dumps(report("not_done", reason=BLOCKED)),
             "worker.verify": verdict(False, next="fail", evidence="No Slots Available at Chennai.")})
    fire(daemon, 61)
    assert run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.posted"))
    run(daemon, lambda: daemon.daemon.journal.append("timer.delivered", {
        "timer_id": "tm-1", "kind": "reminder", "message": "Take the iron tablet.", "overdue_s": 0}))
    script = install(daemon, [sup("reply", "The visa site signed me out.")])
    chat(daemon, "why couldn tyou finisnuh??")
    context = _planner(script)
    block = context.split("Layer 2 — what happened since the owner's previous message", 1)[1]
    assert context.index("Layer 5 — conversation") < context.index("Layer 2 — what happened since")
    card = block.index('"what": "card answered", "card": "task", "question": "Which time: 8 or 9:30?"')
    assert '"answer": "8 pm please"' in block[card:]
    run_ended = block.index('"what": "scheduled run ended"')
    assert '"status": "failed"' in block[run_ended:] and '"result": "not met"' in block[run_ended:]
    assert '"check_found": "No Slots Available at Chennai."' in block[run_ended:]
    assert block.index('"what": "reminder arrived", "message": "Take the iron tablet."') > run_ended > card


# decision: ORCH-591
def test_a_secret_cards_answer_reaches_the_planner_only_as_given(daemon: RunningDaemon) -> None:
    """A code, secret or password card is told as given or not, never its words (DMN-583 never records them)."""
    run(daemon, lambda: daemon.daemon.journal.append("chat.message", {"session_id": "s1", "request_id": "r0",
                                                                      "text": "log in to the portal"}))
    run(daemon, lambda: daemon.daemon.journal.append("question.asked", {
        "approval_id": "ap-1", "kind": "secret", "capability": "browser.login_code", "task_id": "t1.worker",
        "request_id": "r0", "question": "Enter the login code the portal sent"}))
    run(daemon, lambda: daemon.daemon.journal.append("question.answered", {
        "approval_id": "ap-1", "kind": "secret", "state": "granted", "answer": "words"}))
    script = install(daemon, [sup("reply", "You gave the code.")])
    chat(daemon, "what did you need the code for?")
    block = _planner(script).split("Layer 2 — what happened since", 1)[1]
    assert '"card": "secret", "question": "Enter the login code the portal sent", "answer": "given"' in block
