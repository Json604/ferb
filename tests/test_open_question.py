"""ORCH-586: while FERB's clarifying question is open, the owner's next message reaches the planner apart from the
question and the request it asked about, and the planner's answer says whether the message answers it. Only then does
code join that request to it (the owner's words, the task's payload, the chat's pending state, question.answered);
otherwise the message is handled alone and the question stays open. Live 2026-11-10 (Codex): a hotel and a table
request sent while the Amazon question was open were read as its answer, and neither ran."""
import json
from typing import Any

from tests.conftest import RunningDaemon, chat, done, install, task
from tests.support.schedules import run

CABLES = "order me a 2 pack of usb c cables from amazon"
QUESTION = "Which cable length do you want?"
HOTEL = "book me a hotel in north goa for dec 18-20"
DECLINE = "No answer in this test run; decide yourself or report what you need"


def _answer(kind: str, text: str, t: dict[str, Any] | None = None, answers: bool | None = None) -> str:
    out: dict[str, Any] = {"kind": kind, "text": text, "task": t}
    if answers is not None:
        out["answers_question"] = answers
    return json.dumps(out)


def _payloads(daemon: RunningDaemon, seen: list[str]) -> Any:
    def worker(token: str) -> str:
        grant = daemon.daemon.tokens.validate(token)
        record = daemon.daemon.tasks.get(grant.task_id)
        seen.append(daemon.daemon.state.requests.get(record.task.request_id).payload)
        return done("Done.")
    return worker


def _kinds(daemon: RunningDaemon, kind: str) -> list[dict[str, Any]]:
    return run(daemon, lambda: [e.data for e in daemon.daemon.journal.of_kind(kind)])


def _pending(daemon: RunningDaemon) -> tuple[str, str]:
    return run(daemon, lambda: daemon.daemon.supervisor.continuity.pending("s1"))


def _planner_contexts(script: Any) -> list[str]:
    return [c for _, k, c, _ in script.contexts if k == "supervisor"]


# decision: ORCH-586
def test_a_message_that_answers_the_question_continues_its_request(daemon: RunningDaemon) -> None:
    seen: list[str] = []
    script = install(daemon, [_answer("clarify", QUESTION, answers=False),
                              _answer("task", "Ordering.", task("Order 2 m cables"), answers=True)],
                     {"worker": _payloads(daemon, seen)})
    chat(daemon, CABLES)
    chat(daemon, "2 meters")
    second = _planner_contexts(script)[1]
    assert QUESTION in second and CABLES in second                       # shown apart, as the open question's
    assert CABLES in seen[0] and "2 meters" in seen[0]                   # joined: the task continues the request
    assert [a["by"] for a in _kinds(daemon, "question.answered")] == [_kinds(daemon, "chat.message")[1]["request_id"]]
    assert _pending(daemon) == ("", "")


# decision: ORCH-586
def test_an_unrelated_request_while_a_question_is_open_is_handled_alone_and_the_question_waits(
        daemon: RunningDaemon) -> None:
    seen: list[str] = []
    install(daemon, [_answer("clarify", QUESTION, answers=False),
                     _answer("task", "Booking.", task("Book a hotel in north Goa"), answers=False),
                     _answer("task", "Ordering.", task("Order 2 m cables"), answers=True)],
            {"worker": _payloads(daemon, seen)})
    chat(daemon, CABLES)
    chat(daemon, HOTEL)
    assert HOTEL in seen[0] and CABLES not in seen[0] and QUESTION not in seen[0]   # never the old request's words
    assert _kinds(daemon, "question.answered") == [] and len(_kinds(daemon, "question.kept")) == 1
    assert _pending(daemon) == (CABLES, QUESTION)                        # the question is still open, unchanged
    chat(daemon, "2 meters")
    assert CABLES in seen[1] and "2 meters" in seen[1] and HOTEL not in seen[1]
    assert _pending(daemon) == ("", "")


# decision: ORCH-586
def test_three_declines_then_a_new_request_runs_on_its_own(daemon: RunningDaemon) -> None:
    """The runner's case: the owner declines the question three times (each answers it: the request goes on, FERB asks
    again), then sends a new request, which runs alone."""
    seen: list[str] = []
    install(daemon, [_answer("clarify", QUESTION, answers=False),
                     *[_answer("clarify", QUESTION, answers=True)] * 3,
                     _answer("task", "Booking.", task("Book a hotel in north Goa"), answers=False)],
            {"worker": _payloads(daemon, seen)})
    chat(daemon, CABLES)
    for _ in range(3):
        chat(daemon, DECLINE)
    pending, asked = _pending(daemon)
    assert CABLES in pending and pending.count(DECLINE) == 3 and asked == QUESTION
    chat(daemon, HOTEL)
    assert seen and HOTEL in seen[0] and CABLES not in seen[0] and DECLINE not in seen[0]
    assert len(_kinds(daemon, "question.answered")) == 3 and _pending(daemon) == (pending, QUESTION)


# decision: ORCH-586
def test_an_answer_written_before_the_field_continues_the_request_as_before(daemon: RunningDaemon) -> None:
    """STORE-403: a planner answer with no answers_question (an older form) reads as joined, as every continuation
    did before."""
    seen: list[str] = []
    install(daemon, [_answer("clarify", QUESTION), _answer("task", "Ordering.", task("Order 2 m cables"))],
            {"worker": _payloads(daemon, seen)})
    chat(daemon, CABLES)
    chat(daemon, "2 meters")
    assert CABLES in seen[0] and "2 meters" in seen[0]


# decision: ORCH-621
def test_an_answer_to_ferbs_question_routed_to_no_question_starts_its_task_with_no_stray_line(
        daemon: RunningDaemon) -> None:
    """Live 2026-11-11 12:58 (qrun-20261111-125635-codex-1): the planner answered FERB's own question (answers_question)
    and also routed the message as an answer to an open card by the id of the wrapper around that question; the owner
    read "No question is waiting for your answer right now." just before the task started."""
    seen: list[str] = []
    wrong = json.loads(_answer("task", "Ordering.", task("Order 2 m cables"), answers=True))
    wrong["answer_question"] = {"approval_id": "c71432da6150", "answer": "2 meters"}
    install(daemon, [_answer("clarify", QUESTION, answers=False), json.dumps(wrong)],
            {"worker": _payloads(daemon, seen)})
    chat(daemon, CABLES)
    events = chat(daemon, "2 meters")
    assert not [e for e in events if e["type"] == "reply"]
    assert any(e["type"] == "plan" for e in events) and CABLES in seen[0] and "2 meters" in seen[0]
    assert [r["text"] for r in _kinds(daemon, "chat.reply")] == []
    assert _kinds(daemon, "question.routing_unused")[0]["joined"] is True
