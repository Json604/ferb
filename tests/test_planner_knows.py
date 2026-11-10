"""ORCH-589 (owner 2026-11-10: "the perception that a single ferb agent does eveyrhign"): the planner knows what FERB
did. Through the real daemon, gateway and scripted model: the schedules' last runs in Layer 2 (ORCH-592)."""
import json
from datetime import timedelta

from ferb.policy.targets import Targets
from ferb.cli.client import Client
from tests.conftest import RunningDaemon, chat, cond, install, report, sup, task, tool, verdict
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


def _reply(text: str) -> str:
    return json.dumps({"kind": "reply", "answer_question": None, "task": None, "control": None, "remember": [],
                       "timers": [], "schedule": None, "schedule_changes": [], "reminder_cancels": [], "forget": [],
                       "conversation_changes": [], "text": text})


# decision: ORCH-593
def test_the_owners_lookups_reach_the_scheduled_runs_and_ferbs_own_posts(daemon: RunningDaemon) -> None:
    """The planner's chat.history lists the owner's scheduled runs and reads FERB's posted result by its entry id, and
    task.evidence reads the run's records; a chat task's own worker still can't read them (ORCH-517's fence)."""
    _visa_schedule(daemon)
    install(daemon, [sup("task", "Checking.", task("Check visa slots"))], {"worker": json.dumps(report("done", NO_SLOTS))})
    fire(daemon, 61)
    [posted] = run(daemon, lambda: daemon.daemon.journal.of_kind("schedule.posted"))
    ran = str(posted.data["task_id"])
    found: dict[str, object] = {}

    def look(token: str) -> str:
        found["tasks"] = tool(daemon.sock, token, "chat.history", include_tasks=True)
        found["post"] = tool(daemon.sock, token, "chat.history", entry_id=posted.id)
        found["records"] = tool(daemon.sock, token, "task.evidence", task_id=ran)
        return _reply("It found no slots.")
    install(daemon, [look])
    chat(daemon, "what did the visa check find?", session="owner")
    listed = found["tasks"]["content"]["records"]                       # type: ignore[index]
    assert any(r["kind"] == "task" and r["id"] == ran for r in listed)
    assert not found["post"]["is_error"] and NO_SLOTS in json.dumps(found["post"]["content"])   # type: ignore[index]
    assert not found["records"]["is_error"], found["records"]

    def worker(token: str) -> str:
        found["worker"] = tool(daemon.sock, token, "task.evidence", task_id=ran)
        return json.dumps(report("done", "Read nothing."))
    install(daemon, [sup("task", "Looking.", task("Look at the visa run"))], {"worker": worker})
    chat(daemon, "look at the visa run's records", session="owner")
    assert found["worker"]["is_error"]                                  # type: ignore[index]


def _tasks_block(context: str) -> dict[str, object]:
    raw = context.split("Layer 6 — tasks:", 1)[1].split("<untrusted-data", 1)[1].split(">", 1)[1]
    return json.loads(raw.split("</untrusted-data", 1)[0])             # type: ignore[no-any-return]


# decision: ORCH-595
def test_a_running_tasks_row_says_which_card_it_waits_on_and_its_last_step(daemon: RunningDaemon) -> None:
    def asks(token: str) -> str:
        tool(daemon.sock, token, "task.evidence")
        return json.dumps(report("not_done", reason="Two times are free.", question="Which time: 8 or 9:30?"))
    install(daemon, [sup("task", "Booking.", task("Book a table at Toit tonight"))],
            {"worker": [asks, json.dumps(report("done", "Booked 8:00 pm."))]})
    turn = Turn(daemon, "book a table at toit tonight")
    _until(lambda: any(a.capability == "plan.ask" for a in run(daemon, daemon.daemon.state.approvals.pending)))
    script = install(daemon, [sup("reply", "It waits on your pick of a time.")])
    chat(daemon, "what is it waiting on?", session="s2")
    [row] = _tasks_block(_planner(script))["running"]                  # type: ignore[call-overload]
    [card] = row["waiting_on_cards"]
    assert card["card"] == "plan.ask" and card["asks"] == "Which time: 8 or 9:30?" and card["asked_at"]
    assert card["answered_in"] == "chat or its card" and row["last_tool_step"]["tool"] == "task.evidence"
    [asked] = [a for a in run(daemon, daemon.daemon.state.approvals.pending) if a.capability == "plan.ask"]
    run(daemon, lambda: daemon.daemon.state.approvals.resolve(asked.id, True, None, "8 pm"))
    turn.done()


# decision: ORCH-595
def test_a_card_only_its_card_can_answer_is_listed_as_such(daemon: RunningDaemon) -> None:
    """A runtime switch (one tap, never chat words) is shown as open, marked as answered only on its card."""
    from ferb.core.types import RuntimeEvent
    from ferb.orchestrator.supervisor import RUNTIME_ASK
    daemon.daemon.supervisor.worker_types = lambda: ["default", "claude"]
    limit = RuntimeEvent("error", {"message": "You've hit your usage limit.", "account": "usage_limit"})
    install(daemon, [sup("task", "On it.", task("Tidy the desktop"))], {"worker": [limit, json.dumps(report())]})
    turn = Turn(daemon, "tidy my desktop")
    _until(lambda: any(a.capability == RUNTIME_ASK for a in run(daemon, daemon.daemon.state.approvals.pending)))
    script = install(daemon, [sup("reply", "It needs your pick of an AI service.")])
    chat(daemon, "why is it stuck?", session="s2")
    context = _planner(script)
    [row] = _tasks_block(context)["running"]                          # type: ignore[call-overload]
    assert row["waiting_on_cards"][0]["card"] == RUNTIME_ASK
    assert row["waiting_on_cards"][0]["answered_in"] == "only its card (the chat can't answer it)"
    questions = context.split("Layer 6 — open questions and cards", 1)[1]
    assert '"card": "runtime.switch"' in questions and '"chat_can_answer": false' in questions
    from tests.conftest import answer_question
    answer_question(daemon, RUNTIME_ASK, choice="Stop")
    turn.done()


# decision: ORCH-597
def test_a_scheduled_run_is_given_the_owners_own_messages_beside_the_planners_instructions(
        daemon: RunningDaemon) -> None:
    from ferb.orchestrator.supervisor import SCHEDULE_MESSAGES
    from tests.support.schedules import schedule_reply
    words = "every day check the visa site and if a slot is free book it, chennai first"
    install(daemon, [schedule_reply(daemon, "Check the visa site daily; book the earliest slot, Chennai first", words,
                                    daemon.daemon.clock.now() + timedelta(minutes=1), repeat="daily", paths=())])
    chat(daemon, words, session="owner")
    script = install(daemon, [sup("task", "Checking.", task("Check visa slots"))],
                     {"worker": json.dumps(report("done", NO_SLOTS))})
    fire(daemon, 61)
    planner = _planner(script)
    assert "Layer 5 — This scheduled run's instructions (FERB's planner wrote them" in planner
    assert "Layer 5 — owner's complete new message" not in planner and words in planner
    worker = next(c for _, k, c, _ in script.contexts if k == "worker")
    assert "This scheduled run's instructions" in worker and "Owner's complete original request" not in worker
    assert SCHEDULE_MESSAGES in worker and words in worker.split(SCHEDULE_MESSAGES, 1)[1]
